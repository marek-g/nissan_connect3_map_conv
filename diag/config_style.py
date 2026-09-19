#!/usr/bin/env python3
"""Extract the Bosch mapengine road-pen colour table from the renderer theme 3D/config.bin.

Chain (see MAP_format.md §road tiers): a line's feature LOW byte -> DAPIAPP
u8ConvertFeature2LineSubType -> subtypeRoad (0x30..0x37 -> 0x3d..0x44 ; 0x21 -> 0x15) ->
procmapengine GetLineConfigOffsetRoad(subtypeRoad,subAttributes) -> an OFFSET that indexes the
g_LineReferences u16 array in 3D/config.bin; each entry is a u16 index into the line-data
(g_pLineSettings) array whose first u32 is the RGBA body colour (byte order R,G,B,A) and second
u32 the border colour; +0x2c/+0x30 hold day/night widths.

Usage: config_style.py [path/to/config.bin]
"""
import struct
import sys

DEFAULT = ("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/D605_unpacked/"
           "lx001.tar.gz/var/opt/bosch/dynamic/ffs/3D/config.bin")
HASH = 0x332b7867  # ReadHeader hash check


def load(path):
    d = open(path, "rb").read()
    # header: u8 ver, u8 endianness, u32, u32 hash(=0x332b7867), u8 os -> 11 bytes
    assert struct.unpack_from("<I", d, 6)[0] == HASH, "wrong hash / not a MapEngineConfig"
    off = 11
    assert d[off] == 0x01, "expected line-data section tag 0x01"
    off += 1
    count = struct.unpack_from("<I", d, off)[0]
    off += 4
    esize = struct.unpack_from("<H", d, off)[0]
    off += 2
    line_start = off
    roff = line_start + count * esize
    assert d[roff] == 0x02, "expected line-reference section tag 0x02"
    roff += 1
    rsize = struct.unpack_from("<I", d, roff)[0]
    roff += 4
    refs = [struct.unpack_from("<H", d, roff + 2 * i)[0] for i in range(rsize // 2)]
    return d, line_start, esize, count, refs


def entry(d, line_start, esize, s):
    # file layout per entry (esize=0x32): 6x u32 (body,border,f2,f3,f4,f5) @0..20,
    # 2x TValue (day/night width) @24/28, then flag bytes, then 2 trailing u32.
    b = d[line_start + s * esize: line_start + s * esize + esize]
    body = struct.unpack_from("<I", b, 0)[0]
    border = struct.unpack_from("<I", b, 4)[0]
    width_day = struct.unpack_from("<I", b, 24)[0]
    width_ngt = struct.unpack_from("<I", b, 28)[0]
    return body, border, width_day, width_ngt


def rgba(v):
    # Packed as 0xAARRGGBB little-endian: byte0=B, byte1=G, byte2=R, byte3=A.
    # (Verified on-car: 0x30 motorway bytes [0f 11 85 ff] read as B,G,R -> #85110f maroon.)
    return ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff)


# feature -> GetLineConfigOffsetRoad default offset (subtypeRoad base block, subAttributes absent)
ROAD = {0x30: 0x0e, 0x31: 0x17, 0x32: 0x20, 0x33: 0x29, 0x34: 0x32, 0x35: 0x3b, 0x36: 0x44, 0x37: 0x4d}


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT
    d, ls, esize, count, refs = load(path)
    print(f"{path}\n  line entries={count} esize=0x{esize:x} refs={len(refs)}")
    for feat in sorted(ROAD):
        s = refs[ROAD[feat]]
        body, border, wday, wngt = entry(d, ls, esize, s)
        br, bg, bb = rgba(body)
        nr, ng, nb = rgba(border)
        print(f"  feat 0x{feat:02x} idx={s:3d}  body=#{br:02x}{bg:02x}{bb:02x}  "
              f"border=#{nr:02x}{ng:02x}{nb:02x}  w={wday}/{wngt}")


if __name__ == "__main__":
    main()
