# elf_decompressor — unpacker for Bosch/Nissan LX `*.OUT` container files

Decompresses the compressed ELF process images shipped on the navigation card
(`CRYPTNAV/DNL/BIN/NAV/.../*.OUT`, magic `XOZL` or `"ULI "`) back into plain
ARM32 ELF files that Ghidra/IDA can load directly.

## How it works

The container stream is NOT a public codec — it is the byte-oriented LZ token
stream produced/consumed by the LX monitor (`triton_mid.bin`). Instead of
re-implementing the codec, `uli_unpack.py` **emulates the monitor's own
decompressor routine** (file offset `0x10bd90`, called by the installer at
`0x108844`) in the Unicorn ARM CPU emulator, so the output is bit-exact by
construction. For `"ULI "` containers the installer additionally
post-processes every output byte with `b -> ~(b ^ 1)`; the tool applies this
transform automatically based on the container magic.

The full codec grammar and container header spec are in
`doc/TravelMap_format/routing_algorithm.md` §2.

## Requirements

- Python 3 with the `unicorn` package (`pip install unicorn`; a venv is fine).
- The monitor blob `triton_mid.bin`, taken from the head-unit firmware
  (stock copy: `NissanMaps/Firmware/D605/triton_mid.bin`; the tool
  auto-strips its 64-byte `triton_dualos` package header, so both the stock
  file and a stripped blob work). A stripped copy used during RE also lives
  at `/tmp/rnwwork/triton.bin`. The default `--triton` path points at the
  stock firmware copy.

## Usage

    ./uli_unpack.py <container.OUT> <out.elf> [--triton <triton_mid.bin>]

Examples (stock card, unpacked):

    cd src/elf_decompressor
    ./uli_unpack.py \
      "$N/Map_unpacked/CRYPTNAV/DNL/BIN/NAV/COMMON/PROCNAV.OUT" /tmp/PROCNAV_dec.out
    ./uli_unpack.py \
      "$N/Map_unpacked/CRYPTNAV/DNL/BIN/NAV/COMMON/PROCDLSAVER.OUT" /tmp/PROCDLSAVER_dec.out

where `N=/home/marek/Ext/reverse_engineering/NissanMaps/Firmware`.

Batch over every container in a directory:

    for f in "$N/Map_unpacked/CRYPTNAV/DNL/BIN/NAV/COMMON/"*.OUT; do
        ./uli_unpack.py "$f" "/tmp/$(basename "$f" .OUT)_dec.out"
    done

The tool verifies the decompressed size against the container header
(`usize`) and fails loudly on any mismatch. Verified on 7/7 known containers
(PROCNAV, PROCDLSAVER, 5 ISO-stage `nor0/processes` files) — all decode to the
exact header size and yield valid ARM ELFs.

## Exit behaviour

- `not an XOZL/ULI container` — input is not a compressed process file
  (plain ELFs on the card are not compressed).
- `decompressed size N != header usize M` — wrong `--triton` blob or corrupt
  input.
- `emulation faulted` — codec routine diverged; report the printed PC
  (monitor blob offsets are documented in routing_algorithm.md §2).
