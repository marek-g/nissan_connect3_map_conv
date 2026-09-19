#!/usr/bin/env python3
"""Unpacker for Bosch/Nissan LX container files (magic XOZL / "ULI ").

Container layout (all little-endian u32, header size 0x24):
    +0x00 magic "XOZL" or "ULI "
    +0x08 section count (seen 1)
    +0x0c format id (seen 2)
    +0x18 unpacked size (usize)
    +0x1c packed  size (psize)
    +0x24 packed stream

The stream is decompressed by the LX monitor (triton_mid.bin) routine at
file offset 0x10bd90 - a byte-oriented LZ codec.  Instead of re-implementing
it, this tool emulates that routine directly with Unicorn.  For "ULI "
containers the installer additionally post-processes every output byte with
b -> ~(b ^ 1) before writing the file.

Usage:
    ./uli_unpack.py <container.OUT> <out.elf> [--triton triton_mid.bin]

Requires: unicorn (pip install unicorn).  See README.md for details.
"""
import argparse
import struct
import sys

BASE = 0x80000000


def unpack(packed_path, out_path, triton_path):
    from unicorn import Uc, UC_ARCH_ARM, UC_MODE_ARM, UC_PROT_ALL, UC_HOOK_CODE, \
        UC_HOOK_MEM_UNMAPPED, UcError
    from unicorn.arm_const import UC_ARM_REG_SP, UC_ARM_REG_R0, UC_ARM_REG_R1, \
        UC_ARM_REG_R2, UC_ARM_REG_R3, UC_ARM_REG_LR, UC_ARM_REG_PC, UC_ARM_REG_R0 as R0

    blob = open(triton_path, 'rb').read()
    if blob[:4] == b'\x27\x05\x19\x56':   # triton_mid.bin 64-byte package header
        blob = blob[0x40:]
    data = open(packed_path, 'rb').read()
    if data[:4] not in (b'XOZL', b'ULI '):
        sys.exit("not an XOZL/ULI container: %r" % data[:4])
    magic = data[:4]
    usize, psize = struct.unpack_from('<II', data, 24)

    mu = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    mu.mem_map(BASE, (len(blob) & ~0xFFF) + 0x400000, UC_PROT_ALL)
    mu.mem_write(BASE, blob)
    mu.mem_map(0x90000000, 0x02000000, UC_PROT_ALL)   # out + outlen + stack
    mu.mem_map(0x92000000, 0x01000000, UC_PROT_ALL)   # stream

    SRC, DST, OUTLEN, RET = 0x92000000, 0x90001000, 0x91800000, 0x90700000
    if usize + 0x40000 > 0x01800000 or psize > 0x01000000:
        sys.exit("file too large for fixed layout")
    mu.mem_write(SRC, data[0x24:0x24 + psize])
    mu.mem_write(RET, b'\x00\xf0\x20\xe3')
    mu.reg_write(UC_ARM_REG_SP, 0x90800000)
    mu.reg_write(UC_ARM_REG_R0, SRC)
    mu.reg_write(UC_ARM_REG_R1, usize)
    mu.reg_write(UC_ARM_REG_R2, DST)
    mu.reg_write(UC_ARM_REG_R3, OUTLEN)
    mu.reg_write(UC_ARM_REG_LR, RET)

    def on_mem(m, acc, a, s, v, u):
        try:
            m.mem_map(a & ~0xFFFF, 0x10000, UC_PROT_ALL)
            return True
        except Exception:
            return False

    mu.hook_add(UC_HOOK_MEM_UNMAPPED, on_mem)

    try:
        mu.emu_start(BASE + 0x10bd90, RET, count=0)
    except UcError as e:
        sys.exit("emulation faulted: %s PC=%08x" % (e, mu.reg_read(UC_ARM_REG_PC)))

    outlen = struct.unpack('<I', mu.mem_read(OUTLEN, 4))[0]
    if outlen != usize:
        sys.exit("decompressed size %d != header usize %d" % (outlen, usize))
    out = bytes(mu.mem_read(DST, outlen))
    if magic == b'ULI ':
        out = bytes((b ^ 1) ^ 0xFF for b in out)
    open(out_path, 'wb').write(out)
    print("%s -> %s (%d bytes%s)" % (packed_path, out_path, outlen,
                                     ", ULI transform" if magic == b'ULI ' else ""))


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('container')
    ap.add_argument('out')
    ap.add_argument('--triton', default='/home/marek/Ext/reverse_engineering/'
                    'NissanMaps/Firmware/D605/triton_mid.bin')
    a = ap.parse_args()
    unpack(a.container, a.out, a.triton)
