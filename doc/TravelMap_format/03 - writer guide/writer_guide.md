# Writing `.IDX` / `.MAP` — a practical guide for generating your own files

Companion to [`MAP_format.md`](../02%20-%20details/MAP_format.md) (the format reference, read-path) and
its §11 (write-side status). This document is the **how-to**: what bytes to emit, in what order,
and — most importantly — how to deal with the fields whose exact meaning we have not fully
pinned down.

> Target: produce a `.IDX`/`.MAP` pair that `DAPIAPP.OUT` (the car's navigation runtime) will
> read and render correctly. "Correct" is defined operationally, not by matching the original
> build tool byte-for-byte.

---

## 0. The one rule that governs everything

`DAPIAPP.OUT` is a **reader only**. It never writes `.IDX`/`.MAP`; the tools that do
("TpMap2 (Map-Data) for TravelMap", "Linker-SW / MK10_2021.1" — visible in the files' own
metadata) are not present in the firmware. Consequences:

1. There is no writer source to copy. Every write decision is inferred from (a) what the reader
   parses, and (b) what the real data looks like.
2. A file is "valid" if the **reader** can parse it and get sensible geometry — not if it matches
   some canonical byte pattern.
3. Therefore the safest way to handle any field you do not fully understand is one of two:

   - **(A) Avoid it.** Do not emit the feature that requires the unknown bytes. If a field only
     exists *because* a certain feature is present, omit the feature and the field disappears.
   - **(B) Copy it from a reference.** Take the exact bytes for that field from an existing file
     of the same region + profile. This guarantees reader-compatibility even when semantics are
     unknown, because you are reproducing what the reader has already been seen to accept.

   **Prefer (A)** where possible (smaller, cleaner files); fall back to **(B)** for fields the
   reader stores but whose purpose is unclear.

### What actually has to be byte-perfect

Only these drive geometry, and they are all fully understood:

- IDX tile-table slot `{regProf, length(words), offset}` — the pointer into a MAP file.
- The `multi` sub-entry list (§2 below).
- MAP block marker `0xFFFF | (len<<16)` and its `len`.
- Cell / point-pool / annotation / text layout (format reference §5/§8).
- Region BBox (PAU) and the per-level `shift`.

Everything else is bookkeeping or metadata and can be bypassed.

---

## 1. Field-by-field bypass table

Legend: **write** = emit normally (known); **avoid** = don't produce the feature; **copy** =
reuse bytes from a reference file; **const** = a fixed value works.

### `.IDX`

| field | status | what to do when writing | why it's safe |
|-------|--------|-------------------------|---------------|
| header `binOff` (0x00) | known | **write** — offset of the L0 tile table | reader uses it to find L0 |
| header `spare` (0x02) | const | **const** `32` | always 32 in observed files |
| header BBox `west/south/east/north` (0x04..0x13) | known | **write** — your region's corners, PAU | drives coordinate decoding |
| header `partOff` (0x14) | known | **write** — partition-table offset in 4-byte units | reader uses it |
| info-string region `[0x20 .. partOff*4)` | metadata | **avoid** (leave as zeros / minimal) or **copy** | not used by geometry; display only |
| partition table (4 × 12 B) | known | **write** — see §3.1 | fully decoded |
| tile-table slots | known | **write** — `{regProf, length, offset}` | the core pointer |
| `multi` slot | known | **write** when a tile spans profiles — see §2 | decoded this pass, verified on data |
| empty tiles | known | **write** slot with bit 15 set (`empty`) | reader skips it |

### `.MAP`

| field | status | what to do when writing | why it's safe |
|-------|--------|-------------------------|---------------|
| header `binOff` (0x00) | known | **write** — offset of first data block | reader uses it |
| header `infoTbl` (0x02) | const | **const** `0x34` (52) or **copy** | points at metadata region |
| header `fileSize` (0x04) | known | **write** — total file size in bytes | reader uses it |
| header BBox (0x08..0x17) | known | **write** — identical to the IDX | drives coordinate decoding |
| header `@0x18` | unclear | **copy** from reference, or **const** `8` | stored by reader; 8 in every observed file |
| header `@0x1a` | unclear | **copy** from reference, or **const** `4` | stored by reader; 4 in every observed file |
| header `@0x1c` | unclear | **copy**, or `0x400 \| ((binOff − base) >> 1)` | stored by reader; correlates with first-block offset |
| header `@0x1e` | ignored | **anything** (write `0x8400 \| prof` for tidiness) | reader's header object omits this field entirely |
| info-string region `[0x20 .. infoTbl)` | metadata | **avoid** or **copy** | not used by geometry |
| data blocks `[binOff .. fileSize)` | known | **write** — see §3.2 | fully decoded |

---

## 2. The `multi` tile slot (the one non-obvious structure)

A normal tile-table slot is 8 bytes: `{u16 regProf, u16 length(words), u32 offset}`. When a single
tile needs data from **several** profile files (this is the norm for low levels — e.g. L0, which is
the whole region, references one block per profile), the slot becomes a *pointer*:

```
multi slot (8 B):  { u16 0x4000,  u16 count,  u32 ptr }
                         ^bit14=multi   ^how many    ^offset (in this IDX) of the sub-list
sub-list:          count × 8 B real slots  {u16 regProf, u16 length, u32 offset}
```

Verified on `N6E1AA.IDX`: L0 slot = `{0x4000, 9, 0x1ed5ac}` → 9 sub-slots, each resolving to a
valid `0xFFFF…` block in its own `<REGION>1<prof>.MAP`.

**Writer rule:**
- If a tile maps to exactly one profile → write a normal slot.
- If it maps to several → write a `multi` slot and append the sub-list somewhere in the IDX
  (any free space after the tables works), setting `ptr` to that location.
- `regProf` in every real slot = `0x400 | prof` (bit 10 is a flag; the file name is derived from
  the two base32 digits of `prof`). Bit 14 = multi, bit 15 = empty.

---

## 3. Minimal viable file — step-by-step recipe

### 3.1 Generate the `.IDX`

Layout (all offsets are file-absolute; keep everything 4-byte aligned):

```
0x00  ┌ header (32 B)
0x20  ├ info-string region  (optional metadata; may be empty)
      │   ...
partOff*4  ┌ partition table (4 × 12 B)
           ├ L0 tile table @ binOff
           ├ L1 tile table @ (u32b_L1 >> 8)
           ├ L2 tile table @ (u32b_L2 >> 8)
           ├ L3 tile table @ (u32b_L3 >> 8)
           └ (multi sub-lists may live anywhere in the free space after the tables)
```

Steps:

1. **Choose offsets.** Pick `partOff` so `partOff*4 > 0x20` (after the header, leaving room for
   any info region you keep). Pick `binOff` and each level's table offset so they don't overlap
   and are 4-byte aligned. (Reference files use small values: `partOff=126`, `binOff=0x23c`.)
2. **Write the header.** `binOff`, `spare=32`, your region's `west/south/east/north` in PAU,
   `partOff`.
3. **Write the partition table** (4 × 12 B) at `partOff*4`. For level `i` (0..3):
   - byte `+0` = `i`, `+1` = `latPart[i]`, `+2` = `shift[i]`
   - `+3..+6` = `u32a = (tileCnt << 8) | (13 − 3*i)`
   - `+7..+10` = `u32b = (tableOffset << 8)`  ← where this level's tile table lives
   - Fixed for the EUR dataset: `latPart = (1,5,10,10)`, `shift = (13,10,7,4)`,
     `tileCnt = (1,25,2500,250000)`.
4. **Write each tile table.** For every tile `K` at level `i`, one 8-byte slot:
   - data present in one profile → `{0x400 | prof, length(words), offset-in-MAP}`
   - data present in several profiles → a `multi` slot (§2)
   - no data → `{0x8000 | (0x400|prof), 0, 0}` (bit 15 = empty)
   - `length` is the block size **in 4-byte words** and must equal the marker length in the MAP.

### 3.2 Generate the `.MAP`

Layout:

```
0x00  ┌ header (32 B)
0x20  ├ info-string region (optional metadata; may be empty)
      │   ...
binOff  ┌ block 0  {marker, 3×(start,count), cells…, point pool…, annotations/text…}
        ├ block 1  (contiguous, 4-byte aligned)
        └ … up to fileSize
```

Steps:

1. **Write the header.** `binOff` (first block), `infoTbl=0x34`, `fileSize` (final total),
   BBox identical to the IDX, and `@0x18/@0x1a/@0x1c/@0x1e` per the table in §1 (copy from a
   reference file of the same profile is the zero-risk choice).
2. **Write each block** the IDX points to, exactly at the offset/length the slot records:
   - `u32 marker = 0xFFFF | (len << 16)` where `len` = block size in words.
   - 3 × `{u16 start, u16 count}` for lists 0 (polygons), 1 (lines), 2 (POI), with
     `start[i+1] = start[i] + count[i]*3` (one cell = 12 B = 3 words; the header occupies the
     first 4 words, so `start[0] = 4`).
   - The cells: 12 B each. Lists 0/1 → `{state u16, feature u16, pointIdx u16, count u16,
     annotDesc u32}`; list 2 (POI) → `{state u16, feature u16, dlon s16, dlat s16, annotDesc u32}`.
   - The point pool: `count × {s16 dlon, s16 dlat}` at `pointIdx*4` (word offset).
   - The annotations + text: packed `{u8 size, u8 type, payload[size-2]}` after the point pool;
     text records are block-relative (`v*4`).
3. **Keep it consistent.** Blocks are contiguous and 4-byte aligned; `fileSize` must be ≥ the end
   of the last block; every IDX slot's `(offset, length)` must land exactly on a real block in the
   right profile file.

### 3.3 Cross-file consistency checklist

- IDX BBox == MAP BBox (same region).
- Every non-empty IDX slot → a real block at that offset in `<REGION>1<prof>.MAP`.
- Slot `length` (words) == block marker `len` (words) == actual block byte size / 4.
- All offsets 4-byte aligned; all list starts obey the `start[i+1]=start[i]+count[i]*3` chain.

---

## 4. The "copy from a reference" safety net

For anything you are unsure about, keep a real file of the **same region + profile** open and copy
its exact bytes for that field. This is the zero-risk fallback: you reproduce what the reader has
already been observed to accept.

Concrete workflow to author a new `<REGION>1<prof>.MAP`:

1. Open the existing `<REGION>1<prof>.MAP` as a template.
2. Keep its 32-byte header as-is, changing only `binOff`/`fileSize` if your block layout shifts
   (recompute `@0x1c = 0x400 | ((binOff − base) >> 1)` if you move the first block).
3. Keep the info-string region as-is (or truncate it and fix `infoTbl`).
4. Replace only the block data you intend to change; leave unchanged blocks byte-identical so you
   can diff-verify them later.

For a new `.IDX`, do the same: copy a real IDX of the same region, then rewrite the tile-table
slots to point at your new MAP blocks.

---

## 5. Validation

1. **Round-trip through the converter.** Run
   `map2osm_rs <yourIDX> -l 0123 -o /tmp/out` (see format reference §9). If it parses without
   errors and emits geometry, the structural layout is right.
2. **BBox containment.** Every decoded point must fall inside its tile's BBox (the converter can
   report out-of-bbox points; expect zero).
3. **Landmark spot-checks.** A few known places (a capital city, a river, the coast) should land at
   their real coordinates.
4. **Byte-diff unchanged features.** For any feature you copied rather than regenerated, the bytes
   must be identical to the reference — this catches accidental layout drift.
5. **Runtime load (if available).** Load the file in the actual navigation runtime / a debug build
   and confirm no load errors and correct rendering. This is the ultimate acceptance test.

---

## 6. Pitfall checklist (things that silently break output)

- [ ] Offsets are 4-byte aligned (slot `offset`, block offsets, table offsets).
- [ ] `length` / marker `len` are in **words** (bytes ÷ 4), not bytes.
- [ ] Marker = `0xFFFF | (len << 16)` — high 16 bits are `0xFFFF`.
- [ ] List starts chain: `start[i+1] = start[i] + count[i]*3`, and `start[0] = 4`.
- [ ] POI cell deltas are **signed** `s16` (unsigned shifts them by up to `65535 << shift`).
- [ ] Text/annotation positions are **block-relative** (`v * 4`), not file offsets.
- [ ] Coordinate = tile center + `(delta << shift)`; use the per-level `shift`.
- [ ] Upper BBox border is `south + rel_n`, not `north + rel_n`.
- [ ] BBox identical in IDX and MAP, in PAU (`deg * 2^31 / 180`).
- [ ] `multi` slot: bit 14 set, `count` in the length slot, `ptr` in the offset slot.
- [ ] No checksum/CRC is written (none observed); do not invent one.

---

## 7. Writing `.RNW` (NAV/AEX) — the road network

The RNW is far larger than MAP/IDX: a region is `NAV_ROOT.DAT` (root TCI index) +
`NAVnnnnn.DAT` (clusters) + optional `AEX/AEXnnnnn.DAT`. Full field reference is in
`RNW_format.md` §11; this is the practical bypass summary.

### What has to be byte-perfect vs. what you can fake

| field | must be exact? | how |
|-------|----------------|-----|
| Cluster header `B` (flags, bit 6 → coordType) | yes | set per your coordinate type |
| Outline (`refLon/refLat/shift/ooff/ocnt` + points) | yes | the ref is the anchor for every relative delta |
| `listFlags` + descriptor sequence | yes | one `{u16 off,u16 cnt}` per set bit, in bit order 0–10 |
| zerocells / onecells / position list | yes | the actual geometry; positions index == node index |
| DCR from/to (zerocell refs, bit 15) | yes | bit 15 = TO, clear = FROM; 1-based `(v&0x3FF)-1` |
| header `A` (u16@0), `C` (u32@4) | no — reader skips | write `0` or copy from a reference cluster |
| onecell `x` (u32@+4) | no — read but unused | write `0`/copy |
| listFlags bits 1,6,7,9,10 (skip descriptors) | no — payload ignored | emit `0,0` when the bit is set |
| zerocell `f1` | mostly — node type | `0` for a simple node |
| ci1/ci2 neighbour lists | only if multi-cluster | leave **empty** to avoid cross-cluster patching |
| TCI tile partitioning | only if you want the real tiling | one tile + one entry is enough to be loadable |
| AEX files | no — config-gated (`bRNWLoadAexData`) | omit entirely |

### Minimal viable RNW recipe

1. **One cluster, one file.** Put every node (zerocell), road (onecell) and the position list for
   your area into a single cluster in one `NAVnnnnn.DAT`. Set its ci1/ci2 lists empty so no
   cross-cluster `u16PatchCluster` fixup is triggered.
2. **Write the cluster** per §2/§3 of `RNW_format.md`: header (skip `A`,`C`; set `B`), outline,
   `listFlags`+`annOffset`, then the descriptor sequence in bit order, then each list's items at
   its `off`. Keep all offsets cluster-relative and 4-byte aligned.
3. **Write a minimal `NAV_ROOT.DAT`:** a short numeric header + a TCI with **one tile** holding
   one 8-byte entry `{u32 fileOffset, u16 length, u16 fileId}` pointing at your cluster. Copy the
   string-table / metadata block from an existing `NAV_ROOT.DAT` if you want it to look stock.
4. **Omit AEX** and leave `bRNWLoadAexData` unset.
5. **Validate:** round-trip through `rnw_extract_rs` (it should re-emit your roads with correct
   geometry), spot-check coordinates, and — if you have a debug runtime — load it and confirm no
   errors.

### RNW-specific pitfalls

- [ ] Every relative coordinate is `ref + (delta << shift)`; get the cluster `ref`/`shift` right or
      all geometry shifts together.
- [ ] Position-list count must equal the zerocell (node) count, index-aligned (§4).
- [ ] DCR indices are **1-based** (`(v&0x3FF)-1`) and local to the cluster; bit 15 sets direction.
- [ ] Onecell shape bits 1 and 5 are mutually exclusive (rel8 vs absolute); pick one per road.
- [ ] Onecell descriptor stream: each set bit is **one 4-byte slot** in bit order, and **bit 2 is the
      two inline u16 upcell refs inside its own slot — no extra bytes**. Emitting 8 for bit 2 shifts
      the absolute (bit-5) shape read and renders roads "połamana" (see `RNW_format.md` §6 note).
- [ ] Descriptor bits are walked in strict order 0–10; a missing `listFlags` bit shifts every later
      descriptor and corrupts the parse.

---

## 8. Converting FROM OSM XML — drop the closing vertex

When the MAP/IDX writer reads polygons back from OSM XML (e.g. `map2osm_rs` output, or hand-edited
OSM), remember the two formats disagree on how a ring is stored:

- **OSM:** a closed way repeats its first node as the last `<nd>` — that repeated vertex is what makes
  it an area (`<nd ref="1001"/> … <nd ref="1001"/>`).
- **Bosch (`.MAP`):** a polygon cell lists each vertex **once** (open loop); the reader closes it
  implicitly. The `count` field is the number of *distinct* vertices.

So when writing a polygon cell from an OSM way, **drop the final node if it equals the first** before
computing `count` and emitting the point pool:

```
OSM way nodes:      A B C D A        (5 refs, closed)
Bosch point list:   A B C D         (4 distinct vertices; count = 4)
```

`map2osm_rs` does the reverse on export: it takes Bosch's open vertex list and appends the first
vertex to close the ring, so polygons come out as valid OSM areas.
