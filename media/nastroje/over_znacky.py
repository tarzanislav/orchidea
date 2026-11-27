#!/usr/bin/env python3
"""Rozhodca značiek (metadát) a ReplayGain pre LÝRU: náš `media_znacky` proti `ffprobe`.

Súbory vytvorí:
- **FFmpeg** (`-metadata`): MP3 s ID3v2.4 aj ID3v2.3, MP3 len s ID3v1, MP2, FLAC, Matroska (FLAC, Opus), M4A;
- **metaflac** (referenčný nástroj FLAC): skutočne vypočítaný ReplayGain (`--add-replay-gain`) a značky UTF-8;
- **mkvpropedit** (MKVToolNix): značky Matroska s cieľmi skladba (30) a album (50) vrátane `REPLAYGAIN_GAIN`
  a `REPLAYGAIN_PEAK` podľa `matroska_tags.xml`;
- **tento skript priamo podľa špecifikácií**: ID3v2.2 (trojznakové rámce), ID3v2.3 s nesynchronizáciou celej
  značky a UTF-16, ID3v2.4 s nesynchronizáciou rámca, indikátorom dĺžky a pätou, APEv2 + ID3v1 na konci MP3,
  voľné položky `----` (`mean`/`name`/`data`) s ReplayGain v M4A;
- **APEv2**: FFmpeg ho v MP3 nečíta, v súbore WavPack áno – ten istý blok APEv2 (náš aj zapísaný FFmpeg) sa
  pripojí k MP3 aj k súboru `.wv` a referenciou je ffprobe súboru `.wv`.

Porovnanie: všetky položky (kľúč prevedený na spoločný slovník, hodnota) ako multimnožina proti značkám formátu
a prvej zvukovej stopy z ffprobe (bez technických kľúčov MP4: značky, jazyk, obsluha). Pri cieľoch Matroska
a výsledku metaflac sa navyše overí ReplayGain presne podľa očakávaných hodnôt.

**Obal albumu:** bajty `Znacky::obal` proti obrázku `attached_pic`, ktorý vytiahne FFmpeg (APIC v ID3v2.4 a 2.3,
PICTURE vo FLAC z FFmpeg aj metaflac, `covr` v M4A, príloha `cover.png` v Matroske, APEv2 „Cover Art (Front)“ cez
WavPack), alebo proti pôvodnému obrázku, kde má súbor viac obrázkov (zadná strana pred prednou; Matroska s písmom
a `small_cover` pred `cover`) a ID3v2.2 PIC.

Použitie: `over_znacky.py <media_znacky> <adresár>`
"""
import json
import os
import struct
import subprocess
import sys

M = {
    "title": "Svetlo nad jazerom",
    "artist": "Ľubo & Žofia",
    "album": "Suita · 2026",
    "album_artist": "Orchester",
    "date": "2026-11-27",
    "track": "2/6",
    "disc": "1/2",
    "genre": "Classical",
    "composer": "Autor",
    "comment": "Poznámka",
    "REPLAYGAIN_TRACK_GAIN": "-6.20 dB",
    "REPLAYGAIN_TRACK_PEAK": "0.988123",
    "REPLAYGAIN_ALBUM_GAIN": "-5.10 dB",
    "REPLAYGAIN_ALBUM_PEAK": "1.000000",
}

# ffprobe (spoločné kľúče FFmpeg) → náš spoločný slovník
FFPROBE = {"album_artist": "ALBUMARTIST", "track": "TRACKNUMBER", "disc": "DISCNUMBER"}
# ekvivalenty z iných formátov – rovnaké ako `spolocny_kluc` v media/src/znacky.rs
ALIASY = {"DESCRIPTION": "COMMENT", "ALBUM ARTIST": "ALBUMARTIST", "ALBUM_ARTIST": "ALBUMARTIST", "YEAR": "DATE",
          "DATE_RELEASED": "DATE", "TRACK": "TRACKNUMBER", "PART_NUMBER": "TRACKNUMBER", "DISC": "DISCNUMBER"}
TECHNICKE = {"major_brand", "minor_version", "compatible_brands", "language", "handler_name", "vendor_id"}


def kluc(k):
    k = FFPROBE.get(k, k).upper()
    return ALIASY.get(k, k)


def ffprobe(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format_tags:stream_tags:stream=codec_type",
                        "-of", "json", cesta], capture_output=True, text=True, check=True)
    j = json.loads(r.stdout)
    polozky = list(j.get("format", {}).get("tags", {}).items())
    zvuk = [s for s in j.get("streams", []) if s.get("codec_type") == "audio"]
    if zvuk:
        polozky += list(zvuk[0].get("tags", {}).items())
    return sorted((kluc(k), v) for k, v in polozky if k not in TECHNICKE)


def nase(program, cesta):
    r = subprocess.run([program, cesta], capture_output=True, text=True)
    polozky, rg = [], None
    for riadok in r.stdout.splitlines():
        if riadok.startswith("ZNACKA "):
            k, _, v = riadok[7:].partition("\t")
            polozky.append((k, v.replace("\\n", "\n")))
        elif riadok.startswith("REPLAYGAIN "):
            rg = dict(x.split("=") for x in riadok.split()[1:])
    return r.returncode, sorted(polozky), rg


# ---------- súbory podľa špecifikácií ----------

def synchsafe(n):
    return bytes([(n >> 21) & 0x7F, (n >> 14) & 0x7F, (n >> 7) & 0x7F, n & 0x7F])


def nesync(d):
    """Nesynchronizácia (ID3v2.4 §6.1): FF 00 → FF 00 00, FF 111xxxxx → FF 00 111xxxxx."""
    v = bytearray()
    for i, b in enumerate(d):
        v.append(b)
        if b == 0xFF and (i + 1 == len(d) or d[i + 1] == 0 or d[i + 1] >= 0xE0):
            v.append(0)
    return bytes(v)


def zvuk_mp3(adresar):
    cesta = os.path.join(adresar, "_ciste.mp3")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=440:d=1", "-c:a", "libmp3lame",
                    "-id3v2_version", "0", "-write_xing", "0", cesta], check=True)
    return open(cesta, "rb").read()


def id3v22():
    def r(i, obsah):
        return i + len(obsah).to_bytes(3, "big") + obsah
    telo = (r(b"TT2", b"\x00N\xe1zov 2.2") + r(b"TP1", b"\x01\xff\xfeA\x00\xff\x00") + r(b"TAL", b"\x00Album")
            + r(b"TCO", b"\x00(17)") + r(b"TRK", b"\x003/9") + r(b"TYE", b"\x002001")
            + r(b"TXX", b"\x00REPLAYGAIN_TRACK_GAIN\x00+1.25 dB") + r(b"COM", b"\x00eng\x00koment"))
    telo += bytes(16)
    return b"ID3\x02\x00\x00" + synchsafe(len(telo)) + telo


def id3v23_nesync():
    def r(i, obsah):
        return i + struct.pack(">I", len(obsah)) + b"\x00\x00" + obsah
    u16 = lambda s: b"\xff\xfe" + s.encode("utf-16-le")  # noqa: E731
    telo = (r(b"TIT2", b"\x01" + u16("Názov ÿ 2.3")) + r(b"TPE1", b"\x00\xff\xe7 interpret")
            + r(b"TYER", b"\x001998") + r(b"TDAT", b"\x000512") + r(b"TCON", b"\x00(4)Eurodisco")
            + r(b"TXXX", b"\x01" + u16("REPLAYGAIN_ALBUM_GAIN") + b"\x00\x00" + u16("-2.50 dB")))
    telo = nesync(telo) + bytes(8)
    return b"ID3\x03\x00\x80" + synchsafe(len(telo)) + telo


def id3v24_ramce():
    def r(i, obsah, priznaky=0):
        if priznaky & 0x02:  # nesynchronizácia rámca + indikátor dĺžky
            obsah = synchsafe(len(obsah)) + nesync(obsah)
        return i + synchsafe(len(obsah)) + bytes([0, priznaky]) + obsah
    telo = (r(b"TIT2", b"\x03N\xc3\xa1zov \xc3\xbf 2.4", 0x03) + r(b"TPE2", b"\x02" + "Orchester".encode("utf-16-be"))
            + r(b"TDRC", b"\x032024-05-06") + r(b"TCON", b"\x0313")
            + r(b"TXXX", b"\x03REPLAYGAIN_TRACK_GAIN\x00-0.75 dB") + r(b"TXXX", b"\x03REPLAYGAIN_TRACK_PEAK\x000.501", 0x03))
    return b"ID3\x04\x00\x10" + synchsafe(len(telo)) + telo + b"3DI\x04\x00\x10" + synchsafe(len(telo))


def apev2_id3v1(s_id3v1=True):
    polozky = b""
    for k, v in (("Title", "Názov APE"), ("Artist", "Interpret"), ("Year", "1987"), ("Track", "4"),
                 ("REPLAYGAIN_TRACK_GAIN", "-8.30 dB"), ("REPLAYGAIN_TRACK_PEAK", "0.912")):
        b = v.encode()
        polozky += struct.pack("<II", len(b), 0) + k.encode() + b"\x00" + b
    hlava = lambda p: b"APETAGEX" + struct.pack("<IIII", 2000, len(polozky) + 32, 6, p) + bytes(8)  # noqa: E731
    ape = hlava(0xA0000000) + polozky + hlava(0x80000000)
    v1 = b"TAG" + b"Ignoruje sa".ljust(30, b"\0") + b"Iny".ljust(30, b"\0") + b"Album v1".ljust(30, b"\0") \
        + b"1999" + b"pozn".ljust(28, b"\0") + b"\x00\x05" + bytes([17])
    return ape + v1 if s_id3v1 else ape


def boxy(d):
    p = 0
    while p + 8 <= len(d):
        n, t = struct.unpack(">I4s", d[p:p + 8])
        yield t, d[p + 8:p + n]
        p += n


def box(t, o):
    return struct.pack(">I", len(o) + 8) + t + o


def m4a_volne(cesta, nova):
    """Doplní do moov/udta/meta/ilst voľné položky `----` (moov je za mdat – posuny vzoriek sa nezmenia)."""
    d = open(cesta, "rb").read()
    vysl = b""
    for t, o in boxy(d):
        if t == b"moov":
            novy = b""
            for t2, o2 in boxy(o):
                if t2 == b"udta":
                    udta = b""
                    for t3, o3 in boxy(o2):
                        if t3 == b"meta":
                            meta = o3[:4]
                            for t4, o4 in boxy(o3[4:]):
                                if t4 == b"ilst":
                                    for meno, hodnota in nova:
                                        o4 += box(b"----", box(b"mean", bytes(4) + b"com.apple.iTunes")
                                                  + box(b"name", bytes(4) + meno.encode())
                                                  + box(b"data", struct.pack(">II", 1, 0) + hodnota.encode()))
                                meta += box(t4, o4)
                            o3 = meta
                        udta += box(t3, o3)
                    o2 = udta
                novy += box(t2, o2)
            o = novy
        vysl += box(t, o)
    open(nova_cesta := cesta.replace(".m4a", "_volne.m4a"), "wb").write(vysl)
    return nova_cesta


def spoj_viacnasobne(my, ref):
    """Viac hodnôt jedného kľúča (Vorbis comment, APEv2) ffprobe spojí cez „;“ – spojíme rovnako, ak tak sedia."""
    vysl = list(my)
    for k in sorted({k for k, _ in my}):
        hodnoty = [v for kk, v in my if kk == k]
        if len(hodnoty) > 1 and (k, ";".join(hodnoty)) in ref:
            vysl = [x for x in vysl if x[0] != k] + [(k, ";".join(hodnoty))]
    return sorted(vysl)


# súbory, pri ktorých sa FFmpeg odchyľuje od textu špecifikácie: (dôvod, očakávané položky)
OCAKAVANE = {
    "id3v23_nesync.mp3": ("FFmpeg ruší nesynchronizáciu ID3v2.3 po rámcoch, text §3.1 a §5 na celej značke", [
        ("TITLE", "Názov ÿ 2.3"), ("ARTIST", "ÿç interpret"), ("DATE", "1998-12-05"), ("GENRE", "Eurodisco"),
        ("REPLAYGAIN_ALBUM_GAIN", "-2.50 dB")]),
    "apev2_id3v1.mp3": ("FFmpeg pri ID3v1 za APEv2 číta len ID3v1; APEv2 má prednosť, ID3v1 dopĺňa", [
        ("TITLE", "Názov APE"), ("ARTIST", "Interpret"), ("DATE", "1987"), ("TRACKNUMBER", "4"),
        ("REPLAYGAIN_TRACK_GAIN", "-8.30 dB"), ("REPLAYGAIN_TRACK_PEAK", "0.912"), ("ALBUM", "Album v1"),
        ("COMMENT", "pozn"), ("GENRE", "Rock")]),
    "ciele.mka": ("FFmpeg zlúči ciele skladba a album do jedného kľúča; REPLAYGAIN_GAIN podľa cieľa", [
        ("TITLE", "Album Matroska"), ("REPLAYGAIN_ALBUM_GAIN", "-3.40 dB"), ("REPLAYGAIN_ALBUM_PEAK", "0.97"),
        ("ARTIST", "Ľubo"), ("TRACKNUMBER", "5"), ("REPLAYGAIN_TRACK_GAIN", "-1.20 dB"),
        ("REPLAYGAIN_TRACK_PEAK", "0.75")]),
}

MKV_ZNACKY = """<?xml version="1.0" encoding="UTF-8"?>
<Tags>
  <Tag><Targets><TargetTypeValue>50</TargetTypeValue></Targets>
    <Simple><Name>TITLE</Name><String>Album Matroska</String></Simple>
    <Simple><Name>REPLAYGAIN_GAIN</Name><String>-3.40 dB</String></Simple>
    <Simple><Name>REPLAYGAIN_PEAK</Name><String>0.97</String></Simple>
  </Tag>
  <Tag><Targets><TargetTypeValue>30</TargetTypeValue></Targets>
    <Simple><Name>ARTIST</Name><String>Ľubo</String></Simple>
    <Simple><Name>PART_NUMBER</Name><String>5</String></Simple>
    <Simple><Name>REPLAYGAIN_GAIN</Name><String>-1.20 dB</String></Simple>
    <Simple><Name>REPLAYGAIN_PEAK</Name><String>0.75</String></Simple>
  </Tag>
</Tags>
"""


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    meta = [a for k, v in M.items() for a in ("-metadata", f"{k}={v}")]
    subory = []  # (meno, cesta, očakávaný ReplayGain alebo None)
    for meno, arg in (("id3v24.mp3", ["-c:a", "libmp3lame"]), ("id3v23.mp3", ["-c:a", "libmp3lame", "-id3v2_version", "3"]),
                      ("id3v1.mp3", ["-c:a", "libmp3lame", "-id3v2_version", "0", "-write_id3v1", "1"]),
                      ("flac.flac", ["-c:a", "flac"]), ("flac.mkv", ["-c:a", "flac"]),
                      ("opus.mka", ["-c:a", "libopus"]), ("aac.m4a", ["-c:a", "aac"])):
        cesta = os.path.join(adresar, meno)
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=440:d=1"] + meta + arg + [cesta],
                       check=True)
        subory.append((meno, cesta, None, None))
    # metaflac: skutočný ReplayGain referenčného nástroja FLAC
    cesta = os.path.join(adresar, "metaflac.flac")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
                    "aevalsrc=0.4*sin(2*PI*330*t)*(1+0.5*sin(2*PI*0.7*t)):s=44100:d=4", "-c:a", "flac", cesta],
                   check=True)
    subprocess.run(["metaflac", "--remove-all-tags", "--set-tag=TITLE=Žltá ľalia", "--set-tag=ARTIST=A",
                    "--set-tag=ARTIST=B", "--add-replay-gain", cesta], check=True)
    rg = dict(riadok.split("=", 1) for riadok in subprocess.run(
        ["metaflac", "--export-tags-to=-", cesta], capture_output=True, text=True, check=True).stdout.splitlines()
        if riadok.startswith("REPLAYGAIN"))
    subory.append(("metaflac.flac", cesta, {
        "stopa_db": rg["REPLAYGAIN_TRACK_GAIN"].split()[0], "stopa_vrchol": rg["REPLAYGAIN_TRACK_PEAK"],
        "album_db": rg["REPLAYGAIN_ALBUM_GAIN"].split()[0], "album_vrchol": rg["REPLAYGAIN_ALBUM_PEAK"]}, None))
    # mkvpropedit: ciele skladba a album
    cesta = os.path.join(adresar, "ciele.mka")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=440:d=1", "-c:a", "flac", cesta],
                   check=True)
    xml = os.path.join(adresar, "znacky.xml")
    open(xml, "w", encoding="utf-8").write(MKV_ZNACKY)
    subprocess.run(["mkvpropedit", "-q", cesta, "--tags", f"all:{xml}"], check=True)
    subory.append(("ciele.mka", cesta, {"stopa_db": "-1.20", "stopa_vrchol": "0.75", "album_db": "-3.40",
                                        "album_vrchol": "0.97"}, None))
    # podľa špecifikácií
    zvuk = zvuk_mp3(adresar)
    # APEv2: FFmpeg ho v MP3 nečíta, v súbore WavPack áno – referencia je ffprobe toho istého bloku APEv2 v .wv
    wv_ciste = os.path.join(adresar, "_ciste.wv")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=440:d=1", "-c:a", "wavpack",
                    "-map_metadata", "-1", "-fflags", "+bitexact", wv_ciste], check=True)
    wv_ffmpeg = os.path.join(adresar, "apev2_ffmpeg.wv")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=440:d=1", "-c:a", "wavpack"] + meta
                   + [wv_ffmpeg], check=True)
    d = open(wv_ffmpeg, "rb").read()
    ape_ffmpeg = d[d.find(b"APETAGEX"):]
    for meno, ape, ocak in (("apev2.mp3", apev2_id3v1(False), {"stopa_db": "-8.30", "stopa_vrchol": "0.912"}),
                            ("apev2_ffmpeg.mp3", ape_ffmpeg, {"stopa_db": "-6.20", "album_db": "-5.10"})):
        cesta = os.path.join(adresar, meno)
        open(cesta, "wb").write(zvuk_mp3(adresar) + ape)
        ref = cesta[:-4] + "_ref.wv"
        open(ref, "wb").write(open(wv_ciste, "rb").read() + ape)
        subory.append((meno, cesta, ocak, ref))
    for meno, d, ocak in (("id3v22.mp3", id3v22() + zvuk, {"stopa_db": "1.25"}),
                          ("id3v23_nesync.mp3", id3v23_nesync() + zvuk, {"album_db": "-2.50"}),
                          ("id3v24_ramce.mp3", id3v24_ramce() + zvuk, {"stopa_db": "-0.75", "stopa_vrchol": "0.501"}),
                          ("apev2_id3v1.mp3", zvuk + apev2_id3v1(), {"stopa_db": "-8.30", "stopa_vrchol": "0.912"})):
        cesta = os.path.join(adresar, meno)
        open(cesta, "wb").write(d)
        subory.append((meno, cesta, ocak, None))
    volne = m4a_volne(os.path.join(adresar, "aac.m4a"), [("replaygain_track_gain", "-4.50 dB"),
                                                          ("replaygain_track_peak", "0.8")])
    subory.append(("aac_volne.m4a", volne, {"stopa_db": "-4.50", "stopa_vrchol": "0.8"}, None))

    print("| súbor | položiek (náš / ffprobe) | zhoda značiek | ReplayGain (skladba dB, vrchol; album dB, vrchol) | výsledok |")
    print("|---|---|---|---|---|")
    chyb = 0
    for meno, cesta, ocak, ref_cesta in subory:
        kod, my, rg = nase(program, cesta)
        ref = ffprobe(ref_cesta or cesta)
        if meno in OCAKAVANE:
            # FFmpeg sa tu odchyľuje od textu špecifikácie – porovnanie s presne očakávanými položkami
            dovod, polozky = OCAKAVANE[meno]
            zhoda = my == sorted(polozky)
            print(f"| {meno} | {len(my)} / {len(ref)} | {'áno' if zhoda else 'nie'} (očakávané – {dovod}) | ", end="")
        else:
            my = spoj_viacnasobne(my, ref)
            zhoda = my == ref
            print(f"| {meno} | {len(my)} / {len(ref)} | {'áno' if zhoda else 'nie'} | ", end="")
        rg_ok = True
        if ocak:
            rg_ok = rg is not None and all(abs(float(rg[k]) - float(v)) < 5e-7 * max(1, abs(float(v)))
                                           for k, v in ocak.items())
        ok = kod == 0 and zhoda and rg_ok and len(my) > 0
        chyb += not ok
        rgt = "–" if rg is None else f"{rg['stopa_db']}, {rg['stopa_vrchol']}; {rg['album_db']}, {rg['album_vrchol']}"
        print(f"{rgt} | {'✅' if ok else '❌'} |")
        if not zhoda and meno not in OCAKAVANE:
            print(f"  len náš: {sorted(set(my) - set(ref))}")
            print(f"  len ffprobe: {sorted(set(ref) - set(my))}")
    chyb += obaly(program, adresar)
    sys.exit(1 if chyb else 0)


def obaly(program, adresar):
    """Obal albumu: bajty `Znacky::obal` proti obrázku `attached_pic`, ktorý vytiahne FFmpeg, alebo (kde súbor
    obsahuje viac obrázkov a FFmpeg by vzal prvý) proti pôvodnému obrázku."""
    jpg, png = os.path.join(adresar, "obal.jpg"), os.path.join(adresar, "obal.png")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=s=320x320:d=1", "-frames:v", "1", jpg],
                   check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=200x200:d=1", "-frames:v", "1",
                    png], check=True)
    jpg_b, png_b = open(jpg, "rb").read(), open(png, "rb").read()
    zvuk = ["-f", "lavfi", "-i", "sine=440:d=1"]
    subory = []  # (meno, cesta, referencia: cesta súboru pre FFmpeg alebo bajty, očakávaný MIME)
    for meno, arg in (("obal_id3v24.mp3", ["-c:a", "libmp3lame"]), ("obal_id3v23.mp3", ["-c:a", "libmp3lame",
                      "-id3v2_version", "3"]), ("obal.flac", ["-c:a", "flac"]), ("obal.m4a", ["-c:a", "aac"])):
        cesta = os.path.join(adresar, meno)
        subprocess.run(["ffmpeg", "-v", "error", "-y"] + zvuk + ["-i", jpg, "-map", "0", "-map", "1"] + arg
                       + ["-c:v", "copy", "-disposition:v", "attached_pic", "-metadata:s:v", "comment=Cover (front)",
                          cesta], check=True)
        subory.append((meno, cesta, cesta, "image/jpeg"))
    # Matroska: príloha cover.png (FFmpeg) a súbor s písmom a malým obalom pred normálnym (mkvpropedit)
    cesta = os.path.join(adresar, "obal.mka")
    subprocess.run(["ffmpeg", "-v", "error", "-y"] + zvuk + ["-c:a", "flac", "-attach", png, "-metadata:s:t",
                    "mimetype=image/png", "-metadata:s:t", "filename=cover.png", cesta], check=True)
    subory.append(("obal.mka", cesta, cesta, "image/png"))
    cesta = os.path.join(adresar, "obal_poradie.mka")
    subprocess.run(["ffmpeg", "-v", "error", "-y"] + zvuk + ["-c:a", "flac", cesta], check=True)
    pismo = os.path.join(adresar, "pismo.ttf")
    open(pismo, "wb").write(bytes(range(256)) * 64)
    for subor, meno_prilohy, mime in ((pismo, "pismo.ttf", "font/ttf"), (jpg, "small_cover.jpg", "image/jpeg"),
                                      (png, "cover.png", "image/png")):
        subprocess.run(["mkvpropedit", "-q", cesta, "--attachment-name", meno_prilohy, "--attachment-mime-type", mime,
                        "--add-attachment", subor], check=True)
    subory.append(("obal_poradie.mka", cesta, png_b, "image/png"))
    # metaflac: obrázok typu 3 s popisom
    cesta = os.path.join(adresar, "obal_metaflac.flac")
    subprocess.run(["ffmpeg", "-v", "error", "-y"] + zvuk + ["-c:a", "flac", cesta], check=True)
    subprocess.run(["metaflac", f"--import-picture-from=3|image/png|Predná strana||{png}", cesta], check=True)
    subory.append(("obal_metaflac.flac", cesta, cesta, "image/png"))
    # ID3v2.2 PIC a ID3v2.4 so zadnou stranou pred prednou (podľa špecifikácie)
    zvuk_b = zvuk_mp3(adresar)
    pic = b"\x00PNG\x03Obal\x00" + png_b
    telo = b"TT2" + (6).to_bytes(3, "big") + b"\x00Nazov" + b"PIC" + len(pic).to_bytes(3, "big") + pic
    cesta = os.path.join(adresar, "obal_id3v22.mp3")
    open(cesta, "wb").write(b"ID3\x02\x00\x00" + synchsafe(len(telo)) + telo + zvuk_b)
    subory.append(("obal_id3v22.mp3", cesta, png_b, "image/png"))
    def apic(druh, data, mime):
        o = b"\x03" + mime + b"\x00" + bytes([druh]) + "Strana".encode() + b"\x00" + data
        return b"APIC" + synchsafe(len(o)) + b"\x00\x00" + o
    telo = apic(4, png_b, b"image/png") + apic(3, jpg_b, b"image/jpeg")
    cesta = os.path.join(adresar, "obal_zadna_predna.mp3")
    open(cesta, "wb").write(b"ID3\x04\x00\x00" + synchsafe(len(telo)) + telo + zvuk_b)
    subory.append(("obal_zadna_predna.mp3", cesta, jpg_b, "image/jpeg"))
    # APEv2 „Cover Art (Front)“: referencia je ten istý blok v súbore WavPack
    hodnota = b"obal.jpg\x00" + jpg_b
    polozka = struct.pack("<II", len(hodnota), 2) + b"Cover Art (Front)\x00" + hodnota
    hlava = lambda p: b"APETAGEX" + struct.pack("<IIII", 2000, len(polozka) + 32, 1, p) + bytes(8)  # noqa: E731
    ape = hlava(0xA0000000) + polozka + hlava(0x80000000)
    cesta = os.path.join(adresar, "obal_apev2.mp3")
    open(cesta, "wb").write(zvuk_b + ape)
    ref = os.path.join(adresar, "obal_apev2_ref.wv")
    open(ref, "wb").write(open(os.path.join(adresar, "_ciste.wv"), "rb").read() + ape)
    subory.append(("obal_apev2.mp3", cesta, ref, "image/jpeg"))

    print("\n| obal albumu | MIME | bajtov (náš / referencia) | referencia | výsledok |")
    print("|---|---|---|---|---|")
    chyb = 0
    for meno, cesta, ref, mime in subory:
        nas_cesta = cesta + ".obal"
        if os.path.exists(nas_cesta):
            os.remove(nas_cesta)
        r = subprocess.run([program, cesta, nas_cesta], capture_output=True, text=True)
        nas = open(nas_cesta, "rb").read() if os.path.exists(nas_cesta) else b""
        nas_mime = next((x.split()[1][5:] for x in r.stdout.splitlines() if x.startswith("OBAL_ALBUMU ")), "")
        if isinstance(ref, bytes):
            ref_b, druh = ref, "pôvodný obrázok"
        else:
            vyt = cesta + ".ffmpeg"
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", ref, "-map", "0:v:0", "-c", "copy", "-frames:v", "1",
                            "-f", "image2", vyt], check=True)
            ref_b, druh = open(vyt, "rb").read(), "attached_pic FFmpeg"
        ok = r.returncode == 0 and nas == ref_b and len(nas) > 0 and nas_mime == mime
        chyb += not ok
        print(f"| {meno} | {nas_mime} | {len(nas)} / {len(ref_b)} | {druh} | {'✅' if ok else '❌'} |")
    return chyb


if __name__ == "__main__":
    main()
