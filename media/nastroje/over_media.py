#!/usr/bin/env python3
"""Nezávislé overenie čítačiek kontajnerov ORCHIDEE (balík media) – len v CI, nie súčasť programu.

Rozhodcami sú cudzie nástroje: FFmpeg vytvorí skúšobné súbory rôznymi kodekmi a nastaveniami zapisovača,
MKVToolNix (mkvmerge, ak je) ich prebalí iným zapisovačom (lacing, iné rozloženie), ffprobe vypíše pakety
(čas, trvanie, kľúčovosť, veľkosť, MD5 obsahu). Náš program `media_pakety` musí dať bit po bite tie isté pakety.
Porovnáva sa poradie paketov v rámci každej stopy (poradie medzi stopami norma neurčuje; my čítame súvisle
v poradí súboru). PCM sa porovná ako celý tok bajtov stopy (rozdelenie PCM na pakety norma neurčuje).
Pripustené a vypísané rozdiely – každý je overiteľný z ostatných údajov:
- čas ďalších rámcov lacingu a trvanie z DefaultDuration ±1 tik (my k najbližšiemu tiku, FFmpeg nadol),
- trvanie zo súboru, ktoré presne sedí s časom ďalšieho paketu (FFmpeg hlási dĺžku rámca), a trvanie
  posledného paketu stopy (BlockDuration, posledná vzorka),
- stály posun PTS celej stopy pri zhodných DTS: FFmpeg pri záporných posunoch zobrazenia posunie PTS, aby
  PTS ≥ DTS; podľa ISO/IEC 14496-12 platí PTS = DTS + posun,
- MPEG-TS: čas ďalších rámcov zvuku v tom istom PES ±1 tik (my presne z počtu vzoriek a PTS PES, FFmpeg sčíta
  zaokrúhlené trvania rámcov),
- MPEG-TS, video Annex B: nulový bajt (zero_byte) pred počiatočným kódom 00 00 01 patrí podľa ITU-T H.264/H.265
  príloha B k prístupovej jednotke za ním (my), FFmpeg ho nechá v predošlom pakete – overí sa, že celý tok
  stopy je bit po bite rovnaký a hranice paketov sa líšia len o tento nulový bajt (obsah paketov je tým určený).

Použitie: over_media.py <cesta k media_pakety> <pracovný priečinok>
Návratový kód 0 = všetko sedí. Výsledok je tabuľka v Markdowne (pre súhrn CI).
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys

PROGRAM = sys.argv[1]
PRAC = sys.argv[2]
os.makedirs(PRAC, exist_ok=True)
CHYBY = []
RIADKY = []

OBRAZ = ["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=3"]
ZVUK = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3", "-ac", "2"]
ZVUK44 = ["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=44100:duration=3"]

# meno, prípona, argumenty FFmpeg (bez vstupu a výstupu)
VARIANTY = [
    ("h264_flac", "mkv", OBRAZ + ZVUK + ["-c:v", "libx264", "-g", "12", "-bf", "2", "-c:a", "flac"]),
    ("vp9_opus", "webm", OBRAZ + ZVUK + ["-c:v", "libvpx-vp9", "-g", "10", "-deadline", "realtime", "-c:a", "libopus"]),
    ("hevc_ac3", "mkv", OBRAZ + ZVUK + ["-c:v", "libx265", "-x265-params", "log-level=none:keyint=10", "-c:a", "ac3"]),
    ("mpeg2_mp2", "mkv", OBRAZ + ZVUK + ["-c:v", "mpeg2video", "-c:a", "mp2"]),
    ("vp8_vorbis", "webm", OBRAZ + ZVUK + ["-c:v", "libvpx", "-deadline", "realtime", "-c:a", "libvorbis"]),
    ("pcm16_mp3", "mkv", ZVUK44 + ZVUK + ["-map", "0", "-map", "1", "-c:a:0", "pcm_s16le", "-c:a:1", "libmp3lame"]),
    ("pcm24_f32", "mkv", ZVUK + ZVUK44 + ["-map", "0", "-map", "1", "-c:a:0", "pcm_s24le", "-c:a:1", "pcm_f32le"]),
    ("zivy_webm", "webm", OBRAZ + ZVUK + ["-c:v", "libvpx-vp9", "-deadline", "realtime", "-c:a", "libopus", "-live", "1"]),
    ("male_zhluky", "mkv", OBRAZ + ZVUK + ["-c:v", "libx264", "-c:a", "flac", "-cluster_time_limit", "100", "-cluster_size_limit", "20000"]),
    # dlhší súbor (≈ 20 MB) – čítanie ide cez viac dávok po 4 MiB
    ("dlhy_h264_opus", "mkv", ["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=40"] + ZVUK[:3]
     + ["sine=frequency=440:sample_rate=48000:duration=40", "-ac", "2", "-c:v", "libx264", "-preset", "ultrafast",
        "-b:v", "4M", "-c:a", "libopus"]),
    # MP4 a MOV (L3): zoznamy úprav (snímky B, posun AAC), moov na konci aj na začiatku, fragmenty, QuickTime PCM
    ("mp4_h264_aac", "mp4", OBRAZ + ZVUK + ["-c:v", "libx264", "-bf", "2", "-g", "12", "-c:a", "aac"]),
    ("mp4_rychly_start", "mp4", OBRAZ + ZVUK + ["-c:v", "libx264", "-c:a", "aac", "-movflags", "+faststart"]),
    ("mp4_hevc_ac3", "mp4", OBRAZ + ZVUK + ["-c:v", "libx265", "-tag:v", "hvc1", "-x265-params", "log-level=none", "-c:a", "ac3"]),
    ("mp4_vp9_opus", "mp4", OBRAZ + ZVUK + ["-c:v", "libvpx-vp9", "-deadline", "realtime", "-c:a", "libopus"]),
    ("mp4_av1_flac", "mp4", OBRAZ + ZVUK + ["-c:v", "libaom-av1", "-cpu-used", "8", "-g", "10", "-c:a", "flac", "-strict", "-2"]),
    ("mp4_mp3", "mp4", OBRAZ + ZVUK + ["-c:v", "mpeg4", "-c:a", "libmp3lame"]),
    ("mp4_fragmenty", "mp4", OBRAZ + ZVUK + ["-c:v", "libx264", "-g", "12", "-c:a", "aac", "-movflags", "frag_keyframe+empty_moov"]),
    ("mp4_fragmenty_moof", "mp4", OBRAZ + ZVUK + ["-c:v", "libx264", "-g", "12", "-c:a", "aac", "-movflags", "frag_keyframe+empty_moov+default_base_moof"]),
    ("mp4_cmaf", "mp4", OBRAZ + ZVUK + ["-c:v", "libx264", "-g", "25", "-c:a", "aac", "-movflags", "cmaf+frag_keyframe+empty_moov+separate_moof"]),
    ("m4a_aac", "m4a", ZVUK + ["-c:a", "aac"]),
    ("mov_h264_sowt", "mov", OBRAZ + ZVUK + ["-c:v", "libx264", "-c:a", "pcm_s16le"]),
    ("mov_twos", "mov", ZVUK + ["-c:a", "pcm_s16be"]),
    ("mov_in24", "mov", ZVUK + ["-c:a", "pcm_s24be"]),
    ("mov_fl32", "mov", ZVUK + ["-c:a", "pcm_f32be"]),
    ("mov_prores_s24le", "mov", OBRAZ + ZVUK + ["-c:v", "prores_ks", "-c:a", "pcm_s24le"]),
    # MPEG-TS a M2TS (L4): zvuk vyrezaný po rámcoch (ADTS, AC-3, E-AC-3, MPEG audio, Opus), video po PES
    ("ts_h264_aac", "ts", OBRAZ + ZVUK + ["-c:v", "libx264", "-bf", "2", "-g", "12", "-c:a", "aac"]),
    ("ts_hevc_opus", "ts", OBRAZ + ZVUK + ["-c:v", "libx265", "-x265-params", "log-level=none:keyint=10", "-c:a", "libopus"]),
    ("ts_mpeg2_mp2", "ts", OBRAZ + ZVUK + ["-c:v", "mpeg2video", "-c:a", "mp2"]),
    ("ts_mpeg2_mp3_44k", "ts", OBRAZ + ZVUK44 + ["-c:v", "mpeg2video", "-c:a", "libmp3lame"]),
    ("ts_aac_44k", "ts", ZVUK44 + ["-c:a", "aac"]),
    ("ts_mpeg4_eac3", "ts", OBRAZ + ZVUK + ["-c:v", "mpeg4", "-c:a", "eac3"]),
    ("ts_dva_zvuky", "ts", OBRAZ + ZVUK + ZVUK44 + ["-map", "0", "-map", "1", "-map", "2", "-c:v", "libx264",
                                                   "-c:a:0", "ac3", "-c:a:1", "mp2"]),
    ("m2ts_h264_ac3", "m2ts", OBRAZ + ZVUK + ["-c:v", "libx264", "-c:a", "ac3", "-mpegts_m2ts_mode", "1"]),
    ("ts_vysoky_tok", "ts", ["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=20"] + ZVUK[:3]
     + ["sine=frequency=440:sample_rate=48000:duration=20", "-ac", "2", "-c:v", "libx264", "-preset", "ultrafast",
        "-b:v", "6M", "-c:a", "aac", "-mpegts_flags", "+resend_headers"]),
    ("av1_opus", "mkv", OBRAZ + ZVUK + ["-c:v", "libaom-av1", "-cpu-used", "8", "-g", "10", "-c:a", "libopus"]),
]


def ffmpeg(vystup, argumenty):
    r = subprocess.run(["ffmpeg", "-v", "error", "-y"] + argumenty + [vystup], capture_output=True, text=True)
    return r.returncode == 0, r.stderr.strip()[-300:]


def tok_rozhodcu(cesta, index):
    """MD5 všetkých údajov stopy za sebou (FFmpeg kopíruje pakety bez zmeny do surového výstupu)."""
    r = subprocess.run(["ffmpeg", "-v", "error", "-i", cesta, "-map", f"0:{index}", "-c", "copy", "-f", "data", "-"],
                       capture_output=True, check=True)
    return hashlib.md5(r.stdout).hexdigest(), len(r.stdout)


def rozhodca(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_packets", "-show_streams", "-show_data_hash", "MD5",
                        "-of", "json", cesta], capture_output=True, text=True, check=True)
    j = json.loads(r.stdout)
    # PCM: rozdelenie na pakety norma neurčuje (každé je správne) – porovná sa celý tok stopy
    pcm = {s["index"] for s in j.get("streams", []) if s.get("codec_name", "").startswith("pcm_")}
    pakety = []
    for p in j.get("packets", []):
        pakety.append({
            "stopa": p["stream_index"],
            "pts": p.get("pts"),
            "dts": p.get("dts"),
            "trvanie": p.get("duration"),
            "kluc": "K" in p.get("flags", ""),
            "velkost": int(p["size"]),
            "md5": p.get("data_hash", "").split(":")[-1],
        })
    kodeky = {s["index"]: s.get("codec_name", "") for s in j.get("streams", [])}
    return pakety, pcm, kodeky


def nas(cesta):
    r = subprocess.run([PROGRAM, cesta], capture_output=True, text=True)
    pakety, stopy, citani, toky = [], 0, None, {}
    for riadok in r.stdout.splitlines():
        slova = riadok.split()
        if not slova:
            continue
        pole = dict(s.split("=", 1) for s in slova[1:] if "=" in s)
        if slova[0] == "STOPA":
            stopy += 1
        elif slova[0] == "PAKET":
            cislo = lambda k: None if pole[k] == "-" else int(pole[k])
            pakety.append({"stopa": int(pole["stopa"]), "pts": cislo("pts"), "dts": cislo("dts"),
                           "trvanie": cislo("trvanie"),
                           "kluc": pole["kluc"] == "1", "velkost": int(pole["velkost"]), "md5": pole["md5"],
                           "blok": pole.get("blok"), "zaciatok": pole.get("zaciatok", "")})
        elif slova[0] == "TOK":
            toky[int(pole["index"])] = (pole["md5"], int(pole["bajtov"]))
        elif slova[0] == "VYSLEDOK":
            citani = pole.get("citani")
        elif slova[0] == "CHYBA":
            raise RuntimeError(riadok)
    if r.returncode != 0:
        raise RuntimeError(f"návratový kód {r.returncode}: {r.stderr.strip()}")
    return pakety, stopy, citani, toky


def porovnaj(meno, cesta):
    try:
        ocak, pcm, kodeky = rozhodca(cesta)
        moje, stopy, citani, toky = nas(cesta)
        pcm_zhoda = []
        for i in sorted(pcm):
            md5, n = tok_rozhodcu(cesta, i)
            if toky.get(i) != (md5, n):
                raise RuntimeError(f"tok PCM stopy {i}: náš {toky.get(i)}, rozhodca {(md5, n)}")
            pcm_zhoda.append(f"tok PCM stopy {i} ({n} B) bit po bite")
        ocak = [p for p in ocak if p["stopa"] not in pcm]
        moje = [p for p in moje if p["stopa"] not in pcm]
    except Exception as e:  # noqa: BLE001 – každá chyba je výsledok skúšky
        CHYBY.append(f"{meno}: {e}")
        RIADKY.append(f"| {meno} | – | – | ❌ {e} |")
        return
    nezhody, poznamky = [], []
    if len(ocak) != len(moje):
        nezhody.append(f"paketov {len(moje)}, rozhodca {len(ocak)}")
    # poradie medzi stopami norma neurčuje (my čítame súvisle v poradí súboru, FFmpeg niekedy podľa DTS) –
    # porovnáva sa poradie paketov v rámci každej stopy
    for stopa in sorted({p["stopa"] for p in ocak} | {p["stopa"] for p in moje}):
        a_s = [p for p in moje if p["stopa"] == stopa]
        b_s = [p for p in ocak if p["stopa"] == stopa]
        if len(a_s) != len(b_s):
            nezhody.append(f"stopa {stopa}: paketov {len(a_s)}, rozhodca {len(b_s)}")
            continue
        # stály posun PTS pri rovnakých DTS: FFmpeg pri záporných posunoch zobrazenia (trun v1, ctts v1) posunie
        # všetky PTS, aby PTS ≥ DTS; podľa ISO/IEC 14496-12 je PTS = DTS + posun bez ďalšieho posunu
        rozdiely = {b["pts"] - a["pts"] for a, b in zip(a_s, b_s) if a["pts"] is not None and b["pts"] is not None}
        dts_rovnake = all(a["dts"] is not None and a["dts"] == b["dts"] for a, b in zip(a_s, b_s))
        posun_pts = 0
        if len(rozdiely) == 1 and dts_rovnake and 0 not in rozdiely:
            posun_pts = rozdiely.pop()
            poznamky.append(f"stopa {stopa}: FFmpeg posúva PTS o {posun_pts} (PTS ≥ DTS), my PTS = DTS + posun (ISO/IEC 14496-12)")
        if cesta.endswith((".ts", ".m2ts")) and kodeky.get(stopa) in ("h264", "hevc") and \
                [p["velkost"] for p in a_s] != [p["velkost"] for p in b_s]:
            # zero_byte pred počiatočným kódom: hranice sa smú líšiť len o nulový bajt na začiatku nášho paketu
            md5, n = tok_rozhodcu(cesta, stopa)
            if toky.get(stopa) != (md5, n):
                nezhody.append(f"stopa {stopa}: tok {toky.get(stopa)}, rozhodca {(md5, n)}")
                continue
            z = [1 if p["zaciatok"].startswith("00000001") else 0 for p in a_s] + [0]
            # prvý paket toku si svoj nulový bajt nechá aj u FFmpeg (nie je predošlý paket)
            ok = all(b["velkost"] == a["velkost"] - (z[i] if i else 0) + (z[i + 1] if i + 1 < len(a_s) else 0)
                     for i, (a, b) in enumerate(zip(a_s, b_s)))
            if not ok:
                nezhody.append(f"stopa {stopa}: hranice paketov sa líšia viac ako o zero_byte")
                continue
            posunutych = sum(1 for a, b in zip(a_s, b_s) if a["velkost"] != b["velkost"] or a["md5"] != b["md5"])
            poznamky.append(f"stopa {stopa}: tok {n} B bit po bite, {posunutych} paketov s nulovým bajtom zero_byte "
                            "na začiatku (H.26x príloha B), FFmpeg ho dáva na koniec predošlého")
            a_s = [dict(a, md5=b["md5"], velkost=b["velkost"]) for a, b in zip(a_s, b_s)]
        zaokruhlenych = trvani_zaokruhlenych = trvani_zo_suboru = 0
        predosly_blok = None
        for i, (a, b) in enumerate(zip(a_s, b_s)):
            v_lacingu = a["blok"] is not None and predosly_blok == a["blok"]
            predosly_blok = a["blok"]
            b = dict(b, pts=None if b["pts"] is None else b["pts"] - posun_pts)
            if v_lacingu and a["pts"] is not None and b["pts"] is not None and abs(a["pts"] - b["pts"]) == 1:
                zaokruhlenych += 1
                b["pts"] = a["pts"]
            if v_lacingu and a["dts"] is not None and b["dts"] is not None and abs(a["dts"] - b["dts"]) == 1:
                b["dts"] = a["dts"]
            for k in ("pts", "kluc", "velkost", "md5"):
                if a[k] != b[k]:
                    nezhody.append(f"stopa {stopa} paket {i} {k}: náš {a[k]}, rozhodca {b[k]}")
            if a["dts"] is not None and b["dts"] is not None and a["dts"] != b["dts"]:
                nezhody.append(f"stopa {stopa} paket {i} dts: náš {a['dts']}, rozhodca {b['dts']}")
            if a["trvanie"] is not None and b["trvanie"] is not None and a["trvanie"] != b["trvanie"]:
                dalsi = a_s[i + 1]["pts"] if i + 1 < len(a_s) else None
                if abs(a["trvanie"] - b["trvanie"]) == 1:
                    trvani_zaokruhlenych += 1  # DefaultDuration nie je násobok tiku: my k najbližšiemu, FFmpeg nadol
                elif dalsi is not None and a["pts"] is not None and a["trvanie"] == dalsi - a["pts"]:
                    trvani_zo_suboru += 1  # trvanie zo súboru presne sedí s časom ďalšieho paketu
                elif i + 1 == len(a_s):
                    poznamky.append(f"posledný paket stopy {stopa}: trvanie zo súboru {a['trvanie']}, rámec {b['trvanie']}")
                else:
                    nezhody.append(f"stopa {stopa} paket {i} trvanie: náš {a['trvanie']}, rozhodca {b['trvanie']}")
            if len(nezhody) > 5:
                break
        if zaokruhlenych:
            if cesta.endswith((".ts", ".m2ts")):
                poznamky.append(f"stopa {stopa}: čas {zaokruhlenych} ďalších rámcov v PES ±1 tik (my presne z počtu vzoriek, rozhodca súčtom zaokrúhlených trvaní)")
            else:
                poznamky.append(f"stopa {stopa}: čas {zaokruhlenych} rámcov lacingu ±1 tik (my k najbližšiemu, rozhodca nadol)")
        if trvani_zaokruhlenych:
            poznamky.append(f"stopa {stopa}: trvanie {trvani_zaokruhlenych} paketov ±1 tik (DefaultDuration nie je násobok tiku)")
        if trvani_zo_suboru:
            poznamky.append(f"stopa {stopa}: trvanie {trvani_zo_suboru} paketov zo súboru = odstup k ďalšiemu paketu (FFmpeg: dĺžka rámca)")
    poznamky.extend(pcm_zhoda)
    stav = "✅ bit po bite" if not nezhody else "❌ " + "; ".join(nezhody[:5])
    if poznamky:
        stav += " – " + "; ".join(poznamky)
    if nezhody:
        CHYBY.append(f"{meno}: " + "; ".join(nezhody[:5]))
    RIADKY.append(f"| {meno} | {stopy} | {len(moje)} | {stav} |")


def main():
    mkvmerge = shutil.which("mkvmerge")
    for meno, pripona, argumenty in VARIANTY:
        cesta = os.path.join(PRAC, f"{meno}.{pripona}")
        ok, chyba = ffmpeg(cesta, argumenty)
        if not ok:
            RIADKY.append(f"| {meno} | – | – | ⏭ zapisovač FFmpeg nevie ({chyba}) |")
            continue
        porovnaj(meno, cesta)
        if mkvmerge and pripona == "mkv":
            druhy = os.path.join(PRAC, f"{meno}_mkvmerge.mkv")
            r = subprocess.run([mkvmerge, "-q", "-o", druhy, cesta], capture_output=True, text=True)
            if r.returncode in (0, 1):
                porovnaj(f"{meno} (mkvmerge)", druhy)
    print("| Súbor | Stôp | Paketov | Zhoda s rozhodcom (ffprobe) |")
    print("|---|---|---|---|")
    print("\n".join(RIADKY))
    if CHYBY:
        print("\n**Nezhody:**\n")
        for c in CHYBY:
            print(f"- {c}")
        sys.exit(1)


main()
