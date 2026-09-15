//! Canonical **0x77-byte outer header** shared by every raw NL file the author tool emits
//! (`LID2nnnn` name lists, `LID4nnnn` GenAttr, `RELnnnnn` relation matrices, …). Reverse-engineered
//! from stock POL files (byte-identical `[0x18, 0x77)` across all kinds) — the device opens these as a
//! `LISA_tclDataBlock` whose *identity* is the first record; see §11 of `LID_format.md`.
//!
//! Layout:
//! * `0x00` name lists / GenAttr: `rIdxListID` = `{u16 regionIdent; u16 listID; u16 0; u16 0x41ec}` —
//!   **exactly the first 8 bytes of this module's output and of every stock `LID2nnnn`/`LID4nnnn`**;
//!   the device's `bFindRelation`/list lookups compare list records by this struct.
//!   REL files instead carry 12 zero bytes here.
//! * `0x0c` `u32` file kind: `1` = raw name list, `3` = GenAttr, `6` = REL matrix.
//! * `0x10` `u32` = `0x77` (sub-header/split offset), `0x14` `u32` = size of the sub-header region.
//! * `0x18..` fixed 0x5F-byte author block (format u16s, copyright, build date, `TLID_EQUIVALENT_CHAR`).

/// The invariant `[0x18, 0x77)` author block, byte-copied from stock files (all kinds identical).
pub const NL_AUTHOR_BLOCK: [u8; 0x5F] = [
    0x26, 0x00, 0x54, 0x00, 0x5f, 0x00, 0x60, 0x00, 0x0d, 0x00, 0x02, 0x00, 0x76, 0x00, 0x43, 0x6f,
    0x70, 0x79, 0x72, 0x69, 0x67, 0x68, 0x74, 0x20, 0x62, 0x79, 0x20, 0x52, 0x6f, 0x62, 0x65, 0x72,
    0x74, 0x20, 0x42, 0x6f, 0x73, 0x63, 0x68, 0x20, 0x43, 0x61, 0x72, 0x20, 0x4d, 0x75, 0x6c, 0x74,
    0x69, 0x6d, 0x65, 0x64, 0x69, 0x61, 0x20, 0x47, 0x6d, 0x62, 0x48, 0x00, 0x30, 0x37, 0x2e, 0x30,
    0x36, 0x2e, 0x32, 0x30, 0x32, 0x31, 0x00, 0x00, 0x54, 0x50, 0x4c, 0x49, 0x44, 0x5f, 0x45, 0x51,
    0x55, 0x49, 0x56, 0x41, 0x4c, 0x45, 0x4e, 0x54, 0x5f, 0x43, 0x48, 0x41, 0x52, 0x00, 0x00,
];

/// File kinds (u32 at `0x0c`, stock-observed: raw name list).
pub const KIND_NAME_LIST: u32 = 1;
/// GenAttr (`LID4nnnn`).
pub const KIND_GEN_ATTR: u32 = 3;
/// REL relation matrix.
pub const KIND_REL: u32 = 6;

/// Total outer header length; every stock NL file splits at this offset.
pub const NL_HEADER_LEN: usize = 0x77;

/// Build the canonical 0x77-byte outer header. `region`/`list_id` fill the leading `rIdxListID`
/// (REL: pass `0`/`0` ⇒ 12 zero bytes, matching stock REL files). `region_size` goes to `0x14`.
pub fn nl_header(kind: u32, region: u16, list_id: u16, region_size: u32) -> Vec<u8> {
    let mut h = vec![0u8; NL_HEADER_LEN];
    h[0..2].copy_from_slice(&region.to_le_bytes());
    h[2..4].copy_from_slice(&list_id.to_le_bytes());
    // h[4..8] = {0x0000, 0x41ec} constant slot (present in stock name-list/GenAttr AND REL zero slots
    // keep it zero because stock REL writes nothing there — see nl_rel_header).
    if kind != KIND_REL {
        h[6..8].copy_from_slice(&0x41ecu16.to_le_bytes());
    }
    h[0x0c..0x10].copy_from_slice(&kind.to_le_bytes());
    h[0x10..0x14].copy_from_slice(&(NL_HEADER_LEN as u32).to_le_bytes());
    h[0x14..0x18].copy_from_slice(&region_size.to_le_bytes());
    h[0x18..].copy_from_slice(&NL_AUTHOR_BLOCK);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock(dir: &str, f: &str) -> Vec<u8> {
        std::fs::read(format!("{dir}/{f}")).unwrap_or_default()
    }

    /// The builders reproduce the stock outer headers byte-for-byte (identity words + author block).
    #[test]
    #[ignore = "needs stock card copy"]
    fn header_matches_stock_bytes() {
        let dir = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL";
        for (f, kind, region, list) in [
            ("LID20000.DAT", KIND_NAME_LIST, 0x0402u16, 129u16),
            ("LID20001.DAT", KIND_NAME_LIST, 0x0402, 2),
            ("LID20006.DAT", KIND_NAME_LIST, 0x0402, 3),
            ("LID40006.DAT", KIND_GEN_ATTR, 0x0402, 3),
            ("REL00004.DAT", KIND_REL, 0, 0),
        ] {
            let b = stock(dir, f);
            assert!(!b.is_empty(), "missing stock file {f}");
            let want = nl_header(
                kind,
                region,
                list,
                u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()),
            );
            assert_eq!(
                &b[..NL_HEADER_LEN],
                &want[..],
                "outer header mismatch for {f}"
            );
        }
    }
}
