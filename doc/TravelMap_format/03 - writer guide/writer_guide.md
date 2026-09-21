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
| info region `[0x20 .. partOff*4)` | metadata | **copy verbatim** from a stock IDX of the same dataset | not read by the geometry path (see `MAP_format.md` §11), but copying is free and matches every shipped file — zero risk |
| partition table (4 × 12 B) | known | **write** — see §3.1 | fully decoded |
| tile-table slots | known | **write** — `{regProf, length, offset}` | the core pointer |
| `multi` slot | known | **write** when a tile spans profiles — see §2 | decoded this pass, verified on data |
| empty tiles | known | **write** `{regProf=0x8000, len=0, off=0}` — profile bits MUST be cleared | bit15 alone isn't enough: leaving the profile bits set points an "empty" tile at a real MAP file (off=0 → parses the header as cells → reboot). Stock always uses exactly `00 80 00 00 00 00 00 00`. |

### `.MAP`

| field | status | what to do when writing | why it's safe |
|-------|--------|-------------------------|---------------|
| header `binOff` (0x00) | known | **write** — offset of first data block | reader uses it |
| header `infoTbl` (0x02) | const | **const** `0x34` (52) or **copy** | points at metadata region |
| header `fileSize` (0x04) | known | **write** — total file size in bytes | reader uses it |
| header BBox (0x08..0x17) | known | **write** — identical to the IDX | drives coordinate decoding |
| header `@0x18` | unclear | **copy** from reference, or **const** `8` | stored by reader; 8 in every observed file |
| header `@0x1a` | unclear | **copy** from reference, or **const** `4` | stored by reader; 4 in every observed file |
| header `@0x1c` | const-able | **copy** — comes along with the template; do not fabricate (stock binOff=0x7bc → `04 04`) | stored by reader; correlates with first-block offset |
| header `@0x1e` | ignored | **anything** (template carries `0x8400 \| prof`) | reader's header object omits this field entirely |
| info region `[0x20 .. binOff)` | metadata | **copy verbatim** — keep `binOff` at its stock value (~0x7bc) | not read by the geometry path, but copying is free and matches every shipped file. The #02 reboot was NOT this region — it was self-inconsistent IDX↔block offsets / an unaligned container (OOB in `u16Convert`, `MAP_format.md` §11) |
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
 0x20  ├ info region  (copy the stock IDX bytes verbatim — do not zero)
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
    - data present in several profiles → a `multi` slot (§2) (header = bare `0x4000`, no profile bits)
    - no data → **exactly `{0x8000, 0, 0}`** — the profile bits MUST be cleared. Leaving them set
      (`0x8000 | prof`) points an "empty" tile at a real MAP file (off 0 parses its header as cells),
      which is the #04 reboot. Never OR the profile into an empty marker.
    - `length` is the block size **in 4-byte words** and must equal the marker length in the MAP.

    > **Which profile?** Profile ids are fixed per region by a product metadata catalog (`resinf`) —
    > only reuse ids already declared for the target region; do not invent new ones (a mismatch drives
    > a fatal error at region init). Empirically: `0I` = hydrography overlay (coast/water lines +
    > water areas, **no POI / no settlement polygons**) and is present in every region; land content
    > goes into one or more separate shard profiles. A tile references its land shard (+ `0I` when it
    > also has water). Do **not** put roads/POI/urban into `0I`. See MAP_format.md §11 for the measured
    > evidence and the still-open #04/#05 reboot link.


### 3.2 Generate the `.MAP`

Layout:

```
0x00  ┌ header (32 B)
 0x20  ├ info region (copy the stock MAP bytes verbatim; keep binOff = its stock value, ~0x7bc)
      │   ...
binOff  ┌ block 0  {marker, 3×(start,count), cells…, point pool…, annotations/text…}
        ├ block 1  (contiguous, 4-byte aligned)
        └ … up to fileSize
```

Steps:

1. **Write the header.** Best: clone the stock file's whole `[0 .. binOff)` prefix as a template
   (`osm2map` ships `templates/map_header.bin` = 0x7bc, `templates/idx_header.bin` = 0x1f8) and
   patch only `fileSize`, BBox (and `@0x1e` profile). Keeping the stock prefix byte-identical is free
   insurance even though the geometry path (`vConvertMapData`) does not read this region — see §11. The
   invariants that **do** break rendering are: 4-byte container alignment, and every IDX slot landing on
   a real block whose marker `len` matches the slot length (an OOB read in `u16Convert`, or the `&3`
   unaligned guard, is what reboot-looped #02). Otherwise write `binOff`, `infoTbl=0x34`, `fileSize`,
   BBox identical to the IDX, and copy `@0x18/@0x1a/@0x1c` from a reference file of the same dataset.
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
- **The `DATA/CONNECT/MAP/<REGION>AA.IDX` twin MUST ship with the same bytes** as the
  `DATA/DATA/MAP` one (stock: byte-identical, md5-verified; `assemble_trial.py` does this — a
  manual assembly that skips it leaves the card's previous-trial CONNECT IDX in charge and the
  engine then reads tiles from the wrong MAP file: blank render + loader never completes
  — trial 27 rev A, card-confirmed 2026-09-20).

---

## 4. The "copy from a reference" safety net

For anything you are unsure about, keep a real file of the **same region + profile** open and copy
its exact bytes for that field. This is the zero-risk fallback: you reproduce what the reader has
already been observed to accept.

Concrete workflow to author a new `<REGION>1<prof>.MAP`:

1. Open the existing `<REGION>1<prof>.MAP` as a template.
2. Keep its 32-byte header **and the entire info region `[0x20 .. binOff)`** verbatim; change only
   `fileSize` (and BBox). Do not move `binOff` and do not truncate this region — keep geometry at the
   stock `binOff` so the reader sees exactly the metadata layout it accepts.
3. Replace only the block data you intend to change; leave unchanged blocks byte-identical so you
   can diff-verify them later.

For a new `.IDX`, do the same: copy a real IDX of the same region, then rewrite the tile-table
slots to point at your new MAP blocks.

---

## 5. Validation

1. **Round-trip through the converter.** Run
   `map2osm <yourIDX> -l 0123 -o /tmp/out` (see format reference §9). If it parses without
   errors and emits geometry, the structural layout is right.
2. **BBox containment.** Every decoded point must fall inside its tile's BBox (the converter can
   report out-of-bbox points; expect zero).
3. **Landmark spot-checks.** A few known places (a capital city, a river, the coast) should land at
   their real coordinates.
4. **Byte-diff unchanged features.** For any feature you copied rather than regenerated, the bytes
   must be identical to the reference — this catches accidental layout drift.
5. **Strict structural gate — `diag/tmcheck.py`.** Run `python3 diag/tmcheck.py <dir_or_IDX>`: it
   walks every block, checks the `0xFFFF|(len<<16)` marker equals the real length, that every slot /
   sub-entry / annotation / text offset stays **inside its block**, and reports the nearest stock
   file's verdict. It PASSES on shipped N6E2 (8054 blocks) — a PASS means our reader model matches what
   the car renders, so an output that FAILS here will fault or reboot on the HU. Use it as a
   pre-flash gate.
6. **Runtime load (if available).** Load the file in the actual navigation runtime / a debug build
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
- [ ] `multi` slot: bit 14 set, `count` in the length slot, `ptr` in the offset slot; header regProf is
      exactly `0x4000` (no profile bits). Sub-entries carry the real `{regProf,len,off}`.
- [ ] **Empty tile = `{0x8000, 0, 0}` with profile bits cleared** — never `0x8000|prof`. Leaving the
      profile set redirects the "empty" slot to a real MAP file at off=0 → header parsed as cells → OOB
      reboot (this is what broke trials #02/#04 while stock and untouched-IDX edits were fine).
- [ ] Every IDX slot `(offset,length)` lands exactly on a real block whose marker `len` matches, and the
      container is 4-byte aligned — these (not the info region) are what OOB-fault / unaligned-guard.
- [ ] MAP/IDX prefix `[0 .. binOff)` copied verbatim from stock (free insurance; matches shipped files).
- [ ] No checksum/CRC is written (none observed); do not invent one.

---

## 7. Writing `.RNW` (NAV/AEX) — the road network

The RNW is far larger than MAP/IDX. A region is `NAVnnnnn.DAT` (clusters) + `NAV_ROOT.DAT` (a *region
metadata* file: structure header, region profile outlines, annotation/text blob — **not** the position
index) + optional `AEX/AEXnnnnn.DAT`. The thing that actually makes a cluster *findable* lives outside
the RNW tree, in the MAP folder — see the "Cluster locator" section below. Field reference is in
`RNW_format.md`; this is the practical bypass summary.

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
| zerocell `f1` (border/rim marker) | **yes, for stitched multi-cluster** | set bit 1 on every duplicated boundary node |
| onecell `offf` | **yes — must be nonzero** | `listFlags=0` → point offf at a valid 4-byte region or the reader **skips the onecell** |
| cluster **16 KB alignment** | **yes (multi-cluster)** | `rnw2osm` finds clusters on a 0x4000 step; pad each cluster blob to a 16 KB boundary and keep cluster_id ≠ 0 |
| ci2 overlap *links* | no — recommended (byte-faithful) | `osm2rnw` emits them by default (cluster listFlags bit3 + onecell bit4 + ci2 records); `--no-overlaps` falls back to border-marker-only. Validated: `rnw2osm … overlaps=N/N` |
| cluster **header flags byte @0x02** bit 0x80 | **must stay CLEAR** | bit 0x80 ⇒ "patched, patchId=flags&7" and the runtime memcpy's `data/connect/rnw/<prof>/<region>/NAV____<id>.PTH` over the cluster → corruption. osm2rnw uses flags=0x0001 (bit0 coordmode), never 0x80 |
| cluster locator (the `.tci` in `data/data/map/`) | **required for in-car load** | see "Cluster locator" below — this (not `NAV_ROOT`) is how a lon/lat finds a cluster. Emitted by `osm2rnw --tci --map-idx <step-1 MAP out>`; `osm2map` no longer emits `.tci` |
| AEX files | no — config-gated (`bRNWLoadAexData`) | omit entirely |

### Implemented: `osm2rnw`

`src/osm2rnw` is a working writer that inverts `rnw2osm`'s `parse_cluster` byte-for-byte. It reads OSM
(PBF or XML), keeps drivable `highway=*` ways, and emits multi-cluster `NAVnnnnn.DAT`. Validated by
round-trip: `osm2rnw in.osm.pbf -o out/` then `rnw2osm out/<REGION> -b … -o rt.osm` reproduces the road
count, geometry, connectivity, street names and `highway=*` classes. Recipe / decisions:

1. **Many clusters, not one.** Split by a quadtree on segment midpoints, **capped at 1024 onecells**
   (10-bit DCR ref) — the tool targets 700. A single >1024-onecell cluster is unaddressable and also
   overflows the u16 cluster-relative offsets.
2. **Every way → straight onecells at every vertex.** No inline shape (rel-delta shape is the one part
   easy to get wrong); geometry comes entirely from the two endpoint nodes, which is exact.
3. **Shared vertex duplicated at identical PAU + border marker.** A vertex used by >1 cluster is stored
   once per cluster at the *same* decoded coordinate and its zerocell gets `f1` bit 1 (rim). The reader
   stitches these via the border-marker test — this alone reconnects the graph (validated under
   `rnw2osm --no-snap`). Explicit ci2 overlap *links* are the byte-faithful upgrade (Phase 2a) but not
   needed for the offline round-trip.
4. **Pad every cluster to 16 KB**, cluster_id ≠ 0, so `rnw2osm`'s cluster scan finds each blob.
5. **Names**: onecell bit 0 → a `{annOff, cnt=1}` annot → `{u16 size=6, u16 type=0x3C, u16 textOff}` →
   text record `{u8 nVar=1, u8 flag=0xA7, u8 len, bytes}`; the `0xA7` flag is required or the renderer
   drops the name. Class → `highway` inverts `display_class` exactly (see §6a).
6. **Validate**: `rnw2osm out/<REGION> [--no-snap] -b W,S,E,N -o rt.osm`; compare `highway`/`name`
   histograms vs the input (expect drivable-name parity, no invented names). **Car-boot** load is still
   gated on the `.tci` locator (emitted by `osm2rnw --tci`), independent of the cluster bytes being correct here.

### Cluster locator: how the car finds a cluster (`.tci`) — RESOLVED & IMPLEMENTED (`osm2rnw --tci`)

Reverse-engineered from `DAPIAPP.OUT` (call chain `u16SendUniqueIdList` @0x84a2f8 →
`u16GenerateTileIds`/`u16CalcTileId` @0x8cbe0c → `TCICache::u16GetClusterId` @0x8df958 →
`u16LoadClusterIndexTile` @0x8df4a0 → `ClusterLoad::u16LoadCluster` @0x90add4). **The position→cluster
index is a per-tile `.tci` file in `data/data/map/`, NOT `NAV_ROOT.DAT`.** (`NAV_ROOT`'s 0x2014-byte header
holds a *root-cluster/outline* list + annotations; its multi-MB body is admin-area/global-file data; no
runtime reader parses a coordinate index out of it.)

**Algorithm** (a lon/lat → cluster):
```
for each map level L=0..3:
    idxLon = (lon - LLlon) / extLon[L];  idxLat = (lat - LLlat) / extLat[L]   # integer div, PAU
    tileId[L] = Σ_{levels<l} tileCount  +  abs(idxLat*heightCount[L] + idxLon) # TILECNT=[1,25,2500,250000]
(ring,segment) = tile grid → .tci FILE  "%c%d%c%d%s.tci" = N/S+ring, E/W+segment, 3-char profile-code
                                        (e.g. "N6E211A": ring=6, seg=2, profile "11A")  @0x8e12a0
section = tileId/125;  TCITile entry @ partition.offsetList[L] + tileId*8   # 125 tiles / 1000-B section
  TCITile = { u16 nPrimClusters, u16 nAllClusters, u32 clusterListFileOffset }
clusterRefs @ clusterListFileOffset = nAllClusters × TCIClusterId
  TCIClusterId = { u32 fileOffset, u16 length, u16 fileId }     # into data/rnw/<PROF>/<REGION>/NAV%05u.DAT
                                                                  # (len@+4, fid@+6 — disasm @0x8deb20)
load `length` bytes at `fileOffset` in NAV%05u(fileId).DAT  →  that cluster
```
IMPORTANT packing detail (found in `rnw_tclIDBase`): `TCIClusterId.fileOffset` is *packed*. The cluster's
byte offset lives in bits 14–31 (`u32GetClusterFileOffset = w & 0xffffc000`), so clusters are **16 KB
aligned**; the **low 14 bits (`w & 0x3fff`) carry the region ident** (`u16GetRegionIdent`) used to pick the
`<REGION>` folder. i.e. `ref.fileOffset = (clusterByteOffset & 0xffffc000) | regionIdent`. `regionIdent`
itself is `(profile<<10)|codeId` (`u16GetProfile` = bits 10–13, `u16GetCodeId` = bits 0–9); every shipped RNW
region uses profile 1 (`CCP`), so all values are `0x400|codeId`. `fileId` is
literally the `%05u` of the NAV filename (`vFileId2Name` = `sprintf("NAV%05u.DAT", fileId)`, @0x90a7dc).
The reader loads a block sized for `nAll` refs but pushes only the first `nPrim` to the queue, so write
`nPrim = nAll = refs.len()`. Routing always queries the **finest** level (`this[0x3b0]==3`), with no
ancestor walk, so a cluster must be listed in **every finest-level tile its bbox overlaps** (coarse levels
are populated for the render/LOD subsystem). Region-folder + `fileId` are therefore region-context inputs,
not encoded in the tile grid.
The `.tci` header (0x14 B): u16@0,@2; u32@4=filesize; **u16@0x08=partitionTableFileOffset**;
**u16@0x0A=partitionCount (==4)**; rest unused by the reader. Partition table = 4 × `TCIPartition` (0xC B):
`u8 mapLevel, pad×3, u32 maxTileIdx (=TILECNT[L]), u32 offsetListFileOffset`. Confirmed against stock
`N6E210I.TCI` (partitions: lvl 0/1/2/3 → maxIdx 1/25/2500/250000). `fileId→NAV%05u.DAT` confirmed
(`vFileId2Name` @0x90a7dc). NOTE: the ASCII magic "TILE_CLUSTER_INDEX" is a knitter signature the runtime
never checks. The per-level `extLon/extLat/LL` (a `WorldTilePartition` table) are the SAME tile geometry
MAP uses (`osm2map` already encodes `SHIFTS=[13,10,7,4]`, `TILECNT=[1,25,2500,250000]`).

**Consequence for a hand-built region.** Correct cluster bytes are necessary but NOT sufficient to boot:
the car finds a cluster only through a `.tci` tile entry. Ownership of the `.tci` follows the *data* it
indexes (RNW clusters), so **`osm2map` no longer emits any `.tci`** (it writes only `.IDX`/`.MAP`) and
**`osm2rnw` generates it** — pass `--tci --map-idx <step-1 MAP out>`. The tile grid (region bbox +
`SHIFTS`/`TILECNT`) is read from that step-1 `<REGION>AA.IDX`, guaranteeing `osm2rnw`'s tile indices match
`osm2map`/the runtime exactly (not from the OSM data extent). The `regionIdent` packed into ref bits 0–13 is
now **derived from `--region`** via a baked table (`REGION_IDENT` in the source, mirrored in
`doc/region_ident.tsv`, dumped by `--list-region-ids`); `--region-ident` only overrides it, and the shard
name/profile (`--tci-file` / `--tci-prof`, default `<mapregion>1<base32(prof)>`) remain region-specific
inputs. **Region-code table (how it was extracted):** the authoritative `codeId→country` map is a
deserialized region-metadata table (`dap_tclRegProfToString`, built by `u16InterpreteRegionMetaDataJob`, fed
from resinf), with no plaintext list on disk; but the numbers are recoverable from the data directly — a
region's `regionIdent` repeats ~1e3× in its own `RNW/CCP/<code>/NAV_ROOT.DAT` vs ~1 elsewhere, cross-checked
against the constant `regionIdent` in that region's stock `.TCI` shards. That yields the 17-value table
(`{1,2,3,4,6,7,8,9,10,11,12,13,14,17,18,22,42}`, all profile 1); BNL/MLC are by elimination. **Geography:**
a region folder holds the roads of its own geography, so the target's folder is chosen by where its clusters
actually are. Poland/Krakow (krzeszowice) is region `POL` (`0x402`, shard `N6E2102.TCI` — uniform `0x402`):
`rnw2osm` on `CCP/POL` yields ~5888 roads near Krakow while `CCP/EEU` yields 0. `EEU` (`0x42a`, shard
`N6E211A.TCI`) is the separate eastern-Europe aggregate (HU/UA/BY) on the same `N6E2` MAP grid. So Poland
swap targets use `--region POL`. Validated offline: 52-cluster krzeszowice → `.tci` with `nPrim==nAll`, every
cluster at all 4 levels, all refs in-bounds with `regionIdent` `0x402` (now derived, not hardcoded), geometry
self-consistent.

The alternative remains: **reuse stock addressing** — re-author clusters *in place* (same `fileId`s, same
16 KB-aligned `fileOffset`s, same `length` as the stock `.tci` already references) so the stock `.tci` keeps
pointing correctly. `osm2rnw`'s `--tci` path takes the *first* option. Either way the cluster geometry
itself is verifiable offline via `rnw2osm`; the remaining unverified step is an actual in-car boot.



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
- [ ] Cluster header **flags byte @0x02 bit 0x80 must be clear** — else the loader memcpy-patches a
      `NAV____<flags&7>.PTH` from `data/connect/rnw/...` over the cluster (`u16PatchCluster` @0x90ab30),
      corrupting regenerated data. Also remove/neutralise stale `data/connect/rnw/**/*.PTH` for the region.
- [ ] The car finds the cluster via a `data/data/map/*.tci` tile entry, not `NAV_ROOT` — without a matching
      `.tci` (or reused stock cluster addressing) the data is correct but never loaded. See "Cluster locator".

---

## 8. Converting FROM OSM XML — drop the closing vertex

When the MAP/IDX writer reads polygons back from OSM XML (e.g. `map2osm` output, or hand-edited
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

`map2osm` does the reverse on export: it takes Bosch's open vertex list and appends the first
vertex to close the ring, so polygons come out as valid OSM areas.
