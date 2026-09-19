#!/usr/bin/env python3
"""Merge generated clusters into a stock NAV_ROOT.DAT root-cluster list.

NAV_ROOT.DAT layout (decoded from the device's own rnw_tclNavRootKnitter +
u16InterpreteHeader @0x00891e4c, cross-checked on stock POL/DEU/SCA/EEU; see
doc/TravelMap_format/02 - details/RNW_format.md §3a):

  0x00 u32 headerRecordSize  0x04 u32 totalFileSize
  0x08 u16 authorStrOff      0x0a u16 tableOff   (string/profile region, copied through)
  0x0c ListDesc roots {u16 payloadOff,u16 count}; 0x10 ListDesc annots {u16 chainOff,u16 count}
  [after table] count x 24B root records, then per-root SHAPES (ocnt x i16 x/y outline
  points, copied verbatim from the target cluster), then a 2-byte pad, then the annotation
  TLV chain {u16 len,u16 type}, then 2 pad; headerRecordSize: global-area + global-instruction
  records (globally identical between stock regions — copied through untouched).

  Root record (nav_tclClusterInfo::bRead @0x008910cc):
    u32 0  (clusterOff & ~0x3FFF) | regionIdent     0x02 u16 target cluster id (header u16@0)
    0x04 u16 NAV fileId (NAV<fid>.DAT)              0x08 i32 refLon   0x0c i32 refLat
    0x10 u8 shift  0x11 u8 hdrByte17                0x12 u16 shapeOff (in this NAV_ROOT)
    0x14 u16 ocnt  0x16 u16 0x0200
  All fields mirror the target cluster's own header (id, refLon/Lat, shift, byte@17, ocnt) —
  verified byte-exact on every stock root except one EEU record whose target cluster header
  does not match (suspected compressed cluster; not blocking).

Usage:
  merge_nav_root.py <stock_NAV_ROOT.DAT> <out_NAV_ROOT.DAT> --region-ident 0x402 \
      --rnw-dir <dir with NAV<fid>.DAT> --root FILEID:OFF [--root FILEID:OFF ...]
OFF = 16KB-aligned cluster offset inside NAV<fid>.DAT. Shape/refs are read from the cluster
header; the outline is copied verbatim. Re-parses the output (u16InterpreteHeader order) and
verifies the shape chain accounts exactly to the annotation start before writing.
"""
import argparse
import os
import struct
import sys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stock")
    ap.add_argument("out")
    ap.add_argument("--region-ident", type=lambda x: int(x, 0), required=True)
    ap.add_argument("--rnw-dir", required=True)
    ap.add_argument("--root", action="append", required=True)
    a = ap.parse_args()

    d = bytearray(open(a.stock, "rb").read())
    hdr, fsize = struct.unpack_from("<II", d, 0)
    ro, rc, ao, ac = struct.unpack_from("<HHHH", d, 0xC)
    if fsize != len(d):
        sys.exit(f"header size {fsize:#x} != file {len(d):#x}")
    root_end = ro + 24 * rc
    if not (0 < ro <= root_end <= ao < hdr < fsize):
        sys.exit(f"bad regions: root@{ro:#x} end {root_end:#x} annot@{ao:#x} hdr@{hdr:#x}")

    pos = ao
    for _ in range(ac):
        ln, ty = struct.unpack_from("<HH", d, pos)
        if ln < 4:
            sys.exit("bad TLV len")
        pos += ln
    pad = bytes(d[pos:hdr])
    tail = bytes(d[hdr:])

    # build new root records + their shape copies from the generated NAV files
    new_rec, new_shape = bytearray(), bytearray()
    specs = []
    for spec in a.root:
        fid, off = (int(x, 0) for x in spec.split(":"))
        if off & 0x3FFF:
            sys.exit(f"root offset {off:#x} not 16KB-aligned")
        nav = os.path.join(a.rnw_dir, f"NAV{fid:05d}.DAT")
        if not os.path.exists(nav):
            sys.exit(f"missing {nav}")
        n = open(nav, "rb").read(off + 0x8000)
        cid = int.from_bytes(n[off : off + 2], "little")
        rlon, rlat = struct.unpack_from("<ii", n, off + 8)
        shift, b17 = n[off + 16], n[off + 17]
        ooff, ocnt = struct.unpack_from("<HH", n, off + 18)
        outline = bytes(n[off + ooff : off + ooff + 4 * ocnt])
        if len(outline) != 4 * ocnt:
            sys.exit(f"cluster @{off:#x} outline truncated")
        specs.append((fid, off, cid, rlon, rlat, shift, b17, ocnt, outline))
    q = ao + len(a.root) * 24  # old shape area start moves by the inserted records
    for fid, off, cid, rlon, rlat, shift, b17, ocnt, outline in specs:
        new_rec += struct.pack("<IHHiiBBHHH", ((off & ~0x3FFF) | a.region_ident),
                              cid, fid, rlon, rlat, shift, b17, q, ocnt, 0x0200)
        new_shape += outline
        q += 4 * ocnt

    shift_total = len(new_rec) + len(new_shape)
    out = bytearray(d[:root_end])
    out += new_rec
    out += d[root_end:ao]  # existing shapes
    out += new_shape
    struct.pack_into("<H", out, 0xE, rc + len(a.root))
    struct.pack_into("<H", out, 0x10, ao + shift_total)
    struct.pack_into("<I", out, 0, hdr + shift_total)
    out += d[ao:pos]
    out += pad
    out += tail
    struct.pack_into("<I", out, 4, len(out))
    for i in range(rc):  # shift existing root records' shapeOff past the inserted records
        so = int.from_bytes(out[ro + 24 * i + 18 : ro + 24 * i + 20], "little")
        struct.pack_into("<H", out, ro + 24 * i + 18, so + len(new_rec))

    # verify like u16InterpreteHeader: descs -> record loop (shape accounting) -> annots
    e = bytes(out)
    h2, f2 = struct.unpack_from("<II", e, 0)
    ro2, rc2, ao2, ac2 = struct.unpack_from("<HHHH", e, 0xC)
    assert f2 == len(e) and rc2 == rc + len(a.root) and ao2 == ao + shift_total
    p = ro2 + 24 * rc2
    for i in range(rc2):
        _, _, _, _, _, _, _, sh_off, oc, flg = struct.unpack_from("<IHHiiBBHHH", e, ro2 + 24 * i)
        assert sh_off == p and flg == 0x0200, f"shape gap break at root {i}"
        p += 4 * oc
    assert p == ao2, "shape chain does not end at annotation start"
    pos2 = ao2
    types = []
    for _ in range(ac2):
        ln, ty = struct.unpack_from("<HH", e, pos2)
        types.append(ty)
        pos2 += ln
    assert pos2 == h2 - len(pad), "annotation chain end"

    roots = [struct.unpack_from("<IHH", e, ro2 + 24 * i) for i in range(rc2)]
    print(
        f"OK {a.out}: hdrSize={h2:#x} size={f2:#x} roots={rc2} "
        + " ".join(f"{{off:{o & ~0x3FFF:#x} ident:{o & 0x3FFF:#x} id:{c:x} fid:{f}}}" for o, c, f in roots)
        + f" annots={ac2} (+{len(a.root)} roots, +{len(new_shape)} shape bytes)"
    )
    open(a.out, "wb").write(e)


if __name__ == "__main__":
    main()
