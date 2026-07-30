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
import base64
import hashlib
import os
import plistlib
import sqlite3
import struct
import tempfile
import uuid
import zlib
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

def solid_png(width: int, height: int, rgb: tuple[int, int, int]) -> bytes:
    """Encode a solid-color RGB PNG using only stdlib (zlib). Keeps the fixture
    dependency-light while producing visible images for the gallery."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)  # 8-bit RGB
    row = b"\x00" + bytes(rgb) * width  # filter byte 0 + pixels
    raw = row * height
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


# A real 400x300 HEIC (the format most iOS photos use), so the gallery's
# native HEIC->JPEG thumbnailing is exercised. Produced once with macOS `sips`
# and embedded here to keep this generator dependency-free.
HEIC_PHOTO = base64.b64decode(
    "AAAAJGZ0eXBoZWljAAAAAG1pZjFNaVBybWlhZk1pSEJoZWljAAABhW1ldGEAAAAAAAAAIWhkbHIA"
    "AAAAAAAAAHBpY3QAAAAAAAAAAAAAAAAAAAAAJGRpbmYAAAAcZHJlZgAAAAAAAAABAAAADHVybCAA"
    "AAABAAAADnBpdG0AAAAAAAEAAAAjaWluZgAAAAAAAQAAABVpbmZlAgAAAAABAABodmMxAAAAAOVp"
    "cHJwAAAAxGlwY28AAAATY29scm5jbHgAAgACAAaAAAAADGNsbGkAywBAAAAAFGlzcGUAAAAAAAAB"
    "kAAAASwAAAAJaXJvdAAAAAAQcGl4aQAAAAADCAgIAAAAcGh2Y0MBA3AAAACwAAAAAAA88AD8/fj4"
    "AAALA6AAAQAXQAEMAf//A3AAAAMAsAAAAwAAAwA8cCShAAEAIkIBAQNwAAADALAAAAMAAAMAPKAM"
    "iATH3iHuRZVNwICBgCCiAAEACUQBwGFyyEBTJAAAABlpcG1hAAAAAAAAAAEAAQaBAgMFhoQAAAAe"
    "aWxvYwAAAABEAAABAAEAAAABAAABuQAAAI8AAAABbWRhdAAAAAAAAACfAAAAiygBr4ot3MWpSkog"
    "R6Ccf/r3Xe/QleGNl7N8Yu5ll5Ob7uQcPm3F/OIL5+73g/zQAOqAIrDE7BmDUxd4AABBQArDa+Hc"
    "QYAAAAMDYgLV4LCAAAADAAf4BDoAAAMAAAMAB8wAAAMAAAMAAAMCsgAAAwAAAwAAmIAAAAMAAAMA"
    "DsgAAAMAAAMAAAMAICA="
)

# A few visible photos for the gallery, seeded as message attachments so they
# flow through iLEAPP's media check-in into _lava_media_items. Mixed formats
# including HEIC, matching a real camera roll.
GALLERY_PHOTOS = [
    ("Library/SMS/Attachments/aa/00/traceloupe-test.png", "image/png", solid_png(64, 64, (74, 144, 226))),
    ("Library/SMS/Attachments/bb/01/sunset.png", "image/png", solid_png(96, 64, (240, 130, 60))),
    ("Library/SMS/Attachments/cc/02/forest.png", "image/png", solid_png(64, 96, (60, 160, 90))),
    ("Library/SMS/Attachments/dd/03/IMG_0421.heic", "image/heic", HEIC_PHOTO),
]


# A camera-roll asset in CameraRollDomain (not a message attachment), with its
# pre-rendered V2 thumbnail and a Photos.sqlite row — what the native encrypted
# camera-roll reader enumerates. Kept small; the decrypt path is the point.
CAMERA_ROLL_DCIM = ("Media/DCIM/100APPLE/IMG_0001.HEIC", HEIC_PHOTO)
CAMERA_ROLL_THUMB = (
    "Media/PhotoData/Thumbnails/V2/DCIM/100APPLE/IMG_0001.HEIC/5005.JPG",
    solid_png(80, 60, (200, 50, 50)),
)
# ZDATECREATED is a Core Data timestamp (seconds since 2001); 700000000 + the
# 978307200 epoch offset = 1678307200 Unix, which the reader must recover.
CAMERA_ROLL_DATE_COCOA = 700_000_000.0


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

    # Messages carrying image attachments, to exercise the media path and give
    # the gallery several photos. Caption text (rather than NULL) so iLEAPP's
    # chat renderer doesn't choke on a NaN; media check-in is driven by the
    # attachment row regardless of message text.
    captions = [
        "Here's the trailhead 📷",
        "Sunset from the summit 🌅",
        "Into the woods 🌲",
        "Straight off the camera 📸",
    ]
    for i, (rel, mime, blob) in enumerate(GALLERY_PHOTOS):
        att_rowid = len(convo) + 1 + i
        ts = cocoa_ns(base.replace(minute=11 + i))
        name = rel.rsplit("/", 1)[-1]
        con.execute(
            """INSERT INTO message
               (ROWID, text, service, account, date, date_read,
                is_from_me, is_sent, is_delivered, is_read)
               VALUES (?, ?, 'iMessage', 'me@example.com', ?, ?, 1, 1, 1, 1)""",
            (att_rowid, captions[i], ts, ts),
        )
        con.execute("INSERT INTO chat_message_join (chat_id, message_id) VALUES (1, ?)", (att_rowid,))
        con.execute(
            """INSERT INTO attachment
               (ROWID, transfer_name, filename, created_date, mime_type, total_bytes)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (i + 1, name, f"~/{rel}", ts, mime, len(blob)),
        )
        con.execute(
            "INSERT INTO message_attachment_join (message_id, attachment_id) VALUES (?, ?)",
            (att_rowid, i + 1),
        )
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


def seed_addressbook_db(path: Path) -> None:
    """AddressBook.sqlitedb with the ABPerson + ABMultiValue schema our native
    contacts parser reads. iLEAPP's own addressBook lava output is lossy
    (drops names/emails), so we parse this decrypted file directly."""
    con = sqlite3.connect(path)
    con.executescript(
        """
        CREATE TABLE ABPerson (
            ROWID INTEGER PRIMARY KEY, First TEXT, Last TEXT, Middle TEXT,
            Organization TEXT, Department TEXT, JobTitle TEXT, Nickname TEXT,
            Note TEXT, Prefix TEXT, Suffix TEXT, CreationDate REAL, ModificationDate REAL
        );
        CREATE TABLE ABMultiValueLabel (value TEXT);
        CREATE TABLE ABMultiValue (
            UID INTEGER PRIMARY KEY, record_id INTEGER, property INTEGER,
            identifier INTEGER, label INTEGER, value TEXT, guid TEXT
        );
        """
    )
    # iOS stores labels as magic strings; the parser strips the wrapper.
    labels = ["_$!<Mobile>!$_", "_$!<Home>!$_", "_$!<Work>!$_", "_$!<iPhone>!$_"]
    for i, v in enumerate(labels, start=1):
        con.execute("INSERT INTO ABMultiValueLabel (rowid, value) VALUES (?, ?)", (i, v))

    people = [
        # (first, last, org, [(prop, label_idx, value)])
        ("Alex", "Rivera", None, [(3, 1, "+15551234567"), (4, 2, "alex@example.com")]),
        ("Jordan", "Kim", "Acme Corp", [(3, 3, "+15559876543"), (4, 3, "jordan@acme.example")]),
        (None, None, "Bella Vista Pizza", [(3, 1, "+15550001111")]),
        ("Sam", "Taylor", None, [(4, 2, "sam.taylor@example.com")]),
    ]
    base = datetime(2023, 1, 1, tzinfo=timezone.utc)
    uid = 1
    for pk, (first, last, org, values) in enumerate(people, start=1):
        con.execute(
            """INSERT INTO ABPerson (ROWID, First, Last, Organization, CreationDate, ModificationDate)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (pk, first, last, org, cocoa_s(base), cocoa_s(base)),
        )
        for prop, label_idx, value in values:
            con.execute(
                """INSERT INTO ABMultiValue (UID, record_id, property, label, value)
                   VALUES (?, ?, ?, ?, ?)""",
                (uid, pk, prop, label_idx, value),
            )
            uid += 1
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


def seed_photos_sqlite(path: Path) -> None:
    """Photos.sqlite with the ZASSET columns the native camera-roll reader joins
    on (ZDIRECTORY/ZFILENAME → capture date, trashed flag)."""
    con = sqlite3.connect(path)
    con.execute(
        "CREATE TABLE ZASSET (ZDIRECTORY TEXT, ZFILENAME TEXT, ZDATECREATED REAL, ZTRASHEDSTATE INTEGER)"
    )
    con.execute(
        "INSERT INTO ZASSET VALUES ('DCIM/100APPLE', 'IMG_0001.HEIC', ?, 0)",
        (CAMERA_ROLL_DATE_COCOA,),
    )
    con.commit()
    con.close()


def seed_tcc_db(path: Path) -> None:
    """TCC.db — Apple's Transparency, Consent and Control store: which apps were
    granted the camera, microphone, photos and so on.

    Modelled on the MODERN schema (`auth_value`, 0 denied / 2 allowed /
    3 limited). The artifact module carries a second query for older devices
    that have `allowed` instead; see crates/traceloupe-core/modules/tcc.toml.

    Deliberately includes a denial and an unrecognised auth_value, so the
    module's CASE is exercised past its happy path.
    """
    con = sqlite3.connect(path)
    con.execute(
        "CREATE TABLE access ("
        "  service TEXT, client TEXT, client_type INTEGER,"
        "  auth_value INTEGER, auth_reason INTEGER, auth_version INTEGER,"
        "  last_modified INTEGER)"
    )
    rows = [
        ("kTCCServiceCamera", "com.example.chatapp", 0, 2, 2, 1, 1_700_000_000),
        ("kTCCServiceMicrophone", "com.example.chatapp", 0, 2, 2, 1, 1_700_000_100),
        ("kTCCServicePhotos", "com.example.chatapp", 0, 3, 2, 1, 1_700_000_200),
        ("kTCCServiceLocation", "com.example.weather", 0, 0, 2, 1, 1_700_000_300),
        ("kTCCServiceAddressBook", "com.example.social", 0, 2, 2, 1, 1_700_000_400),
        # An auth_value the CASE does not name, so the "Unknown (n)" arm is real.
        ("kTCCServiceReminders", "com.example.todo", 0, 9, 2, 1, 1_700_000_500),
    ]
    con.executemany(
        "INSERT INTO access (service, client, client_type, auth_value,"
        " auth_reason, auth_version, last_modified) VALUES (?,?,?,?,?,?,?)",
        rows,
    )
    con.commit()
    con.close()


def seed_accounts3(path: Path) -> None:
    """Accounts3.sqlite — accountsd's register of every service signed in on the
    device. A Core Data store, hence the Z-prefixed names.

    Includes the cases the module claims to handle: an account whose ZACCOUNTTYPE
    row is MISSING (the module LEFT JOINs, so it survives; iLEAPP's inner join
    would drop it and the count would fall silently), a NULL username, and a NULL
    ZACTIVE. See crates/traceloupe-core/modules/accounts.toml.

    ZDATE is Cocoa/Core Data seconds (since 2001-01-01), not Unix — the module
    declares `epoch = "cocoa"`. 726000000 is 2024-01-03.
    """
    con = sqlite3.connect(path)
    con.executescript(
        """CREATE TABLE ZACCOUNT (
             Z_PK INTEGER PRIMARY KEY, ZACTIVE INTEGER, ZAUTHENTICATED INTEGER,
             ZACCOUNTTYPE INTEGER, ZPARENTACCOUNT INTEGER, ZDATE TIMESTAMP,
             ZACCOUNTDESCRIPTION VARCHAR, ZIDENTIFIER VARCHAR,
             ZOWNINGBUNDLEID VARCHAR, ZUSERNAME VARCHAR);
           CREATE TABLE ZACCOUNTTYPE (
             Z_PK INTEGER PRIMARY KEY, ZACCOUNTTYPEDESCRIPTION VARCHAR,
             ZIDENTIFIER VARCHAR, ZOWNINGBUNDLEID VARCHAR);"""
    )
    con.executemany(
        "INSERT INTO ZACCOUNTTYPE (Z_PK, ZACCOUNTTYPEDESCRIPTION, ZIDENTIFIER)"
        " VALUES (?,?,?)",
        [
            (1, "Gmail", "com.apple.account.Google"),
            (2, "Holiday Calendar", "com.apple.account.HolidayCalendar"),
            # No description: the module's middle COALESCE rung must fall to the
            # type's own reverse-DNS identifier.
            (3, None, "com.apple.account.undescribed"),
        ],
    )
    # ZIDENTIFIER holds GUIDs, as on a real device. The module must never print one
    # as a service name; a friendly string here would hide that.
    con.executemany(
        "INSERT INTO ZACCOUNT (Z_PK, ZACTIVE, ZAUTHENTICATED, ZACCOUNTTYPE,"
        " ZPARENTACCOUNT, ZDATE, ZACCOUNTDESCRIPTION, ZIDENTIFIER,"
        " ZOWNINGBUNDLEID, ZUSERNAME) VALUES (?,?,?,?,?,?,?,?,?,?)",
        [
            (1, 1, 1, 1, None, 726_000_000, "Gmail",
             "6D60660E-344F-4E62-97A0-0A9EA8174CDE", "com.apple.mobilemail", "person@example.com"),
            (2, 1, 1, 2, None, 725_000_000, "US Holidays",
             "AD041785-D028-495F-9008-62F26C114CBA", "dataaccessd", None),
            # No ZACCOUNTTYPE row: only the LEFT JOIN keeps this one.
            (3, 0, 0, None, None, 724_000_000, None,
             "B61380AE-7269-4769-A39F-69D7935848EA", "appstored", "local"),
            (4, None, None, 1, None, 723_000_000, "Unrecorded",
             "C9FA6B49-5667-4CE7-A88A-60C0543E82B5", "accountsd", None),
            # A CHILD of account 1 — what makes one sign-in look like duplicates.
            (5, 1, 1, 3, 1, 722_000_000, None,
             "0EE306D8-66AF-47E5-8FD1-CF2EAF5DC8C2", "accountsd", None),
        ],
    )
    con.commit()
    con.close()


def seed_bluetooth_paired(path: Path) -> None:
    """com.apple.MobileBluetooth.ledevices.paired.db — bluetoothd's register of
    completed LE pairings.

    `LastSeenTime` / `LastConnectionTime` are device-relative counters, NOT any
    epoch — iLEAPP passes them through raw too. The module declares them as
    integers on purpose; see crates/traceloupe-core/modules/bluetooth_paired.toml.

    One row advertises a Random address that resolves to a different Public one,
    which is the pair the module exists to show.
    """
    con = sqlite3.connect(path)
    con.execute(
        "CREATE TABLE PairedDevices(Uuid TEXT, Name TEXT, NameOrigin INT,"
        " Address TEXT, ResolvedAddress TEXT, LastSeenTime INT,"
        " LastConnectionTime INT, GATTServiceChangeConfig INT, Tags TEXT,"
        " iCloudIdentifier TEXT)"
    )
    con.executemany(
        "INSERT INTO PairedDevices (Uuid, Name, NameOrigin, Address,"
        " ResolvedAddress, LastSeenTime, LastConnectionTime, iCloudIdentifier)"
        " VALUES (?,?,?,?,?,?,?,?)",
        [
            ("E3B37CA8-1AA5-AD44-B0FE-A617BB09B64A", "Fitness Band", 2,
             "Public B4:C2:6A:7F:D3:7A", "Public B4:C2:6A:7F:D3:7A", 395_626, 2_143, ""),
            ("6C0C35A0-84CE-3572-2E72-4CF3D03BD1AF", "Example Watch", 2,
             "Random 50:32:66:45:35:EF", "Public F8:6F:C1:4E:FF:6A", 4_315_986, 9_639, ""),
            ("C4E4E254-6060-26CA-7C80-EE01F3C5C346", "Nameless Tag", 2,
             "Random E8:F0:58:00:C0:FB", None, 748_458, 3_662, None),
        ],
    )
    con.commit()
    con.close()


def seed_data_usage(path: Path) -> None:
    """DataUsage.sqlite — per-app network usage, the store behind Settings ›
    Cellular › Cellular Data.

    Modern lineage (Wi-Fi and WWAN both present); the module carries a WWAN-only
    fallback for older devices, as iLEAPP does. See
    crates/traceloupe-core/modules/data_usage.toml.

    Includes the ROLLUP row: no bundle id, and a total equal to the sum of every
    other row. The module must exclude it, and iLEAPP's `ZKIND != 257` constant
    would not — on a real iOS 17 device the rollup is ZKIND 255.
    """
    con = sqlite3.connect(path)
    con.executescript(
        """CREATE TABLE ZLIVEUSAGE (
             Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER, ZHASPROCESS INTEGER,
             ZTIMESTAMP TIMESTAMP, ZWIFIIN FLOAT, ZWIFIOUT FLOAT,
             ZWWANIN FLOAT, ZWWANOUT FLOAT);
           CREATE TABLE ZPROCESS (
             Z_PK INTEGER PRIMARY KEY, ZFIRSTTIMESTAMP TIMESTAMP,
             ZTIMESTAMP TIMESTAMP, ZBUNDLENAME VARCHAR, ZPROCNAME VARCHAR,
             ZEXTENSIONNAME VARCHAR);"""
    )
    con.executemany(
        "INSERT INTO ZPROCESS (Z_PK, ZBUNDLENAME, ZPROCNAME) VALUES (?,?,?)",
        [
            (1, "com.example.chatapp", "ChatApp/com.example.chatapp"),
            (2, "com.example.photos", "nsurlsessiond/com.example.photos"),
            (3, "com.example.plain", "plainproc"),
            (4, None, "CumulativeUsageTracker"),
        ],
    )
    con.executemany(
        "INSERT INTO ZLIVEUSAGE (Z_PK, ZKIND, ZHASPROCESS, ZTIMESTAMP, ZWIFIIN,"
        " ZWIFIOUT, ZWWANIN, ZWWANOUT) VALUES (?,?,?,?,?,?,?,?)",
        [
            (1, 0, 1, 726_000_000, 1000, 2000, 3000, 4000),
            (2, 0, 1, 726_001_000, 500, 600, 700, 800),
            (3, 0, 2, 725_000_000, 0, 0, 900_000, 10_000),
            (4, 0, 3, 724_000_000, 10, 20, 30, 40),
            (5, 255, 4, 726_002_000, 1510, 2620, 903_730, 14_840),
        ],
    )
    con.commit()
    con.close()


def seed_cellular_usage(path: Path) -> None:
    """CellularUsage.db — the SIMs that have been in the device.

    Column names are the real ones and they mislead on purpose: `subscriber_id`
    holds the SIM's ICCID and `subscriber_mdn` the phone number. A fixture with
    friendly names would let the module read them the wrong way round unnoticed.
    See crates/traceloupe-core/modules/sim_cards.toml.
    """
    con = sqlite3.connect(path)
    con.executescript(
        """CREATE TABLE subscriber_info (
             ROWID INTEGER PRIMARY KEY AUTOINCREMENT, subscriber_id TEXT,
             subscriber_mdn TEXT, tag INTEGER, last_update_time INTEGER,
             slot_id INTEGER, home_budget INTEGER, roaming_budget INTEGER,
             user_entered_bill_end_dom INTEGER, low_data_mode INTEGER,
             reliable_network_fallback INTEGER, smart_data_mode INTEGER,
             interface_cost INTEGER, privacy_proxy INTEGER);
           CREATE TABLE bundle_info (
             ROWID INTEGER PRIMARY KEY AUTOINCREMENT, bundle_id TEXT, flags INTEGER);"""
    )
    con.executemany(
        "INSERT INTO subscriber_info (subscriber_id, subscriber_mdn, tag,"
        " last_update_time, slot_id) VALUES (?,?,?,?,?)",
        [
            ("8901260971148676693", "+15550100", 1, 726_000_000, 1),
            ("8944500000000000001", "+15550199", 1, 725_000_000, 2),
        ],
    )
    # Present but deliberately unread: an opaque flag with a single value.
    con.execute("INSERT INTO bundle_info (bundle_id, flags) VALUES ('com.example.watchapp',48)")
    con.commit()
    con.close()


def build_known_networks_plist() -> bytes:
    """com.apple.wifi.known-networks.plist — the first PLIST-backed artifact.

    A root dictionary whose KEYS name the networks, which is why the module needs
    `plist.key_column`. Mirrors what `explore_real_backup ... plist` printed for
    the validation image. See crates/traceloupe-core/modules/wifi_networks.toml.

    Includes a network with no `__OSSpecific__` subtree, one joined automatically
    rather than by the user, and a key that does not carry Apple's
    `wifi.network.ssid.` prefix — so the module's "trim only when it really is the
    prefix" rule is exercised rather than assumed.

    The `SSID` Data field is here because the real store has it, but no column
    reads it: the key is the better source and the module says why.
    """
    def at(secs: int) -> datetime:
        # NAIVE UTC: plistlib's binary writer subtracts a naive epoch, so an
        # aware datetime raises. The value is still UTC; only the tzinfo is
        # dropped.
        return datetime.fromtimestamp(secs, tz=timezone.utc).replace(tzinfo=None)

    return plistlib.dumps(
        {
            "wifi.network.ssid.HomeNet": {
                "SSID": b"HomeNet",
                "SupportedSecurityTypes": "WPA2 Personal",
                "Hidden": False,
                "JoinedByUserAt": at(1_688_243_921),
                "JoinedBySystemAt": at(1_689_450_000),
                "AddedAt": at(1_688_243_920),
                "LastDiscoveredAt": at(1_689_450_218),
                "__OSSpecific__": {"BSSID": "6a:22:32:98:f4:df", "CHANNEL": 153},
            },
            "wifi.network.ssid.Cafe Wifi": {
                "SSID": b"Cafe Wifi",
                "SupportedSecurityTypes": "None",
                "Hidden": True,
                "AddedAt": at(1_700_000_000),
            },
            # No namespace prefix: must be shown whole.
            "legacy-entry": {"SSID": b"\xff\xfe\x00A", "AddedAt": at(1_710_000_000)},
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_bluetooth_devices_plist() -> bytes:
    """com.apple.MobileBluetooth.devices.plist — classic (non-LE) accessories,
    keyed by MAC address.

    One device is named after someone other than the owner, which is why the
    module keeps the owner's name, the device's own name and its class as three
    separate columns; one was never renamed, so its owner-name column must be null
    rather than falling back to the model.
    """
    return plistlib.dumps(
        {
            "08:65:18:75:5E:75": {
                "UserNameKey": "Alex's AirPods",
                "Name": "AirPods 3",
                "DefaultName": "Headphones",
                "LastAVCTPVersion": b"\x01\x04",  # radio state, deliberately unread
            },
            "7C:04:D0:89:89:A0": {
                "UserNameKey": "Sam's AirPods",
                "Name": "AirPods",
                "DefaultName": "Headphones",
            },
            "F8:6F:C1:4E:FF:6A": {"Name": "Apple Watch", "DefaultName": "Watch"},
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_private_mac_plist() -> bytes:
    """com.apple.wifi-private-mac-networks.plist — the randomised address the phone
    presented to each network. Rows are an ARRAY under a key containing spaces.

    One address is marked invalid: bytes that are present but not in use are not
    an address this phone presented.
    """
    def at(secs: int) -> datetime:
        return datetime.fromtimestamp(secs, tz=timezone.utc).replace(tzinfo=None)

    return plistlib.dumps(
        {
            "List of scanned networks with private mac": [
                {
                    "SSID_STR": "HomeNet",
                    "BSSID": "6a:22:32:98:f4:df",
                    "IsOpenNetwork": False,
                    "PresentInKnownNetworks": True,
                    "lastJoined": at(1_689_450_273),
                    "MacGenerationTimeStamp": at(1_700_312_363),
                    "PRIVATE_MAC_ADDRESS": {
                        "PRIVATE_MAC_ADDRESS_VALID": True,
                        "PRIVATE_MAC_ADDRESS_VALUE": b"\x8a\x1b\x2c\x3d\x4e\x5f",
                    },
                },
                {
                    "SSID_STR": "Cafe Wifi",
                    "IsOpenNetwork": True,
                    "PresentInKnownNetworks": False,
                    "lastJoined": at(1_700_000_000),
                    "MacGenerationTimeStamp": at(1_699_000_000),
                    "PRIVATE_MAC_ADDRESS": {
                        "PRIVATE_MAC_ADDRESS_VALID": False,
                        "PRIVATE_MAC_ADDRESS_VALUE": b"\x00\x11\x22\x33\x44\x55",
                    },
                },
            ]
        },
        fmt=plistlib.FMT_BINARY,
    )


def seed_bluetooth_nearby(path: Path) -> None:
    """…ledevices.other.db — devices seen in range but never paired.

    `ResolvedAddress` stays NULL throughout, which is what the real store holds:
    resolving a rotating address needs the key exchanged during pairing, so an
    unpaired sighting cannot be resolved. No column reads it.
    """
    con = sqlite3.connect(path)
    con.execute(
        "CREATE TABLE OtherDevices(Uuid TEXT, Name TEXT, NameOrigin INT,"
        " Address TEXT, ResolvedAddress TEXT, LastSeenTime INT,"
        " LastConnectionTime INT, GATTServiceChangeConfig INT, Tags TEXT,"
        " iCloudIdentifier TEXT)"
    )
    con.executemany(
        "INSERT INTO OtherDevices (Uuid, Name, Address, ResolvedAddress, LastSeenTime)"
        " VALUES (?,?,?,?,?)",
        [
            ("11111111-0000-0000-0000-000000000001", None, "Random AA:BB:CC:DD:EE:01", None, 4_000_000),
            ("11111111-0000-0000-0000-000000000002", "Garage Opener", "Public CC:6A:10:54:65:FF", None, 4_352_299),
            ("11111111-0000-0000-0000-000000000003", "", "Random AA:BB:CC:DD:EE:03", None, 4_100_000),
            ("11111111-0000-0000-0000-000000000004", "Fitness Band", "Random ED:FD:03:AC:36:76", None, 4_337_974),
        ],
    )
    con.commit()
    con.close()


def build_global_preferences_plist() -> bytes:
    """.GlobalPreferences.plist — a SINGLE-RECORD artifact: the root dictionary is
    the row, so the module declares neither `rows` nor `key_column`.

    `AppleLanguages` has a second entry that is deliberately not shown: a second
    preferred language is real, but it is not what the device is set to.
    """
    return plistlib.dumps(
        {
            "AppleLanguages": ["en-US", "sv-SE"],
            "AppleLocale": "en_US",
            "AKLastLocale": "en_US",
            "AppleICUForce24HourTime": True,
            "ApplePasscodeKeyboards": ["en_US@sw=QWERTY;hw=Automatic"],
            "PKKeychainVersionKey": 8,  # internal, unread
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_clock_plist() -> bytes:
    """com.apple.mobiletimerd.plist — BOTH collections the real file holds.

    `MTAlarms` (ordinary alarms) and `MTSleepAlarms` (the sleep schedule) are two
    different artifacts sharing one file, read by two modules. Each element is
    wrapped in Apple's `$MTAlarm` class marker, which the modules step over — a
    fixture without the wrapper would let a module drop that path segment and
    still pass.
    """
    def at(secs: int) -> datetime:
        return datetime.fromtimestamp(secs, tz=timezone.utc).replace(tzinfo=None)

    return plistlib.dumps(
        {
            "MTAlarms": {
                "MTAlarms": [
                    {
                        "$MTAlarm": {
                            "MTAlarmHour": 10,
                            "MTAlarmMinute": 41,
                            "MTAlarmEnabled": False,
                            "MTAlarmAllowsSnooze": True,
                            "MTAlarmLastModifiedDate": at(1_722_177_663),
                            "MTAlarmDismissDate": at(1_722_177_663),
                            "MTAlarmID": "4ABC24C8-A16E-440D-A56D-0F7C2D46825E",
                            "MTAlarmRepeatSchedule": 0,  # undocumented, unread
                        }
                    }
                ],
                "MTSleepAlarms": [
                    {
                        "$MTAlarm": {
                            "MTAlarmHour": 6,
                            "MTAlarmMinute": 0,
                            "MTAlarmBedtimeHour": 22,
                            "MTAlarmBedtimeMinute": 45,
                            "MTAlarmEnabled": False,
                            "MTAlarmSleepTrackingKey": True,
                            "MTAlarmKeepOffUntilDate": at(1_689_849_000),
                            "MTAlarmLastModifiedDate": at(1_722_076_501),
                        }
                    }
                ],
            },
            "MTTimerDefaultDuration": 900.0,  # unread
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_siri_plist() -> bytes:
    """com.apple.assistant.backedup.plist — Siri's backed-up preferences.

    Includes the nested `Output Voice` dictionary the module reaches into, and the
    undocumented `Footprint` key it deliberately leaves alone.
    """
    return plistlib.dumps(
        {
            "Output Voice": {
                "Language": "en-US",
                "Name": "nora",
                "Gender": 2,
                "Custom": True,
                "Footprint": 2,  # undocumented, unread
            },
            "Cloud Sync Enabled": True,
            "MultiUser VoiceIdentification Enabled": False,
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_location_clients_plist() -> bytes:
    """locationd's clients.plist — everything that asked for location.

    Covers an app client with a BundleId, the SAME app with a second sub-bundle
    session (only the key separates them), and a system bundle with no BundleId,
    which must still produce a row rather than be filtered out.

    Timestamps are Cocoa REALS, not plist Dates, so the module has to declare an
    epoch — a fixture using Dates would hide that.
    """
    return plistlib.dumps(
        {
            "icom.example.chatapp:": {
                "BundleId": "com.example.chatapp",
                "BundlePath": "/private/var/containers/Bundle/Application/ChatApp.app",
                "Registered": "",
                "ReceivingLocationInformationTimeStopped": 744_322_588.28,
            },
            "lcom.example.chatapp:p/System/Library/LocationBundles/Nav.bundle": {
                "BundleId": "com.example.chatapp",
                "LocationTimeStopped": 744_291_564.14,
            },
            "p/System/Library/LocationBundles/TraceHarvest.bundle": {
                "BundlePath": "/System/Library/LocationBundles/TraceHarvest.bundle",
                "Registered": "",
                "ReceivingLocationInformationTimeStopped": 744_000_000.0,
                "SupportedAuthorizationMask": 7,  # undocumented, unread
            },
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_watch_apps_plist() -> bytes:
    """ACXRemoteAppList.plist — apps on the paired Apple Watch.

    Seeded at a CONCRETE DeviceRegistry path, because the module's `path` is a
    pattern: the segment is the paired device's UUID and differs on every phone.
    Writing it at the literal pattern would let `*` match itself and prove nothing.

    One app is on the watch and one is only listed — `isLocallyAvailable` is the
    difference between "this app exists for the watch" and "it is on it".
    """
    return plistlib.dumps(
        {
            "appList": {
                "com.example.chatapp.watchkitapp": {
                    "companionAppBundleID": "com.example.chatapp",
                    "bundleShortVersion": "2.4",
                    "bundleVersion": "2401",
                    "isLocallyAvailable": True,
                    "minimumOSVersion": "9.6",
                    "sequenceNumber": 6,  # unread
                },
                "com.example.todo.watchkitapp": {
                    "companionAppBundleID": "com.example.todo",
                    "bundleShortVersion": "1.0",
                    "isLocallyAvailable": False,
                },
            }
        },
        fmt=plistlib.FMT_BINARY,
    )


def build_mobile_backup_plist() -> bytes:
    """com.apple.MobileBackup.plist — PreflightSizing is a dict of DOMAIN -> BYTES.

    The shape `plist.value_column` exists for: the row IS a number, so there is no
    dictionary of fields to name. Includes a daemon-internal key the module leaves.
    """
    return plistlib.dumps(
        {
            "PreflightSizing": {
                "KeyboardDomain": 2_535_424,
                "CameraRollDomain": 3_221_225_472,
                "AppDomainGroup-group.com.example.chat": 175_961,
            },
            "FetchMissingKeysAtNextUnlock": 0,  # unread
        },
        fmt=plistlib.FMT_BINARY,
    )


# domain, relativePath, seeder(fn writing plaintext bytes to a temp path)
def seed_files(workdir: Path) -> list[tuple[str, str, bytes]]:
    """Return (domain, relativePath, plaintext_bytes) for each backed-up file."""
    sms_path = workdir / "sms.db"
    seed_sms_db(sms_path)
    safari_path = workdir / "History.db"
    seed_safari_db(safari_path)
    calls_path = workdir / "CallHistory.storedata"
    seed_callhistory_db(calls_path)
    ab_path = workdir / "AddressBook.sqlitedb"
    seed_addressbook_db(ab_path)
    photos_path = workdir / "Photos.sqlite"
    seed_photos_sqlite(photos_path)
    tcc_path = workdir / "TCC.db"
    seed_tcc_db(tcc_path)
    accounts_path = workdir / "Accounts3.sqlite"
    seed_accounts3(accounts_path)
    bt_path = workdir / "ledevices.paired.db"
    seed_bluetooth_paired(bt_path)
    usage_path = workdir / "DataUsage.sqlite"
    seed_data_usage(usage_path)
    cell_path = workdir / "CellularUsage.db"
    seed_cellular_usage(cell_path)
    nearby_path = workdir / "ledevices.other.db"
    seed_bluetooth_nearby(nearby_path)
    files = [
        ("HomeDomain", "Library/SMS/sms.db", sms_path.read_bytes()),
        ("HomeDomain", "Library/Safari/History.db", safari_path.read_bytes()),
        ("HomeDomain", "Library/CallHistoryDB/CallHistory.storedata", calls_path.read_bytes()),
        ("HomeDomain", "Library/AddressBook/AddressBook.sqlitedb", ab_path.read_bytes()),
        ("HomeDomain", "Library/TCC/TCC.db", tcc_path.read_bytes()),
        ("HomeDomain", "Library/Accounts/Accounts3.sqlite", accounts_path.read_bytes()),
        (
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Database/com.apple.MobileBluetooth.ledevices.paired.db",
            bt_path.read_bytes(),
        ),
        ("WirelessDomain", "Library/Databases/DataUsage.sqlite", usage_path.read_bytes()),
        ("WirelessDomain", "Library/Databases/CellularUsage.db", cell_path.read_bytes()),
        (
            "SystemPreferencesDomain",
            "com.apple.wifi.known-networks.plist",
            build_known_networks_plist(),
        ),
        (
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Preferences/com.apple.MobileBluetooth.devices.plist",
            build_bluetooth_devices_plist(),
        ),
        (
            "SystemPreferencesDomain",
            "SystemConfiguration/com.apple.wifi-private-mac-networks.plist",
            build_private_mac_plist(),
        ),

        (
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Database/com.apple.MobileBluetooth.ledevices.other.db",
            nearby_path.read_bytes(),
        ),
        (
            "HomeDomain",
            "Library/Preferences/.GlobalPreferences.plist",
            build_global_preferences_plist(),
        ),

        (
            "HomeDomain",
            "Library/Preferences/com.apple.mobiletimerd.plist",
            build_clock_plist(),
        ),

        (
            "HomeDomain",
            "Library/Preferences/com.apple.assistant.backedup.plist",
            build_siri_plist(),
        ),

        (
            "RootDomain",
            "Library/Caches/locationd/clients.plist",
            build_location_clients_plist(),
        ),
        (
            "HomeDomain",
            "Library/Preferences/com.apple.MobileBackup.plist",
            build_mobile_backup_plist(),
        ),

        (
            "HomeDomain",
            # A real UUID segment, matched by the module's pattern.
            "Library/DeviceRegistry/48BEB26F-3064-4BEF-A616-AB96D8C5BD15"
            "/AppConduit/ACXRemoteAppList.plist",
            build_watch_apps_plist(),
        ),
    ]
    files += [("MediaDomain", rel, blob) for rel, _mime, blob in GALLERY_PHOTOS]
    # A real camera roll: the DCIM original, its V2 thumbnail, and Photos.sqlite.
    files += [
        ("CameraRollDomain", CAMERA_ROLL_DCIM[0], CAMERA_ROLL_DCIM[1]),
        ("CameraRollDomain", CAMERA_ROLL_THUMB[0], CAMERA_ROLL_THUMB[1]),
        ("CameraRollDomain", "Media/PhotoData/Photos.sqlite", photos_path.read_bytes()),
    ]
    return files


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
    ap.add_argument("--password", default="traceloupe-test", help="backup password")
    args = ap.parse_args()
    make_backup(args.out, args.password)
    n = sum(1 for _ in args.out.rglob("*") if _.is_file())
    print(f"Wrote encrypted backup to {args.out} ({n} files), password: {args.password!r}")


if __name__ == "__main__":
    main()
