# Map data integrity & signature — what is protected, and how to work with it

Companion to [`statistics.md`](./statistics.md) (per-level census + POI storage/conversion) and
[`MAP_format.md`](../02%20-%20details/MAP_format.md) (format reference). This document answers one
question: **if I swap or regenerate map data, will the car reject it on integrity grounds?**

Everything below was verified directly in the `NISSAN Connect LCN3 V7 2022_2023` image and in
`DAPIAPP.OUT` / `procmapengine.out` (Ghidra).

**Data layout — base vs patch/deploy layer.** The nav medium has two layers, and confusing them
changes the whole answer:

- **Base data — `DATA/DATA/`:** the full map (`MAP/*.MAP` / `.IDX` / `.TCI`) and the full road
  network (`RNW/CCP/<GROUP>/NAVxxxxx.DAT`, ~16k files). This is what you regenerate. **None of it
  is signed.**
- **Patch / deploy layer — `DATA/CONNECT/`:** small deltas applied on top of the base at load time.
  RNW patches (`RNW/CCP/<COUNTRY>/NAV____0-3.PTH`, 67 files) indexed by `NAV_ROOT.DAT`, plus a
  deploy copy of the MAP index (`MAP/*.IDX` + `IDX_CNT.TBL` / `RPITABLE.RPI`). **Most of what is
  signed lives here** (plus two `DATA/DATA/MISC` metadata files).

"Modify the base" and "modify the patches" are therefore different operations with different
signature *and* corruption implications — see §7.

---

## 0. TL;DR

- **The base data (`DATA/DATA/`) is not signed.** Adding roads to *existing* regions/countries in
  the base MAP and RNW changes no signed file → the signature stays valid, check on or off.
- **What is signed is mostly the patch/deploy layer** (`DATA/CONNECT/…`) plus two `DATA/DATA/MISC`
  metadata files. Modifying the base does not touch these — they are region/country *directories*,
  not per-road data — as long as you do not add new regions/countries.
- **Signature verification is delegated to an OSAL ioctl** on the medium (hardware/OS layer), gated
  by a `CHECK_SIGNATURE` registry flag; if the OSAL returns *NotSupported* it auto-passes.
- **The real risk of base modification is not the signature — it is the RNW patches.** Patches are
  applied as blind fixed-offset `memcpy`s into the base buffer; changing base roads shifts those
  offsets and **silently corrupts the base** (affects *routing*, not rendering). See §7.
- You **cannot re-sign** a modified patch/image without Bosch's private key.

---

## 1. The two integrity mechanisms

| mechanism | where | covers | purpose |
|-----------|-------|--------|---------|
| **`RBCM_SIGNATURE`** (ECDSA) | `DATA/DATA/MISC/SDX_META.DAT` (XML) | a fixed list of **control** files (`HASH_FILE_LIST`, see §2) | provenance / anti-piracy of the nav medium (SD card / partition) |
| **`CHKSUMS.MD5`** | `DNL/BIN/NAV/CHKSUMS.MD5` | **nav executables only** (`DAPIAPP.OUT`, `NAVAPP.ERG`, `PROCNAV.OUT`, …) | app-binary integrity |

The **per-region map data** (`.MAP` / `.IDX` / `.TCI`) and the search/config databases
(`GLOB_POI.DAT`, `POI_MAPPING.DAT`) are covered by **neither**.

Signature details from `SDX_META.DAT`: `OPERATOR = LCN2KAI`, `PROGRAM = sd_sign2 v1.18`;
`BOSCH_SIGNATURE` 48 B (ECDSA P-384), `OPERATOR_PUBLIC_KEY` 40 B, `OPERATOR_SIGNATURE` 48 B.

---

## 2. What the signature covers (`HASH_FILE_LIST`)

Each entry is `{offset, numberofbytes, filename}`. `numberofbytes = 2048` means only the first 2 KB
are hashed; `0` is listed without a length.

The two right-hand columns ask the question that actually matters for a base edit: *does this signed
file change if I add roads to existing regions/countries in the base (`DATA/DATA`)?* The answer is
**no for all of them** — because every one of these files is an index/directory keyed by region or
country identity, not a record of per-road data.

| signed file | bytes hashed | role | changes if base MAP roads change? | changes if base RNW roads change? |
|-------------|-------------:|------|:---:|:---:|
| `MEDIUM.CFG` | 0 | nav-medium configuration | no | no |
| `DATA/DATA/MISC/TP_META.DAT` | 0 | build/tool metadata | no (unless you emit new build strings) | no |
| `DATA/DATA/MISC/CONTENT.DAT` | 2048 | content manifest | **verify** — may list base file sizes | **verify** |
| `DATA/CONNECT/RNW/NAV_ROOT.DAT` | 2048 | CONNECT RNW **patch** root / country dataset dir | no (indexes patches, not base) | no |
| `DATA/CONNECT/LID/CONNECT.DAT` | 2048 | per-region Connect service content | no | no |
| `DATA/CONNECT/MAP/IDX_CNT.TBL` | 0 | MAP region directory (411 IDs) | no (same region set) | no |
| `DATA/CONNECT/MAP/RPITABLE.RPI` | 0 | MAP region-profile directory (411 IDs) | no (same region set) | no |
| `DNL/BIN/NAV/CHKSUMS.MD5` | 0 | executable checksums | no (unless you touch the app) | no |

Two notes:

- **`NAV_ROOT.DAT` is the CONNECT *patch* root, not the base RNW.** It decompresses (`CPRNAV_2`) to
  a country dataset directory — `DATASET{'DEU'|'/…/nav2-…_DEU_…/v1'|''|''|''}` — i.e. it lists which
  *patch* datasets exist. Editing base roads adds no new country → it does not change. (It is the
  index of the `DATA/CONNECT/RNW/*.PTH` patches, which is a separate concern — §7.)
- **`IDX_CNT.TBL` / `RPITABLE.RPI`** decompress to build metadata + a directory of **411 region IDs**
  (constant low half per entry, **no per-region MD5 or size field**). They record *which regions
  exist*, not *how many bytes each holds*, so adding roads within them leaves both byte-identical —
  which matters because these two are hashed **in full** (`numberofbytes = 0`).

---

## 3. How the runtime verifies it (`DAPIAPP.OUT`, Ghidra)

Call chain: `u16ProcessNavCheck` @ `0x8b5418` → `bCheckSigniture` @ `0x8b2ee4`.

Key facts from the decompilation:

1. **The check is delegated, not computed in-app.** `bCheckSigniture` opens the medium and issues
   an OSAL ioctl — the real verification happens in the OS / hardware security layer:
   ```c
   fd  = OSAL_IOOpen(devPath, RDONLY);
   ret = OSAL_s32IOControl(fd, 0x40d, &status);   // 0x40d = SIGN_VERIFY
   ```
   `status`: `0`=PASSED, `-1`=UNKNOWN, `1`=INPROGRESS, `2`=FAILED.

2. **There is a built-in disable.** The ioctl path is gated by a registry flag
   (`this[+0x122]` of `dap_dev_tclDeviceTableWorker`):
   ```c
   if (checkSignatureEnabled) {
       if (bCheckSigniture(...) != 1) { log "SignatureCheck failed -> dont use NavMedium"; return 0x302; }
   } else {
       log "ProcessNavCheck CHECK_SIGNATURE disabled in Registry";   // <-- skipped entirely
   }
   ```

3. **NotSupported auto-passes.** If the OSAL ioctl returns error `0x72015` (NotSupported), the code
   logs "`Error == NotSupported --> Device OK`" and accepts the medium. On platforms without the
   secure-verify driver, the check is effectively a no-op.

4. **Failure rejects the medium, it does not crash the app.** On FAILED the device's media type is
   set to `7` and `u16ProcessNavCheck` returns `0x302` ("dont use NavMedium"). The device manager
   simply has no valid nav medium (nav shows "no map" / falls back); the navigation process itself
   keeps running.

5. **A separate decryption gate exists.** Opening `MEDIUM.CFG` with OSAL error `0x72026` logs
   "`Decryption failed --> illegal NavCopy`" — a media-auth/decryption layer distinct from the
   signature.

---

## 4. Implications for data replacement

| operation | signed files changed? | signature still valid? | note |
|-----------|:---:|:---:|------|
| **Add roads to base MAP** (`DATA/DATA/MAP`, existing regions) | no | yes | renders cleanly — the renderer applies no patches (§7) |
| **Add roads to base RNW** (`DATA/DATA/RNW`, existing countries) | no | yes | signature fine, **but** the CONNECT RNW patches will corrupt it (§7) |
| **Add a new region / country** | `IDX_CNT.TBL`/`RPITABLE.RPI` and/or `NAV_ROOT.DAT` | **no** | new directory entry; cannot re-sign |
| **Modify the CONNECT patch layer** | `NAV_ROOT.DAT` (+ `.PTH`) | **no** | patches are signed via their root |

So **editing the base (same region/country set) is signature-valid with the check on or off**; only
adding new regions/countries, or editing the patch layer itself, breaks the signature. The signature
is therefore *not* the obstacle to a base edit — the RNW-patch corruption risk (§7) is.

---

## 5. Ways forward

1. **Disable `CHECK_SIGNATURE` in the registry** — the built-in bypass (§3.2). Finding and setting
   the exact config key is the single highest-leverage step for full regeneration. *(key to be
   traced — see §6.)*
2. **Run on a platform whose OSAL returns NotSupported** for ioctl `0x40d` (dev board / emulator)
   → the check auto-passes (§3.3).
3. **Stay within the existing region/country set.** Base edits do this automatically; it is adding a
   *new* region/country that changes the signed directory files (`NAV_ROOT.DAT` / `IDX_CNT.TBL` /
   `RPITABLE.RPI`), which cannot be re-signed. Avoid it, or disable the check (§5.1).
4. **For rendering tests, edit only the base MAP** — the renderer applies no patches, so this is
   clean on both the signature and the data side (§7). For routing tests you must also neutralize
   the RNW patches (§7).

---

## 6. Open questions

- The exact **registry/config key** behind `CHECK_SIGNATURE` (`dap_dev_tclDeviceTableWorker +0x122`)
  — where it is loaded from and how to set it.
- Whether the **production** OSAL implements the `0x40d` verify ioctl (vs. returning NotSupported).
- The **`MEDIUM.CFG` decryption** path (`0x72026` "illegal NavCopy") — a second media-auth layer that
  may also need handling for a fully regenerated medium.

---

## 7. Editing the base without touching the signature — and the patch trap

Question: if I commit to **not modifying any signed file** (i.e. I only edit the base in
`DATA/DATA/`), how far can I go? Two separate questions, with different answers:

### 7.1 Signature — you can add roads freely within existing regions/countries

- **The road data lives in unsigned base files:** MAP `DATA/DATA/MAP/*.MAP/.IDX/.TCI` and RNW
  `DATA/DATA/RNW/CCP/<GROUP>/NAVxxxxx.DAT`. Adding roads just grows these.
- **No signed file changes** for a same-region/country-set edit (§2): the signed files are all in the
  CONNECT patch layer (+ two MISC metadata files), and they are region/country *directories*, not
  per-road records. `IDX_CNT.TBL`/`RPITABLE.RPI` hold 411 region IDs with a constant low half (no size
  field); `NAV_ROOT.DAT` lists patch datasets by country. Adding roads adds none of those entries.

**Signature hard limits** — these *would* change a signed file and cannot be re-signed:
- adding a **new region/country** (outside the current 411-region set) → new entry in
  `IDX_CNT.TBL`/`RPITABLE.RPI` and/or a new `DATASET{…}` line in `NAV_ROOT.DAT`;
- **extending a region's geographic extent** (new tiles beyond current coverage);
- editing the **CONNECT patch layer** itself.

"More roads in Europe I already cover" is entirely within the safe zone on the signature axis.

### 7.2 Data integrity — the RNW patches will corrupt a modified base (the real trap)

The signature is not the problem; **the patches are.** `DAPIAPP.OUT` applies the CONNECT RNW
patches onto the base at load time, and each patch is a **blind fixed-offset copy** into the base
buffer (`rnw_tclPatch::vApplyPatch` @ `0x91355c`):

```c
void rnw_tclPatch::vApplyPatch(rnw_tclPatch *p, uchar *base) {
    memcpy(base + p->offset,          // destination = base buffer + FIXED offset
           p->data,                    // source      = patch payload
           p->length);                 // blind copy — no ID lookup, no target checksum
}
```

Patches are calibrated to the **exact byte layout of the original base**. If you add/remove/move roads
in the base, byte offsets shift, so every patch now writes to a **wrong location** — silently
corrupting your new base. The runtime's only guard (`"INCONSISTENT PATCHES bNext=%d remaining=%d"` in
`rnw_tclPatchCluster::vApplyToCluster`) validates the *patch file's* internal count, **not** that it
still matches your base — so this corruption is not caught.

Consequences:

- **Rendering is safe.** `procmapengine.out` (the renderer) has **no patch references at all** — it
  reads the base MAP directly. And `DATA/CONNECT/MAP/*.IDX` are byte-same-size as the base `.IDX`
  (e.g. `N6E2AA.IDX` = 1 012 276 B in both), i.e. deploy copies, not offset deltas. So editing the
  base MAP renders cleanly with no patch interference.
- **Routing is at risk.** The RNW access path (`rnw_tclAccessMAPWorker`, `DAPIAPP`) applies the
  `.PTH` patches over the base RNW. A modified base + untouched patches = corrupted routing data.

### 7.3 How to test safely

| goal | do this |
|------|---------|
| **Render test** (new roads appear on map) | edit `DATA/DATA/MAP` only. Patches don't touch rendering. Nothing else needed. |
| **Routing test** (new roads are routable) | edit `DATA/DATA/RNW` **and** neutralize the CONNECT RNW patches, else they corrupt it. Missing patches are tolerated (`"LOAD PATCH dir does not exist → NO ERROR"`), so: disable `CHECK_SIGNATURE` (§3/§5) and remove/empty `DATA/CONNECT/RNW`, leaving only your base RNW. |

Regenerating matching patches is not an option (they are signed via `NAV_ROOT.DAT` and cannot be
re-signed without Bosch's key).

### 7.4 Confirmation to do once a writer exists

- The "directory, not size-table" claim for `IDX_CNT.TBL`/`RPITABLE.RPI` rests on strong structural
  evidence (constant low halves, identical 411 IDs in both, ±0x10000 = ID stepping). Definitive check:
  regenerate one region with more roads and confirm both come out **byte-identical**.
- Check whether `DATA/DATA/MISC/CONTENT.DAT` (signed, first 2 KB) encodes base file sizes — if it
  does, a base edit that changes file sizes would need it regenerated (and it is signed).

### 7.5 Removing the patches without breaking the signature

To run on a modified base RNW you must stop the CONNECT RNW patches from being applied (else they
corrupt it, §7.2). This can be done **without invalidating the signature**, because the `.PTH` data
files are *not* in the signed `HASH_FILE_LIST` — only `NAV_ROOT.DAT` is.

Do:
- delete the patch payload: `DATA/CONNECT/RNW/CCP/<COUNTRY>/NAV____*.PTH` (67 files), or the whole
  `DATA/CONNECT/RNW/CCP/` tree;
- **keep** `DATA/CONNECT/RNW/NAV_ROOT.DAT` — it *is* signed; removing it makes a listed file missing
  and the verify cannot hash it → likely FAILED.

Why validation still passes: `bCheckSigniture` (via OSAL ioctl `0x40d`) hashes **only** the files named
in `HASH_FILE_LIST` (`MEDIUM.CFG`, `TP_META.DAT`, `CONTENT.DAT[2K]`, `NAV_ROOT.DAT[2K]`,
`CONNECT.DAT[2K]`, `IDX_CNT.TBL`, `RPITABLE.RPI`, `CHKSUMS.MD5`) — not the whole medium. Deleting the
unlisted `.PTH` files leaves every listed file present and unchanged, so all hashes still match →
**PASSES**.

Why the loader tolerates it: `rnw_tclPatch*` logs `"LOAD PATCH file does not exist → NO ERROR"` /
`"LOAD PATCH dir does not exist → NO ERROR"` and skips — so `NAV_ROOT.DAT` pointing at now-missing
`.PTH` files is not an error; the base RNW is used as-is.

Caveat: per-file (vs whole-medium) verification is inferred from the `HASH_FILE_LIST` structure; the
`0x40d` handler lives in the OS/driver layer and is not inspectable here. Confirm empirically by
deleting `CCP/`, keeping `NAV_ROOT.DAT`, and booting — a clean start on base-only routing confirms it.
Note this only matters for **routing**; rendering never depended on the patches.
