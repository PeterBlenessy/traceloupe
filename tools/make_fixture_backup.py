#!/usr/bin/env python3
"""Generate a tiny, valid *encrypted* iOS backup for tests and the iLEAPP spike.

Real backups are tens of GB; this produces a few-KB backup with the same
on-disk cryptographic structure, so it exercises the whole decrypt path
(keybag -> KEK -> class keys -> Manifest.db -> per-file blobs) without the
size. The format is reproduced from the iTunes-backup spec as implemented by
iLEAPP's scripts/search_files.py (the reference we also parse in production).

Structure produced under <out>/:
    Manifest.plist   plaintext: IsEncrypted, BackupKeyBag (keybag), ManifestKey
    Manifest.db      AES-CBC(0-IV) encrypted SQLite listing every file
    Info.plist       device metadata (plaintext, as Finder writes it)
    Status.plist     backup status (plaintext)
    ab/abcdef...     per-file encrypted blobs at <fileID[:2]>/<fileID>

Crypto (per the spec):
    k0  = PBKDF2-HMAC-SHA256(passcode, DPSL, DPIC)
    KEK = PBKDF2-HMAC-SHA1(k0, SALT, ITER, dklen=32)
    class_key   = AES-unwrap(KEK, WPKY)
    manifest_key= AES-unwrap(class_key, ManifestKey[4:])
    file_key    = AES-unwrap(class_key, blob.EncryptionKey[4:])
    plaintext   = AES-CBC-decrypt(key, 0-IV, ciphertext)[:Size]

This script is intentionally dependency-light: stdlib + `cryptography`.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import plistlib
import sqlite3
import struct
import tempfile
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.keywrap import aes_key_wrap

ZERO_IV = b"\x00" * 16

# Protection class used for every file in the fixture. Real backups spread
# files across classes 1-11; the decrypt path is identical, so one suffices.
CLASS_ID = 3
# Iteration counts: real backups use ~10k; kept low here for fast tests.
DPIC = 10_000
ITER = 10_000

# Domain -> the seed backup's files. Cocoa/Core Data epoch is 2001-01-01.
COCOA_EPOCH = datetime(2001, 1, 1, tzinfo=timezone.utc)

# A minimal valid 1x1 PNG, used as a seeded SMS image attachment so the media
# path (_lava_media_items -> extraction_path -> real bytes) is exercised.
TINY_PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d494844520000000100000001080600000"
    "01f15c4890000000d49444154789c62f8cfc0f01f0005000155a2b4e0"
    "0000000049454e44ae426082"
)
# Path of the seeded attachment, relative to the MediaDomain root. sms.db's
# attachment.filename references it as "~/<this>" (~ = private/var/mobile).
ATTACHMENT_REL = "Library/SMS/Attachments/aa/00/salvage-test.png"


def cocoa_ns(dt: datetime) -> int:
    """Seconds since 2001 as nanoseconds (modern iOS message.date encoding)."""
    return int((dt - COCOA_EPOCH).total_seconds() * 1_000_000_000)


def now_naive() -> datetime:
    """plistlib's binary writer requires naive datetimes for CFDate fields."""
    return datetime.now(timezone.utc).replace(tzinfo=None)


def pkcs_pad16(data: bytes) -> bytes:
    """Pad to a 16-byte boundary with zero bytes (backups don't use PKCS#7;
    the real length is recovered from the Size field on decrypt)."""
    if len(data) % 16 == 0:
        return data
    return data + b"\x00" * (16 - len(data) % 16)


def aes_cbc_encrypt(key: bytes, data: bytes) -> bytes:
    enc = Cipher(algorithms.AES(key), modes.CBC(ZERO_IV)).encryptor()
    return enc.update(pkcs_pad16(data)) + enc.finalize()


def tlv(tag: bytes, value: bytes) -> bytes:
    """One keybag entry: 4-byte tag, 4-byte big-endian length, value."""
    assert len(tag) == 4
    return tag + struct.pack(">I", len(value)) + value


def build_keybag(kek_salt: bytes, dpsl: bytes, class_wpky: bytes) -> bytes:
    """Assemble the BackupKeyBag blob the way search_files.py parses it:
    a leading keybag UUID and global params, then one protection class
    (opened by a fresh UUID, carrying CLAS/KTYP/WRAP/WPKY)."""
    kb = b""
    kb += tlv(b"VERS", struct.pack(">I", 3))
    kb += tlv(b"TYPE", struct.pack(">I", 1))  # backup keybag
    kb += tlv(b"UUID", uuid.uuid4().bytes)     # keybag UUID (first UUID)
    kb += tlv(b"HMCK", os.urandom(40))
    kb += tlv(b"WRAP", struct.pack(">I", 1))
    kb += tlv(b"SALT", kek_salt)
    kb += tlv(b"ITER", struct.pack(">I", ITER))
    kb += tlv(b"DPSL", dpsl)
    kb += tlv(b"DPIC", struct.pack(">I", DPIC))
    # A second UUID starts the first protection class record. WRAP=3 marks the
    # class key as wrapped under both the device key and the passcode-derived
    # key (WRAP_DEVICE|WRAP_PASSCODE), matching real encrypted backups.
    kb += tlv(b"UUID", uuid.uuid4().bytes)
    kb += tlv(b"CLAS", struct.pack(">I", CLASS_ID))
    kb += tlv(b"WRAP", struct.pack(">I", 3))
    kb += tlv(b"KTYP", struct.pack(">I", 0))
    kb += tlv(b"WPKY", class_wpky)
    return kb


def seed_sms_db(path: Path) -> None:
    """Create an sms.db with the tables/columns iLEAPP's SMS module queries."""
    con = sqlite3.connect(path)
    con.executescript(
        """
        CREATE TABLE chat (
            ROWID INTEGER PRIMARY KEY,
            chat_identifier TEXT,
            account_login TEXT
        );
        CREATE TABLE message (
            ROWID INTEGER PRIMARY KEY,
            text TEXT,
            service TEXT,
            account TEXT,
            date INTEGER,
            date_read INTEGER,
            is_from_me INTEGER,
            is_sent INTEGER,
            is_delivered INTEGER,
            is_read INTEGER,
            attributedBody BLOB
        );
        CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER);
        CREATE TABLE attachment (
            ROWID INTEGER PRIMARY KEY, transfer_name TEXT, filename TEXT,
            created_date INTEGER, mime_type TEXT, total_bytes INTEGER
        );
        CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER);
        """
    )
    con.execute(
        "INSERT INTO chat (ROWID, chat_identifier, account_login) VALUES (1, '+15551234567', 'e:me@example.com')"
    )
    convo = [
        # (text, is_from_me, minutes_offset)
        ("Hey, are you around this weekend?", 0, 0),
        ("Yeah! What did you have in mind?", 1, 3),
        ("Thinking of hiking Mission Peak", 0, 5),
        ("I'm in. Saturday morning?", 1, 7),
        ("Perfect, I'll pick you up at 8", 0, 9),
    ]
    base = datetime(2024, 6, 8, 10, 0, tzinfo=timezone.utc)
    for rowid, (text, from_me, off) in enumerate(convo, start=1):
        ts = cocoa_ns(base.replace(minute=off))
        con.execute(
            """INSERT INTO message
               (ROWID, text, service, account, date, date_read,
                is_from_me, is_sent, is_delivered, is_read)
               VALUES (?, ?, 'iMessage', 'me@example.com', ?, ?, ?, ?, 1, 1)""",
            (rowid, text, ts, ts, from_me, from_me),
        )
        con.execute(
            "INSERT INTO chat_message_join (chat_id, message_id) VALUES (1, ?)",
            (rowid,),
        )

    # A 6th message carrying an image attachment, to exercise the media path.
    att_rowid = len(convo) + 1
    ts = cocoa_ns(base.replace(minute=11))
    # Caption text (rather than NULL) so iLEAPP's chat renderer doesn't choke
    # on a NaN when building the HTML report; media check-in is driven by the
    # attachment row regardless of message text.
    con.execute(
        """INSERT INTO message
           (ROWID, text, service, account, date, date_read,
            is_from_me, is_sent, is_delivered, is_read)
           VALUES (?, 'Here''s the trailhead 📷', 'iMessage', 'me@example.com', ?, ?, 1, 1, 1, 1)""",
        (att_rowid, ts, ts),
    )
    con.execute("INSERT INTO chat_message_join (chat_id, message_id) VALUES (1, ?)", (att_rowid,))
    con.execute(
        """INSERT INTO attachment
           (ROWID, transfer_name, filename, created_date, mime_type, total_bytes)
           VALUES (1, 'salvage-test.png', ?, ?, 'image/png', ?)""",
        (f"~/{ATTACHMENT_REL}", ts, len(TINY_PNG)),
    )
    con.execute("INSERT INTO message_attachment_join (message_id, attachment_id) VALUES (?, 1)", (att_rowid,))
    con.commit()
    con.close()


def cocoa_s(dt: datetime) -> float:
    """Seconds since 2001 (Core Data / CFAbsoluteTime encoding)."""
    return (dt - COCOA_EPOCH).total_seconds()


def seed_safari_db(path: Path) -> None:
    """Safari History.db with the tables iLEAPP's safariHistory module queries."""
    con = sqlite3.connect(path)
    con.executescript(
        """
        CREATE TABLE history_items (id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER);
        CREATE TABLE history_visits (
            id INTEGER PRIMARY KEY, history_item INTEGER, visit_time REAL, title TEXT,
            redirect_source INTEGER, redirect_destination INTEGER, origin INTEGER
        );
        """
    )
    base = datetime(2024, 6, 7, 20, 0, tzinfo=timezone.utc)
    visits = [
        ("https://www.apple.com/", "Apple", 12, 0),
        ("https://news.ycombinator.com/", "Hacker News", 34, 0),
        ("https://en.wikipedia.org/wiki/Mission_Peak", "Mission Peak - Wikipedia", 2, 1),
    ]
    for i, (url, title, count, origin) in enumerate(visits, start=1):
        con.execute("INSERT INTO history_items (id, url, visit_count) VALUES (?, ?, ?)", (i, url, count))
        con.execute(
            """INSERT INTO history_visits (id, history_item, visit_time, title, origin)
               VALUES (?, ?, ?, ?, ?)""",
            (i, i, cocoa_s(base) + i * 3600, title, origin),
        )
    con.commit()
    con.close()


def seed_callhistory_db(path: Path) -> None:
    """CallHistory.storedata (Core Data) with the ZCALLRECORD columns iLEAPP reads."""
    con = sqlite3.connect(path)
    con.execute(
        """CREATE TABLE ZCALLRECORD (
            Z_PK INTEGER PRIMARY KEY, ZDATE REAL, ZDURATION REAL,
            ZSERVICE_PROVIDER TEXT, ZCALLTYPE INTEGER, ZORIGINATED INTEGER,
            ZADDRESS BLOB, ZANSWERED INTEGER, ZFACE_TIME_DATA BLOB,
            ZDISCONNECTED_CAUSE INTEGER, ZISO_COUNTRY_CODE TEXT, ZLOCATION TEXT
        )"""
    )
    base = datetime(2024, 6, 7, 18, 0, tzinfo=timezone.utc)
    # (address, calltype, originated, answered, duration_s, minutes_offset)
    calls = [
        (b"+15551234567", 1, 1, 1, 312.0, 0),   # outgoing phone, answered
        (b"+15559876543", 1, 0, 0, 0.0, 30),     # incoming phone, missed
        (b"friend@icloud.com", 16, 0, 1, 128.0, 60),  # incoming FaceTime audio
    ]
    for pk, (addr, ctype, orig, ans, dur, off) in enumerate(calls, start=1):
        con.execute(
            """INSERT INTO ZCALLRECORD
               (Z_PK, ZDATE, ZDURATION, ZSERVICE_PROVIDER, ZCALLTYPE, ZORIGINATED,
                ZADDRESS, ZANSWERED, ZFACE_TIME_DATA, ZDISCONNECTED_CAUSE,
                ZISO_COUNTRY_CODE, ZLOCATION)
               VALUES (?, ?, ?, 'com.apple.Telephony', ?, ?, ?, ?, NULL, 0, 'us', NULL)""",
            (pk, cocoa_s(base + timedelta(minutes=off)), dur, ctype, orig, addr, ans),
        )
    con.commit()
    con.close()


# domain, relativePath, seeder(fn writing plaintext bytes to a temp path)
def seed_files(workdir: Path) -> list[tuple[str, str, bytes]]:
    """Return (domain, relativePath, plaintext_bytes) for each backed-up file."""
    sms_path = workdir / "sms.db"
    seed_sms_db(sms_path)
    safari_path = workdir / "History.db"
    seed_safari_db(safari_path)
    calls_path = workdir / "CallHistory.storedata"
    seed_callhistory_db(calls_path)
    return [
        ("HomeDomain", "Library/SMS/sms.db", sms_path.read_bytes()),
        ("MediaDomain", ATTACHMENT_REL, TINY_PNG),
        ("HomeDomain", "Library/Safari/History.db", safari_path.read_bytes()),
        ("HomeDomain", "Library/CallHistoryDB/CallHistory.storedata", calls_path.read_bytes()),
    ]


def build_manifest_db(path: Path, files: list[tuple[str, str, str]]) -> None:
    """Create the Manifest.db SQLite the backup indexes files with.

    `files` is (fileID, domain, relativePath, file_blob) — file_blob is the
    per-file metadata plist iLEAPP reads EncryptionKey/Size from.
    """
    con = sqlite3.connect(path)
    con.execute(
        """CREATE TABLE Files (
            fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT,
            flags INTEGER, file BLOB
        )"""
    )
    for file_id, domain, rel, blob in files:
        con.execute(
            "INSERT INTO Files (fileID, domain, relativePath, flags, file) VALUES (?, ?, ?, 1, ?)",
            (file_id, domain, rel, blob),
        )
    con.commit()
    con.close()


def make_backup(out: Path, passcode: str) -> None:
    out.mkdir(parents=True, exist_ok=True)

    # 1. Derive the KEK from the passcode (two-stage PBKDF2, per spec).
    kek_salt = os.urandom(20)
    dpsl = os.urandom(32)
    k0 = hashlib.pbkdf2_hmac("sha256", passcode.encode(), dpsl, DPIC)
    kek = hashlib.pbkdf2_hmac("sha1", k0, kek_salt, ITER, dklen=32)

    # 2. A class key, wrapped under the KEK -> WPKY in the keybag.
    class_key = os.urandom(32)
    class_wpky = aes_key_wrap(kek, class_key)
    keybag = build_keybag(kek_salt, dpsl, class_wpky)

    # 3. Manifest key, wrapped under the class key.
    manifest_key = os.urandom(32)
    manifest_wrapped = aes_key_wrap(class_key, manifest_key)
    manifest_key_field = struct.pack("<I", CLASS_ID) + manifest_wrapped

    with tempfile.TemporaryDirectory() as td:
        workdir = Path(td)

        # 4. Encrypt each file blob; build its Manifest.db metadata plist.
        manifest_rows: list[tuple[str, str, str, bytes]] = []
        for domain, rel, plaintext in seed_files(workdir):
            file_id = hashlib.sha1(f"{domain}-{rel}".encode()).hexdigest()
            file_key = os.urandom(32)
            file_wrapped = aes_key_wrap(class_key, file_key)
            enc_key_field = struct.pack("<I", CLASS_ID) + file_wrapped
            ciphertext = aes_cbc_encrypt(file_key, plaintext)

            blob_dir = out / file_id[:2]
            blob_dir.mkdir(exist_ok=True)
            (blob_dir / file_id).write_bytes(ciphertext)

            # iLEAPP reads file["EncryptionKey"]["NS.data"] and file["Size"].
            file_blob = plistlib.dumps(
                {
                    "EncryptionKey": {"NS.data": enc_key_field},
                    "Size": len(plaintext),
                    "Birth": 0,
                    "LastModified": 0,
                },
                fmt=plistlib.FMT_BINARY,
            )
            manifest_rows.append((file_id, domain, rel, file_blob))

        # 5. Build + encrypt Manifest.db (SQLite size is a multiple of 512,
        #    hence of 16, so CBC needs no padding and decrypts cleanly).
        manifest_plain = workdir / "Manifest.db"
        build_manifest_db(manifest_plain, manifest_rows)
        manifest_ct = aes_cbc_encrypt(manifest_key, manifest_plain.read_bytes())
        (out / "Manifest.db").write_bytes(manifest_ct)

    # 6. Manifest.plist (plaintext) carries the keybag and manifest key.
    plistlib.dump(
        {
            "Version": "10.0",
            "Date": now_naive(),
            "SystemDomainsVersion": "20.0",
            "IsEncrypted": True,
            "WasPasscodeSet": True,
            "ManifestKey": manifest_key_field,
            "BackupKeyBag": keybag,
            "Lockdown": {
                "ProductType": "iPhone14,2",
                "ProductVersion": "17.5.1",
                "DeviceName": "Fixture iPhone",
                "SerialNumber": "F2LFIXTURE01",
            },
        },
        (out / "Manifest.plist").open("wb"),
        fmt=plistlib.FMT_BINARY,
    )

    # 7. Info.plist / Status.plist (plaintext), as Finder writes them.
    plistlib.dump(
        {
            "Device Name": "Fixture iPhone",
            "Display Name": "Fixture iPhone",
            "Product Type": "iPhone14,2",
            "Product Version": "17.5.1",
            "Serial Number": "F2LFIXTURE01",
            "Last Backup Date": now_naive(),
            "IMEI": "000000000000000",
        },
        (out / "Info.plist").open("wb"),
        fmt=plistlib.FMT_XML,
    )
    plistlib.dump(
        {
            "IsFullBackup": True,
            "Version": "3.3",
            "BackupState": "new",
            "Date": now_naive(),
            "SnapshotState": "finished",
        },
        (out / "Status.plist").open("wb"),
        fmt=plistlib.FMT_BINARY,
    )


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("out", type=Path, help="output backup directory")
    ap.add_argument("--password", default="salvage-test", help="backup password")
    args = ap.parse_args()
    make_backup(args.out, args.password)
    n = sum(1 for _ in args.out.rglob("*") if _.is_file())
    print(f"Wrote encrypted backup to {args.out} ({n} files), password: {args.password!r}")


if __name__ == "__main__":
    main()
