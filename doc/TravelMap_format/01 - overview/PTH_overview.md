# The `CONNECT` tree and `.PTH` patch files — overview & writer reference

Companion to [`RNW_format.md`](../02%20-%20details/RNW_format.md) (the base RNW format) and
[`writer_guide.md`](../03%20-%20writer%20guide/writer_guide.md) (MAP write strategy). This document
explains the **second** map tree that ships on the card — `DATA/CONNECT` — what its files are, how the
runtime combines them with the base data in `DATA/DATA`, and what still has to be decoded before we can
write a byte-exact RNW (or reproduce the runtime's post-patch state).

> **Why this exists:** we have been reading the map from `DATA/DATA` only. This doc records the finding
> that there is a *parallel* tree `DATA/CONNECT`, that the app reads **both**, and that `CONNECT` is the
> **patch / update layer** (NissanConnect), not a different map version. Return here when writing the
> writer — it determines whether a written base file must be paired with a `CONNECT` stub, and what the
> `.PTH` delta format looks like.

---

## 1. TL;DR

- The card holds **two** sibling trees under `CRYPTNAV/DATA/`:
  - **`DATA/`** = the **base map dataset** (full tiles + road clusters + everything). This is what we
    read for OSM conversion. It is authoritative and complete.
  - **`CONNECT/`** = a **patch / index layer**: small delta files (`.PTH`), linked-table metadata
    (`.RPI`/`.TBL`), copies of the `.IDX` indexes, and a "knit" global `NAV_ROOT.DAT`. It holds **no bulk
    geometry**.
- The runtime loads a base cluster from `DATA`, then **applies any matching `.PTH` patch** from `CONNECT`
  at load time (class `rnw_tclClusterPatcher`). Classic **base + delta** map-update model, delivered over
  the connected service.
- It is **not** a different version of the map or a different app build: the `.IDX` files are byte-identical
  across both trees and both carry the same Bosch build string. `CONNECT` is the *update layer of the same
  base*.
- **For OSM→RNW we only need `DATA`.** `CONNECT` adds no geometry. It becomes relevant only if we want to
  (a) reproduce the runtime's exact post-patch state, or (b) write a file the real app will patch in place.

---

## 2. The two trees on disk

```
CRYPTNAV/
├── CFG/            LID, S_DIALOG, TIMA, VOICE        (config — not map data)
├── DATA/
│   ├── CONNECT/    LID, MAP, RNW, TMC                ← PATCH / INDEX layer
│   └── DATA/       INSTRUCT, LID, MAP, MISC, RNW,     ← BASE dataset
│                   S_DIALOG, TMC, VOICE
└── DNL/BIN/NAV     (dynamic nav layer — separate topic)
```

`MISC/` (only in `DATA`) holds `SDX_META.DAT`, the per-card license/CID file (referenced by the app as
`/DATA/DATA/MISC/SDX_META.DAT`; see top-level `cid.txt`).

### File-type census (counts of regular files)

| tree | MAP | RNW | LID | TMC |
|------|-----|-----|-----|-----|
| `CONNECT` | 411 `.IDX`, 1 `.RPI` (`RPITABLE.RPI`), 1 `.TBL` (`IDX_CNT.TBL`) — **no `.MAP`** | 1 `.DAT` (top-level `NAV_ROOT.DAT`), 67 `.PTH` — **no clusters** | 3 `.DAT` | (empty) |
| `DATA` | 411 `.IDX`, 466 `.MAP`, 76 `.TCI` | 16063 `.DAT` (the `NAVnnnnn.DAT` clusters) + per-region `NAV_ROOT.DAT` | 2109 `.DAT` | 14 `.TMC` |

Both `CONNECT/RNW/CCP` and `DATA/RNW/CCP` contain the **same 17 region codes**:
`ACL BNL CHS DEU EAD EEU ELL FRM GBI GRC IBE INT ISV MLC POL SCA TUR`.

Per-region contrast (Poland):
- `CONNECT/RNW/CCP/POL/` → exactly 4 files: `NAV____0.PTH … NAV____3.PTH` (~29 KB each, e.g.
  `NAV____0.PTH` = 29742 B).
- `DATA/RNW/CCP/POL/` → 342 entries: `AEX/` + ~340 `NAVnnnnn.DAT` clusters + `NAV_ROOT.DAT`.

So `CONNECT` holds a **handful of small delta files per region**, never the cluster bodies.

---

## 3. What `CONNECT` actually is: base + delta

The runtime has a dedicated patcher. Decompiling
`rnw_tclClusterPatcher::u16LoadPatchFile` @`0x0090a8cc` (DAPIAPP.OUT) shows the whole mechanism:

```c
// rnw_tclClusterPatcher::u16LoadPatchFile(regionIdent, patchIndex)
sprintf(buf, "NAV____%01u.PTH", patchIndex);              // build the .PTH name
rnw_tclPatchExistCache::oGetHeader(..., regionIdent);      // does a patch exist for this region?
if (header != INVALID) {
    u16LoadPatchHeader(region, buf);                       // read the patch header
    ...
    dap_tclDataAccess::u16LoadDataBlockConnect(            // ← load the bytes FROM CONNECT
        dataAccess, region, 1, buf, 0, size, outBuf);
    for (i = 0; i < sectionCount; i++)                     // read N section records
        rnw_tclClusterSection::bReadPSF(section, access);  //   (PSF = patch-section format)
}
```

Consequences:
- **`.PTH` = a cluster patch file** — a sequence of `rnw_tclClusterSection` records describing deltas to
  apply to base clusters.
- It is loaded through the **`…Connect…`** data-access path (`u16LoadDataBlockConnect`,
  `u16GetFileSizeConnectGlobal`) — i.e. explicitly from the `CONNECT` tree.
- `rnw_tclPatchExistCache` caches which patches exist per region (so a region with no patch is a clean
  no-op); `rnw_tclPSFDataCache` caches decoded PSF data.
- The applied patch is merged into the cluster by `u16PatchCluster` @`0x0090ac3c`.

> **Correction to `RNW_format.md`:** we documented `u16PatchCluster` as a "post-load fixup that resolves
> cross-cluster references by position". That is only part of it — `u16PatchCluster` is **also** where the
> `CONNECT` `.PTH` patch is applied to the base cluster (base + delta merge). Update that note.

---

## 4. Which tree does the app read? Both. (with evidence)

String literals in DAPIAPP.OUT reference **both** trees directly:

| literal | address |
|---------|---------|
| `/data/connect/map/` | `0x00e1c0e0` |
| `/data/data/map/` | `0x00e1c0f3` |
| `/DATA/CONNECT/RNW/` | `0x00e22428` |
| `/DATA/DATA/RNW/` | `0x00e2243b` |
| `/DATA/CONNECT` | `0x00e2244b` |
| `/DATA/CONNECT/RNW` | `0x00e22459` |
| `/data/connect/` | `0x00e29e59` |
| `/data/data/` | `0x00e29e68` |
| `/DATA/DATA/MISC/SDX_META.DAT` (license) | `0x00e275a6` |
| `/data/data/map/3DLM/` | `0x00e2b342` |

Filename patterns the app builds:

| literal | address |
|---------|---------|
| `NAV____%01u.PTH` | `0x00e2252b` |
| `nav_root.dat` / `NAV_ROOT.DAT` / `NAV_ROOT.DA?` | `0x00e22550` / `0x00e225fa` / `0x00e22610` |
| `IDX_CNT.TBL` / `IDX_CNT1.TBL` / `idx_cnt.tbl` | `0x00e1c38d` / `0x00e1c3a6` / `0x00e2b7aa` |
| `RPITABLE.RPI` / `rpitable.rpi` | `0x00e1c399` / `0x00e2b813` |
| `"Knitting NAV_ROOT.DAT"` (log) | `0x00e225fa` |

Cross-references to the `NAV____%01u.PTH` literal land in:
`u16RenamePatchFile` @`0x0087f9b4`, `u16LoadPatchFile` @`0x0090a8cc`, `u16PatchCluster` @`0x0090ac3c`,
`u16ProcessJob` @`0x008805dc`.

Source-file paths embedded in the binary (confirms the subsystems):
- `…/rnw/internal/base/rnw_tclClusterLoad.cpp`, `…/rnw/internal/base/rnw_tclPatchExistCache.cpp`,
  `…/rnw/internal/base/rnw_tclPSFDataCache.cpp`
- `…/rnw/internal/knitter/rnw_tclKnittingWorker.cpp`, `…/knitter/rnw_tclNavRootKnitter.cpp`,
  `…/knitter/rnw_tclPatchFile.cpp`
- `…/map/knitter/map_tclKnittingWorker.cpp`, `…/map/LinkedTable.cpp`, `…/map/LinkedTableController.cpp`

**Net:** base content from `DATA/DATA`, patches + linked-table indexes from `CONNECT`, merged at load.

---

## 5. The `.PTH` patch file — what we know vs. don't

### Known (from a raw hexdump of `CONNECT/RNW/CCP/POL/NAV____0.PTH`, 29742 B)

```
00000000: 6410 1008 2e74 0000 4010 1000 8000 0000   d....t..@.......
00000010: 0204 2c00 cc75 554e 6410 0000 fe00 0100   ..,..uUNd.......
00000020: 0284 2e00 784a 554e 6211 0000 8200 0100   ....xJUNb.......
00000030: 02c4 0000 f045 ca4e e411 0000 8200 0100   .....E.N........
00000040: 0244 0100 2c58 ca4e 6612 0000 f801 0200   .D..,X.Nf.......
00000050: 02c4 0100 d454 ca4e 5e14 0000 8200 0100   .....T.N^.......
```

- A short fixed preamble, then a run of **~16-byte records**. The first `u32` of each record is an
  **increasing offset** (`0x1064, 0x1162, 0x11E4, 0x1266, …`) — consistent with a table of section
  pointers / cluster entries.
- The records are read by `rnw_tclClusterSection::bReadPSF` (one per entry, count from the header).

### Not yet decoded (writer blockers)
1. **`.PTH` header layout** — meaning of the preamble words (`0x1064`, `0x102e7400`, `0x1040`, …), where
   the section count lives, and the per-region `patchIndex` (the `%01u`, 0–3) selection rule.
2. **`rnw_tclClusterSection::bReadPSF` record format** — what a single patch entry carries (which base
   cluster it targets, what delta it applies: replaced onecells? new nodes? attribute edits?).
3. **Why 4 `.PTH` per region** — the mapping region → patch index, and whether the 4 are size-based shards
   or something else.

---

## 6. The linked tables (`.RPI` / `.TBL`) and the knit `NAV_ROOT.DAT`

Both `CONNECT/MAP/RPITABLE.RPI` (2444 B) and `CONNECT/MAP/IDX_CNT.TBL` (5380 B) open with the same
Bosch build header, then table records:

```
00000000: c001 0800 c001 1100 1800 4800 5800 9000
00000010: 0a00 1700 b000 0000 436f 7079 7269 6768   ........Copyrigh
          "t by Robert Bosch GmbH Hildesheim 2021\0" "1B9.15. 6:37:39\0" "Configur…"
```

- Handled by `dap_map_tclLinkedTable` (`u32EstimateRPITableFileSize`, `u16StoreRPITable`,
  `u16InitializeRPITable`) + `LinkedTableController`. Log strings: `"LinkedTable: WRITE_IDX_CNT …"`,
  `"LinkedTable: WRITE_RPITABLE …"`.
- The **`NAV_ROOT.DAT`** in `CONNECT/RNW/` (202962 B) is a **global** root index, distinct from the
  per-region `DATA/RNW/CCP/<RGN>/NAV_ROOT.DAT` (e.g. POL = 1506396 B). It is produced by the
  **"knitter"** (`rnw_tclNavRootKnitter`, log `"Knitting NAV_ROOT.DAT"`) — i.e. assembled from the base +
  patch index information, not stored as a base file.

Not yet decoded: the record layout of `RPITABLE.RPI` / `IDX_CNT.TBL`, and exactly what the global
`NAV_ROOT.DAT` indexes (all regions? all patches?) versus the per-region TCI we already parse.

---

## 7. Version determination (answers "is one a different version?")

**No — same map, same build.** Evidence:
- The `.IDX` files are **byte-identical** across the two trees. Example `N1E10AA.IDX`:
  md5 `da7b0b6ab5da5dd6989bc23bdf92f20d` in both `CONNECT/MAP/` and `DATA/MAP/`.
- The `.RPI`/`.TBL` embed the same build stamp: *"Copyright by Robert Bosch GmbH Hildesheim 2021"*,
  build `1B9.15`, time `6:37:39`.
- Top-level `prod_info.txt`: `…;PowerSoftware;2.0.2.19;SFSD016GL3BM1TO;;15nm MLC;Hyperstone S8;16GB;schwarz`
  (single software version for the whole card).

So `CONNECT` is the **update/patch cut of the same base dataset**, delivered by NissanConnect — not an
alternate map or a different application revision.

---

## 8. Implications for the writer

1. **A base RNW file is self-sufficient.** The runtime treats "no patch" as a clean no-op
   (`rnw_tclPatchExistCache` returns `INVALID` → skip). So a written `DATA`-only cluster set will load and
   route; it simply reflects the **pre-patch** base state.
2. **Our OSM output reflects the base, not the post-patch state.** If a real cluster was patched by a
   `.PTH`, the car's live geometry may differ slightly from what we extracted from `DATA`. For a
   conversion target this is fine; note it if pixel-level parity with the car ever matters.
3. **Do we need to emit a `CONNECT` tree?** Only if:
   - (a) the app is observed to *require* `CONNECT` to exist (crash / hard error when absent) — **unverified**;
     or
   - (b) we want the app to apply real updates on top of our base.
   Otherwise a minimal writer can omit `CONNECT` entirely (consistent with the "avoid it" rule in
   `writer_guide.md`).
4. **If we must reproduce the runtime exactly**, decode the `.PTH` section format (§5) and the linked
   tables (§6), then emit a matching `CONNECT` stub + patched base.

---

## 9. Open questions / next steps (pick up here for the writer)

- [ ] **Does the app tolerate a missing `CONNECT` tree?** Determine whether `CONNECT` is optional at load.
      This single fact decides if the minimal writer needs a `CONNECT` stub. (Look at how
      `u16LoadDataBlockConnect` / `PatchExistCache` behave on a missing path; or test empirically.)
- [ ] **Decode `rnw_tclClusterSection::bReadPSF`** — the per-entry patch record. Find its `bRead` and map
      each field (target cluster id, delta type, payload).
- [ ] **Decode the `.PTH` header** (preamble words + section count) and the region→`patchIndex` rule.
- [ ] **Decode `RPITABLE.RPI` / `IDX_CNT.TBL`** record layout (`dap_map_tclLinkedTable`).
- [ ] **Understand the knitter** — what the global `CONNECT/RNW/NAV_ROOT.DAT` indexes vs. the per-region TCI,
      and whether it is built at first run on-device or shipped pre-built.
- [ ] **Update `RNW_format.md`**: expand the `u16PatchCluster` note to include the base+delta patch merge
      (see §3 correction).

---

## 10. Quick evidence log (reproducible)

```bash
BASE=…/Firmware/Map_unpacked/CRYPTNAV/DATA
# .IDX identical across trees:
md5sum $BASE/CONNECT/MAP/N1E10AA.IDX  $BASE/DATA/MAP/N1E10AA.IDX   # same md5
# NAV_ROOT differs (global vs per-region):
ls -la $BASE/CONNECT/RNW/NAV_ROOT.DAT            # 202962 B
ls -la $BASE/DATA/RNW/CCP/POL/NAV_ROOT.DAT       # 1506396 B
# CONNECT has no bulk data:
find $BASE/CONNECT -type f | sed 's/.*\.//' | sort | uniq -c        # IDX/DAT/PTH/RPI/TBL only
find $BASE/DATA   -type f | sed 's/.*\.//' | sort | uniq -c         # + MAP/TCI + 16k DAT clusters
# .PTH header:
xxd $BASE/CONNECT/RNW/CCP/POL/NAV____0.PTH | head
```

Ghidra (DAPIAPP.OUT) anchors: `u16LoadPatchFile` @`0x0090a8cc`, `u16PatchCluster` @`0x0090ac3c`,
`u16RenamePatchFile` @`0x0087f9b4`, `u16ProcessJob` @`0x008805dc`; path/filename string addresses in §4.
