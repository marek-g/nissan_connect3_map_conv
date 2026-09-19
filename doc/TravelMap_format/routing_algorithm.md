# Routing algorithm — where it lives and how it consumes the RNW

Status: **ENGINE DECODED (symbol level)**. The route-search core is `PROCNAV.OUT` — unpacked
(codec cracked 2026-09-19, §2), imported to Ghidra (`/tmp_re2/PROCNAV_dec.out`, 94 450 functions,
`.dynsym` names live) and its data model, cost encoding and search flow documented in §4. The
whole data-side surface the search consumes (§3) is byte-decoded. Open items are at the end (§6).
Written 2026-09-18, restructured 2026-09-19. Address anchors: `DAPIAPP.OUT` unless a section says
otherwise; §4 anchors are `PROCNAV_dec.out` vaddrs.

Companion docs: [`RNW_format.md`](02%20-%20details/RNW_format.md) (container/cluster formats;
§8a annotation dispatch, §8b NAV_ROOT global speed tables),
[`PTH_overview.md`](01%20-%20overview/PTH_overview.md) (patch flow),
[`LID_format.md`](02%20-%20details/LID_format.md) (address gazetteer feeding the destination side).
Unpacker tool: [`src/elf_decompressor/`](../../src/elf_decompressor/README.md).

## 1. Process architecture

### 1.1 Observed live processes (diag capture `diag/result/02/navdiag/`)

| process | role |
|---|---|
| `procbaselx_out.out` | platform/base process |
| `prochmi_out.out` | HMI/UI |
| `procmapengine.out` | map rendering + guidance drawing (no routing symbols inside — see 1.2) |
| `DAPIAPP.OUT` | data access + server for the `fi_tcl_*` navigation protocol (94.5k functions) |
| `procsds.out` | SD/data-card service; the on-target package INDEXER (`UNC ` SDS reader) |
| `PROCNAV.OUT` | **the route calculator** — lazy-launched, see 1.3 |

### 1.2 Where the search engine is NOT

* `procmapengine.out` (13 102 functions): zero matches for `*Route*Calc*`, `Dijkstra`, `AStar`,
  `rnw_`, `nav_`, `dap_`, `network` namespaces → renderer, not router.
* `DAPIAPP.OUT`: contains only the protocol/serialization layer of routing (constructor/copy/dtor
  stubs of `fi_tcl_NavRoute*Desc`, `fi_tcl_RouteDefinition`, `fi_tcl_RouteDescriptor`,
  `fi_tcl_RouteSegmentDefinition`, `fi_tcl_e8_NavRCalcRouteStartPoint` @0x00d4c0f4 …) plus the RNW
  **data access** classes (`rnw_tcl*`). No cost/open-list/Dijkstra-style symbols exist
  (`Cost`, `Dijkstra`, `AStar` searches: 0 hits). `rs_tclRegion::…`, `rnw_tclStepEntry` @0x0090e420
  are route *storage*, not search.

### 1.3 Where it IS: `PROCNAV.OUT`

`<card>/CRYPTNAV/DNL/BIN/NAV/COMMON/PROCNAV.OUT` (9 565 108 B, dated 2017-05-11, stack
`NAV_13.2C5P10` per the sibling `VERSION.TXT`, same build as `DAPIAPP.OUT` — the knitter sources in
DAPIAPP are `DI_SWNAVI_13.2C5P10/.../dapi/rnw/...`). The card DNL ships the whole nav stack
(`DAPIAPP.OUT` raw ELF, `PROCNAV.OUT`, `PROCDLSAVER.OUT` — the latter two packed); the head-unit FW
itself has no NAV binaries. `PROCNAV` was **not running** at capture time
(`20_PROCNAV_status.txt: NOT FOUND`) → lazy launch by the DNL deploy/start service (likely
`procsds`). `PROCNAV_dec.out` decodes to a 19 MB ARM ELF EXEC (entry 0x5ea418, .text 8.6 MB, full
`.dynsym`) — the analysis target of §4.

## 2. Unpacking the containers (ULI/XOZL codec, solved 2026-09-19)

The monitor (triton) installer `@0x108844` (blob vaddr; blob = `triton_mid.bin` minus its 64-byte
`triton_dualos` package header) reads the container header, allocates ONE output buffer of
`usize + 0x40` (the 52-byte file header is copied in at offset 0, sections start at +0x40), then
per section descriptor (stride 24: {…, packedOff, packedLen?, unpackedOff, unpackedLen?}) calls
the decompressor **`0x10bd90`** at `0x1089b4`; for magic `"ULI "` each section's OUTPUT bytes are
then post-processed in place at `0x1089d0`: `b = ~(b ^ 1)`. (`XOZL` skips the transform.)

Codec (window consts 0x801/0xffff0000 @blob 0x10c04c/50) — byte tokens, T = stream byte, B = next
byte(s); `T & 3` inline literal bytes trail EVERY token:

| token T | meaning |
|---|---|
| first: `T>0x11` | literal run of `T-0xE` bytes |
| first: `1≤T≤0xF` / `T==0` | literal run `T+6` / escape `0xFF·zeros + B + 0x15` |
| `T≥0x40` | near match: off = `((T>>2)&7)·1 + 8B + 1`, len = `(T>>5)+4` |
| `0x20..0x3F` | off = `(B>>2)+1` (off<3 ⇒ 1/2/3-byte RLE pattern fill), len = `(T&0x1F)+5`, `T&0x1F==0` ⇒ escape `0xFF·zeros+B+0x24` |
| `0x10..0x1F` | FAR match: off = `(((T&8)<<13) \| u16)>>2`, `0` ⇒ end marker; src = `out − 0x4000 − off` (reaches previous sections), len = `(T&7)+5`, escape `+0xC` |
| `T<0x10` | 2-byte match: off = `(T>>2) + 4B + 1` |

Verified: PROCDLSAVER.OUT + all 5 ISO `nor0/processes/*.out` + PROCNAV.OUT decode to EXACT header
`usize`, all valid ARM ELFs. Tool: `src/elf_decompressor/uli_unpack.py <in> <out>
[--triton triton_mid.bin]` (emulates the monitor codec verbatim under Unicorn;
`pip install unicorn`).

> Historical note: the earlier "bitstream coder @0x109070" theory was WRONG (those are region pool
> allocators; the SVC #4–#12 handler table at blob 0x555c0 is PC-card window I/O used by the
> installer's verification path, not the codec).

**Engine anatomy from `.dynsym`** (namespaces, detail in §4): `rc_rif_tclRouteInterface` (RPC),
`rc_calc_tclCALCULATIONWORK` (tables + wavefront), `rc_calc_*` (cost/policy helpers),
`rc_gcl_*`/`rs_*` (glue-area graph assembly), `rc_rlist_*` (result assembly),
`cid_FindOneCellToCoordinate`/`cid_SearchThroughClusters` (cell lookup).

## 3. Data-side pipeline the search relies on (fully decoded)

Everything below is how `DAPIAPP` presents the network to the engine; it is also exactly the
surface an `osm2rnw`-written region must satisfy.

### 3.1 Region registration (which regions/files exist at all)

* Region identity: `dap_tclRegionIdent` = `(profile<<10)|codeId`, 14 bit; FINAL table of all 17
  regions: [`doc/region_ident.tsv`](../../region_ident.tsv) — triple-confirmed (TCI-ref join,
  ci-adjacency scan, and the NAV_ROOT root-cluster records, all agreeing).
* **Region registry = `DATA/DATA/MISC/CONTENT.DAT`, NOT any `NAV_ROOT.DAT`** (decoded 2026-09-19
  from `rs_tclDataContext::u16LoadContentData` @0x008fc864 → `rs_tclContentData::u16AddAllRegions`
  @0x008fb224 → `u32AddRegion` @0x008f9e14 → `rs_tclRegion::u16FillRegionHeader` @0x0090139c). The
  whole file is one blob: header `{u32 fileSize, …, u32 globalDescOff @0x0c, u16 globalDescLen @0x10,
  u16 regionCount @0x12}` then **`regionCount` × fixed 0x34-byte `trRegionInfo` records** at
  `+0x14 + i*0x34` (the last slot is a `version=0 / nRoot=0` sentinel). Record field map (on-disk):
  `+0x00 u16 regionIdent · +0x02 u16 releaseStrOff · +0x04 u16 displayTextStrOff (both offsets into
  the blob's string pool, 0=none) · +0x06 char[16] creationDate ("1B6.<build>:HH:MM…") ·
  +0x16 u16 numberOfRootCluster · +0x18 u16 outlineOff · +0x1a u16 version (ACCEPTED when < 0xb,
  shipped = 4; ≥ 0xb ⇒ unsupported 0x218) · +0x1c u16 status · +0x1e u16(=0x98) · +0x20 u32 dataSize ·
  +0x24/+0x28 u16 outline/alias-list offsets · +0x2a u16 subCnt · +0x2c u32 fileRecordOff ·
  +0x30 u32 fileRecordLen`.
  **Cross-validated:** all 17 real records' `regionIdent` match `region_ident.tsv` exactly, and their
  `numberOfRootCluster` sums to **52 = the `rootCnt` in the global `CONNECT/RNW/NAV_ROOT.DAT`** (third
  independent confirmation of both the ident table and the gateway-cluster census). The string pool
  holds per-region multilingual `<_D>/<_S>` display names (the DATASET build strings are the separate
  `CONNECT/RNW/NAV_ROOT.DAT` provenance, §3.2).
* **Per-region file manifest** = `trFileList` blob at the record's `fileRecordOff`/`fileRecordLen`
  (parsed by `rs_tclRegion::u16FillFileList` @0x00900ca8 → `u16AddFilesOfDirectory` @0x00900b78):
  `{u16 hdr=0, u16 dirGroupCount, dirGroups[]}`, each dir group `{u32 pathStrOff, u16 fileCnt,
  fileCnt × 14-byte entries}` where an entry = 12-char lowercase filename (e.g. `nav20003.dat`) +
  2 status bytes, `pathStrOff` into the blob for a relative dir string, dir-group stride
  `= fileCnt*0xE + 6`. POL groups = `data/data/lid/CCP/POL` (LID/REL/GLOBAL_POI), `data/data/map`
  (the `N6E2102.map/.tci` + `N6E2AA.idx` shards — same tiles our trial registers), `data/data/rnw/CCP/POL`
  (341 `nav*.dat` + `nav_root.dat`), `data/data/rnw/CCP/POL/aex` (324 `aex*.dat`, ids mirroring the NAV
  files). This is the authoritative per-region file census; it confirms the AEX *file set/naming*
  layout without touching AEX *content* (§6, parked). Routing cluster load is driven by the NAV_ROOT
  root-refs + NAV inventory, NOT this list, so a trial that ADDS a `NAVnnnnn` file works unregistered
  here (our POL file-id 20001 is absent from the stock list yet loads via NAV_ROOT).
* Free-space bookkeeping per region: `rnw_…::enProcessGetFreeMediaSpace` reads
  `u32GetNumberOfAllRootCluster` @0x008f8058.

### 3.2 `NAV_ROOT.DAT` = region entry point + root-cluster gateways (decoded 2026-09-18)

Recovered end-to-end from the device's own **writer** + readers (RNW_format.md §3a has the byte
layout). Structure relevant to routing:

* Root-cluster list: `rnw_tclListDesc<nav_tclClusterInfo>::bRead` @0x0089030c — descriptor
  `{u16 payloadOff, u16 count}` at file `0x0c`, records (24 B each, stream stride 24, in-memory
  stride 0x34) parsed by `nav_tclClusterInfo::bRead` @0x008910cc — i.e. **exactly the same
  cluster-reference record** as the ci1/ci2 adjacency lists:
  `{u32 (clusterOffset & ~0x3FFF)|regionIdent, u16 length, u16 fileId, i32 refLon, i32 refLat,
  u8 shift, …}` interpreted in PAU units.
* Global header annotations (§8b of RNW_format.md): the header ListDesc at file `+0x10` carries
  the per-country **routing tables** consumed by PROCNAV (`0x16 SpeedFactors`, `0x39 SpeedLimits`,
  `0x55 StatusTable`, `0x2b InstMat`, `0x41 CatTranslate`) — MANDATORY for route calculation;
  see §4 "speed tabs" and RNW_format.md §8b for payloads + verified POL/DEU values.
 * Annotation-frame TLVs `{u16 len, u16 type}` at the header ListDesc `+0x10` (chain walked by
   `u16InterpreteAnnotations` @0x00891c4c — reads `{u16 chainOff, u16 count}`, then `count` frames,
   each `{u16 len, u16 type, payload[len-4]}`). The dataset loader ONLY parses three types and
   `bSkipBuffer`s every other (POL ground-truth: 20 frames, 0x2c len=12 = header+8 confirms):
   `0x2b` global-instruction `{u32,u32}` (`bInterpreteGlobalInstructionAnnot` @0x00891b98; POL
   `0x2014,0x1090`), `0x2c` global-areas = 2×u32 marker pair (`bInterpreteGlobalAreasAnnot`
   @0x00891be0 → stores to `this+0x58/0x5c`; POL `0x30a4,0x102c`), `0x3e` prefix name table
   (`u16InterpretePrefixTable` @0x00891c28 → `rnw_tclPrefixTable::bReadPSF` @0x0089431c =
   `{u16 nStates, (u16 stateCode, u16 nPref, (u16 flag, u16 relocStrOff→NUL str)×nPref)×nStates}`;
   POL = 1 state `0x41ec` × 6 prefixes — a display-layer road-name-composition table, NOT routing).
   The FULL area data is a separate `0x22` data block (`rnw_tclBaseWorker::u16ReadGlobalAreaRecord`
   @0x00909d94 → `bInterpreteGlobalAreaRecord` @0x00891de0), distinct from the tiny 0x2c marker.
   None of the skipped frames price per-edge cost; see §6 item 5 for the full census.
* The knitter (`rnw_tclNavRootKnitter::u16Knit` @0x008848f8, `bCreateHeader` @0x008812ec,
  `bKnitRootClusterList` @0x00883a9c, `bKnitRootClusterShapes` @0x008838b0,
  `u16UpdateHeaderRecord` @0x00881b98) is the device-side *merger* run when a second content
  source joins the root: it merges both regions' root-cluster lists + annotations and patches
  sizes/offsets — proof that the root-cluster list is the **region-composition mechanism**: a
  route reaches one region from another through these gateway clusters.
* Card evidence: every stock region root lists 1–7 gateway clusters (POL: exactly one, in
  `NAV00001` at 0x4000, len 0xe660, `regionIdent 0x402`; DEU: four — cluster #1/42/90/136 of
  `NAV00001`). **This is why Poland routes fine although no `.TCI` shard references its clusters**
  (the Kraków shard `N6E2102.TCI` is an empty stub): the engine enters POL through the root
  cluster and expands over ci-adjacency (3.4). [INFERENCE from structure + counts; on-device
  confirmation planned via `diag/routeprobe_logger.sh`.]

### 3.3 Tile→cluster index (TCI) — the *map-side* entry into the net

* Per-MAP-shard `<TILEID>.TCI` in `DATA/DATA/MAP/`; registered by **directory scan**
  `data/data/map/*.tci` with tile ids parsed from filenames
  (`dap_map_tclTCICache::u16InitTciFileList` @0x008de860 → `u16InitTciIdList` @0x008de624 →
  `dap_map_tclTileFileId::vSet`; availability `bIsTciFileAvail` @0x008de1fc).
* Tile → refs: `TCITile {u16 nPrim, u16 nAll, u32 listOff}` @0x008e019c, refs
  `dap_map_tclTCIClusterId::bRead` @0x008e01ec `{u32 packedFileOffset, u16 length, u16 fileId}`
  (order proven at the load site `u16LoadClusterIdListAndStoreInQ` @0x008de974, key disasm
  0x008deb20/0x008deb2c: struct+4 → length, struct+6 → fileId); `nPrim` refs are the loadable
  primary set.
* Routing queries map at the **finest tile level** (bbox of the query point), so a start/POI
  match yields candidate clusters; cluster selection by position, per-ref load via 3.4.

### 3.4 Cluster load + cross-cluster connectivity (the graph edges)

Load: `u16ReadCluster` @0x0088670c → `u16LoadCluster` @0x0090add4 (fileId→`NAV%05u.DAT` via
`vFileId2Name` @0x0090a16c) → `rnw_tclClusterInternal::bRead` @0x008906ec (flags 0x3060313) →
`u16PatchCluster` @0x0090ab30 (byte[+2]&0x80 → `NAV____<n>.PTH` CONNECT patch).

Connectivity as the search sees it:

* Node identity across clusters is **positional** (boundary nodes duplicated) — there is no
  global node id anywhere in the format.
* ci1/ci2 neighbour lists (24-B `nav_tclClusterInfo` records, same packing as 3.2) declare which
  neighbouring cluster records cover which boundary area; `refLon/refLat/shift` give a coarse
  local frame for quick rejection.
* Within-cluster: onecells (edges, `rnw_tclOneCellRef` 0x34-byte in-memory form; stream record:
  `u32 hdr`, `u32 storedLength` via `u32GetLength` @0x00913c7c, `u16 listFlags`, `u16 offf`),
  zerocells (junction nodes) and the overlap refs `{u16 A: onecell idx+1 | side flags<<10, u16 B:
  ci2 idx+1}` (`rnw_tclLocalCellRef::bRead` @0x00892494) bind a local node to a neighbour slot.
* `rnw_tclRefineOCList` materializes the cross links per cluster pair:
  `oGetOverlapInCluster` @0x0088c084, relevance filter `bRelevantCrossingBetween` @0x0088c78c;
  access facade `rnw_tclAccessWorker::vProcessClusterInfo` @0x00908648. Traversal costs are
  computed engine-side (§4).

### 3.5 Route result consumption

Saved-route storage on the DAPI side: `rnw_tclStoreRegionInfo` @0x0090e448, `rnw_tclStepEntry`
@0x0090e420 (per-step records); protocol answers carry `fi_tcl_NavRouteOneCellDesc` /
`fi_tcl_NavRouteInstructionDesc` / `fi_tcl_NavRouteDestinationDesc` (constructors @0x00ab5b14 /
@0x00ab5994 / @0x00ab5a48) — i.e. the engine answers with **onecell lists + instruction
annotations**, which are serialized by DAPIAPP and rendered by procmapengine (server-side bodies
in PROCNAV `rc_rif_*`/`rc_rlist_*`, §4).

## 4. Engine internals (PROCNAV_dec.out, 2026-09-19)

**Data model.** The RNW network is loaded through `dap_rnw_if_tclLoader` as in §3, but the
calculator flattens the working glue-area (set of participating clusters, `tagRSGLUEAREADESC`)
into five scratch tables built per calculation by `rc_calc_tclCALCULATIONWORK::vCreateCalcTables`
@0x0060737c (all indexed by **cluster index within the glue area**):

* `tagRS_REFERENCETAB` — cluster→local index + per-cluster OC reference list (`prGetRefTabOCElement`).
* `tagRS_RESISTANCETAB` — per (cluster, one-cell-direction) **resistance matrix**:
  `pu16GetResistanceMatrix(clusterElem, resistanceDesc)` → u16 row; row index = cobounding-OC
  slot; `u32DecodeZCResistanceEth(ocPair, idx)` decodes a traversal cost; `0xFFFF` = prohibited.
* `tagRS_ROUTETAB` — the LABEL table: element = {word0 packed `(OC & 0x3ff) | cluster<<10 | ...`,
  +4 from-direction record, +0x10 to-direction record} — each direction record carries
  {cost (24-bit), ROSA (+4), wave-chain link, flags (+8): bit 0x40000 = has-wave,
  0x8000000 = QUEUED-fresh-in-generation (set at relaxation, CLEARED when expanded),
  0x10000 = direction, 0x2000000-bit25 = entry-point marker}.
* `tagRS_MULTIWAVETAB` — the FRONTIER RING: 20-byte elements `prGetMultiWaveTabElement(u16)`
  (1-based, ≤ count@+8), `iu16Add` pushes a {props, cost} copy and returns its index; ring wraps
  at 0x3FFF entries; chain link inside the flags word at bits 10..15.
* `tagRS_UPLINKTAB` + `rc_calc_trVGSTACK` — the WAVE-GENERATION stacks: two per side slot,
  8-byte entries {packed OC|cluster<<10|dir<<30|chain<<16, props}; the VGSTACKSET header keeps
  the current/alt stack ids in bytes +8/+9, `vSwitch` swaps them (once per generation).

**Cost encoding.** A cost is a u32: `cost&0xFFFFFF` = accumulated resistance (saturating add in
`rc_calc_u32CombineROSAValues` @0x004441d8), byte3 = penalty/priority bitfield
(`rc_calc_u32SetROSAPriority` @0x00444136: `|= 0x1000000<<prio & 0xff000000`); combination ORs
the bytes and saturates the 24-bit sum. Penalty sources folded in: `rc_calc_u32CalculateROSAValue`
(toll/"ROSA"), `rc_calc_u32CalculateAvoidResistance` (avoid list), complex-intersection U-turn
(`u32GetCmplxUTurnResistance`, ×0x390), turn/vehicle/load/road-condition prohibitions
(`bTurnIsProhibited`, `bIsRestrictedOrBlocked` — restriction bits live in the OC flag word
(`ONECELL +0x1c`/`+0x14` bits 0x4000000/0x8000000): a transition whose endpoints differ on them
takes the +4 class penalty), street-class
penalties (+4/+0x10/+0x3f buckets from `rc_calc_trRouteCriteria`).

**Search flow.**
1. `rc_rif_tclRouteInterface::s32SetEntryPointsAndUpdateRoute_E` (RPC) → build glue area
   (`rc_gcl_*`: touch/overlap/bridge cluster neighbourhoods, §3.4 ci-walk + area outlines).
2. `vCreateCalcTables` per glue area; `vInitOptimizeWaveFront` @0x0060874c seeds BOTH sides:
   iterates the interface one-cell set, resolves the cluster's local index, stamps the OC's
   ROUTETAB direction record (cost from the entry property, wave number, bit 0x20000), pushes
   the OC onto the side's VGSTACK (`prGetNewEntry`; destination side additionally floods complex
   intersections `vAddComplexPathesToMWTabAndInOCsToVGStack`); then `vSwitch`.
3. `vOptimize` @0x00608acc = the loop, organized in **generations over the ALT-side VGSTACK**
   (it scans the stack of the side that was expanded last, pushes the next generation onto the
   current-side stack, then `vSwitch`es): pop = sequential index into the stack (not LIFO);
   an entry is accepted only while its wave flag carries 0x8000000 (queued) — stale duplicates
   and infinite-cost waves are skipped. For the entry OC enumerate cobounding OCs (via
   zero-cell lists / complex-intersection member lists), compute candidate cost
   (`pu16GetResistanceMatrix` row + `u32DecodeZCResistanceEth` + ROSA/avoid/turn-prohibition/
   U-turn/restriction penalties), and **relax (`rc_calc_bResistanceDescriptionIsBetter`
   @0x0044441c) into the neighbour's ROUTETAB direction record + push its MULTIWAVE index and
   a fresh 8-byte entry onto the next-generation stack (flag `|= 0x8000000`)**. Expansion is
   PRUNED two ways: an adaptive per-generation cost cutoff (next-gen min + |Δmin vs previous
   gen|·4 + 1, recomputed at `code_r0x0060b418`) and a ROSA threshold (`+0x51`, disabled
   automatically when the next-gen stack exceeds ~69 % fill). OCs marked as entry/via points
   (OCDESC `+0x14` bit 0x20000) are relaxed into but NOT expanded — this is where the two
   sides' waves meet. **The loop simply terminates when the next-generation stack comes back
   empty** (frontier exhausted / everything pruned); there is no explicit "FROM meets TO"
   break and no best-meeting-cost field — meeting detection lives downstream in route
   assembly over the ROUTETAB records. Status codes on abnormal exit: `0x22800582` (a calc
   table missing at entry), `0x2280001d`/`0x2280001f` (entry-point/complex-intersection data
   inconsistent), `0x22400019` (VGSTACK full); statuses merge by "keep higher class" (`& 0xf00000`).
4. Route assembly: prev-wave chain → `rc_rlist_*` route list (`prInitRawRouteList`,
   `rs_GetDownClOfRoute`, `rc_rlist_vPackRoutelist`), travel-time/distances via
   `rc_master_s32GetTravelTimeDelta`, annotations re-evaluated by `rc_rdb_*`
   (bEvaluateLimitation etc. against `rc_calc_trRouteCriteria`).

**Multi-route / route-update pipeline (no in-loop alternative penalties).** A second opinion is
never produced inside `vOptimize` — alternates come from **separate calculation jobs** (background
calculations with different criteria/avoid lists; `rc_master_tclRCalcMaster::
vSwitchToBackgroundRouteHandle`, `vUpdatePropertiesForBackgroundCalcFinished`) and are reconciled
with the guided route by two `rc_rlist` primitives:
* `rc_rlist_vCompareRoutes` @0x004d86b2 — walks two RouteLists in lock-step from a common start
  `cellId` over **non-link OCs**, comparing identity per step (OC id, cluster record word0+6,
  plus a strict mode on turn/mask fields) and filling a `rc_rlist_trRLPathComparisonResults`
  (0x50 B: {cellId, clusterId, routeQuality, time-s, len, fuel-µl, wid, ROSA, GBWZ} twice —
  offsets 0x08/0x0c.. and 0x30/0x34..): results say where the paths fork and what each costs.
  Divergence → status `0x2200019d`, one-path-ends-first → `0x2200019c`, side lookups failing →
  `0x2200019a/b`.
* `rc_rlist_trMergeRouteList::s32Merge` @0x004d9020 — splices a freshly calculated RouteList
  into the guided trip at a given **segment index** (segment table stride 0x3c, per-segment
  route-list handles at +0xb8): segments before the join point are `s32CopySegment`-ed from the
  old lists, from the join on `s32StoreNewSegment`-ed from the new list, old lists beyond are
  `vDeleteRouteList`-d; totals re-accumulated (`vUpdateLegthTimeFuelInfo`). This is the
  reroute/segment-replacement mechanism the HMI drives.

**Per-OC resistance model (`tclClExConverter::vCalcDrivingResistance` @0x00632808).** An ONE-CELL
record is 40 bytes: `+0x14` flags {road class bits 0..2, class<<8 road-type in 0xf00, bit
0x400000/0x40000000 = urban/freeway applicability}, `+0x24` = stored length (**2⁸ PAU ≈ 2.3886 m**,
calibrated 2026-09-19 via `rnw2osm --routetest`; rare signed-metre override = annot 0x1b). The OC is split into
at most three PARTIAL segments — built-up length (annotation 0x19, `prGetBuiltUpLengthAnnotation` @0x004e9e42, half length-units = ×2 fixed-point), freeway length (annotation 0x2F, absent from stock
RNW — class-flag fallback), remainder — each priced by `vCalcPartlyDrivingResistance` @0x006323e4
as `len·0x895/speed` with the active 8×3 speed tab ({road class × urban/open/freeway} km/h; the
ECO/user profile replaces it via criteria +0x9c; `tagCSDBlock::rfrGetSpeedTab`), plus a
`rc_tclUserSpeedFuelValues` fuel accumulator; results scaled by road-type multipliers
(×1.3/×1.5 at 0x100/0x300/0x400, ×10 at 0x700/0xa00 classes). Three accumulators come out
{time, alt-time, fuel}; `rc_calc_u32CalculateResistanceCombination(criteria, t1, t2, fuel)` folds
them into the single 24-bit cost, then `tagRS_RESISTANCETAB::vCreateOnecellResistance /
vCreateZerocellResistance` bake per-OC and per-transition (turn) costs into the RESISTANCETAB
matrices (u16 × 2^scale, `0xFFFF` = prohibited), with MOC (`moc_tclClientInterface` = live
traffic) delay manipulation (`vManipulateOnecellResistanceOnBorder/InsideGluearea`). Transition
side: `u32GetTransitionResistance` adds turn/direction costs from ZEROCELLELEMENT
(turn-prohibition, complex-intersection lane matrices) — these come from the **zerocell node
annotations** (`tagZEROCELLELEMENT::u32GetTimeValue`/`u32GetDistanceValue`/`prGetProhibQuadMatrix`
0x77/`prGetComplexIntersection`), a degree² per-junction matrix; on-disk format + PROCNAV-only
consumption in RNW_format.md §8c (the DAPIAPP display loader skips them; `osm2rnw` emits none →
PROCNAV prices all turns free).

**Where the speed tabs come from (`tclRouteCountryInfo::vLoadSpeedTables` @0x006309bc).** The
8×3 tabs are NOT compiled in — they are **global header annotations of `NAV_ROOT.DAT`** (§3.2):
type **0x16 SpeedFactors** (achievable avg km/h) and **0x39 SpeedLimits** (legal km/h, 386 =
no-limit) per Bosch country id (POL 0x41EC, DEU 0x10B5), plus **0x55 StatusTable / 0x2b InstMat /
0x41 CatTranslate** — full payloads + POL/DEU value tables in RNW_format.md §8b. A missing
0x16/0x39 makes the calculator fail with 0x21800111 before the first wavefront.

## 5. Implications for the writers (osm2rnw / trial card)

1. A shipped region **must** carry `NAV_ROOT.DAT` with a valid root-cluster list — a bare
   `NAV*.DAT` set is loadable but unreachable (done: `diag/merge_nav_root.py`).
2. `NAV_ROOT.DAT` **must** carry the global header annotations (0x16/0x39 at minimum, plus
   0x55/0x2b/0x41 for full behaviour) — without them every route request fails with 0x21800111
   (RNW_format.md §8b; the merge-based root carries them over from stock).
3. Root-cluster records must use the region's FINAL `regionIdent` in the packed `fileOffset`
   (low 14 bits) and 16-KB-aligned offsets; gateway clusters should be *big* boundary-spanning
   clusters (stock pattern: 34–59 KB) whose ci lists chain into the rest.
4. `.TCI` shards (per MAP shard) remain required for map-click/tile queries; shard filename =
   stock tile id (parsed by dir-scan); refs `{off|ident, len, fid}`, `nPrim == nAll`.
5. ci1/ci2 words pack the ident too (`osm2rnw` fixed 2026-09-18; `diag/rnwcheck.py --generated`
   enforces it).
6. Keep cluster flags bit 0x80 clear (else a CONNECT `.PTH` memcpy is attempted —
   `u16PatchCluster`) and note `.PTH` absence is *not* an error.
7. Address-search entry (destination side) needs the LID files incl. `LID20000` gazetteer
   (LID_format.md §11.2/§11.8) — orthogonal to RNW but required end-to-end.
8. The OC `x` stored length and the BuiltUpLen 0x19 payload are **NOT metres**: length unit =
   **2⁸ PAU ≈ 2.3886 m**, 0x19 in half length-units (×2 fixed-point). `osm2rnw` converts metres→
   these units via `LEN_RAW_UNIT_M` (calibrated 2026-09-19). The `seg_len` helper also had a
   longitude/latitude `cos()` axis swap (was scaling the wrong delta — inflated E–W OCs by
   1/cos(lat)≈1.56 at Kraków); fixed, verified ratio→1.00 end-to-end (OSM→RNW→`--routetest`).
   Realistic urban times still need **0x19** on settlement OCs — `osm2rnw` emits it from
   `place=*`/`boundary=place` polygons present in the input; without it every OC prices as open road.
9. What stresses the search most: ci-chain completeness (glue-area neighbour discovery) and OC
   cobounding zero-cell lists (wave expansion) — §4 steps 1/3 fail silently to "no route" when
   either is broken.

## 6. Open questions / next steps (ordered)

1. ~~Wavefront termination + multi-route pass~~ **DONE 2026-09-19** — see §4 step 3 (generation
   stacks, adaptive pruning, entry-OC meeting, status codes) and the multi-route paragraph
   (`vCompareRoutes` + `MergeRouteList::s32Merge`; alternates = separate background calc jobs, no
   in-loop penalty).
2. ~~Offline `diag/` engine-sim checker~~ **DONE 2026-09-19** — implemented as
   `rnw2osm --routetest lon1,lat1:lon2,lat2 --root NAV_ROOT.DAT --country HEX`: rebuilds the OC
   graph (junction snap-grid), reads the §8b 8×3 speed tab out of the (merged) NAV_ROOT, prices
   each OC with the per-OC 3-segment model (`len·0x895/speed`), runs a Dijkstra + component-probe,
   and prints the per-edge stored-vs-geodesic ratio. Confirmed unit model (p50=1.00 on stock DEU +
   our regenerated POL) and already caught the `seg_len` cos-axis writer bug. Catches glue/zero-cell
   graph holes (NO-ROUTE + component sizes) before the card test.
3. On-device `routeprobe` capture (car access pending): PROCNAV launch + file-open order
   (root clusters → ci chain) during a real Kraków calculation.
4. ~~`trRegionInfo` registry record width/field map~~ **RESOLVED 2026-09-19** — registry is
   `DATA/DATA/MISC/CONTENT.DAT` (NOT any `NAV_ROOT`): header `{…, regionCount@0x12}` + `regionCount`
   × **0x34-byte** records at `+0x14+i*0x34`; full field map + string pool in §3.1. Cross-validated:
   17 idents = `region_ident.tsv`, root counts sum to the 52 global gateway clusters. Native per-region
   writer needs no registry (regionIdent 0x402 reuse); a new market region = one plain 0x34 record there.
 5. **RESOLVED 2026-09-19** — NAV_ROOT global annotation frames fully characterised. Chain =
    `{u16 chainOff,u16 count}` @header `+0x10` → `count` frames `{u16 len,u16 type}`. Ground-truth
    census (POL & DEU per-region, 20 frames identical type set; CONNECT superset-sized + adds
    `0x53`, drops `0x4d`): `0x3×5:20, 0x11:12, 0x3e:36, 0x16:116, 0x41:84, 0x2b:12, 0x2c:12,
    0x39:56, 0x55:464, 0x56:68, 0x4d:12, 0x4e:12, 0x60:10, 0x7c:24, 0x89:156, 0x8a:16`.
    Dataset loader (`u16InterpreteAnnotations`) parses ONLY `0x2b`(instruction 2×u32), `0x2c`(area
    marker 2×u32), `0x3e`(prefix name table) — all others `bSkipBuffer`'d. Of the skipped ones the
    **speed subsystem** reads `0x16/0x39/0x41/0x55/0x56` (§8b); the rest are ident-keyed scalar
    tables (regionIdent `0x402`/globalIdent `0x41ec`: `0x11 {n,(u16 id,u32 v)}`, `0x60
    {n,(u16 id,u16 v)}`, `0x4e {n,(u16 regionIdent,…)}`, `0x8a {n,(u16 regionIdent,u16 globalId,…)}`
    — 1 entry per-region, expanded to all-regions in CONNECT), plus opaque `0x3×5` 16-byte
    sub-records, region-only `0x4d` 2×u32, and variable display tables `0x7c`/`0x89`. **None price
    per-edge routing cost**; the merge-based root carries them verbatim, so our generator is unaffected.
6. `AEX` auxiliary cluster data — container + wrapper decoded (RNW_format.md §13; extended road
   attributes, mostly speed limits); sub-record type table (AEX-own space, 0x46…) parked per
   user decision — speed-limit need meanwhile covered by §8b globals. (Per-region AEX *file set +
   naming*, `aex2NNNN.dat` mirroring `nav2NNNN.dat`, now documented in §3.1 CONTENT.DAT manifest.)
7. Place-polygon sourcing for the trial: krk_frag2.osm has no `place` boundaries — regenerate the
   trial RNW from an extract with boundaries (activates the 0x19 writer).
