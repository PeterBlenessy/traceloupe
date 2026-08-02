#!/usr/bin/env python3
"""Fetch ONE member out of a remote ZIP with HTTP range requests.

Digital Corpora's iOS 13.3.1 archive is 8.9 GB; the iTunes backup inside it is
396 MB. Downloading the whole thing to reach 4% of it is the kind of thing that
fills a disk for no reason.
"""
import struct, sys, urllib.request, zlib

URL = sys.argv[1]
WANT = sys.argv[2]
OUT = sys.argv[3]

def get(start, end):
    r = urllib.request.Request(URL, headers={"Range": f"bytes={start}-{end}"})
    with urllib.request.urlopen(r, timeout=120) as f:
        return f.read()

def size():
    r = urllib.request.Request(URL, method="HEAD")
    with urllib.request.urlopen(r, timeout=120) as f:
        return int(f.headers["Content-Length"])

total = size()
print(f"archive: {total/1e9:.2f} GB", flush=True)

# End of Central Directory, possibly ZIP64.
tail = get(max(0, total - 70000), total - 1)
i = tail.rfind(b"PK\x05\x06")
assert i >= 0, "no EOCD"
cd_size, cd_off = struct.unpack("<II", tail[i + 12:i + 20])
if cd_off == 0xFFFFFFFF or cd_size == 0xFFFFFFFF:
    j = tail.rfind(b"PK\x06\x06")
    assert j >= 0, "no ZIP64 EOCD"
    cd_size, cd_off = struct.unpack("<QQ", tail[j + 40:j + 56])
print(f"central directory: {cd_size} bytes at {cd_off}", flush=True)

cd = get(cd_off, cd_off + cd_size - 1)
p, found = 0, None
while p < len(cd) and cd[p:p+4] == b"PK\x01\x02":
    method, = struct.unpack("<H", cd[p+10:p+12])
    csize, usize = struct.unpack("<II", cd[p+20:p+28])
    nlen, elen, clen = struct.unpack("<HHH", cd[p+28:p+34])
    lho, = struct.unpack("<I", cd[p+42:p+46])
    name = cd[p+46:p+46+nlen].decode("utf-8", "replace")
    extra = cd[p+46+nlen:p+46+nlen+elen]
    # ZIP64 extended info, when any field is maxed out.
    if 0xFFFFFFFF in (csize, usize, lho) and extra:
        q = 0
        while q + 4 <= len(extra):
            hid, hsz = struct.unpack("<HH", extra[q:q+4])
            if hid == 1:
                vals = extra[q+4:q+4+hsz]
                k = 0
                if usize == 0xFFFFFFFF: usize, = struct.unpack("<Q", vals[k:k+8]); k += 8
                if csize == 0xFFFFFFFF: csize, = struct.unpack("<Q", vals[k:k+8]); k += 8
                if lho == 0xFFFFFFFF: lho, = struct.unpack("<Q", vals[k:k+8]); k += 8
                break
            q += 4 + hsz
    if WANT in name and not name.endswith('/') and usize > 0:
        found = (name, method, csize, usize, lho)
        break
    p += 46 + nlen + elen + clen

assert found, f"{WANT!r} not in the archive"
name, method, csize, usize, lho = found
print(f"member: {name}\n  {csize/1e6:.1f} MB compressed, method {method}", flush=True)

# Local header, to skip its variable-length fields.
lh = get(lho, lho + 29)
nlen, elen = struct.unpack("<HH", lh[26:30])
data_at = lho + 30 + nlen + elen

CHUNK = 32 * 1024 * 1024
d = zlib.decompressobj(-15) if method == 8 else None
written = 0
with open(OUT, "wb") as out:
    got = 0
    while got < csize:
        end = min(got + CHUNK, csize) - 1
        buf = get(data_at + got, data_at + end)
        got += len(buf)
        out.write(d.decompress(buf) if d else buf)
        written = out.tell()
        print(f"  {got/1e6:.0f}/{csize/1e6:.0f} MB", flush=True)
    if d:
        out.write(d.flush())
        written = out.tell()
print(f"wrote {written/1e6:.1f} MB to {OUT}")
