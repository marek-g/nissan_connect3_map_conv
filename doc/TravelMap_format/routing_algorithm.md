# Routing algorithm — where it lives and how it consumes the RNW

Status: **PARTIAL**. The route-**search** core was located (a separate process binary, found on the
data card but still ULI-packed — see §1.3/§7). Everything the search consumes through `DAPIAPP`
(region registration, root-cluster gateways, cluster loading, cross-cluster connectivity) is
**fully decoded** and documented here with code anchors. Written 2026-09-18; all addresses are in
`DAPIAPP.OUT` unless noted (Ghidra project "Nissan Ghidra Project").

Companion docs: [`RNW_format.md`](02%20-%20details/RNW_format.md) (container/cluster formats),
[`PTH_overview.md`](01%20-%20overview/PTH_overview.md) (patch flow),
[`LID_format.md`](02%20-%20details/LID_format.md) (address gazetteer feeding the destination side).

## 1. Process architecture

### 1.1 Observed live processes (diag capture `diag/result/02/navdiag/`)

| process | role |
|---|---|
| `procbaselx_out.out` | platform/base process |
| `prochmi_out.out` | HMI/UI |
| `procmapengine.out` | map rendering + guidance drawing (no routing symbols inside — see 1.2) |
| `DAPIAPP.OUT` | data access + server for the `fi_tcl_*` navigation protocol (this binary, 94.5k functions) |
| `procsds.out` | SD/data-card service; the **DNL installer / ULI unpacker** (only FW binary containing the `ULI ` magic, string @`0x006b8b78`) |

### 1.2 Where the search engine is NOT

* `procmapengine.out` (13 102 functions): zero matches for `*Route*Calc*`, `Dijkstra`, `AStar`,
  `rnw_`, `nav_`, `dap_`, `network` namespaces → renderer, not router.
* `DAPIAPP.OUT`: contains only the protocol/serialization layer of routing (constructor/copy/dtor
  stubs of `fi_tcl_NavRoute*Desc`, `fi_tcl_RouteDefinition`, `fi_tcl_RouteDescriptor`,
  `fi_tcl_RouteSegmentDefinition`, `fi_tcl_e8_NavRCalcRouteStartPoint` @0x00d4c0f4 …) plus the RNW
  **data access** classes (`rnw_tcl*`). No cost/open-list/Dijkstra-style symbols exist
  (`Cost`, `Dijkstra`, `AStar` searches: 0 hits). `rs_tclRegion::…`, `rnw_tclStepEntry` @0x0090e420
  are route *storage*, not search.

### 1.3 Where it IS: `PROCNAV.OUT` (found 2026-09-18)

`<card>/CRYPTNAV/DNL/BIN/NAV/COMMON/PROCNAV.OUT` (9 565 108 B, dated 2017-05-11, stack
`NAV_13.2C5P10` per the sibling `VERSION.TXT`, same build as `DAPIAPP.OUT` — the knitter sources in
DAPIAPP are `DI_SWNAVI_13.2C5P10/.../dapi/rnw/...`). The card DNL ships the whole nav stack
(`DAPIAPP.OUT` raw ELF, `PROCNAV.OUT`, `PROCDLSAVER.OUT` — the latter `XOZL`-packed); the head-unit
FW itself has no NAV binaries. `PROCNAV` was **not running** at capture time
(`20_PROCNAV_status.txt: NOT FOUND`) → started on demand (lazy launch by the DNL deploy/start
service, likely `procsds`).

`PROCNAV.OUT` is not ELF on disk: container `"ULI "` — **SOLVED 2026-09-19, see 1.4.**

### 1.4 ULI/XOZL container unpacker + engine location (2026-09-19)

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
`usize`, all valid ARM ELFs. Tool: `src/elf_decompressor/uli_unpack.py <in> <out> --triton <triton_mid.bin>`
(emulates the monitor codec verbatim under Unicorn; `pip install unicorn`). PROCNAV decodes to a
19 MB ARM ELF EXEC (entry 0x5ea418, .text 8.6 MB, full .dynsym 1.9 MB) — imported to Ghidra as
`/tmp_re2/PROCNAV_dec.out` (94 450 functions, dynamic symbols resolved).

**Engine anatomy (from PROCNAV .dynsym):** the search lives in the `rc_*` namespaces —
`rc_rif_tclRouteInterface` (RPC: `s32SetEntryPointsAndUpdateRoute_*`), `rc_calc_tclCALCULATIONWORK`
(the workhorse: `vCreateCalcTables` builds FIVE per-glue-area tables — `tagRS_REFERENCETAB`,
`tagRS_RESISTANCETAB`, `tagRS_ROUTETAB`, `tagRS_MULTIWAVETAB`, `tagRS_UPLINKTAB`;
`vInitOptimizeWaveFront` seeds the frontier; **`vOptimize` @0x608acc is the main wavefront loop**;
`u32CalculateResistanceToEntryPoint`, `u32GetResistanceOfEntryPointToDestination`), cost/policy
helpers `rc_calc_*` (`rc_calc_bResistanceDescriptionIsBetter` = the relaxation compare,
`rc_calc_u32CalculateROSAValue`/`CombineROSAValues` = toll/time penalty combine,
`tagZEROCELLELEMENT::bTurnIsProhibited`, `tagONECELLELEMENT::bIsRestrictedOrBlocked`), graph
assembly `rc_gcl_*`/`rs_*` (glue clusters, neighbours, root clusters), result assembly
`rc_rlist_*`, cell lookup `cid_FindOneCellToCoordinate`/`cid_SearchThroughClusters`.
`vOptimize` iterates the MULTIWAVE frontier queue; for each wave entry it walks the OC's route
edges, computes candidate cost = prev + resistance matrix value (`tagRS_RESISTANCETAB::
pu16GetResistanceMatrix`, `u32DecodeZCResistanceEth`) + ROSA penalty + prohibition/avoidance
penalties (encoded in the high bits of the 32-bit cost: 0x4000000/0x8000000 restriction flags,
`+4/+0x10/+0x3f` class penalties), relaxes into ROUTETAB elements, marks processed with bit
0x8000000 — i.e. **Dijkstra label-correcting over the ONE-CELL (OC) graph with an explicit
wavefront table**, seeded bidirectionally (from/to), over the glue-area cluster neighbourhood.

## 2. Data-side pipeline the search relies on (fully decoded)

Everything below is how `DAPIAPP` presents the network to the engine; it is also exactly the
surface an `osm2rnw`-written region must satisfy.

### 2.1 Region registration (which regions/files exist at all)

* Region identity: `dap_tclRegionIdent` = `(profile<<10)|codeId`, 14 bit; FINAL table of all 17
  regions: [`doc/region_ident.tsv`](../../region_ident.tsv) — triple-confirmed (TCI-ref join,
  ci-adjacency scan, and the NAV_ROOT root-cluster records, all agreeing).
* On card load, `rs_tclRegion::u16FillRegionHeader(char const* strPool, trRegionInfo*)`
  @0x0090139c builds each region header from a `trRegionInfo` registry record: `ident` @+0x20,
  `numberOfRootCluster` @+0x2c (stored via
  `rs_tclRegionHeader::vSetNumberOfRootCluster`, setter @0x008ff380), `size` @+0x40, `status`
  @+0x38, outline/bbox via `vSetOutline(trAbsClstrOutline*)`, release/displayText string-pool
  offsets @+0x04/+0x08, creation date @+0x0c; version field @+0x34 must be ≥ 11. The registry
  records come from the per-region `NAV_ROOT.DAT` table (offset `u16@0x0a`, see 2.2).
* Free-space bookkeeping per region: `rnw_…::enProcessGetFreeMediaSpace` reads
  `u32GetNumberOfAllRootCluster` @0x008f8058.

### 2.2 `NAV_ROOT.DAT` = region entry point + root-cluster gateways (decoded 2026-09-18)

Recovered end-to-end from the device's own **writer** + readers (RNW_format.md §3a has the byte
layout). Structure relevant to routing:

* Root-cluster list: `rnw_tclListDesc<nav_tclClusterInfo>::bRead` @0x0089030c — descriptor
  `{u16 payloadOff, u16 count}` at file `0x0c`, records (24 B each, stream stride 24, in-memory
  stride 0x34) parsed by `nav_tclClusterInfo::bRead` @0x008910cc — i.e. **exactly the same
  cluster-reference record** as the ci1/ci2 adjacency lists:
  `{u32 (clusterOffset & ~0x3FFF)|regionIdent, u16 length, u16 fileId, i32 refLon, i32 refLat,
  u8 shift, …}` interpreted in PAU units.
* Annotation TLVs `{u16 len, u16 type}` (`u16InterpreteAnnotations` @0x00891c4c): type `0x2b` =
  global-instruction `{u32,u32}` (`bInterpreteGlobalInstructionAnnot` @0x00891b98), `0x2c` =
  global-areas `rnw_tclAreaCtrl::bReadPSF`, `0x3e` = prefix table; others skipped. Appended
  global-area/instruction records behind the header record; runtime pulls the area block via
  `dap_tclDataAccess::u16LoadDataBlockConnectGlobal(…,0x22,1,"NAV_ROOT.DAT",off,size)`
  (`rnw_tclBaseWorker::u16ReadGlobalAreaRecord` @0x00909d94 → interpretation
  `rnw_tclGlobalDatasetData::bInterpreteGlobalAreaRecord` @0x00891de0).
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
  cluster and expands over ci-adjacency (2.4). [INFERENCE from structure + counts; on-device
  confirmation planned via `diag/routeprobe_logger.sh`.]

### 2.3 Tile→cluster index (TCI) — the *map-side* entry into the net

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
  match yields candidate clusters; cluster selection by position, per-ref load via 2.4.

### 2.4 Cluster load + cross-cluster connectivity (the graph edges)

Load: `u16ReadCluster` @0x0088670c → `u16LoadCluster` @0x0090add4 (fileId→`NAV%05u.DAT` via
`vFileId2Name` @0x0090a16c) → `rnw_tclClusterInternal::bRead` @0x008906ec (flags 0x3060313) →
`u16PatchCluster` @0x0090ab30 (byte[+2]&0x80 → `NAV____<n>.PTH` CONNECT patch).

Connectivity as the search sees it:

* Node identity across clusters is **positional** (boundary nodes duplicated) — there is no
  global node id anywhere in the format.
* ci1/ci2 neighbour lists (24-B `nav_tclClusterInfo` records, same packing as 2.2) declare which
  neighbouring cluster records cover which boundary area; `refLon/refLat/shift` give a coarse
  local frame for quick rejection.
* Within-cluster: onecells (edges, `rnw_tclOneCellRef` 0x34-byte in-memory form; stream record:
  `u32 hdr`, `u32 storedLength` via `u32GetLength` @0x00913c7c, `u16 listFlags`, `u16 offf`),
  zerocells (junction nodes) and the overlap refs `{u16 A: onecell idx+1 | side flags<<10, u16 B:
  ci2 idx+1}` (`rnw_tclLocalCellRef::bRead` @0x00892494) bind a local node to a neighbour slot.
* `rnw_tclRefineOCList` materializes the cross links per cluster pair:
  `oGetOverlapInCluster` @0x0088c084, relevance filter `bRelevantCrossingBetween` @0x0088c78c;
  access facade `rnw_tclAccessWorker::vProcessClusterInfo` @0x00908648. Traversal costs are
  computed engine-side from onecell attributes (class bits, `storedLength`, direction bit15)
  [INFERENCE — cost tables live in PROCNAV].

### 2.5 Route result consumption

Saved-route storage on the DAPI side: `rnw_tclStoreRegionInfo` @0x0090e448, `rnw_tclStepEntry`
@0x0090e420 (per-step records); protocol answers carry `fi_tcl_NavRouteOneCellDesc` /
`fi_tcl_NavRouteInstructionDesc` / `fi_tcl_NavRouteDestinationDesc` (constructors @0x00ab5b14 /
@0x00ab5994 / @0x00ab5a48) — i.e. the engine answers with **onecell lists + instruction
annotations**, which are serialized by DAPIAPP and rendered by procmapengine. [PARTIAL — the
server-side handler bodies live in PROCNAV, see §7.]

## 3. Implications for the writers (osm2rnw / trial card)

1. A shipped region **must** carry `NAV_ROOT.DAT` with a valid root-cluster list — a bare
   `NAV*.DAT` set is loadable but unreachable (writer task in `TODO.md`).
2. Root-cluster records must use the region's FINAL `regionIdent` in the packed `fileOffset`
   (low 14 bits) and 16-KB-aligned offsets; gateway clusters should be *big* boundary-spanning
   clusters (stock pattern: 34–59 KB) whose ci lists chain into the rest.
3. `.TCI` shards (per MAP shard) remain required for map-click/tile queries; shard filename =
   stock tile id (parsed by dir-scan); refs `{off|ident, len, fid}`, `nPrim == nAll`.
4. ci1/ci2 words pack the ident too (`osm2rnw` fixed 2026-09-18; `diag/rnwcheck.py --generated`
   enforces it).
5. Keep cluster flags bit 0x80 clear (else a CONNECT `.PTH` memcpy is attempted —
   `u16PatchCluster`) and note `.PTH` absence is *not* an error.
6. Address-search entry (destination side) needs the LID files incl. `LID20000` gazetteer
   (LID_format.md §11.2/§11.8) — orthogonal to RNW but required end-to-end.

## 4. Open questions / next steps (ordered)

1. ~~**Unpack `PROCNAV.OUT`**~~ **DONE 2026-09-19** — codec found (triton monitor `0x10bd90`,
   byte-token LZ, see §1.4), full container spec + `src/elf_decompressor/uli_unpack.py` verified on 7 files
   (all decode to exact header size, all valid ARM ELFs). PROCNAV now in Ghidra
   (`/tmp_re2/PROCNAV_dec.out`, 94 450 functions, .dynsym names live). Historical notes kept
   below — the earlier "bitstream coder @0x109070" theory was WRONG (those are region pool
   allocators; the SVC #4-#12 handler table at blob 0x555c0 is PC-card window I/O used by the
   installer's verification path, not the codec).
   `procsds.out` (`UNC `-only SDS reader) remains the on-target package INDEXER only.
2. On-device `routeprobe` capture (car access pending): confirms PROCNAV launch + file-open order
   (root clusters → ci chain) during a real Kraków calculation.
3. `trRegionInfo` registry record width/field map inside `NAV_ROOT` `@a` table (partially read:
   ident/name-offset/date fields; needs a per-region diff pass).
4. Annotation types beyond 0x2b/0x2c/0x3e (0x3, 0x11, 0x16, 0x39, 0x41, 0x4d, 0x4e, 0x55, 0x56,
   0x60, 0x7c, 0x89, 0x8a seen on POL) — semantics unknown, skipped by the runtime reader, so
   likely optional for our output.
 5. `AEX` auxiliary cluster data — optional (`bRNWLoadAexData` unset path), unexplored.

## 5. Engine internals (PROCNAV_dec.out, 2026-09-19)

Container-unpacked PROCNAV now RE'd at symbol level (Ghidra `/tmp_re2/PROCNAV_dec.out`).

**Data model.** The RNW network is loaded through `dap_rnw_if_tclLoader` as before (§2), but the
calculator flattens the working glue-area (set of participating clusters, `tagRSGLUEAREADESC`)
into five scratch tables built per calculation by `rc_calc_tclCALCULATIONWORK::vCreateCalcTables`
@0x0060737c (all indexed by **cluster index within the glue area**):

* `tagRS_REFERENCETAB` — cluster→local index + per-cluster OC reference list (`prGetRefTabOCElement`).
* `tagRS_RESISTANCETAB` — per (cluster, one-cell-direction) **resistance matrix**:
  `pu16GetResistanceMatrix(clusterElem, resistanceDesc)` → u16 row; row index = cobounding-OC
  slot; `u32DecodeZCResistanceEth(ocPair, idx)` decodes a traversal cost; `0xFFFF` = prohibited.
* `tagRS_ROUTETAB` — the LABEL table: element = {word0 packed `(OC & 0x3ff) | cluster<<10 | ...`,
  +4 from-direction record, +0x10 to-direction record} — each direction record carries
  {cost (24-bit), wave number, prev-wave link, flags bit 0x40000 = has-wave, 0x8000000 = settled}.
* `tagRS_MULTIWAVETAB` — the FRONTIER RING: 20-byte elements `prGetMultiWaveTabElement(u16)`
  (1-based, ≤ count@+8), `iu16Add` pushes a {props, cost} copy and returns its index; ring wraps
  at 0x3FFF entries.
* `tagRS_UPLINKTAB` + `rc_calc_trVGSTACK` — the current-wave generation stack (per side;
  `rc_calc_trVGSTACKSET::vSwitch` swaps FROM/TO generations).

**Cost encoding.** A cost is a u32: `cost&0xFFFFFF` = accumulated resistance (saturating add in
`rc_calc_u32CombineROSAValues` @0x004441d8), byte3 = penalty/priority bitfield
(`rc_calc_u32SetROSAPriority` @0x00444136: `|= 0x1000000<<prio & 0xff000000`); combination ORs
the bytes and saturates the 24-bit sum. Penalty sources folded in: `rc_calc_u32CalculateROSAValue`
(toll/"ROSA"), `rc_calc_u32CalculateAvoidResistance` (avoid list), complex-intersection U-turn
(`u32GetCmplxUTurnResistance`, ×0x390), turn/vehicle/load/road-condition prohibitions
(`bTurnIsProhibited`, `bIsRestrictedOrBlocked` → bits 0x4000000/0x8000000), street-class
penalties (+4/+0x10/+0x3f buckets from `rc_calc_trRouteCriteria`).

**Search flow.**
1. `rc_rif_tclRouteInterface::s32SetEntryPointsAndUpdateRoute_E` (RPC) → build glue area
   (`rc_gcl_*`: touch/overlap/bridge cluster neighbourhoods, §2.4 ci-walk + area outlines).
2. `vCreateCalcTables` per glue area; `vInitOptimizeWaveFront` @0x0060874c seeds BOTH sides:
   iterates the interface one-cell set, resolves the cluster's local index, stamps the OC's
   ROUTETAB direction record (cost from the entry property, wave number, bit 0x20000), pushes
   the OC onto the side's VGSTACK (`prGetNewEntry`; destination side additionally floods complex
   intersections `vAddComplexPathesToMWTabAndInOCsToVGStack`); then `vSwitch`.
3. `vOptimize` @0x00608acc = the loop: pop wave entry from the frontier (ROUTETAB records marked
   0x8000000 chain through MULTIWAVE elements), for the entry OC enumerate cobounding OCs (via
   zero-cell lists / complex-intersection member lists), compute candidate cost
   (`pu16GetResistanceMatrix` row + `bUpdateResistanceValues` + penalties), and
   **relax (`rc_calc_bResistanceDescriptionIsBetter` @0x0044441c) into the neighbour's ROUTETAB
   direction record, push its MULTIWAVE index (iu16Add) and set 0x8000000 = settled** — i.e.
   two-ended Dijkstra on the OC adjacency graph with wave number = iteration stamp; terminates
   when FROM and TO waves settle the same OC/edge (or status 0x22800xxx timeout/overflow).
4. Route assembly: prev-wave chain → `rc_rlist_*` route list (`prInitRawRouteList`,
   `rs_GetDownClOfRoute`, `rc_rlist_vPackRoutelist`), travel-time/distances via
   `rc_master_s32GetTravelTimeDelta`, annotations re-evaluated by `rc_rdb_*`
   (bEvaluateLimitation etc. against `rc_calc_trRouteCriteria`).

Implication for `osm2rnw` (§3): the engine reads exactly the §2 surface, but the cost model
above demands one addition for realistic travel times: the **BuiltUpLen annotation (0x19,
`{u32 fwd, u32 bwd}` m)** on OCs inside settlements (OSM: split way at `place` polygon boundary;
stock DEU has ~40 per 1 MB of RNW) — without it every OC is priced with the open-road speed
table. 0x2f FreewayLen appears to be absent from stock RNW (class-flag fallback). Also stressed:
ci-chain completeness (glue-area neighbour discovery) and OC cobounding zero-cell lists (wave
expansion). The trial-card probe should watch PROCNAV (now unpackable and analyzable) instead of
guessing.

**Per-OC resistance model (`tclClExConverter::vCalcDrivingResistance` @0x00632808).** An ONE-CELL
record is 40 bytes: `+0x14` flags {road class bits 0..2, class<<8 road-type in 0xf00, bit
0x400000/0x40000000 = urban/freeway applicability}, `+0x24` = length (m). The OC is split into
at most three PARTIAL segments — built-up length (`prGetBuiltUpLengthAnnotation`), freeway length
(`prGetFreewayLengthAnnotation`), remainder — each priced by `vCalcPartlyDrivingResistance` with
the active 8×3 speed-table profile (`tclSpeedTabIdentifier.type=2`; ECO/user profile switches the
table via criteria +0x9c; live TMC/CSD block `tagCSDBlock` participates) plus a `rc_tclUserSpeed
FuelValues` fuel accumulator; results are then scaled by road-type multipliers (×1.3/×1.5 at
0x100/0x300/0x400, ×10 at 0x700/0xa00 classes). Three accumulators come out {time, alt-time,
fuel}; `rc_calc_u32CalculateResistanceCombination(criteria, t1, t2, fuel)` folds them into the
single 24-bit cost, then `tagRS_RESISTANCETAB::vCreateOnecellResistance/vCreateZerocellResistance
` bake per-OC and per-transition (turn) costs into the RESISTANCETAB matrices (u16 × 2^scale,
0xFFFF = prohibited), with MOC (`moc_tclClientInterface` = live traffic) delay manipulation
(`vManipulateOnecellResistanceOnBorder/InsideGluearea`). Transition side:
`u32GetTransitionResistance` adds turn/direction costs from ZEROCELLELEMENT (turn-prohibition,
complex-intersection lane matrices).
