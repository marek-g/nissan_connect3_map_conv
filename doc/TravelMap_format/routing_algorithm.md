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

`PROCNAV.OUT` is not ELF: container `ULI ` (magic `"ULI "` + `u32 0` + `u32 1` + `u32 2`, then
`u32 0`, `u32 0x24` header size, `u32 0x120b974` ≈ 18.9 MB unpacked size, `u32 0x91f348` packed
size; payload from `0x24`). Unpacking is pending (§7).

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

1. **Unpack `PROCNAV.OUT`**: analyze the ULI container. Findings 2026-09-18: `procsds.out`
   (imported+analyzed, 39 777 functions) hosts the DNL package reader `SDS_tclDataPackage` —
   `GetPackageFormat` @0x006b87b4 (V1/V2/V3), `GetFileEntryV1/2/3` @0x006b8584/@0x006b8344/@0x006b8078,
   `s32ExtractFileToBuffer` @0x006b89d8, `s32ReadFileIntoBuffer` @0x006b7f90 (only the `UNC `
   uncompressed marker `0x20434e55` is handled there; V1 entries are tagged with the `ULI ` marker
   `0x20494c55`, literal pool @0x006b8b78 — `s32ExtractFileToBuffer` V1 branch passes the `ULI ` tag
   straight into that UNC-only reader, so the SDS path INDEXES ULI entries but CANNOT expand them).
   ULI header on PROCNAV: `{ "ULI ", u32 0, u32 1, u32 2, u32 0, u32 0x24 hdrsize,
   u32 unpacked=0x120b974, u32 packed=0x91f348 }`, payload @0x24.
   **Codec located 2026-09-18 (firmware ISO pass):** `container.iso.bin` (DNL installer ISO, full
   filesystem unpacked to `/tmp/rnwwork/dnl/`) → `nor0/processes/` stages `PROCNAV.OUT` in the SAME
   ULI form; the only code on the whole firmware+card that knows `XOZL`/`ULI` is the **triton
   dual-OS monitor** (`triton_dualos.bin.uimage`, raw ARM blob, extracted at
   `/tmp/rnwwork/triton.bin`): format checker @`0x108814` (`cmp` vs `"XOZL"`/`"ULI "` literals
   @0x108838/0x10883c), installer/reader loop @`0x108844`, header-parse wrappers @`0x10857c`/
   `0x108620`/`0x1086c4` (format codes 0/2/4) selecting three unpack primitives @`0xde5cc`,
   `0xdd4ec`, `0x109070`; `0x109070` delegates to `0x10d66c/0x10d85c/0x10daf4/0x10d88c` with an
   LSB bit-reader/bitter set of helpers @`0x109230-0x1092f0` → the codec is a BITSTREAM coder,
   not byte-escape LZSS (a classic-LZSS brute force over 4k parameter sets against the small
   `PROCDLSAVER.OUT` XOZL oracle — unpacked=0xf5e4 raw ELF — found no hit). `PROCDLSAVER.OUT`
   (XOZL, same 0x24-byte header, payload with near-literal ELF prefix) is the small oracle for
   further codec work. Options: (a) continue RE of the triton bitstream codec (medium-heavy);
   (b) once car access returns, look for an already-expanded PROCNAV on the live head unit
   (`find / -name 'PROCNAV*'`, /tmp, /dev/shm, /opt/bosch/processes) and/or trace the DNL install
   flow with the existing rootshell tooling.
2. On-device `routeprobe` capture (car access pending): confirms PROCNAV launch + file-open order
   (root clusters → ci chain) during a real Kraków calculation.
3. `trRegionInfo` registry record width/field map inside `NAV_ROOT` `@a` table (partially read:
   ident/name-offset/date fields; needs a per-region diff pass).
4. Annotation types beyond 0x2b/0x2c/0x3e (0x3, 0x11, 0x16, 0x39, 0x41, 0x4d, 0x4e, 0x55, 0x56,
   0x60, 0x7c, 0x89, 0x8a seen on POL) — semantics unknown, skipped by the runtime reader, so
   likely optional for our output.
5. `AEX` auxiliary cluster data — optional (`bRNWLoadAexData` unset path), unexplored.
