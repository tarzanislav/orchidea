#!/usr/bin/env python3
"""Skúšobné certifikáty pre overenie servera HTTPS (siet::x509, A12 krok 4).

Vytvára ich NEZÁVISLÁ knižnica (Python `cryptography` / OpenSSL) – rozhodca, nie súčasť ORCHIDEA OS. Výsledok sú
súbory DER v `siet/skusky/` (len certifikáty – súkromné kľúče sa neukladajú; živá skúška proti `openssl s_server`
si reťaz vytvorí sama).

Spustenie: python3 -I nastroje/skusobne_certifikaty.py   (platnosť 1. 1. 2026 – 1. 1. 2036; skúšky používajú
čas 28. 11. 2026)
"""

import datetime
import ipaddress
import pathlib

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, padding, rsa
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

CIEL = pathlib.Path(__file__).resolve().parent.parent / "siet" / "skusky"
OD = datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)
DO = datetime.datetime(2036, 1, 1, tzinfo=datetime.timezone.utc)


def meno(cn):
    return x509.Name([x509.NameAttribute(NameOID.ORGANIZATION_NAME, "ORCHIDEA skúška"),
                      x509.NameAttribute(NameOID.COMMON_NAME, cn)])


def cert(subjekt, kluc, vydavatel, kluc_vydavatela, *, ca=None, eku=None, san=None, od=OD, do=DO,
         hash_=hashes.SHA256(), pss=False, ku=None, obmedzenia=None, kriticke_nezname=False):
    b = (x509.CertificateBuilder().subject_name(meno(subjekt)).issuer_name(meno(vydavatel))
         .public_key(kluc.public_key()).serial_number(x509.random_serial_number())
         .not_valid_before(od).not_valid_after(do))
    if ca is not None:
        b = b.add_extension(x509.BasicConstraints(ca=True, path_length=ca), critical=True)
        b = b.add_extension(x509.KeyUsage(digital_signature=False, content_commitment=False, key_encipherment=False,
                                          data_encipherment=False, key_agreement=False, key_cert_sign=True,
                                          crl_sign=True, encipher_only=False, decipher_only=False), critical=True)
    else:
        b = b.add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        b = b.add_extension(ku or x509.KeyUsage(digital_signature=True, content_commitment=False,
                                                key_encipherment=False, data_encipherment=False,
                                                key_agreement=False, key_cert_sign=False, crl_sign=False,
                                                encipher_only=False, decipher_only=False), critical=True)
    if eku:
        b = b.add_extension(x509.ExtendedKeyUsage(eku), critical=False)
    if san:
        mena = [x509.IPAddress(ipaddress.ip_address(m)) if m[0].isdigit() else x509.DNSName(m) for m in san]
        b = b.add_extension(x509.SubjectAlternativeName(mena), critical=False)
    if obmedzenia:
        b = b.add_extension(x509.NameConstraints(permitted_subtrees=[x509.DNSName(obmedzenia)],
                                                 excluded_subtrees=None), critical=True)
    if kriticke_nezname:
        b = b.add_extension(x509.UnrecognizedExtension(x509.ObjectIdentifier("1.3.6.1.4.1.99999.1"), b"\x05\x00"),
                            critical=True)
    if pss:
        return b.sign(kluc_vydavatela, hash_, rsa_padding=padding.PSS(mgf=padding.MGF1(hash_),
                                                                      salt_length=hash_.digest_size))
    return b.sign(kluc_vydavatela, hash_)


def uloz(nazov, c):
    (CIEL / f"{nazov}.der").write_bytes(c.public_bytes(serialization.Encoding.DER))


def main():
    CIEL.mkdir(parents=True, exist_ok=True)
    server = [ExtendedKeyUsageOID.SERVER_AUTH]
    koren_k = ec.generate_private_key(ec.SECP384R1())
    koren = cert("Koren", koren_k, "Koren", koren_k, ca=1, hash_=hashes.SHA384())
    medzi_k = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    medzi = cert("Medzi", medzi_k, "Koren", koren_k, ca=0, eku=server, hash_=hashes.SHA384())
    list_k = ec.generate_private_key(ec.SECP256R1())
    list_ = cert("skuska.orchidea", list_k, "Medzi", medzi_k, eku=server,
                 san=["skuska.orchidea", "*.web.orchidea", "127.0.0.1"])
    rsa_k = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    list_pss = cert("pss.orchidea", rsa_k, "Medzi", medzi_k, eku=server, san=["pss.orchidea"], pss=True)
    stary = cert("stary.orchidea", list_k, "Medzi", medzi_k, eku=server, san=["stary.orchidea"],
                 od=datetime.datetime(2020, 1, 1, tzinfo=datetime.timezone.utc),
                 do=datetime.datetime(2025, 1, 1, tzinfo=datetime.timezone.utc))
    klient = cert("klient.orchidea", list_k, "Medzi", medzi_k, eku=[ExtendedKeyUsageOID.CLIENT_AUTH],
                  san=["klient.orchidea"])
    # list podpísaný iným listom (nie autoritou)
    falosny = cert("falosny.orchidea", list_k, "skuska.orchidea", list_k, eku=server, san=["falosny.orchidea"])
    # autorita s obmedzením mien na povolene.orchidea a list mimo neho
    obm_k = ec.generate_private_key(ec.SECP256R1())
    obm = cert("Obmedzena", obm_k, "Koren", koren_k, ca=0, eku=server, obmedzenia="povolene.orchidea",
               hash_=hashes.SHA384())
    v_obm = cert("a.povolene.orchidea", list_k, "Obmedzena", obm_k, eku=server, san=["a.povolene.orchidea"])
    mimo_obm = cert("zakazane.orchidea", list_k, "Obmedzena", obm_k, eku=server, san=["zakazane.orchidea"])
    nezname = cert("nezname.orchidea", list_k, "Medzi", medzi_k, eku=server, san=["nezname.orchidea"],
                   kriticke_nezname=True)
    cudzi_k = ec.generate_private_key(ec.SECP256R1())
    cudzi = cert("Koren", cudzi_k, "Koren", cudzi_k, ca=1)  # rovnaké meno ako koreň, iný kľúč
    cudzi_list = cert("skuska.orchidea", list_k, "Koren", cudzi_k, eku=server, san=["skuska.orchidea"])
    for nazov, c in [("koren", koren), ("medzi", medzi), ("list", list_), ("list-pss", list_pss), ("stary", stary),
                     ("klient", klient), ("falosny", falosny), ("obmedzena", obm), ("v-obmedzeni", v_obm),
                     ("mimo-obmedzenia", mimo_obm), ("nezname", nezname), ("cudzi-koren", cudzi),
                     ("cudzi-list", cudzi_list)]:
        uloz(nazov, c)


if __name__ == "__main__":
    main()
