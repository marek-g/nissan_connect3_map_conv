// Bosch TravelMap (Nissan LCN2KAI) OSM -> MAP/IDX writer. Emits the DECOMPRESSED
// .IDX/.MAP layout (the same layout map2osm_rs reads), to be compressed with
// cprnav_compress_rs for deployment.
//
// M2 milestone: parse a real OSM XML extract, tile its objects across levels 0-3
// of a fixed region (N6E2), and emit a multi-level .IDX/.MAP that round-trips
// through map2osm_rs. Semantics (feature codes / names) are minimal here; M3 refines.

use quick_xml::events::Event;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

const SHIFTS: [i32; 4] = [13, 10, 7, 4]; // coordinate delta shift per level (u32a low byte)
const LATPART: [u8; 4] = [1, 5, 10, 10]; // partition bytes +1 and +2
const TILECNT: [usize; 4] = [1, 25, 2500, 250000];

// Target region bbox in PAU, resolved at startup (see setup_region). Defaults = the N6E2 region
// (measured from the stock decompressed N6E2AA.IDX): 18.00E / 47.25N / 36.00E / 56.70N.
static BBOX: OnceLock<(i64, i64, i64, i64)> = OnceLock::new();
const DEF_W: i64 = 0x0CCCCCCC; // 18.00 deg E
const DEF_S: i64 = 0x21999997; // 47.25 deg N
const DEF_E: i64 = 0x19999998; // 36.00 deg E
const DEF_N: i64 = 0x2851EB82; // 56.70 deg N
fn W() -> i64 { BBOX.get().map_or(DEF_W, |b| b.0) }
fn S() -> i64 { BBOX.get().map_or(DEF_S, |b| b.1) }
fn E() -> i64 { BBOX.get().map_or(DEF_E, |b| b.2) }
fn N() -> i64 { BBOX.get().map_or(DEF_N, |b| b.3) }

const PAU: f64 = (1i64 << 31) as f64 / 180.0;
fn deg2pau(d: f64) -> i64 {
    (d * PAU).round() as i64
}

const STATE: u16 = 0x41EC; // stock N6E2 cell state word (all levels/kinds read state=0x41EC; matches
                            // the stock map byte-for-byte so the renderer applies the same styling)

// Profile model (see doc/TravelMap_format §11). Ground truth from stock N6E2 (Kraków bbox): EVERY
// element — roads, POI, land-use polygons AND inland water (371 water lines + 261 water polygons) —
// lives in the single land shard profile `02` (regProf 0x402); profile `0I` (0x412) does NOT occur
// inland. `0I` is Bosch's hydrography OVERLAY used where water needs a separate layer (coastlines /
// maritime regions). So the default (validated on the car, trial #09p) emits everything into `02`
// (OSM2MAP_HYDRO=land, single profile, no multi-slot). The `0I` overlay + multi-profile-slot path is
// retained behind OSM2MAP_HYDRO=overlay for coastal regions but is not the default. Both `02` and
// `0I` are already declared for region N6E2 in the resinf metadata catalog. base32(low) -> file name
// via `<REGION>1<B32[low/32]><B32[low%32]>`.
const HYDRO_PROF: u16 = 0x12; // "0I" -> N6E210I.MAP : waterways + water areas, no POI
const LAND_PROF: u16 = 0x02; // "02" -> N6E2102.MAP : roads + POI + land-use polygons (default)
// Emitted LAND profile id. Default 0x02 (car-validated on N6E2/N6E1, the two stock regions that
// carry 0x02). For a region whose signed RPI declares a different land profile, set OSM2MAP_PROFILE
// (hex "0x16" or decimal) to a profile id that region already ships, so its RPI metadata matches.
static LAND_P: OnceLock<u16> = OnceLock::new();
fn land_prof() -> u16 {
    *LAND_P.get().unwrap_or(&LAND_PROF)
}
// Road-class feature-code emission. Default ON: per-class line feature LOW byte (0x30..0x33 by netclass,
// 0x21 for minor). OSM2MAP_ROADCLASS=0 reverts EVERY road to the proven #09p encoding (feat 0x30 + 0x11
// roadinfo, no 0x21 minor special-case, no tier variation) — isolates the road-class change from the
// street-name change when a build reboots the head unit.
static ROADCLASS: OnceLock<bool> = OnceLock::new();
fn roadclass_on() -> bool {
    *ROADCLASS.get().unwrap_or(&true)
}

// Sub-switch of ROADCLASS: emit the minor-local tier as feature 0x21 (type-2 line, stock's minor class,
// NEVER car-flashed before #10) vs folding nc>=7 into the proven local-road tier 0x33 + 0x11. Default on
// (stock model). OSM2MAP_MINOR21=0 keeps road tiers 0x31/0x32/0x33 but avoids the risky 0x21 code, so a
// road-class card can be split into "safe tiers" vs "the 0x21 experiment" if a flash reboots.
static MINOR21: OnceLock<bool> = OnceLock::new();
fn minor21_on() -> bool {
    *MINOR21.get().unwrap_or(&true)
}

// water-area polygon feature low byte (area_feat returns this for natural/landuse water)
fn is_water_area_feat(feat: u16) -> bool {
    feat & 0xFF == 0x48
}

// ---- polygon (list-0) feature full code -----------------------------------
// Bosch LINE cells (list1) always carry a HIGH byte of 0x00 in `feature` (confirmed: every stock line
// low code 0x10/0x20/0x21/0x30/0x31/0x32/0x33 has high==0), which is why our roads render fine. Stock
// POLYGON cells (list0) span high 0x20..0x95 (values < 0x20 never occur) and encode fill style/detail.
// NOTE: an earlier note here blamed the polygon reboot on a zero high byte (#06c); that is DISPROVEN.
// The high byte only feeds PolygonConfigMatrix::u16GetDisplayScale (a clamped display-scale bucket),
// and the true #06c cause was the MISSING per-polygon annotation (see build_block). poly_full_feat is
// kept anyway because it yields stock-valid, in-band full codes; observed stock mode per category. The
// feature LOW code is not the crash source: 0x9c is the single most common stock polygon code (2.78M).
fn poly_full_feat(low_only: u16) -> u16 {
    let low = (low_only & 0xFF) as u8;
    let hi: u8 = match low {
        0x9c => 0x20, // residential / building land-use   -> 0x209c (dominant stock value)
        0x38 => 0x60, // grass / meadow / vegetation        -> 0x6038
        0x2b => 0x60, // forest / woodland                  -> 0x602b
        0x48 => 0x50, // water area                         -> 0x5048
        0x39 => 0x40, // cemetery                           -> 0x4039
        0x3a => 0x50, // commercial                         -> 0x503a
        _ => 0x40,    // safe in-band default
    };
    ((hi as u16) << 8) | (low_only & 0xFF)
}

// Target region id (e.g. "N6E2"). Resolved at startup via setup_region(); whole-region replace,
// so ids must be declared in that region's resinf catalog (hence the per-region IDX header).
const DEF_REGION: &str = "N6E2";
static REGION_NAME: OnceLock<String> = OnceLock::new();
fn region() -> &'static str {
    REGION_NAME.get().map(String::as_str).unwrap_or(DEF_REGION)
}
// profile id -> MAP/TCI file base name: <REGION> + "1" + base32(low byte as 2 chars).
fn prof_file(prof: u16) -> String {
    const B32: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let v = (prof & 0xFF) as usize;
    let s: String = [B32[v / 32], B32[v % 32]].iter().map(|&c| c as char).collect();
    format!("{}1{}", region(), s)
}

// Stock header/info/metadata templates (verbatim Bosch bytes). The MAP info/partition/metadata
// block [0x20..binOff) is Bosch-static across every region; only W/S/E/N + profile word + total
// size differ, which we patch at runtime. Emitting these verbatim (instead of a fabricated header
// full of zeros) is what keeps the head unit's strict parser from walking garbage pointers in
// [0x20..binOff) and rebooting.
const MAP_HEADER: &[u8] = include_bytes!("../templates/map_header.bin"); // [0x00..0x7bc), binOff=0x7bc
// IDX descriptive prefix [0x00..partOff*4) (partOff=0x7e, =504 B). Defaults to the baked N6E2
// template; setup_region() replaces it with the target region's OWN stock <REGION>AA.IDX prefix,
// because that block carries region-specific bytes (the resinf catalog reference) the renderer
// needs. Everything from partOff*4 onward is regenerated by emit_idx.
const IDX_HEADER: &[u8] = include_bytes!("../templates/idx_header.bin"); // [0x00..0x1f8), partOff=0x7e
static IDX_HDR: OnceLock<Vec<u8>> = OnceLock::new();
fn idx_header() -> &'static [u8] {
    IDX_HDR.get().map(Vec::as_slice).unwrap_or(IDX_HEADER)
}

// Default directory holding the stock decompressed MAP set (*AA.IDX / *1XX.MAP), used to source
// each region's real IDX header + bbox. Override with the STOCK_DIR env var.
const DEF_STOCK_DIR: &str =
    "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/MAP";

// Resolve the target region from argv/env BEFORE any geometry runs: set the region name, read the
// region bbox + per-region IDX descriptive prefix from the stock <REGION>AA.IDX, and (when no
// explicit --bbox was passed) adopt that stock bbox as the clip window. Non-default regions REQUIRE
// their stock IDX (for the resinf catalog); with none present this aborts rather than emitting an
// IDX that points the reader at the wrong catalog. Default = N6E2 (works from the baked template).
fn setup_region(region_arg: Option<String>, bbox_given: bool) {
    let name = region_arg
        .or_else(|| env::var("OSM2MAP_REGION").ok())
        .unwrap_or_else(|| DEF_REGION.to_string());
    let stock_dir =
        env::var("STOCK_DIR").unwrap_or_else(|_| DEF_STOCK_DIR.to_string());
    REGION_NAME.set(name.clone()).ok();

    // Emitted land profile id (default 0x02). OSM2MAP_PROFILE accepts hex ("0x16"/"16" with 0x) or
    // decimal. Lets a region lacking 0x02 emit a profile its signed RPI already declares.
    if let Ok(p) = env::var("OSM2MAP_PROFILE") {
        let t = p.trim();
        let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            u16::from_str_radix(h, 16)
        } else {
            t.parse::<u16>()
        };
        match v {
            Ok(v) if v != HYDRO_PROF => {
                LAND_P.set(v).ok();
            }
            Ok(_) => {
                eprintln!("error: OSM2MAP_PROFILE must not be the hydro profile 0x12");
                std::process::exit(2)
            }
            Err(_) => {
                eprintln!("error: OSM2MAP_PROFILE '{p}' is not a valid hex (0x..) or decimal id");
                std::process::exit(2)
            }
        }
    }

    // Road-class code emission (default on). OSM2MAP_ROADCLASS=0/off reverts roads to #09p 0x30+0x11.
    if let Ok(v) = env::var("OSM2MAP_ROADCLASS") {
        ROADCLASS.set(!matches!(v.trim(), "0" | "off" | "false")).ok();
    }
    // Minor-tier 0x21 emission (default on). OSM2MAP_MINOR21=0 folds nc>=7 into tier 0x33+0x11 instead.
    if let Ok(v) = env::var("OSM2MAP_MINOR21") {
        MINOR21.set(!matches!(v.trim(), "0" | "off" | "false")).ok();
    }

    let stock_path = format!("{}/{}AA.IDX", stock_dir, name);
    let have = std::path::Path::new(&stock_path).exists();
    if have {
        let d = fs::read(&stock_path).expect("read stock IDX");
        let g32 = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as i64;
        let w = g32(4);
        let s = g32(8);
        let e = g32(12);
        let n = g32(16);
        if !bbox_given {
            BBOX.set((w, s, e, n)).ok();
        }
        let part_off = u16::from_le_bytes([d[0x14], d[0x15]]) as usize;
        let plen = (part_off * 4).min(d.len());
        IDX_HDR.set(d[..plen].to_vec()).expect("set IDX header");
        let d2 = |v: i64| (v as u32 as f64) / PAU;
        eprintln!(
            "[region {}] stock {} bbox W{:.2} S{:.2} E{:.2} N{:.2}  (IDX prefix {} B)",
            name, stock_path, d2(w), d2(s), d2(e), d2(n), plen
        );
    } else if name != DEF_REGION {
        eprintln!(
            "error: region '{}' has no stock IDX at {} — cannot resolve its resinf catalog / bbox.\n\
             set STOCK_DIR to the stock MAP directory, or run region '{}'.",
            name, stock_path, DEF_REGION
        );
        std::process::exit(2);
    } else {
        eprintln!(
            "[region {}] no stock IDX ({}); using baked template + default bbox",
            name, stock_path
        );
    }
}


fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn write_cell(b: &mut [u8], w: u32, feat: u16, w3: u16, w4: u16, t0: u16, t1: u16) {
    let o = (w as usize) * 4;
    put_u16(&mut b[o..o + 2], 0, STATE);
    put_u16(&mut b[o + 2..o + 4], 0, feat);
    put_u16(&mut b[o + 4..o + 6], 0, w3);
    put_u16(&mut b[o + 6..o + 8], 0, w4);
    put_u16(&mut b[o + 8..o + 10], 0, t0); // annotDesc low word = start (in words)
    put_u16(&mut b[o + 10..o + 12], 0, t1); // annotDesc high word = count
}

// A line feature (road or waterway). `ann` is the per-feature annotation entry
// (type, u16 payload w, u32 payload d): roads -> (0x11, roadinfo_w, 0),
// waterways -> (0x10, watercode, 0). None = no annotation.
#[derive(Clone)]
struct LineCell {
    pts: Vec<(i64, i64)>,
    feat: u16,
    ann: Option<(u8, u16, u32)>,
    rn: Option<String>, // road number (OSM `ref`) -> 0x14 annotation + text record
    nm: Option<String>, // street name (OSM `name`) -> 0x7A annotation + text record
}

// A parsed road: (geometry, roadinfo_w word, OSM `ref` -> 0x14, OSM `name` -> 0x7A label,
// feature-override). The 5th field forces an exact road feature byte for that way, bypassing
// road_feat_low; it is set to 0x35 for drivable local streets so they render as the thin WHITE +
// dark-outline pen (see road_feat_low / the emit step). None = derive the feature from netclass.
type Road = (Vec<(i64, i64)>, u16, Option<String>, Option<String>, Option<u16>);

// Words a road-number (0x14) feature contributes beyond its 0x11: the 8-byte annotation record
// (2 words) plus its text-pool record ({u8 n,u8 0,u8 len,bytes,NUL} word-aligned). 0 if no ref.
fn rn_words(s: Option<&str>) -> u32 {
    match s {
        Some(r) if label_ok(r) => 2 + text_rec_words(r),
        _ => 0,
    }
}

// Annotation entry size in words (0x11 roadinfo = 8 bytes; everything else = 4).
fn ann_words(typ: u8) -> u32 {
    match typ {
        0x11 => 2,
        _ => 1,
    }
}

// Name/number text-record string-variant flag byte (the PRIMARY/display variant flag). A text
// record is  { u8 n_strings, (u8 <flag>, u8 len) x n, bytes x n, u8 NUL }  (4-byte aligned pool).
// CAR-VALIDATED: 0x00 = "no display string" -> the label engine is skipped entirely, so labels sit
// in the data but NOTHING is drawn (this was the "street names missing" bug). 0xA7 is the real
// "display label" flag. Ground truth from the stock N6E2 text pool (scanned): stock labels are all
// UPPERCASE, and when a name carries Polish diacritics it is stored as a TWO-variant record
//   02 a7 <lenPL> c5 <lenASCII> <PL-UTF8> <ASCII-translit> 00
// (71k records of the (a7,c5) pair vs only ~0.4k single a7-with-diacritics); pure-ASCII labels are
// single  01 a7 <len> <bytes> 00  (133k). Emitting 0xA7 with raw MIXED-CASE text reboots the head
// unit (trial #12), so labels MUST be Bosch-uppercased first. Default is now 0xA7 (labels ON);
// OSM2MAP_TEXTVARIANT=0 restores the old blank-but-safe behaviour for experiments.
fn name_str_flag() -> u8 {
    let s = env::var("OSM2MAP_TEXTVARIANT").unwrap_or_else(|_| "0xa7".to_string());
    let s = s.trim();
    if s.eq_ignore_ascii_case("0") || s.eq_ignore_ascii_case("none") {
        return 0;
    }
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(h, 16).unwrap_or(0xA7)
    } else {
        s.parse::<u8>().unwrap_or_else(|_| u8::from_str_radix(s, 16).unwrap_or(0xA7))
    }
}

// Second (ASCII / transliterated) variant flag, seen beside 0xA7 in stock two-variant records.
const TXT_ASCII_FLAG: u8 = 0xC5;

// Upper-case + fold to pure ASCII the way Bosch labels store the latinised variant: map the Polish
// letters to their base letter (Ą->A ... Ż->Z) and drop any other non-ASCII byte. Input is assumed
// already upper-cased; kept defensive (handles lower-case too).
fn ascii_fold(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        let m = match c {
            'Ą' | 'ą' => Some('A'), 'Ć' | 'ć' => Some('C'), 'Ę' | 'ę' => Some('E'),
            'Ł' | 'ł' => Some('L'),  'Ń' | 'ń' => Some('N'), 'Ó' | 'ó' => Some('O'),
            'Ś' | 'ś' => Some('S'),  'Ź' | 'ź' => Some('Z'), 'Ż' | 'ż' => Some('Z'),
            _ => {
                if c.is_ascii() {
                    Some(c)
                } else {
                    None
                }
            }
        };
        if let Some(x) = m {
            o.push(x);
        }
    }
    o
}

// Full text-record payload for a label: one upper-cased UTF-8 variant (pure-ASCII names) or the
// stock two-variant upper-cased PL + ASCII pair (names with diacritics). flag 0 -> legacy blank.
fn text_rec_bytes(raw: &str) -> Vec<u8> {
    let mut v = Vec::new();
    let flag = name_str_flag();
    if flag == 0 {
        let pl = raw.as_bytes();
        v.push(1); v.push(0); v.push(pl.len() as u8); v.extend_from_slice(pl); v.push(0);
        return v;
    }
    let up = raw.to_uppercase();
    let pl = up.as_bytes();
    let ab = ascii_fold(&up).into_bytes();
    if text_variants_ascii() {
        v.push(1); v.push(flag); v.push(ab.len() as u8); v.extend_from_slice(&ab); v.push(0);
    } else if pl == ab {
        v.push(1); v.push(flag); v.push(pl.len() as u8); v.extend_from_slice(pl); v.push(0);
    } else {
        v.push(2); v.push(flag); v.push(pl.len() as u8); v.push(TXT_ASCII_FLAG); v.push(ab.len() as u8);
        v.extend_from_slice(pl); v.extend_from_slice(&ab); v.push(0);
    }
    v
}

// Word-aligned pool cost of a label record (header+payloads+NUL, rounded up to a 4-byte word).
fn text_rec_words(raw: &str) -> u32 {
    ((text_rec_bytes(raw).len() as u32) + 3) / 4
}

// A label is emit-safe when non-empty and short enough that both variant length bytes fit in a u8.
fn label_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 250
}

// Per-label-class emit gates (default ON), for the on-car name-crash bisect: each trial differs from
// the car-crashing #23 by exactly ONE label class so we can see which 0x7A/0x14 path the render
// engine faults on. OSM2MAP_LABEL_STREET / _POI / _ROADNUM.
fn label_street_on() -> bool { env_on("OSM2MAP_LABEL_STREET") }
fn label_poi_on() -> bool { env_on("OSM2MAP_LABEL_POI") }
fn label_roadnum_on() -> bool { env_on("OSM2MAP_LABEL_ROADNUM") }

// Label variant shape (bisect for the diacritic/2-variant reboot hypothesis). OSM2MAP_TEXT_VARIANTS:
//   "stock" (default) = pure-ASCII single a7 variant, or the stock 2-variant (a7 PL + c5 ASCII) pair;
//   "ascii"           = ALWAYS a single variant whose payload is the ASCII-folded, uppercase string
//                       (pure ASCII, no diacritics ever) — tests whether the a7 payload's raw UTF-8
//                       diacritics (or the 2-variant record itself) are what reboot the label engine.
fn text_variants_ascii() -> bool {
    matches!(env::var("OSM2MAP_TEXT_VARIANTS").unwrap_or_else(|_| "stock".into()).as_str(),
        "ascii" | "single" | "fold")
}

// A POI may carry a name (0x7A) only when its feature renders a label safely. Feature 0x0001 is the
// Bosch SETTLEMENT marker: the POI label renderer resolves it to a city style and REQUIRES the 0x21
// city annotation to be present (measured on stock N6E2: EVERY named feat-0x01 cell carries a 0x21;
// stock has ZERO bare {7a} feat-0x01 cells). A bare 0x7A name on a feat-0x0001 POI with no 0x21 makes
// the renderer dereference a missing style entry and HARD-FAULTS the head unit. Root cause of #22/#23
// (bisected by #24b POI-off = boots vs #24a street-off / #24d roadnum-off = both still crash, i.e. the
// fault is the POI-name class). Our poi_feat() falls through to 0x0001 for any named node with an
// unmapped amenity/tourism/shop sub-value (and city_bits()==0 with no place=*) -> ~6.8k such POIs in
// Krakow metro alone. Suppress the name for exactly that case (a feat-0x0001 marker with no 0x21);
// real settlements ({21,7a}) and every categorised POI (stock emits bare {7a} for 0x05/0x06/0x07) stay.
fn poi_can_name(feat: u16, city: u16, name: &str) -> bool {
    label_ok(name) && !(feat == 0x0001 && city == 0)
}

// Generic boolean env switch; default ON unless set to 0/n/none/false/off. Used by the #09
// isolation ladder so each rung differs from the car-proven land baseline by exactly one mechanism.
fn env_on(key: &str) -> bool {
    !matches!(env::var(key).unwrap_or_else(|_| "1".into()).as_str(),
        "0" | "n" | "none" | "false" | "off")
}

// Which annotation a land-use polygon must carry, chosen by CATEGORY (measured on stock N6E2):
//   * feat low 0x9c (OSM landuse=residential / settlement area) ALWAYS carries 0x04 (DCM = 3D city
//     model) — 2.77M cells, ~100% of that category, and it is the ONLY category that ever does.
//   * grass 0x38 / forest 0x2b / cemetery 0x39 ... carry no annotation ~79-96% of the time (a name
//     0x7A otherwise). Water-area 0x48 always carries 0x10 (not 0x04) — added separately for hydro.
// Emitting 0x04 on our 0x9c polygons is what stops the reboot (the renderer dereferences a
// style/config entry the DCM record supplies; see build_block). OSM2MAP_POLY_ANN:
//   "cat" (default) = faithful: 0x04 only on 0x9c      "all" = 0x04 on every polygon (ladder 6c3)
//   "0"/"none"      = no polygon annotation (ladder baselines 6c/6c2)
// Returns the annotation type byte to emit, or 0 for none.
fn poly_annot_type(feat_low: u8) -> u8 {
    match env::var("OSM2MAP_POLY_ANN").unwrap_or_else(|_| "cat".into()).as_str() {
        "0" | "n" | "none" | "false" => 0,
        "all" | "1" | "y" | "true" => 0x04,
        _ => {
            if feat_low == 0x9c {
                0x04
            } else {
                0
            }
        }
    }
}

// A land/water-area polygon's annotation KIND: 0 = none, 0x04 = DCM (residential, 2 words),
// 0x10 = water (water-area 0x48, 1 word). Stock N6E2: residential 0x9c always carries 0x04; water
// 0x48 areas always carry 0x10 (type=2/class=8 dominant) — the water record is what the water-area
// reader dereferences, its absence is the #09f reboot suspect. Water 0x10 gated by OSM2MAP_WATER_POLY_ANN
// (default on). Controlled independently of POLY_ANN so the 0x04 ladder stays intact.
fn poly_ann_kind(feat_low: u8) -> u8 {
    let d = poly_annot_type(feat_low);
    if d != 0 {
        return d; // 0x04 on 0x9c (cat/all)
    }
    if feat_low == 0x48 && env_on("OSM2MAP_WATER_POLY_ANN") {
        return 0x10;
    }
    0
}
// Record size in words for a polygon annotation kind (0x04 DCM = 2 words, 0x10 water = 1 word).
fn poly_ann_words(kind: u8) -> u32 {
    match kind {
        0x04 => 2,
        0x10 => 1,
        _ => 0,
    }
}
// The 0x10 payload for a water-area polygon (u16: high nibble=type, low nibble=class). Stock N6E2
// water_area values cluster on type=2 (146/261) with class 8/7/6 and type=3 class 0/1; the single
// dominant lake value is type=2,class=8 = 0x28.
fn area_watercode() -> u16 {
    0x28
}

// ---- decompressed MAP block (inverted from map2osm parse_block) ------------
fn build_block(
    shift: i32,
    cx: i64,
    cy: i64,
    polys: &[(Vec<(i64, i64)>, u16)], // (open ring, landuse feat)
    lines: &[LineCell],
    pois: &[(i64, i64, u16, Option<&str>, u16)],
) -> Vec<u8> {
    let (np, nl, nq) = (polys.len(), lines.len(), pois.len());
    let start0: u32 = 4;
    let start1 = start0 + (np as u32) * 3;
    let start2 = start1 + (nl as u32) * 3;
    let cells_end = start2 + (nq as u32) * 3;

    // point pool: one word {i16 dlon, i16 dlat} per vertex, after all cells.
    let mut pp: Vec<u8> = Vec::new();
    let mut poly_idx = vec![0u32; np];
    let mut line_idx = vec![0u32; nl];
    let mut cur = cells_end;
    for (i, (pts, _)) in polys.iter().enumerate() {
        poly_idx[i] = cur;
        for &(lo, la) in pts {
            pp.extend_from_slice(&(((lo - cx) >> shift) as i16).to_le_bytes());
            pp.extend_from_slice(&(((la - cy) >> shift) as i16).to_le_bytes());
            cur += 1;
        }
    }
    for (i, lc) in lines.iter().enumerate() {
        line_idx[i] = cur;
        for &(lo, la) in &lc.pts {
            pp.extend_from_slice(&(((lo - cx) >> shift) as i16).to_le_bytes());
            pp.extend_from_slice(&(((la - cy) >> shift) as i16).to_le_bytes());
            cur += 1;
        }
    }
    let pool_end = cur;

    // Land-use polygon annotations are CATEGORY-driven (see poly_annot_type). The reboot root cause
    // (confirmed on the car, ladder #06c/#06c2 -> #06c3) is that a residential/settlement polygon
    // (feat low 0x9c) MUST carry its 0x04 DCM annotation: the renderer (map_tclMapElm_Landuse_Area::
    // ReadFrom@0x003ebc64) dereferences a style/config entry that the DCM record supplies, and with
    // annDesc=(0,0) the entry is absent -> hard fault. Stock NEVER puts 0x04 on grass/forest/water,
    // and those boot fine unannotated — so we emit 0x04 only where stock does (0x9c by default).
    // Dispatch: dap_map_tclAnnotationConverter::u16WriteAttrib@0x920744 / bReadWithOutBase@0x8d9784.

    // annotation region (after point pool): per-polygon 0x04 DCM (residential only by default), then
    // one variable-size entry per line that has an ann, then one 0x7A text entry per named POI,
    // then the POI text records.
    let mut w = pool_end;
    let mut poly_ann = vec![0u32; np]; // 0 = none; else start word of the annotation record
    let mut poly_ann_k = vec![0u8; np]; // annotation kind per polygon (0x04 DCM | 0x10 water)
    for i in 0..np {
        let k = poly_ann_kind((polys[i].1 & 0xff) as u8);
        if k != 0 {
            poly_ann[i] = w;
            poly_ann_k[i] = k;
            w += poly_ann_words(k);
        }
    }
    // Line annotations live in the `w` region; a road may carry 0x11 (roadinfo), 0x14 (road
    // number) and 0x7A (street name) laid back-to-back (annotDesc count = number of records here,
    // per map2osm's annotation walk; each record self-describes size+type). The 0x14 and 0x7A
    // payloads point at text records in the shared text pool, reserved after the POI names.
    let mut line_ann = vec![None::<u32>; nl]; // 0x11/0x10 record word (if present)
    let mut line_rn = vec![None::<u32>; nl]; // 0x14 record word (if present)
    let mut line_nm = vec![None::<u32>; nl]; // 0x7A street-name record word (if present)
    let mut line_nrec = vec![0u16; nl]; // annotDesc count for the cell
    for i in 0..nl {
        if let Some((typ, _, _)) = lines[i].ann {
            line_ann[i] = Some(w);
            w += ann_words(typ);
            line_nrec[i] += 1;
        }
        if label_roadnum_on() && rn_words(lines[i].rn.as_deref()) != 0 {
            line_rn[i] = Some(w);
            w += 2; // 0x14 = 8 bytes
            line_nrec[i] += 1;
        }
        if label_street_on() {
            if let Some(s) = lines[i].nm.as_deref() {
                if label_ok(s) {
                    line_nm[i] = Some(w);
                    w += 1; // 0x7A = 4 bytes {size, type, u16 textRef}
                    line_nrec[i] += 1;
                }
            }
        }
    }
    // POI annotations: 0x7A name (1 word) + optional 0x21 city (1 word), laid contiguously; annotDesc
    // count = how many a cell carries. ann_word = first record, city_word = the 0x21 record.
    let mut ann_word = vec![0u32; nq];
    let mut city_word = vec![0u32; nq];
    let mut poi_nrec = vec![0u16; nq];
    for i in 0..nq {
        let has_name = label_poi_on() && pois[i].3.map_or(false, |s| poi_can_name(pois[i].2, pois[i].4, s));
        let has_city = pois[i].4 != 0;
        if has_name {
            ann_word[i] = w;
            w += 1;
            poi_nrec[i] += 1;
        } else if has_city {
            ann_word[i] = w; // city annotation is the first (and only) record
        }
        if has_city {
            city_word[i] = w;
            w += 1;
            poi_nrec[i] += 1;
        }
    }
    let mut tw = w;
    let mut text_word = vec![0u32; nq];
    for i in 0..nq {
        if let Some(s) = pois[i].3 {
            if label_ok(s) {
                text_word[i] = tw;
                tw += text_rec_words(s);
            }
        }
    }
    let mut rn_text = vec![0u32; nl];
    for i in 0..nl {
        if let Some(s) = lines[i].rn.as_deref() {
            if label_ok(s) {
                rn_text[i] = tw;
                tw += text_rec_words(s);
            }
        }
    }
    let mut nm_text = vec![0u32; nl];
    for i in 0..nl {
        if let Some(s) = lines[i].nm.as_deref() {
            if label_ok(s) {
                nm_text[i] = tw;
                tw += text_rec_words(s);
            }
        }
    }
    let total_words = tw;

    let mut b = vec![0u8; (total_words as usize) * 4];
    b[0..4].copy_from_slice(&(((0xFFFFu32 << 16) | (total_words & 0xFFFF)).to_le_bytes()));
    put_u16(&mut b, 4, start0 as u16);
    put_u16(&mut b, 6, np as u16);
    put_u16(&mut b, 8, start1 as u16);
    put_u16(&mut b, 10, nl as u16);
    put_u16(&mut b, 12, start2 as u16);
    put_u16(&mut b, 14, nq as u16);

    let mut cw = start0;
    for (i, (rpts, feat)) in polys.iter().enumerate() {
        let (t0, t1) = if poly_ann[i] != 0 { (poly_ann[i] as u16, 1u16) } else { (0, 0) };
        write_cell(&mut b, cw, poly_full_feat(*feat), poly_idx[i] as u16, rpts.len() as u16, t0, t1);
        cw += 3;
    }
    for (i, lc) in lines.iter().enumerate() {
        // annotDesc start = first record laid (order 0x11, 0x14, 0x7A); count = line_nrec[i].
        let t0 = match (line_ann[i], line_rn[i], line_nm[i]) {
            (Some(aw), _, _) => aw as u16,
            (None, Some(rw), _) => rw as u16,
            (None, None, Some(nw)) => nw as u16,
            (None, None, None) => 0,
        };
        let t1 = if line_nrec[i] != 0 { line_nrec[i] } else { 0 };
        write_cell(&mut b, cw, lc.feat, line_idx[i] as u16, lc.pts.len() as u16, t0, t1);
        cw += 3;
    }
    for i in 0..nq {
        let (lo, la, feat) = (pois[i].0, pois[i].1, pois[i].2);
        let dlon = ((lo - cx) >> shift) as i16 as u16;
        let dlat = ((la - cy) >> shift) as i16 as u16;
        let (t0, t1) = if poi_nrec[i] != 0 { (ann_word[i] as u16, poi_nrec[i]) } else { (0, 0) };
        write_cell(&mut b, cw, feat, dlon, dlat, t0, t1);
        cw += 3;
    }

    b[(cells_end as usize) * 4..(pool_end as usize) * 4].copy_from_slice(&pp);

    // line annotations (variable size): 0x11 roadinfo {8,0x11,u16 w,u32 d} or
    // 0x10 water {4,0x10,u16 w}.
    for i in 0..nl {
        if let (Some(aw), Some((typ, wval, dval))) = (line_ann[i], lines[i].ann) {
            let bo = (aw as usize) * 4;
            match typ {
                0x11 => {
                    b[bo] = 8;
                    b[bo + 1] = 0x11;
                    put_u16(&mut b, bo + 2, wval);
                    b[bo + 4..bo + 8].copy_from_slice(&dval.to_le_bytes());
                }
                _ => {
                    b[bo] = 4;
                    b[bo + 1] = typ;
                    put_u16(&mut b, bo + 2, wval);
                }
            }
        }
    }

    // road-number 0x14 annotations (8 bytes): {size=8, type=0x14, u16 textRef, u16 mid, u16 status}.
    // textRef -> the road's text record (written below, same format as names). mid=0 / status=0 are
    // the neutral codes the decoder reads back as tm:roadnum_mid/status=0 (unclassified shield).
    for i in 0..nl {
        if let (Some(rw), Some(s)) = (line_rn[i], lines[i].rn.as_deref()) {
            if label_ok(s) {
                let bo = (rw as usize) * 4;
                b[bo] = 8;
                b[bo + 1] = 0x14;
                put_u16(&mut b, bo + 2, rn_text[i] as u16);
                put_u16(&mut b, bo + 4, 0); // mid
                put_u16(&mut b, bo + 6, 0); // status
            }
        }
    }

    // street-name 0x7A annotations (4 bytes): {size=4, type=0x7A, u16 textRef -> road's name text
    // record (same multi-string format as POI names, with the 0xA7 variant flag)}.
    for i in 0..nl {
        if let (Some(nw), Some(s)) = (line_nm[i], lines[i].nm.as_deref()) {
            if label_ok(s) {
                let bo = (nw as usize) * 4;
                b[bo] = 4;
                b[bo + 1] = 0x7A;
                put_u16(&mut b, bo + 2, nm_text[i] as u16);
            }
        }
    }

    // polygon annotations, dispatched by kind:
    //   0x04 DCM (8 B): {size=8, type=0x04, u16=0x0020, 0x11,0,0,0}  (residential; verbatim stock)
    //   0x10 water (4 B): {size=4, type=0x10, u16 watercode}          (water-area 0x48; stock value)
    for i in 0..np {
        if poly_ann[i] == 0 {
            continue;
        }
        let bo = (poly_ann[i] as usize) * 4;
        match poly_ann_k[i] {
            0x10 => {
                b[bo] = 4;
                b[bo + 1] = 0x10;
                put_u16(&mut b, bo + 2, area_watercode());
            }
            _ => {
                b[bo] = 8;
                b[bo + 1] = 0x04;
                b[bo + 2..bo + 8].copy_from_slice(&[0x20, 0x00, 0x11, 0x00, 0x00, 0x00]);
            }
        }
    }

    // POI city 0x21 annotations (4 bytes): {size=4, type=0x21, u16 city_bits}. Emitted for settlement
    // (place=*) POIs; city_bits from city_bits() (display|size<<4|admin<<8). Laid after the name record.
    for i in 0..nq {
        if pois[i].4 != 0 {
            let bo = (city_word[i] as usize) * 4;
            b[bo] = 4;
            b[bo + 1] = 0x21;
            put_u16(&mut b, bo + 2, pois[i].4);
        }
    }

    for i in 0..nq {
        if let Some(s) = pois[i].3 {
            if label_poi_on() && poi_can_name(pois[i].2, pois[i].4, s) {
                let aw = (ann_word[i] as usize) * 4;
                b[aw] = 4; // size
                b[aw + 1] = 0x7A; // type = TEXT
                put_u16(&mut b, aw + 2, text_word[i] as u16);
                let tp = (text_word[i] as usize) * 4;
                let rec = text_rec_bytes(s); // stock 1- or 2-variant upper-cased label
                b[tp..tp + rec.len()].copy_from_slice(&rec);
            }
        }
    }
    // road-number text records (refs are ASCII -> single variant; same encoder as names).
    for i in 0..nl {
        if let Some(s) = lines[i].rn.as_deref() {
            if label_ok(s) {
                let tp = (rn_text[i] as usize) * 4;
                let rec = text_rec_bytes(s);
                b[tp..tp + rec.len()].copy_from_slice(&rec);
            }
        }
    }
    // street-name text records (single ASCII variant, or the stock 2-variant PL+ASCII pair).
    for i in 0..nl {
        if let Some(s) = lines[i].nm.as_deref() {
            if label_ok(s) {
                let tp = (nm_text[i] as usize) * 4;
                let rec = text_rec_bytes(s);
                b[tp..tp + rec.len()].copy_from_slice(&rec);
            }
        }
    }
    b
}

// Tile center (PAU) for tile K of `level` — mirrors map2osm_rs tile_extent+tile_box.
fn tile_center(level: usize, k: i64) -> (i64, i64) {
    let w = E() - W();
    let h = N() - S();
    let (rw, rs, re, rn) = match level {
        0 => (0, 0, w, h),
        1 => {
            let c = k % 5;
            let r = k / 5;
            (w * c / 5, h * r / 5, w * (c + 1) / 5, h * (r + 1) / 5)
        }
        2 => {
            let p = k / 100;
            let t = k % 100;
            let col = (p % 5) * 10 + (t % 10);
            let row = (p / 5) * 10 + (t / 10);
            (w * col / 50, h * row / 50, w * (col + 1) / 50, h * (row + 1) / 50)
        }
        _ => {
            let p = k / 10000;
            let s = (k / 100) % 100;
            let t = k % 100;
            let col = (p % 5) * 100 + (s % 10) * 10 + (t % 10);
            let row = (p / 5) * 100 + (s / 10) * 10 + (t / 10);
            (w * col / 500, h * row / 500, w * (col + 1) / 500, h * (row + 1) / 500)
        }
    };
    let a = SHIFTS[level] as i64 + 1;
    let al = |x: i64| (x >> a) << a;
    let w2 = al(W() + rw);
    let s2 = al(S() + rs);
    let e2 = al(W() + re);
    let n2 = al(S() + rn);
    ((w2 + e2) / 2, (s2 + n2) / 2)
}

// ---- shape division across the per-level tile grid --------------------------
// The TravelMap format stores each tile's geometry as i16 deltas from the tile center, so a
// shape can only live in one tile if all its vertices fit that tile's delta range. We therefore
// split every line/polygon along the level's axis-aligned tile grid: each tile receives exactly
// the slice of the shape inside its extent (Liang-Barsky for lines, Sutherland-Hodgman for
// polygons). Slices meet on shared boundary vertices, so the reassembled map is continuous and
// every stored delta stays within i16 range.

fn grid_size(level: usize) -> i64 {
    [1, 5, 50, 500][level]
}

// Aligned rectangular extent (PAU) of cell (col,row) at `level` — mirrors map2osm tile_extent.
fn cell_rect(level: usize, col: i64, row: i64) -> (i64, i64, i64, i64) {
    let w = E() - W();
    let h = N() - S();
    let G = grid_size(level);
    let rw = w * col / G;
    let rs = h * row / G;
    let re = w * (col + 1) / G;
    let rn = h * (row + 1) / G;
    let a = SHIFTS[level] as i64 + 1;
    let al = |x: i64| (x >> a) << a;
    (al(W() + rw), al(S() + rs), al(W() + re), al(S() + rn))
}

// (col,row) -> tile index K, inverse of the level's space-filling mapping.
fn cell_to_k(level: usize, col: i64, row: i64) -> i64 {
    match level {
        0 => 0,
        1 => row * 5 + col,
        2 => {
            let p = 5 * (row / 10) + (col / 10);
            let t = 10 * (row % 10) + (col % 10);
            p * 100 + t
        }
        _ => {
            let p = 5 * (row / 100) + (col / 100);
            let s = 10 * ((row / 10) % 10) + ((col / 10) % 10);
            let t = 10 * (row % 10) + (col % 10);
            p * 10000 + s * 100 + t
        }
    }
}

// Which cell a point falls in at `level` (0..G-1 per axis).
fn point_col_row(level: usize, lon: i64, lat: i64) -> (i64, i64) {
    let w = E() - W();
    let h = N() - S();
    let G = grid_size(level);
    let fx = ((lon - W()) as f64 / w as f64).clamp(0.0, 1.0) * (1.0 - 1e-9);
    let fy = ((lat - S()) as f64 / h as f64).clamp(0.0, 1.0) * (1.0 - 1e-9);
    ((fx * G as f64) as i64, (fy * G as f64) as i64)
}

// The grid cells a shape's bounding box spans, padded by one cell and clamped to the grid. A
// straight segment only ever crosses columns/rows between its endpoints', so this covers every
// cell the shape can touch (the +1 margin guards against boundary rounding).
fn cell_span(level: usize, pts: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    let G = grid_size(level);
    let mut cmin = i64::MAX;
    let mut cmax = i64::MIN;
    let mut rmin = i64::MAX;
    let mut rmax = i64::MIN;
    for &(lo, la) in pts {
        let (c, r) = point_col_row(level, lo, la);
        cmin = cmin.min(c);
        cmax = cmax.max(c);
        rmin = rmin.min(r);
        rmax = rmax.max(r);
    }
    (
        cmin.saturating_sub(1).max(0),
        cmax.saturating_add(1).min(G - 1),
        rmin.saturating_sub(1).max(0),
        rmax.saturating_add(1).min(G - 1),
    )
}

// Liang-Barsky clip of segment a->b to rect; returns the portion inside (both endpoints), or None.
// Slab method: for each axis the visible t-range is [min(crossings), max(crossings)]; intersect
// with [0,1]. A zero-delta axis just requires the start coordinate be within bounds.
fn clip_segment(ax: i64, ay: i64, bx: i64, by: i64, rect: (i64, i64, i64, i64)) -> Option<((i64, i64), (i64, i64))> {
    let (rw, rs, re, rn) = rect;
    let x0 = ax as f64;
    let y0 = ay as f64;
    let dx = bx as f64 - x0;
    let dy = by as f64 - y0;
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
    if dx == 0.0 {
        if x0 < rw as f64 || x0 > re as f64 {
            return None;
        }
    } else {
        let a = (rw as f64 - x0) / dx;
        let b = (re as f64 - x0) / dx;
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    if dy == 0.0 {
        if y0 < rs as f64 || y0 > rn as f64 {
            return None;
        }
    } else {
        let a = (rs as f64 - y0) / dy;
        let b = (rn as f64 - y0) / dy;
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    if t0 > t1 {
        return None;
    }
    let start = ((x0 + t0 * dx).round() as i64, (y0 + t0 * dy).round() as i64);
    let end = ((x0 + t1 * dx).round() as i64, (y0 + t1 * dy).round() as i64);
    Some((start, end))
}

// Clip a polyline to rect: the portion inside, with boundary intersection points inserted.
fn clip_polyline(pts: &[(i64, i64)], rect: (i64, i64, i64, i64)) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for i in 0..pts.len().saturating_sub(1) {
        if let Some((p, q)) = clip_segment(pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1, rect) {
            if out.last() != Some(&p) {
                out.push(p);
            }
            if out.last() != Some(&q) {
                out.push(q);
            }
        }
    }
    out
}

// Sutherland-Hodgman clip of a polygon against one half-plane (keep where `inside(axis(pt))`).
fn sh_clip<A, I>(poly: &[(i64, i64)], axis: A, val: i64, inside: I) -> Vec<(i64, i64)>
where
    A: Fn((i64, i64)) -> i64,
    I: Fn(i64) -> bool,
{
    let n = poly.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    for i in 0..n {
        let s = poly[i];
        let e = poly[(i + 1) % n];
        let sc = axis(s);
        let ec = axis(e);
        let sin = inside(sc);
        let ein = inside(ec);
        if sin {
            out.push(s);
            if !ein && ec != sc {
                let t = (val as f64 - sc as f64) / (ec as f64 - sc as f64);
                let ix = (s.0 as f64 + t * (e.0 as f64 - s.0 as f64)).round() as i64;
                let iy = (s.1 as f64 + t * (e.1 as f64 - s.1 as f64)).round() as i64;
                out.push((ix, iy));
            }
        } else if ein && ec != sc {
            let t = (val as f64 - sc as f64) / (ec as f64 - sc as f64);
            let ix = (s.0 as f64 + t * (e.0 as f64 - s.0 as f64)).round() as i64;
            let iy = (s.1 as f64 + t * (e.1 as f64 - s.1 as f64)).round() as i64;
            out.push((ix, iy));
        }
    }
    out
}

// Clip a polygon to rect (all four half-planes). Returns the intersection (open loop) or empty.
fn clip_polygon(poly: &[(i64, i64)], rect: (i64, i64, i64, i64)) -> Vec<(i64, i64)> {
    let (rw, rs, re, rn) = rect;
    let mut p = sh_clip(poly, |p| p.0, rw, |c| c >= rw);
    p = sh_clip(&p, |p| p.0, re, |c| c <= re);
    p = sh_clip(&p, |p| p.1, rs, |c| c >= rs);
    p = sh_clip(&p, |p| p.1, rn, |c| c <= rn);
    p
}

// Max vertices a polygon ring may have. The head-unit renderer triangulates every polygon in
// map_tclTriangulate::u16TessellatePolygon (procmapengine 0x003b8df0), whose ear-clip keeps the
// ring's vertex indices in a single BYTE and fills its index list with `do { idx[i]=..; i=(i+1)&0xff }
// while (i < n)`. For n >= 256 that loop wraps and can never reach n -> INFINITE LOOP -> watchdog
// reboot (roads, never tessellated, are unaffected). This cap is PREVENTIVE hardening: per-tile
// clipping already bounds our rings to <=198 points, so it is inactive for current data, and it was
// NOT the #06c reboot cause (that was the missing per-polygon annotation — see build_block). Keep
// well under the 256/signed-byte cliff for margin against the `(char)` arithmetic there too.
const MAX_RING_PTS: usize = 250;

// Perpendicular (squared) distance from p to the infinite line through a-b, in coordinate units.
fn perp_dist2(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> f64 {
    let (ax, ay) = (a.0 as f64, a.1 as f64);
    let (bx, by) = (b.0 as f64, b.1 as f64);
    let (px, py) = (p.0 as f64, p.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    if l2 == 0.0 {
        let (ex, ey) = (px - ax, py - ay);
        return ex * ex + ey * ey;
    }
    let num = (dy * px - dx * py + bx * ay - by * ax).abs();
    (num * num) / l2
}

// Open Douglas-Peucker (keeps endpoints) as a stack loop; `eps2` is squared tolerance.
fn rdp(pts: &[(i64, i64)], eps2: f64) -> Vec<(i64, i64)> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let (mut dmax, mut idx) = (0.0f64, s);
        for i in (s + 1)..e {
            let d = perp_dist2(pts[s], pts[e], pts[i]);
            if d > dmax {
                dmax = d;
                idx = i;
            }
        }
        if dmax > eps2 {
            keep[idx] = true;
            stack.push((s, idx));
            stack.push((idx, e));
        }
    }
    pts.iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, &p)| p).collect()
}

// Reduce a closed ring (open loop, no duplicated first point) to <= MAX_RING_PTS vertices using
// closed Douglas-Peucker at growing tolerance, with a uniform-stride hard fallback so the bound is
// guaranteed regardless of shape complexity.
fn decimate_ring(ring: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let n = ring.len();
    if n <= MAX_RING_PTS {
        return ring.to_vec();
    }
    // Split the closed ring into two open chains at the vertex farthest from ring[0], simplify both,
    // and stitch so shared endpoints are not duplicated.
    let mut ek = 1usize;
    let mut dk = -1.0f64;
    for i in 1..n {
        let dx = (ring[i].0 - ring[0].0) as f64;
        let dy = (ring[i].1 - ring[0].1) as f64;
        let d = dx * dx + dy * dy;
        if d > dk {
            dk = d;
            ek = i;
        }
    }
    let mut eps2 = 1.0f64;
    loop {
        let a = rdp(&ring[..=ek], eps2);
        let b = rdp(&ring[ek..], eps2);
        let mut out = a;
        if b.len() >= 2 {
            // Drop only the shared seam b[0] (= ring[ek], already a's last). KEEP b[last]
            // (= ring[n-1]): the ring is an open loop, so that vertex closes back to ring[0];
            // dropping it removed a real corner and turned rectangles into triangles.
            out.extend_from_slice(&b[1..]);
        }
        if out.len() <= MAX_RING_PTS {
            return out;
        }
        if eps2 > 1e15 {
            break;
        }
        eps2 *= 4.0;
    }
    // Guaranteed fallback: keep evenly spaced vertices.
    let stride = (n + MAX_RING_PTS - 1) / MAX_RING_PTS;
    ring.iter().step_by(stride).cloned().collect()
}

// Closed-ring Douglas-Peucker at a fixed tolerance (squared `eps2`), independent of the vertex cap.
// Splits at the vertex farthest from ring[0], simplifies both open chains, stitches without
// duplicating the shared seam. Used for the per-level LOD pass before the <=MAX_RING_PTS cap.
fn simplify_ring(ring: &[(i64, i64)], eps2: f64) -> Vec<(i64, i64)> {
    let n = ring.len();
    if n < 4 {
        return ring.to_vec();
    }
    let mut ek = 1usize;
    let mut dk = -1.0f64;
    for i in 1..n {
        let dx = (ring[i].0 - ring[0].0) as f64;
        let dy = (ring[i].1 - ring[0].1) as f64;
        let d = dx * dx + dy * dy;
        if d > dk {
            dk = d;
            ek = i;
        }
    }
    let a = rdp(&ring[..=ek], eps2);
    let b = rdp(&ring[ek..], eps2);
    let mut out = a;
    if b.len() >= 2 {
        // Keep b[last] (= ring[n-1]); only b[0] (the seam at ring[ek]) duplicates a's end.
        out.extend_from_slice(&b[1..]);
    }
    out
}

// Squared bounding-box diagonal of a shape - used to drop sub-visible (smaller than one level
// tolerance) slices at coarse zoom.
fn bbox_diag2(pts: &[(i64, i64)]) -> f64 {
    if pts.is_empty() {
        return 0.0;
    }
    let (mut minx, mut maxx) = (pts[0].0, pts[0].0);
    let (mut miny, mut maxy) = (pts[0].1, pts[0].1);
    for &(x, y) in pts {
        if x < minx { minx = x; } if x > maxx { maxx = x; }
        if y < miny { miny = y; } if y > maxy { maxy = y; }
    }
    let dx = (maxx - minx) as f64;
    let dy = (maxy - miny) as f64;
    dx * dx + dy * dy
}

// Per-tile geometry after clipping.
#[derive(Default)]
struct TileShapes {
    roads: Vec<Road>,                         // (slice, roadinfo_w, ref, name)
    waterways: Vec<(Vec<(i64, i64)>, u16)>, // (slice, watercode)
    areas: Vec<(Vec<(i64, i64)>, u16)>,     // (clipped ring, landuse feat)
    pois: Vec<(i64, i64, u16, String, u16)>, // (lon, lat, feat, name, city_bits)
}

// Split every shape across the level's tile grid. POIs go to their point's cell; lines and
// polygons are clipped so each cell holds only the slice inside its extent.
fn distribute(
    level: usize,
    roads: &[Road],
    waterways: &[(Vec<(i64, i64)>, u16)],
    areas: &[(Vec<(i64, i64)>, u16)],
    pois: &[(i64, i64, u16, String, u16)],
) -> HashMap<i64, TileShapes> {
    let mut map: HashMap<i64, TileShapes> = HashMap::new();
    // Per-level level-of-detail: simplify geometry to sub-pixel tolerance and drop shapes smaller
    // than one tolerance unit. Coarser levels (small `level`) carry far less detail.
    let eps = sim_eps(level) as f64;
    let eps2 = eps * eps;

    for (lo, la, feat, name, city) in pois {
        let (c, r) = point_col_row(level, *lo, *la);
        map.entry(cell_to_k(level, c, r))
            .or_default()
            .pois
            .push((*lo, *la, *feat, name.clone(), *city));
    }

    for (geom, w, rn, nm, fo) in roads {
        // Level-of-detail by rank. Low-class roads (netclass 5/6/7: unclassified/residential/
        // living_street, the white drivable-local 0x35 override, and service/track/path 0x21) are
        // stock-stored ONLY at the finest level L3, so they show only in the street view and vanish
        // on zoom-out (0x21 carries no 0x11/netclass, so without this the engine never hides them
        // and they outlived the white streets). The engine composites levels 0..Z, so nc0..4 (auto-
        // way..tertiary) stay visible at every zoom and must NOT be gated.
        if (*w & 7) >= 5 && level < 3 {
            continue;
        }
        let (c0, c1, r0, r1) = cell_span(level, geom);
        for c in c0..=c1 {
            for r in r0..=r1 {
                let rect = cell_rect(level, c, r);
                let mut slice = clip_polyline(geom, rect);
                if slice.len() >= 3 {
                    slice = rdp(&slice, eps2);
                }
                if slice.len() >= 2 && bbox_diag2(&slice) >= eps2 {
                    map.entry(cell_to_k(level, c, r))
                        .or_default()
                        .roads
                        .push((slice, *w, rn.clone(), nm.clone(), *fo));
                }
            }
        }
    }

    for (geom, wc) in waterways {
        let (c0, c1, r0, r1) = cell_span(level, geom);
        for c in c0..=c1 {
            for r in r0..=r1 {
                let rect = cell_rect(level, c, r);
                let mut slice = clip_polyline(geom, rect);
                if slice.len() >= 3 {
                    slice = rdp(&slice, eps2);
                }
                if slice.len() >= 2 && bbox_diag2(&slice) >= eps2 {
                    map.entry(cell_to_k(level, c, r))
                        .or_default()
                        .waterways
                        .push((slice, *wc));
                }
            }
        }
    }

    let area_th = min_area_diag(level) as f64;
    let area_th2 = area_th * area_th;
    for (geom, feat) in areas {
        let (c0, c1, r0, r1) = cell_span(level, geom);
        for c in c0..=c1 {
            for r in r0..=r1 {
                let rect = cell_rect(level, c, r);
                let slice = clip_polygon(geom, rect);
                // Per-level area LOD: drop landuse smaller than the level threshold (dense
                // buildings never reach the coarse tiles). L3 threshold 0 -> keep all.
                if slice.len() >= 3 && bbox_diag2(&slice) >= area_th2 {
                    // Per-level LOD first, then tessellator-safe cap below 256 vertices.
                    let sim = simplify_ring(&slice, eps2);
                    let ring = decimate_ring(&sim);
                    if ring.len() >= 3 && bbox_diag2(&ring) >= eps2 {
                        map.entry(cell_to_k(level, c, r))
                            .or_default()
                            .areas
                            .push((ring, *feat));
                    }
                }
            }
        }
    }

    map
}

// ---- OSM parsing -----------------------------------------------------------
fn attr<'i>(e: &'i quick_xml::events::BytesStart<'i>, key: &str) -> Option<&'i str> {
    use std::borrow::Cow;
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| match a.value {
            Cow::Borrowed(s) => Some(s),
            Cow::Owned(_) => None, // OSM values always borrow from the input buffer
        })
}

fn node_coords(e: &quick_xml::events::BytesStart<'_>) -> Option<(i64, i64, i64)> {
    let id = attr(e, "id")?;
    let la = attr(e, "lat")?;
    let lo = attr(e, "lon")?;
    let id: i64 = id.parse().ok()?;
    let la: f64 = la.parse().ok()?;
    let lo: f64 = lo.parse().ok()?;
    Some((id, deg2pau(lo), deg2pau(la)))
}

fn poi_feat(tags: &HashMap<String, String>) -> u16 {
    let g = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(a) = g("amenity") {
        match a {
            "parking" => return 0x02,
            "charging_station" | "charging" => return 0x03,
            "fuel" => return 0x04,
            "restaurant" | "fast_food" | "cafe" | "food_court" => return 0x06,
            "car_rental" => return 0x09,
            "school" | "college" | "university" => return 0x10,
            "bar" | "pub" | "beer_house" => return 0x11,
            "pharmacy" => return 0x13,
            "bank" | "atm" => return 0x15,
            "place_of_worship" | "church" | "temple" | "mosque" | "synagogue" => return 0x16,
            _ => {}
        }
    }
    if let Some(t) = g("tourism") {
        match t {
            "hotel" | "guest_house" | "hostel" => return 0x05,
            "attraction" | "museum" | "viewpoint" | "artwork" | "theme_park" => return 0x17,
            _ => {}
        }
    }
    if let Some(s) = g("shop") {
        match s {
            "supermarket" | "convenience" | "greengrocer" | "bakery" | "butcher" | "mall" => {
                return 0x14
            }
            "car" => return 0x07,
            _ => {}
        }
    }
    if let Some(l) = g("leisure") {
        match l {
            "sports_centre" | "stadium" | "pitch" | "golf_course" => return 0x12,
            _ => {}
        }
    }
    if g("railway").map(|r| r == "station").unwrap_or(false) {
        return 0x22;
    }
    if g("office").map(|o| !o.is_empty()).unwrap_or(false) {
        return 0x08;
    }
    if tags.contains_key("place") {
        return 0x01; // settlement marker (stock city POIs use feature low 0x01 + a 0x21 annotation)
    }
    0x01
}

// Settlement `place=*` -> TravelMap `0x21` city-annotation payload (u16), decoded from stock N6E2:
//   bits 0-3 display level (label min-zoom; smaller=shown earlier), bits 4-7 size class
//   (importance, 1=biggest..15=hamlet), bits 8-10 admin level (1=voivodeship capital, 7=ordinary),
//   bit 15 name-overlap. Stock correlation: cities size~6 disp~4-6, towns 9-11, villages 12-15,
//   admin=7 for all non-capitals. Display is clamped at 12 for the smallest settlements.
fn city_bits(tags: &HashMap<String, String>) -> u16 {
    if !env_on("OSM2MAP_CITY_ANN") {
        return 0; // #09 ladder: suppress the 0x21 city annotation to isolate it
    }
    let (disp, size) = match tags.get("place").map(|s| s.as_str()) {
        Some("city") => (5u16, 6u16),
        Some("town") => (9, 9),
        Some("suburb") | Some("municipality") => (11, 11),
        Some("village") => (12, 13),
        Some("hamlet") | Some("locality") => (12, 15),
        Some(_) => (12, 13), // other place values -> village-sized
        None => return 0,    // not a settlement -> no city annotation
    };
    disp | (size << 4) | (7u16 << 8) // admin=7 (ordinary), overlap bit=0
}

// OSM highway -> TravelMap roadinfo `w` payload (the 0x11 annotation). Bit layout is
// Ghidra-confirmed: bits 0-2 netclass (0=motorway..7=service); 4-5 toll; 6-7 ferry;
// 8-9 closed; 12-15 road type (1=long ramp, 2=roundabout, 3=parallel, 9=interconnect/link).
// `w & 7` (netclass) also drives per-level selection. Link roads take road_type=9 so the
// renderer emits <class>_link; roundabouts take road_type=2.
fn roadinfo_w(hw: &str, junction: Option<&str>, toll: bool) -> u16 {
    let is_link = hw.ends_with("_link");
    let base = hw.strip_suffix("_link").unwrap_or(hw);
    let nc = match base {
        "motorway" => 0,
        "trunk" => 1,
        "primary" => 2,
        "secondary" => 3,
        "tertiary" => 4,
        "unclassified" | "road" => 5,
        "residential" | "living_street" => 6,
        _ => 7, // service, track, path, ...
    };
    let mut w = (nc as u16) & 0b111;
    if is_link {
        w |= 9 << 12; // interconnect/link -> <class>_link
    } else if junction == Some("roundabout") {
        w |= 2 << 12; // roundabout
    }
    if toll {
        w |= 0x10; // toll bits 4-5 (decodes to "3" = toll)
    }
    w
}

// Road-tier line feature LOW byte from the 3-bit netclass. The pen is f(feature, netclass): the
// feature maps 0x30..0x37 -> subtypeRoad 0x3d..0x44 (type-4 ROAD) and 0x21 -> 0x15 (type-2 thin
// LINE); GetLineConfigOffsetRoad(subtypeRoad, subAttributes) then indexes the theme line-data. On-car
// legend (trials/18+19) proved netclass also drives the pen: netclass 1,2,3,4,7 are feature-driven
// (0x32=orange, 0x33=green, 0x34/0x35/0x36=thin white+dark), but netclass 5 and 6 force a THICK
// yellow/red pen regardless of the feature. So drivable local streets (residential/unclassified/
// living_street) are emitted upstream with a feature-override of 0x35 + netclass 7 (thin white,
// L3-only), which bypasses this function; it therefore only ever sees nc0..4 + nc7 here.
fn road_feat_low(nc: u16) -> u16 {
    match nc & 7 {
        0 => 0x30,
        1 => 0x31,
        2 => 0x32,
        3 | 4 => 0x33,
        _ => 0x21, // nc7 = service/track/path: thin local line
    }
}

// OSM area tags -> TravelMap landuse/natural feature code (polygon low byte). None = not a
// recognized area. Inverted from map2osm landuse_osm.
fn area_feat(tags: &HashMap<String, String>) -> Option<u16> {
    let g = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(n) = g("natural") {
        match n {
            "water" => return Some(0x48),
            "wood" | "forest" => return Some(0x2B),
            _ => {}
        }
    }
    if let Some(l) = g("landuse") {
        match l {
            "residential" => return Some(0x9C),
            "grass" | "meadow" => return Some(0x38),
            "forest" | "wood" => return Some(0x2B),
            "cemetery" => return Some(0x39),
            "commercial" => return Some(0x3A),
            "water" | "basin" | "reservoir" => return Some(0x48),
            _ => {}
        }
    }
    None
}

// OSM waterway value -> 0x10 payload u16 (high nibble = type, low nibble = class). Type codes
// inverted from map2osm add_semantic: 1=river, 2=canal, 3=stream, 4=ditch.
fn watercode(ww: &str) -> u16 {
    let typ = match ww {
        "river" => 1,
        "canal" => 2,
        "stream" | "brook" | "creek" => 3,
        "ditch" | "drain" => 4,
        _ => 0,
    };
    typ << 4 // class = 0
}

// POI importance: lower = more important = shown at coarser zoom.
fn poi_rank(tags: &HashMap<String, String>) -> u8 {
    let g = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(p) = g("place") {
        return match p {
            "city" => 0,
            "town" => 1,
            "village" | "suburb" => 2,
            _ => 4, // hamlet, isolated_dwelling: finest zoom only
        };
    }
    // Services (major or minor) are an L3 surface-detail concern: shown only at the finest level.
    if g("amenity").map(|a| matches!(a, "fuel" | "hospital")).unwrap_or(false)
        || g("tourism")
            .map(|t| matches!(t, "hotel" | "attraction" | "museum"))
            .unwrap_or(false)
        || g("shop").map(|s| s == "supermarket").unwrap_or(false)
    {
        return 3;
    }
    3
}

// Level-of-detail is toggled by OSM2MAP_LOD (default OFF). Off = the car-validated geometry (#09p:
// pre-LOD per-level caps [2,4,7,7]/[1,2,3,3], no RDP simplification). The LOD path (sub-pixel
// Douglas-Peucker + coarser caps) is implemented and stock-motivated but has NOT been car-validated
// on its own (the earlier #08 reboot was the water-line code, not LOD) — set OSM2MAP_LOD=1 to test it.
fn lod_on() -> bool {
    matches!(
        env::var("OSM2MAP_LOD").unwrap_or_else(|_| "0".into()).as_str(),
        "1" | "y" | "yes" | "on" | "true"
    )
}
// Max netclass shown at each populated level (higher threshold = more detail). On: measured stock
// N6E2AA caps (L0=0 motorway, L1=1, L2=3, L3=7). Off: [2,4,6,7] — the minor local tier (nc7 =
// service/path/footway, feature 0x21) appears at L3 only, matching stock (its 0x21 lines are L3-only).
fn max_road_nc() -> [u8; 4] {
    if lod_on() { [0, 1, 3, 7] } else { [2, 4, 6, 7] }
}
// Max POI rank shown at each level (lower rank = more important). On: settlements scale by size,
// amenities/hamlets (rank>=3) L3-only. Off: the pre-LOD [1,2,3,3].
fn max_poi_rank() -> [u8; 4] {
    if lod_on() { [1, 2, 2, 4] } else { [1, 2, 3, 3] }
}
// Per-level RDP simplification tolerance in PAU: ~ tile_width/1024 (sub-pixel), where tile_width =
// regionW/grid_size[L], regionW = 214_748_364 PAU. Coarser levels simplify harder (L0 ~1.95 km,
// L3 ~3.9 m); also the min-shape size so sub-visible stubs/slivers drop at coarse zoom. Off => 1.
const SIM_EPS_ON: [i64; 4] = [209715, 41943, 4194, 419];
fn sim_eps(level: usize) -> i64 {
    if lod_on() { SIM_EPS_ON[level] } else { 1 }
}

// Per-level minimum AREA size (polygon bbox diagonal, PAU) that a tile keeps. Coarse landuse
// fragments (grass/meadow/wood/residential patches) otherwise flood the L1/L2 tiles past the
// stock-observed per-tile budget (stock N6E2 keeps <=5 sub-blocks/tile; the real cliff, not 15).
// LOD-ON floors are MEASURED from stock N6E2 dense tiles (Krakow/Warszawa L0/L1/L2 smallest kept
// poly ~820 / 510 / 85 m bbox-diag), decoupled from the sub-pixel RDP tolerance (SIM_EPS_ON).
// L3 (street view) threshold 0 keeps everything, so the street-level look is unchanged. LOD-OFF
// keeps the historical SIM_EPS values (the car-validated sparse path). Override the coarse three
// with OSM2MAP_MINAREA="t0,t1,t2" (PAU) to tune density without a rebuild.
fn min_area_diag(level: usize) -> i64 {
    let mut t = if lod_on() {
        [88_000, 55_000, 9_000, 0]
    } else {
        [SIM_EPS_ON[0], SIM_EPS_ON[1], SIM_EPS_ON[2], 0]
    };
    if let Some(s) = env::var("OSM2MAP_MINAREA").ok() {
        for (i, part) in s.split(',').take(3).enumerate() {
            if let Ok(v) = part.trim().parse::<i64>() {
                t[i] = v;
            }
        }
    }
    t[level]
}

struct OsmData {
    pois: Vec<(i64, i64, u16, String, u8, u16)>, // (lon_pau, lat_pau, feat, name, rank, city_bits)
    roads: Vec<Road>,                        // (geometry, roadinfo_w, ref, name)
    waterways: Vec<(Vec<(i64, i64)>, u16)>,  // (geometry, watercode)
    areas: Vec<(Vec<(i64, i64)>, u16)>,      // (open ring, landuse feat)
}

// Join OSM way node-chains (each an open sequence of node ids) into closed rings.
// Two chains connect when an endpoint of one equals an endpoint of the other; they are
// merged (reversing either side as needed). Chains that close on themselves (>=4 nodes,
// first==last) become rings, returned WITHOUT the repeated closing vertex (open loop, the
// convention the area pipeline expects). Open stubs are dropped. Used to assemble
// multipolygon/boundary relations whose member ways are individual boundary arcs.
fn stitch_ways(chains: &[Vec<i64>]) -> Vec<Vec<i64>> {
    let mut rest: VecDeque<Vec<i64>> =
        chains.iter().filter(|c| c.len() >= 2).cloned().collect();
    let mut rings: Vec<Vec<i64>> = Vec::new();
    while let Some(mut cur) = rest.pop_front() {
        let mut joined = true;
        while joined {
            joined = false;
            let head = cur[0];
            let tail = *cur.last().unwrap();
            if let Some(i) = (0..rest.len()).find(|&i| {
                let c = &rest[i];
                c.first() == Some(&tail) || c.last() == Some(&tail)
            }) {
                let mut c = rest.remove(i).unwrap();
                if c.first() != Some(&tail) {
                    c.reverse();
                }
                cur.extend(c[1..].iter());
                joined = true;
                continue;
            }
            if let Some(i) =
                (0..rest.len()).find(|&i| rest[i].last() == Some(&head))
            {
                let mut c = rest.remove(i).unwrap();
                c.pop(); // drop the shared `head` node
                c.extend(cur);
                cur = c;
                joined = true;
                continue;
            }
        }
        if cur.len() >= 4 && cur[0] == *cur.last().unwrap() {
            cur.pop();
            rings.push(cur);
        }
    }
    rings
}

fn parse_osm(path: &str, bw: i64, bs: i64, be: i64, bn: i64) -> OsmData {
    use std::io::BufReader;
    // Stream from disk (not fs::read) so multi-GB extracts don't need a full in-memory buffer.
    let file = fs::File::open(path).expect("open osm");
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(file));
    let mut nodes: HashMap<i64, (i64, i64)> = HashMap::new();
    let mut pois: Vec<(i64, i64, u16, String, u8, u16)> = Vec::new();
    let mut roads: Vec<Road> = Vec::new();
    let mut waterways: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();
    let mut areas: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();

    // Multipolygon/boundary relation support: OSM area relations carry their tags on the
    // <relation>, not the member ways, and members are usually open boundary arcs (never
    // closed on their own -> the closed-way path below misses them entirely). We keep every
    // way's node-id list, then after parsing assemble relation outer rings. Standalone closed
    // area ways are deferred so a way also used as a relation member is emitted once (relation
    // wins). Interior rings (role="inner") are dropped: the single-ring area pipeline fills
    // donuts (the TravelMap cell has no interior-ring / hole representation).
    let mut way_nodes: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut rel_member: HashSet<i64> = HashSet::new();
    let mut standalone_areas: Vec<(i64, Vec<(i64, i64)>, u16)> = Vec::new();

    let mut cur_node: Option<i64> = None;
    let mut cur_tags: HashMap<String, String> = HashMap::new();
    let mut cur_way_id: Option<i64> = None;
    let mut cur_way_ids: Option<Vec<i64>> = None;
    let mut cur_way_tags: HashMap<String, String> = HashMap::new();
    let mut in_relation = false;
    let mut cur_rel_tags: HashMap<String, String> = HashMap::new();
    let mut cur_rel_members: Vec<(String, i64)> = Vec::new(); // (role, way_ref)

    let mut buf = Vec::with_capacity(1024);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some((id, lo, la)) = node_coords(&e) {
                        nodes.insert(id, (lo, la));
                        cur_node = Some(id);
                        cur_tags.clear();
                    }
                }
                "way" => {
                    cur_way_id = attr(&e, "id").and_then(|s| s.parse::<i64>().ok());
                    cur_way_ids = Some(Vec::new());
                    cur_way_tags.clear();
                }
                "relation" => {
                    in_relation = true;
                    cur_rel_tags.clear();
                    cur_rel_members.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some((id, lo, la)) = node_coords(&e) {
                        nodes.insert(id, (lo, la));
                    }
                }
                "nd" => {
                    if let Some(ids) = &mut cur_way_ids {
                        if let Some(r) = attr(&e, "ref") {
                            if let Ok(v) = r.parse::<i64>() {
                                ids.push(v);
                            }
                        }
                    }
                }
                "member" => {
                    if in_relation && attr(&e, "type") == Some("way") {
                        if let Some(r) = attr(&e, "ref").and_then(|s| s.parse::<i64>().ok()) {
                            let role = attr(&e, "role").unwrap_or("").to_string();
                            cur_rel_members.push((role, r));
                        }
                    }
                }
                "tag" => {
                    if let (Some(k), Some(v)) = (attr(&e, "k"), attr(&e, "v")) {
                        if cur_node.is_some() {
                            cur_tags.insert(k.to_string(), v.to_string());
                        } else if cur_way_ids.is_some() {
                            cur_way_tags.insert(k.to_string(), v.to_string());
                        } else if in_relation {
                            cur_rel_tags.insert(k.to_string(), v.to_string());
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some(nid) = cur_node.take() {
                        let is_poi = cur_tags.keys().any(|k| {
                            matches!(k.as_str(), "amenity" | "tourism" | "shop" | "place")
                        });
                        if is_poi {
                                if let Some((lo, la)) = nodes.get(&nid) {
                                if *lo >= bw && *lo <= be && *la >= bs && *la <= bn {
                                    let name = cur_tags.get("name").cloned().unwrap_or_default();
                                    let cb = city_bits(&cur_tags);
                                    pois.push((*lo, *la, poi_feat(&cur_tags), name, poi_rank(&cur_tags), cb));
                                }
                            }
                        }
                        cur_tags.clear();
                    }
                }
                "way" => {
                    if let Some(ids) = cur_way_ids.take() {
                        let wid = cur_way_id.take();
                        if let Some(w) = wid {
                            way_nodes.insert(w, ids.clone());
                        }
                        let tags = std::mem::take(&mut cur_way_tags);
                        let g = |k: &str| tags.get(k).map(|s| s.as_str());
                        let pts: Vec<(i64, i64)> =
                            ids.iter().filter_map(|id| nodes.get(id).copied()).collect();
                        if !pts.is_empty()
                            && pts
                                .iter()
                                .any(|&(lo, la)| lo >= bw && lo <= be && la >= bs && la <= bn)
                        {
                            // Priority: highway > waterway > closed area. Clipping later drops any
                            // part that falls outside the region, so a shape only needs to overlap.
                            if let Some(hw) = g("highway") {
                                if pts.len() >= 2 {
                                    let toll = matches!(g("toll"), Some("yes") | Some("1"));
                                    let refn = g("ref").map(|s| s.to_string());
                                    let stname = g("name").map(|s| s.to_string());
                                    let mut w = roadinfo_w(hw, g("junction"), toll);
                                    // Drivable local streets -> the thin WHITE + dark-outline pen:
                                    // feature 0x35 with netclass forced to 4. Measured on-car
                                    // (trials/18-20): the pen is f(feature, netclass); netclass 5/6
                                    // force a THICK yellow/red pen regardless of the feature byte, while
                                    // netclass 4 is feature-driven, so 0x35 there renders white. netclass
                                    // 4 (not 7) is deliberate for LOD: the thin grey 0x21 lines carry no
                                    // netclass hint so the engine can only hide them once the L3 tile
                                    // unloads, which let them outlive the white streets on zoom-out.
                                    // Giving white netclass 4 (shown from the coarser tiers like tertiary)
                                    // keeps it visible no shorter than the thin lines -> correct ordering.
                                    // (service/track/path also use netclass 7 but keep feature 0x21, so
                                    // they remain thin grey and L3-only.)
                                    let local = matches!(
                                        hw.strip_suffix("_link").unwrap_or(hw),
                                        "residential" | "living_street" | "unclassified"
                                    );
                                    if local {
                                        w = (w & !7) | 4;
                                    }
                                    let fo = if local { Some(0x35) } else { None };
                                    roads.push((pts, w, refn, stname, fo));
                                }
                            } else if let Some(ww) = g("waterway") {
                                if pts.len() >= 2 {
                                    waterways.push((pts, watercode(ww)));
                                }
                            } else if ids.len() >= 4 && ids.first() == ids.last() {
                                if let Some(feat) = area_feat(&tags) {
                                    let mut ring = pts;
                                    ring.pop(); // drop the closing vertex -> open loop (Bosch convention)
                                    if ring.len() >= 3 {
                                        standalone_areas.push((wid.unwrap_or(-1), ring, feat));
                                    }
                                }
                            }
                        }
                    }
                }
                "relation" => {
                    in_relation = false;
                    let tags = std::mem::take(&mut cur_rel_tags);
                    let members = std::mem::take(&mut cur_rel_members);
                    let rtype = tags.get("type").map(|s| s.as_str());
                    if matches!(rtype, Some("multipolygon") | Some("boundary")) && !members.is_empty()
                    {
                        // Only relations whose own tags map to a land-use area both emit a
                        // polygon and claim their member ways (suppressing those ways' standalone
                        // emission). A relation with no area mapping (e.g. administrative
                        // boundary) claims nothing, so a member way's own area tags still count.
                        if let Some(feat) = area_feat(&tags) {
                            for (_, w) in &members {
                                rel_member.insert(*w);
                            }
                            let chains: Vec<Vec<i64>> = members
                                .iter()
                                .filter(|(role, _)| role != "inner") // holes unsupported -> filled
                                .filter_map(|(_, w)| way_nodes.get(w).cloned())
                                .collect();
                            for ring_ids in stitch_ways(&chains) {
                                let ring: Vec<(i64, i64)> =
                                    ring_ids.iter().filter_map(|id| nodes.get(id).copied()).collect();
                                if ring.len() >= 3
                                    && ring
                                        .iter()
                                        .any(|&(lo, la)| lo >= bw && lo <= be && la >= bs && la <= bn)
                                {
                                    areas.push((ring, feat));
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    // Emit deferred standalone closed-area ways, skipping any that a relation already owns.
    for (wid, ring, feat) in standalone_areas {
        if !rel_member.contains(&wid) {
            areas.push((ring, feat));
        }
    }
    OsmData { pois, roads, waterways, areas }
}

// ---- .MAP / .IDX emitters --------------------------------------------------
// MAP header + info/partition/metadata region [0x00..binOff) is copied verbatim from the stock
// template (Bosch-static across regions); only W/S/E/N, profile word and total file size are
// patched. Geometry blocks begin at binOff = MAP_HEADER.len() (0x7bc).
fn emit_map(path: &Path, data: &[u8], prof: u16) {
    let binoff = MAP_HEADER.len(); // 0x7bc
    let filesize = binoff + data.len();
    let mut d = MAP_HEADER.to_vec();
    d.resize(filesize, 0);
    d[4..8].copy_from_slice(&(filesize as u32).to_le_bytes()); // fileSize
    d[8..12].copy_from_slice(&(W() as u32).to_le_bytes());
    d[12..16].copy_from_slice(&(S() as u32).to_le_bytes());
    d[16..20].copy_from_slice(&(E() as u32).to_le_bytes());
    d[20..24].copy_from_slice(&(N() as u32).to_le_bytes());
    d[0x1e..0x20].copy_from_slice(&(0x8400u16 | prof).to_le_bytes()); // profile word @0x1e
    d[binoff..].copy_from_slice(data);
    fs::write(path, &d).expect("write MAP");
}

// Map-region bbox into a header buffer at `base` (W,S,E,N as u32 LE, 4 consecutive words).
fn patch_bbox(d: &mut [u8], base: usize) {
    d[base..base + 4].copy_from_slice(&(W() as u32).to_le_bytes());
    d[base + 4..base + 8].copy_from_slice(&(S() as u32).to_le_bytes());
    d[base + 8..base + 12].copy_from_slice(&(E() as u32).to_le_bytes());
    d[base + 12..base + 16].copy_from_slice(&(N() as u32).to_le_bytes());
}

// ---- sub-block packing -----------------------------------------------------
// A block's length lives in a u16 word count, so a single block (and its IDX slot/sub-entry
// length) MUST stay <= 65535 words; the in-block marker is written as `total_words & 0xFFFF`, so
// exceeding it wraps the marker to a tiny blen and the strict renderer / tmcheck walks the
// annotation+text region past the (wrapped) block -> SIGSEGV signature. The packing budget is set
// just under 65536 (4-word header + feature costs). Feature cost estimates below MUST match what
// build_block actually lays, word for word, or a group's real size can blow past this cap.
const MAX_BLOCK_WORDS: u32 = 65500; // < 65535-word u16 limit, small margin for the 4-word header

fn poi_cost(name: &str) -> u32 {
    if !label_ok(name) {
        3
    } else {
        // cell(3) + one 0x7A ann word(1) + the actual (possibly 2-variant) text record
        3 + 1 + text_rec_words(name)
    }
}

// Partition a tile's features into sub-blocks (each <= MAX_BLOCK_WORDS) and build them.
fn pack_and_build_blocks(
    shift: i32,
    cx: i64,
    cy: i64,
    polys: &[(Vec<(i64, i64)>, u16)],
    lines: &[LineCell],
    pois: &[(i64, i64, u16, Option<&str>, u16)],
) -> Vec<Vec<u8>> {
    let np = polys.len();
    let nl = lines.len();
    let n = np + nl + pois.len();
    if n == 0 {
        return Vec::new();
    }
    // word cost per feature (3 cell words + point-pool words + annotation words)
    let mut cost: Vec<u32> = Vec::with_capacity(n);
    for (pts, feat) in polys {
        cost.push(3 + pts.len() as u32 + poly_ann_words(poly_ann_kind((*feat & 0xff) as u8)));
    }
    for lc in lines {
        let aw = match lc.ann {
            Some((t, _, _)) => ann_words(t),
            None => 0,
        };
        // 0x7A street-name: 1 annotation word + its (possibly 2-variant) text record, when named.
        let nm_w = match lc.nm.as_deref() {
            Some(s) if label_ok(s) => 1 + text_rec_words(s),
            _ => 0,
        };
        cost.push(3 + lc.pts.len() as u32 + aw + rn_words(lc.rn.as_deref()) + nm_w);
    }
    for p in pois {
        cost.push(poi_cost(p.3.unwrap_or("")) + if p.4 != 0 { 1 } else { 0 });
    }
    // greedy first-fit into blocks (each starts with the 4-word header)
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut used: u32 = 4;
    for i in 0..n {
        if !cur.is_empty() && used + cost[i] > MAX_BLOCK_WORDS {
            groups.push(std::mem::take(&mut cur));
            used = 4;
        }
        cur.push(i);
        used += cost[i];
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(groups.len());
    for g in &groups {
        let mut gp = Vec::new();
        let mut gl = Vec::new();
        let mut gq = Vec::new();
        for &ri in g {
            if ri < np {
                gp.push(polys[ri].clone());
            } else if ri < np + nl {
                gl.push(lines[ri - np].clone());
            } else {
                gq.push(pois[ri - np - nl]);
            }
        }
        out.push(build_block(shift, cx, cy, &gp, &gl, &gq));
    }
    out
}

// ---- .IDX emitter (single + multi-entry slots) ------------------------------
// Header + descriptive/metadata block [0x00..partOff*4) copied verbatim from the stock template;
// only the region bbox is patched. The partition table sits at partOff*4 and the per-level tile
// tables follow immediately after it (binOff = first tile table). Multi-entry slots keep their
// sub-entry arrays appended after all four tile tables, exactly like stock dense tiles do.
fn emit_idx(path: &Path, slots: &[Vec<Option<Vec<(u16, u16, u32)>>>; 4]) {
    let mut d = idx_header().to_vec();
    let part_off_word = u16::from_le_bytes([d[0x14], d[0x15]]); // stock value (0x7e), kept as-is
    let pt = (part_off_word as usize) * 4; // partition table byte offset
    if d.len() < pt {
        d.resize(pt, 0);
    }
    let bin_off = (pt + 4 * 12) as u32; // first tile table, right after the 4 partition entries
    let mut tbl_offs = [0u32; 4];
    let mut off = bin_off;
    for i in 0..4 {
        tbl_offs[i] = off;
        off += (TILECNT[i] as u32) * 8;
    }
    let fixed_end = off; // end of the four tile tables

    // Assign byte offsets for each multi-tile's sub-entry table (appended after the tables).
    let mut sub_off = fixed_end;
    let mut sub_table: HashMap<(usize, usize), u32> = HashMap::new();
    for L in 0..4usize {
        for k in 0..TILECNT[L] {
            if let Some(e) = &slots[L][k] {
                if e.len() > 1 {
                    sub_table.insert((L, k), sub_off);
                    sub_off += (e.len() * 8) as u32;
                }
            }
        }
    }
    let total = sub_off as usize;
    d.resize(total, 0);

    d[0..2].copy_from_slice(&(bin_off as u16).to_le_bytes()); // binOff @0x00
    patch_bbox(&mut d, 4); // W/S/E/N at 0x04 (partOff@0x14 kept from template)

    // partition table: id, lat bytes, then {tileCnt<<8|shift} and {tblOff<<8}
    for i in 0..4usize {
        let o = pt + i * 12;
        d[o] = i as u8;
        d[o + 1] = LATPART[i];
        d[o + 2] = LATPART[i];
        let u32a = (TILECNT[i] as u32) << 8 | SHIFTS[i] as u32;
        d[o + 3..o + 7].copy_from_slice(&u32a.to_le_bytes());
        d[o + 7..o + 11].copy_from_slice(&(tbl_offs[i] << 8).to_le_bytes());
    }

    for L in 0..4usize {
        let tbl = tbl_offs[L] as usize;
        for k in 0..TILECNT[L] {
            let so = tbl + k * 8;
            match &slots[L][k] {
                None => {
                    // Empty tile MUST be exactly 0x8000 (bit15 set, profile bits CLEARED), len=0,
                    // off=0 — matching stock. OR-ing the profile in makes the reader resolve an empty
                    // slot to a real <REGION>1XX.MAP at offset 0 and parse the MAP header as cells →
                    // OOB read → head-unit reboot. (The bytes so+2..so+8 are already zero from resize.)
                    d[so..so + 2].copy_from_slice(&0x8000u16.to_le_bytes());
                }
                Some(e) if e.len() == 1 => {
                    let (rp, len, offb) = e[0]; // rp = 0x400|profile, bits 14-15 clear
                    d[so..so + 2].copy_from_slice(&rp.to_le_bytes());
                    d[so + 2..so + 4].copy_from_slice(&len.to_le_bytes());
                    d[so + 4..so + 8].copy_from_slice(&offb.to_le_bytes());
                }
                Some(e) => {
                    let sbo = sub_table[&(L, k)];
                    // multi header regProf = exactly 0x4000 (bit14 only, profile bits cleared) like stock
                    let a: u32 = ((e.len() as u32) << 16) | 0x4000u32;
                    d[so..so + 4].copy_from_slice(&a.to_le_bytes());
                    d[so + 4..so + 8].copy_from_slice(&sbo.to_le_bytes());
                    // each sub-entry carries its OWN profile word (land shard and/or hydro 0I overlay)
                    for (j, &(rp, len, offb)) in e.iter().enumerate() {
                        let q = (sbo as usize) + j * 8;
                        let aa: u32 = ((len as u32) << 16) | (rp as u32);
                        d[q..q + 4].copy_from_slice(&aa.to_le_bytes());
                        d[q + 4..q + 8].copy_from_slice(&offb.to_le_bytes());
                    }
                }
            }
        }
    }
    fs::write(path, &d).expect("write IDX");
}

// ---- TCI (TILE_CLUSTER_INDEX) emission -------------------------------------
// Per-MAP-file sub-index. Layout reverse-engineered from DAPIAPP.OUT (dap_map_tclTCIHeader /
// TCIPartition / u16LoadPartitionTable / u16LoadClusterIndexTile) and confirmed against the
// stock N6E2 10I/11A .TCI files:
//   [0x00] header (20B): u16 f0=0, u16 f1=92, u32 filesize, u16 partOff=0x84, u16 partCnt=4,
//                        u16 f5=122, u16 f6=16, u16 f7=12, u16 f8=106  (format constants)
//   [0x14] descriptive block (112B): copyright/version/"TILE_CLUSTER_INDEX"/"TPNAV2" metadata
//   [0x84] partition table: 4 x {u32 level, u32 tileCount, u32 sectionOffset}
//   [0xb4] per-level tile records: tileCount x {u16 primCl, u16 cl, u32 clusterOffset}
// The runtime (dap_map_tclIdController::u16GenerateTileIds) resolves a region's tiles via the
// .IDX path; the TCI is only consulted for "cluster" tiles and, if the file is absent, logs
// 0x307 "Could not read tci file" and skips them. For profile 10I the stock TCI's cluster pool
// is entirely empty (no geometry), so a structurally-valid all-empty TCI matches what Bosch
// ships: every tile record is zeroed and no cluster data follows.
fn emit_tci(path: &Path, tilecnt: &[usize; 4]) {
    const DESCRIPTIVE_BLOCK: [u8; 112] = [
        0x43, 0x6f, 0x70, 0x79, 0x72, 0x69, 0x67, 0x68, 0x74, 0x20, 0x52, 0x6f, // "Copyright Ro"
        0x62, 0x65, 0x72, 0x74, 0x2d, 0x42, 0x6f, 0x73, 0x63, 0x68, 0x2d, 0x47, // "bert-Bosch-G"
        0x6d, 0x62, 0x48, 0x20, 0x20, 0x32, 0x30, 0x30, 0x33, 0x00, 0x31, 0x42, // "mbH  2003\01B"
        0x39, 0x2e, 0x30, 0x33, 0x2e, 0x31, 0x38, 0x3a, 0x31, 0x33, 0x3a, 0x30, // "9.03.18:13:0"
        0x39, 0x00, 0x54, 0x49, 0x4c, 0x45, 0x5f, 0x43, 0x4c, 0x55, 0x53, 0x54, // "9\0TILE_CLUST"
        0x45, 0x52, 0x5f, 0x49, 0x4e, 0x44, 0x45, 0x58, 0x00, 0x00, 0x00, 0x00, // "ER_INDEX\0\0\0"
        0x14, 0x00, 0x36, 0x00, 0x00, 0x00, 0x46, 0x00, 0x03, 0x00, 0x01, 0x00, // (20,54,0,70,3,1)
        0x00, 0x00, 0x31, 0x42, 0x39, 0x2e, 0x30, 0x33, 0x2e, 0x31, 0x38, 0x3a, // \0\0"1B9.03.18:"
        0x31, 0x31, 0x3a, 0x31, 0x34, 0x00, 0x54, 0x50, 0x4e, 0x41, 0x56, 0x32, // "11:14\0TPNAV2"
        0x00, 0x00, 0x00, 0x00,
    ];
    let prefix = 20 + 112 + 4 * 12; // header + descriptive + partition table = 180
    let rec_total: u32 = tilecnt.iter().map(|&c| c as u32).sum::<u32>() * 8;
    let filesize = (prefix as u32) + rec_total;
    let mut d = vec![0u8; filesize as usize]; // record arrays start zeroed (empty records)

    // header
    d[0..2].copy_from_slice(&0u16.to_le_bytes());
    d[2..4].copy_from_slice(&92u16.to_le_bytes());
    d[4..8].copy_from_slice(&filesize.to_le_bytes());
    d[8..10].copy_from_slice(&0x84u16.to_le_bytes()); // partOff
    d[10..12].copy_from_slice(&4u16.to_le_bytes()); // partCnt
    d[12..14].copy_from_slice(&122u16.to_le_bytes());
    d[14..16].copy_from_slice(&16u16.to_le_bytes());
    d[16..18].copy_from_slice(&12u16.to_le_bytes());
    d[18..20].copy_from_slice(&106u16.to_le_bytes());
    // descriptive block (verbatim stock metadata)
    d[0x14..0x84].copy_from_slice(&DESCRIPTIVE_BLOCK);
    // partition table: sectionOffset[L] = 180 + sum(tilecnt[0..L]) * 8
    let mut off = prefix as u32;
    for (lvl, &cnt) in tilecnt.iter().enumerate() {
        let p = 0x84 + lvl * 12;
        d[p..p + 4].copy_from_slice(&(lvl as u32).to_le_bytes());
        d[p + 4..p + 8].copy_from_slice(&(cnt as u32).to_le_bytes());
        d[p + 8..p + 12].copy_from_slice(&off.to_le_bytes());
        off += cnt as u32 * 8;
    }
    fs::write(path, &d).expect("write TCI");
}

fn main() {
    let argv: Vec<String> = env::args().collect();
    // Robust parse: options --region=NAME / --bbox=W,S,E,N (anywhere) + positional
    // osm_in [outdir [bbox]]. Keeps the legacy `osm2map in.osm outdir "W,S,E,N"` form working and
    // lets --region=N6E1 appear in any position without being mistaken for the bbox.
    let mut region_arg: Option<String> = None;
    let mut bbox_arg: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--region=").or_else(|| a.strip_prefix("-r=")) {
            region_arg = Some(v.to_string());
        } else if a == "--region" || a == "-r" {
            if let Some(v) = it.next() {
                region_arg = Some(v.clone());
            }
        } else if let Some(v) = a.strip_prefix("--bbox=") {
            bbox_arg = Some(v.to_string());
        } else if a == "--bbox" {
            if let Some(v) = it.next() {
                bbox_arg = Some(v.clone());
            }
        } else if bbox_arg.is_none() && pos.len() >= 2 && a.contains(',') {
            bbox_arg = Some(a.clone()); // 3rd positional = bbox, legacy form
        } else {
            pos.push(a.clone());
        }
    }
    region_arg = region_arg.or_else(|| env::var("OSM2MAP_REGION").ok());
    let osm_in = pos
        .get(0)
        .cloned()
        .unwrap_or_else(|| "/home/marek/Ext/reverse_engineering/NissanMaps/OSM-map/krzeszowice.osm".into());
    let outdir = pos
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/tmp/opencode/wt2".into());
    fs::create_dir_all(&outdir).ok();

    // setup_region loads the target region's real stock <REGION>AA.IDX header + bbox (resinf
    // catalog). MUST run before any geometry, which reads W()/S()/E()/N().
    setup_region(region_arg, bbox_arg.is_some());

    // bbox in PAU from --bbox/legacy 3rd positional (degrees), else the region's stock bounds.
    let (bw, bs, be, bn) = match &bbox_arg {
        Some(s) => {
            let mut it = s.split(',').map(|x| deg2pau(x.trim().parse().unwrap()));
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        None => (W(), S(), E(), N()),
    };

    let t0 = std::time::Instant::now();
    let mut osm = parse_osm(&osm_in, bw, bs, be, bn);

    // ---- #06 isolation ladder: content-reduction modes (env OSM2MAP_MODE) ----
    // Each mode is a CUMULATIVE superset of the previous, so the first rung that reboots pins the
    // single mechanism it introduced. full == the #07 build (known to reboot => positive control).
    //   empty      : no features; all slots 0x8000. Tests generated headers/IDX/TCI/container only.
    //   roads      : + road polylines into land `02` (line cells + feat 0x30..0x33/0x21 per class + annot 0x11 on road tiers). Single profile.
    //   land       : + land-use polygons (polygon cells + area feats; water polygons stripped). Still `02` only.
    //   poi_noname : + POI point cells + POI feats, names STRIPPED (no text annotation).
    //   poi_name   : + POI name annotations (the 0x7A TEXT record) — isolates the name/text encoder.
    //   full       : + waterway lines (annot 0x10) and water-area polygons -> hydro `0I` overlay + multi
    //                [02,0I] tiles == #07 behaviour.
    let mode = env::var("OSM2MAP_MODE").unwrap_or_else(|_| "full".into());
    match mode.as_str() {
        "empty" => {
            osm.roads.clear();
            osm.waterways.clear();
            osm.areas.clear();
            osm.pois.clear();
        }
        "roads" => {
            osm.waterways.clear();
            osm.areas.clear();
            osm.pois.clear();
        }
        "land" | "poly_min" | "poi_noname" | "poi_name" => {
            // No water at all in these rungs (neither lines nor water polygons): keep it single-profile.
            osm.waterways.clear();
            osm.areas.retain(|(_, f)| f & 0xFF != 0x48);
            if mode == "land" || mode == "poly_min" {
                osm.pois.clear();
            }
        }
        _ => {} // "full": everything (== #07)
    }
    // poly_min: keep only the N area features nearest the boot-view target, so a SMALL but
    // in-view polygon set is rendered. Decides the "#06c reboots because there are too many
    // polygons" hypothesis: if ~N polygons still reboot exactly like full #06c, it is NOT quantity.
    if mode == "poly_min" {
        let max_areas: usize =
            env::var("OSM2MAP_MAX_AREAS").ok().and_then(|s| s.parse().ok()).unwrap_or(50);
        let tx = deg2pau(19.656422f64); // target lon (Krzeszowice)
        let ty = deg2pau(50.139191f64); // target lat
        osm.areas.sort_by_key(|(ring, _)| {
            let cx: i64 = ring.iter().map(|p| p.0).sum::<i64>() / ring.len() as i64;
            let cy: i64 = ring.iter().map(|p| p.1).sum::<i64>() / ring.len() as i64;
            let dx = cx - tx;
            let dy = cy - ty;
            dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
        });
        osm.areas.truncate(max_areas);
    }
    let emit_names = matches!(mode.as_str(), "full" | "poi_name");
    // OSM2MAP_HYDRO: "land" (DEFAULT, car-validated #09p) = put water into the land shard `02`, a
    // single profile with no 0I and no multi-profile slots — the model stock uses inland (every
    // element incl. water is profile 02). "overlay" = split water into a separate `0I` profile +
    // multi-profile slots (Bosch's coastline/maritime model); retained for coastal regions, off by
    // default.
    let hydro_overlay = matches!(
        env::var("OSM2MAP_HYDRO").unwrap_or_else(|_| "land".into()).as_str(),
        "overlay" | "0I" | "0i" | "split" | "multi"
    );
    // Optional water sub-filters (both on by default) for isolating lines vs areas within a rung.
    let water_lines = env_on("OSM2MAP_WATER_LINES");
    let water_areas = env_on("OSM2MAP_WATER_AREAS");
    // Water-LINE feature code (low byte). Stock N6E2 water lines = 0x20 (feature_type "line" => 3,
    // the hydrography reader that consumes the 0x10 annotation). 0x30 is a ROAD line (reader expects
    // 0x11 roadinfo) — emitting 0x10 on a 0x30 line misparses and reboots (#09l evidence). Default 0x20.
    let waterline_feat: u16 =
        u16::from_str_radix(&env::var("OSM2MAP_WATERLINE_FEAT").unwrap_or_else(|_| "20".into()), 16)
            .unwrap_or(0x20);
    eprintln!("OSM2MAP_MODE={}  (emit_names={})", mode, emit_names);

    eprintln!(
        "parsed {} pois, {} roads, {} waterways, {} areas in bbox ({}s)",
        osm.pois.len(),
        osm.roads.len(),
        osm.waterways.len(),
        osm.areas.len(),
        t0.elapsed().as_secs_f64()
    );

    let map_binoff: u32 = MAP_HEADER.len() as u32; // geometry begins at binOff (0x7bc), after the header/info template
    // Two independent MAP files, each with its own geometry starting at map_binoff: land shard and hydro.
    let mut map_land: Vec<u8> = Vec::new();
    let mut map_hydro: Vec<u8> = Vec::new();
    // slots[L][k] = None (empty tile) or Some(list of (regProfWord, lenWords, mapOffset) sub-blocks).
    // regProfWord = 0x400 | profile id (bits 14-15 clear); a tile may mix the land shard and 0I overlay.
    let mut slots: [Vec<Option<Vec<(u16, u16, u32)>>>; 4] = [
        vec![None; TILECNT[0]],
        vec![None; TILECNT[1]],
        vec![None; TILECNT[2]],
        vec![None; TILECNT[3]],
    ];

    for L in 0..4usize { // populate all four levels (L3 = finest, 500x500 grid)
        let shift = SHIFTS[L];
        let (mnc_arr, mpr_arr) = (max_road_nc(), max_poi_rank());
        let mnc = mnc_arr[L];
        let mpr = mpr_arr[L];
        let roadnum_on = env_on("OSM2MAP_ROADNUM");
        let streetname_on = env_on("OSM2MAP_STREETNAMES");

        // Per-level selection: roads by netclass (w & 7), POIs by rank. Waterways and landuse
        // areas are local detail, so they're only emitted at L1/L2 (not the whole-country L0).
        // OSM2MAP_ROADNUM=0 clears every `ref` (suppresses 0x14 + text); OSM2MAP_STREETNAMES=0 clears
        // every street `name` (suppresses the 0x7A line label + text).
        // The feature-override (fo=0x35 white local) is a COLOUR choice, NOT an LOD exemption: those
        // ways still carry their true netclass (4), so they MUST be gated by `w & 7` like every other
        // road. Letting `fo.is_some()` bypass the gate is what flooded dense-city L1/L2 tiles (every
        // local street appeared at every zoom -> thousands of lines/tile -> past the stock <=5 blk cap).
        let roads: Vec<Road> = osm
            .roads
            .iter()
            .filter(|(_, w, _, _, _)| (*w & 7) <= mnc as u16)
            .map(|(g, w, rn, nm, fo)| {
                (
                    g.clone(),
                    *w,
                    if roadnum_on { rn.clone() } else { None },
                    if streetname_on { nm.clone() } else { None },
                    *fo,
                )
            })
            .collect();
        let pois: Vec<(i64, i64, u16, String, u16)> = osm
            .pois
            .iter()
            .filter(|p| p.4 <= mpr)
            .map(|(a, b, c, d, _, ct)| (*a, *b, *c, d.clone(), *ct))
            .collect();
        let show_detail = L >= 1;
        let waterways: Vec<(Vec<(i64, i64)>, u16)> = if show_detail {
            osm.waterways.clone()
        } else {
            Vec::new()
        };
        let areas: Vec<(Vec<(i64, i64)>, u16)> = if show_detail {
            osm.areas.clone()
        } else {
            Vec::new()
        };

        // Divide every shape across this level's tile grid (each tile gets its clipped slice).
        let mut dist = distribute(L, &roads, &waterways, &areas, &pois);
        eprintln!(
            "L{}: {} roads(nc<={}) + {} pois(rank<={}) + {} water + {} areas across {} tiles",
            L,
            roads.len(),
            mnc,
            pois.len(),
            mpr,
            waterways.len(),
            areas.len(),
            dist.len()
        );

        let mut keys: Vec<i64> = dist.keys().copied().collect();
        keys.sort();
        let ntiles = keys.len();
        let mut nsub = 0usize;
        for K in keys {
            let ts = dist.remove(&K).unwrap();
            let (cx, cy) = tile_center(L, K);

            // Split this tile's content into LAND (shard profile `02`) vs HYDRO (`0I` overlay).
            // Bosch's `0I` carries ONLY water: waterway lines + water-area polygons, never roads/POI.
            let mut lines_land: Vec<LineCell> = Vec::with_capacity(ts.roads.len());
            for (geom, w, rn, nm, fo) in &ts.roads {
                let nc = *w & 7;
                // Road feature (see road_feat_low): netclass 0..4 -> type-4 tiers 0x30/0x31/0x32/0x33/
                // 0x33 (carry the 0x11 roadinfo + 0x14 ref + 0x7A name). Drivable local streets arrive
                // here with fo=0x35 (thin white + dark outline, nc7) -> also type-4, keeps 0x11. Only
                // service/track/path (nc7, fo=None) fall through to 0x21 type-2, which carries no
                // roadinfo/label (stock model). roadclass off => proven flat #09p (every road 0x30 + 0x11).
                // OSM2MAP_MINOR21=0 folds those 0x21 streets into the thinnest SAFE type-4 tier 0x33.
                let rc = roadclass_on();
                let feat = match fo {
                    Some(f) => *f,
                    None => {
                        if !rc {
                            0x30
                        } else {
                            let f = road_feat_low(nc);
                            if f == 0x21 && !minor21_on() { 0x33 } else { f }
                        }
                    }
                };
                // 0x21 thin local lines carry no roadinfo/label (stock); all type-4 tiers keep 0x11.
                if feat == 0x21 {
                    lines_land.push(LineCell { pts: geom.clone(), feat, ann: None, rn: None, nm: None });
                } else {
                    // Road tiers (type-4): proven 0x11 roadinfo (+ 0x14 ref + 0x7A street name).
                    lines_land.push(LineCell {
                        pts: geom.clone(),
                        feat,
                        ann: Some((0x11, *w, 0)),
                        rn: rn.clone(),
                        nm: nm.clone(),
                    });
                }
            }
            let mut lines_hydro: Vec<LineCell> = Vec::with_capacity(ts.waterways.len());
            if water_lines {
                for (geom, wc) in &ts.waterways {
                    lines_hydro.push(LineCell { pts: geom.clone(), feat: waterline_feat, ann: Some((0x10, *wc, 0)), rn: None, nm: None });
                }
            }
            let mut land_polys: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();
            let mut hydro_polys: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();
            for a in ts.areas.iter().cloned() {
                if is_water_area_feat(a.1) {
                    if water_areas { hydro_polys.push(a); }
                } else {
                    land_polys.push(a);
                }
            }
            let tpois_named: Vec<(i64, i64, u16, Option<&str>, u16)> = ts
                .pois
                .iter()
                .map(|(a, b, c, d, ct)| {
                    (*a, *b, *c, if emit_names { Some(d.as_str()) } else { None }, *ct)
                })
                .collect();

            // Append a profile's sub-blocks to its own MAP buffer, tagging each slot entry with that
            // profile's regProf word. Land first, then the 0I hydro overlay (matches stock co-refs).
            // In HYDRO=land mode, water is merged into the land shard `02` (single profile, no 0I,
            // no multi-slot) — the inland model stock actually uses.
            let mut entries: Vec<(u16, u16, u32)> = Vec::new();
            if hydro_overlay {
                for blk in pack_and_build_blocks(shift, cx, cy, &land_polys, &lines_land, &tpois_named) {
                    let offb = map_binoff + map_land.len() as u32;
                    let lw = (blk.len() / 4) as u16;
                    map_land.extend_from_slice(&blk);
                    entries.push((0x400 | land_prof(), lw, offb));
                }
                for blk in pack_and_build_blocks(shift, cx, cy, &hydro_polys, &lines_hydro, &[]) {
                    let offb = map_binoff + map_hydro.len() as u32;
                    let lw = (blk.len() / 4) as u16;
                    map_hydro.extend_from_slice(&blk);
                    entries.push((0x400 | HYDRO_PROF, lw, offb));
                }
            } else {
                let mut lp = land_polys;
                lp.extend(hydro_polys);
                let mut ll = lines_land;
                ll.extend(lines_hydro);
                for blk in pack_and_build_blocks(shift, cx, cy, &lp, &ll, &tpois_named) {
                    let offb = map_binoff + map_land.len() as u32;
                    let lw = (blk.len() / 4) as u16;
                    map_land.extend_from_slice(&blk);
                    entries.push((0x400 | land_prof(), lw, offb));
                }
            }
            nsub += entries.len();
            if entries.len() > 15 {
                eprintln!("WARN L{} tile {}: {} sub-entries (> 15 multi-slot cap)", L, K, entries.len());
            }
            if !entries.is_empty() {
                slots[L][K as usize] = Some(entries);
            }
        }
        eprintln!("L{}: {} non-empty tiles -> {} sub-blocks", L, ntiles, nsub);
    }

    let idx_path = format!("{}/{}AA.IDX", outdir, region());
    let land_file = prof_file(land_prof()); // e.g. N6E2102 (profile 0x02)
    let hydro_file = prof_file(HYDRO_PROF); // N6E210I
    emit_map(Path::new(&format!("{}/{}.MAP", outdir, land_file)), &map_land, land_prof());
    emit_map(Path::new(&format!("{}/{}.MAP", outdir, hydro_file)), &map_hydro, HYDRO_PROF);
    emit_idx(Path::new(&idx_path), &slots);
    emit_tci(Path::new(&format!("{}/{}.TCI", outdir, land_file)), &TILECNT);
    emit_tci(Path::new(&format!("{}/{}.TCI", outdir, hydro_file)), &TILECNT);

    eprintln!(
        "wrote {} + land {}.MAP/.TCI ({} B) + hydro {}.MAP/.TCI ({} B), {}s total",
        idx_path,
        land_file,
        map_land.len(),
        hydro_file,
        map_hydro.len(),
        t0.elapsed().as_secs_f64()
    );
}
