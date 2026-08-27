// Bosch TravelMap (Nissan LCN2KAI) RNW (road network) -> roads extractor.
// Rust port of rnw_extract.py. Output JSONL is semantically identical to the
// Python version; formatting uses native Rust conventions (plain float
// display, raw UTF-8 in strings instead of \uXXXX escapes).
//
// Usage: rnw_extract_rs <CCP_DIR> <out.jsonl> [-b W,S,E,N|none]
//
// Format notes: see rnw_extract.py in the parent directory.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::exit;

// Geographic sanity filter for the 16KB-aligned cluster scan. The scan is a
// heuristic (the runtime locates clusters via the CCP container index), so a
// plausible reference position helps reject false positives in padding.
// Default covers the whole EUR dataset (Iceland..Turkey, Morocco..N. Scandinavia).
struct BBox {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
}

impl BBox {
    fn contains(&self, lon_pau: f64, lat_pau: f64) -> bool {
        self.west * PAU < lon_pau
            && lon_pau < self.east * PAU
            && self.south * PAU < lat_pau
            && lat_pau < self.north * PAU
    }

    fn parse(spec: &str) -> Option<BBox> {
        if spec.eq_ignore_ascii_case("none") {
            return None;
        }
        let mut it = spec.split(',');
        let west: f64 = it.next()?.parse().ok()?;
        let south: f64 = it.next()?.parse().ok()?;
        let east: f64 = it.next()?.parse().ok()?;
        let north: f64 = it.next()?.parse().ok()?;
        if it.next().is_some() || !(west < east && south < north) {
            return None;
        }
        Some(BBox { west, south, east, north })
    }
}

const PAU: f64 = (1i64 << 31) as f64 / 180.0;
const BLOCK: usize = 0x4000;
const ORDER: &[(u16, &str)] = &[
    (0, "ann"), (1, "skip"), (2, "ci1"), (3, "ci2"), (4, "zero"),
    (5, "one"), (6, "skip"), (7, "skip"), (8, "pos"), (9, "skip"),
    (10, "skip"),
];

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn i16le(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}
fn i32le(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn s8(b: u8) -> i32 {
    if b & 0x80 != 0 { b as i32 - 256 } else { b as i32 }
}
fn s24(b0: u8, b1: u8, b2: u8) -> i32 {
    let v = (b0 as i32) | ((b1 as i32) << 8) | (((b2 as i32) & 0x7F) << 16);
    if b2 & 0x80 != 0 { v - (1 << 24) } else { v }
}

// Minimal JSON string escaping: quote, backslash and control characters.
// Non-ASCII is written as raw UTF-8.
fn json_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

fn read_pts(cd: &[u8], off: usize, cnt: u16, ctype: u8) -> Option<Vec<(i32, i32)>> {
    let ptsize = if ctype == 3 { 4 } else { 6 };
    let mut out = Vec::with_capacity(cnt as usize);
    for i in 0..cnt as usize {
        let q = off + i * ptsize;
        if q + ptsize > cd.len() {
            return None;
        }
        if ctype == 3 {
            out.push((i16le(cd, q) as i32, i16le(cd, q + 2) as i32));
        } else {
            out.push((
                s24(cd[q], cd[q + 1], cd[q + 2]),
                s24(cd[q + 3], cd[q + 4], cd[q + 5]),
            ));
        }
    }
    Some(out)
}

fn read_string(cd: &[u8], off: usize) -> Option<Vec<String>> {
    if off == 0 || off >= cd.len() {
        return None;
    }
    let n = cd[off] as usize;
    if !(1..=16).contains(&n) || off + 1 + 2 * n > cd.len() {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    let mut q = off + 1 + 2 * n;
    for i in 0..n {
        let l = cd[off + 1 + 2 * i + 1] as usize;
        if l == 0 || l > 300 || q + l > cd.len() {
            return None;
        }
        match std::str::from_utf8(&cd[q..q + l]) {
            Ok(s) => out.push(s.to_string()),
            Err(_) => return None,
        }
        q += l;
    }
    Some(out)
}

fn read_names(cd: &[u8], ann_off: usize, ann_cnt: u16) -> Option<Vec<String>> {
    let mut names = None;
    let mut q = ann_off;
    for _ in 0..ann_cnt as usize {
        if q + 4 > cd.len() {
            break;
        }
        let size = u16le(cd, q) as usize;
        let typ = u16le(cd, q + 2);
        if size < 4 || size > 64 {
            break;
        }
        if typ == 0x3C && q + 6 <= cd.len() {
            let toff = u16le(cd, q + 4) as usize;
            if let Some(v) = read_string(cd, toff) {
                names = Some(v);
            }
        }
        q += size;
    }
    names
}

struct Road {
    k: u32,
    pts: Option<Vec<(i64, i64)>>,
    name: Option<Vec<String>>,
    rc: u32,
    nc: u32,
    rt: u32,
    link: u32,
    sec: u32,
    fw: u32,
}

fn parse_cluster(d: &[u8], start: usize, end: usize, bbox: &Option<BBox>) -> Option<Vec<Road>> {
    let cd = &d[start..end];
    let cluster_id = u16le(cd, 0);
    let hdr_flags = u16le(cd, 2);
    if cluster_id == 0 || hdr_flags == 0 {
        return None;
    }
    let lon = i32le(cd, 8) as i64;
    let lat = i32le(cd, 12) as i64;
    let shift = cd[0x10] as i8;
    let ooff = u16le(cd, 0x12) as usize;
    let ocnt = u16le(cd, 0x14);
    let lf = u16le(cd, 0x16);
    if lf & 0x30 == 0 {
        return None;
    }
    if let Some(bb) = bbox {
        if !bb.contains(lon as f64, lat as f64) {
            return None;
        }
    }
    if !(shift >= 0 && shift <= 12) || ooff >= cd.len() || ocnt > 0x4000 {
        return None;
    }
    let ctype: u8 = if hdr_flags & 0x40 != 0 { 4 } else { 3 };
    let ptsize = if ctype == 4 { 6 } else { 4 };
    if ooff + ptsize * ocnt as usize > cd.len() {
        return None;
    }
    let mut p = ooff + ptsize * ocnt as usize;
    let mut descs: HashMap<&'static str, (usize, u16)> = HashMap::new();
    for &(bit, name) in ORDER {
        if lf & (1 << bit) != 0 {
            if name == "skip" {
                p += 4;
                continue;
            }
            if p + 4 > cd.len() {
                return None;
            }
            descs.insert(name, (u16le(cd, p) as usize, u16le(cd, p + 2)));
            p += 4;
        }
    }
    let Some(&(o_one, oc_)) = descs.get("one") else {
        return None;
    };

    // position list -> node positions
    let mut nodes: Vec<(i64, i64)> = Vec::new();
    if let (Some(&(po, pc)), Some(&(_, zc))) = (descs.get("pos"), descs.get("zero")) {
        if pc == zc {
            if let Some(pts) = read_pts(cd, po, pc, ctype) {
                for (dx, dy) in pts {
                    nodes.push((
                        lon + ((dx as i64) << shift),
                        lat + ((dy as i64) << shift),
                    ));
                }
            }
        }
    }

    // zerocells -> toNode/fromNode per onecell
    let mut to_node = vec![-1i32; oc_ as usize];
    let mut from_node = vec![-1i32; oc_ as usize];
    if let Some(&(zo, zc)) = descs.get("zero") {
        for i in 0..zc as usize {
            let qz = zo + i * 6;
            if qz + 6 > cd.len() {
                break;
            }
            let lzf = u16le(cd, qz + 2);
            let offz = u16le(cd, qz + 4) as usize;
            let mut q = offz;
            for bit in 0..2u16 {
                if lzf & (1 << bit) == 0 {
                    continue;
                }
                if q + 4 > cd.len() {
                    break;
                }
                let o2 = u16le(cd, q) as usize;
                let c2 = u16le(cd, q + 2);
                q += 4;
                if bit == 1 {
                    for j in 0..c2 as usize {
                        let r = o2 + j * 2;
                        if r + 2 > cd.len() {
                            break;
                        }
                        let v = u16le(cd, r);
                        let oi = (v & 0x3FF) as i32 - 1;
                        if oi >= 0 && oi < oc_ as i32 {
                            // bit 15: set = TO node, clear = FROM node (1-based idx).
                            // Confirmed vs rnw_tclBaseExtensionGenerate::u16AddFromAndToZerocell.
                            if v & 0x8000 != 0 {
                                to_node[oi as usize] = i as i32;
                            } else {
                                from_node[oi as usize] = i as i32;
                            }
                        }
                    }
                }
            }
        }
    }

    // onecells -> roads
    let mut roads: Vec<Road> = Vec::new();
    for k in 0..oc_ as usize {
        let p2 = o_one + k * 12;
        if p2 + 12 > cd.len() {
            break;
        }
        let hdr = u32le(cd, p2);
        let lfo = u16le(cd, p2 + 8);
        let offf = u16le(cd, p2 + 10) as usize;
        if offf == 0 || offf + 4 > cd.len() {
            continue;
        }
        let mut q = offf;
        let (mut ann_off, mut ann_cnt) = (0usize, 0u16);
        let mut shape1: Option<(usize, u16)> = None;
        let mut shape5: Option<(usize, u16)> = None;
        for bit in 0..6u16 {
            if lfo & (1 << bit) == 0 {
                continue;
            }
            if q + 4 > cd.len() {
                break;
            }
            let o2 = u16le(cd, q) as usize;
            let c2 = u16le(cd, q + 2);
            q += 4;
            match bit {
                0 => {
                    ann_off = o2;
                    ann_cnt = c2;
                }
                1 => shape1 = Some((o2, c2)),
                2 => q += 4, // two inline u16 (upcell refs)
                3 | 4 => {}   // downcells / overlaps: not needed for geometry
                5 => shape5 = Some((o2, c2)),
                _ => unreachable!(),
            }
        }
        if shape1.is_none() && shape5.is_none() {
            continue;
        }

        let mut pts: Option<Vec<(i64, i64)>> = None;
        if let Some((so, sc)) = shape5 {
            if let Some(raw) = read_pts(cd, so, sc, ctype) {
                if !raw.is_empty() {
                    pts = Some(
                        raw.iter()
                            .map(|&(dx, dy)| {
                                (
                                    lon + ((dx as i64) << shift),
                                    lat + ((dy as i64) << shift),
                                )
                            })
                            .collect(),
                    );
                }
            }
        } else {
            let (so, sc) = shape1.unwrap();
            if sc >= 1 && so + 2 * sc as usize <= cd.len() {
                let dvec: Vec<(i64, i64)> = (0..sc as usize)
                    .map(|i| {
                        (
                            s8(cd[so + i * 2]) as i64 * 256,
                            s8(cd[so + i * 2 + 1]) as i64 * 256,
                        )
                    })
                    .collect();
                let tn = to_node[k];
                if !nodes.is_empty() && tn >= 0 && tn < nodes.len() as i32 {
                    let (tx, ty) = nodes[tn as usize];
                    let (dl_, dt_) = dvec[dvec.len() - 1];
                    pts = Some(
                        dvec.iter()
                            .map(|&(dx, dy)| (tx + dx - dl_, ty + dy - dt_))
                            .collect(),
                    );
                }
            }
        }

        // MAP line = [fromNode] + shapePts + [toNode]; add known endpoints
        if pts.as_ref().map_or(false, |v| !v.is_empty()) {
            let base_pts = pts.take().unwrap();
            let mut out_pts: Vec<(i64, i64)> = Vec::new();
            let fn_ = from_node[k];
            if !nodes.is_empty() && fn_ >= 0 && fn_ < nodes.len() as i32 {
                out_pts.push(nodes[fn_ as usize]);
            }
            out_pts.extend(base_pts.iter().copied());
            let tn = to_node[k];
            if !nodes.is_empty() && tn >= 0 && tn < nodes.len() as i32 {
                out_pts.push(nodes[tn as usize]);
            }
            let thr = 2.0 / PAU;
            let mut res: Vec<(i64, i64)> = Vec::new();
            for pnt in out_pts {
                if res.is_empty()
                    || (pnt.0 - res[res.len() - 1].0).abs() as f64 > thr
                    || (pnt.1 - res[res.len() - 1].1).abs() as f64 > thr
                {
                    res.push(pnt);
                }
            }
            pts = Some(res);
        }

        let mut names = None;
        if ann_cnt != 0 {
            names = read_names(cd, ann_off, ann_cnt);
        }
        roads.push(Road {
            k: k as u32,
            pts,
            name: names,
            rc: hdr & 0x7,
            nc: (hdr >> 4) & 0x7,
            rt: (hdr >> 8) & 0xF,
            link: (hdr >> 13) & 1,
            sec: (hdr >> 15) & 1,
            fw: (hdr >> 30) & 1,
        });
    }
    Some(roads)
}

fn find_clusters(d: &[u8], bbox: &Option<BBox>) -> Vec<usize> {
    let mut starts = Vec::new();
    let n = d.len();
    let stop = n.saturating_sub(0x20);
    let mut start = 0usize;
    while start < stop {
        let cluster_id = u16le(d, start);
        let hdr_flags = u16le(d, start + 2);
        if cluster_id != 0 && hdr_flags != 0 {
            let lon = i32le(d, start + 8) as f64;
            let lat = i32le(d, start + 12) as f64;
            if bbox.as_ref().map_or(true, |bb| bb.contains(lon, lat)) {
                let lf = u16le(d, start + 0x16);
                if lf & 0x30 != 0 && lf & !0x7FF == 0 {
                    let ooff = u16le(d, start + 0x12) as usize;
                    let ocnt = u16le(d, start + 0x14);
                    let shift = d[start + 0x10];
                    if shift <= 128 && ooff < BLOCK && ocnt <= 0x4000 {
                        starts.push(start);
                    }
                }
            }
        }
        start += BLOCK;
    }
    starts
}

fn extract_file(path: &Path, bbox: &Option<BBox>) -> Vec<Road> {
    let d = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("warning: cannot read {}: {}", path.display(), e);
            return Vec::new();
        }
    };
    let starts = find_clusters(&d, bbox);
    let mut out = Vec::new();
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(d.len());
        if let Some(c) = parse_cluster(&d, start, end, bbox) {
            out.extend(c);
        }
    }
    out
}

fn write_line(fh: &mut impl Write, f_field: &str, r: &Road) -> io::Result<()> {
    let mut s = String::with_capacity(256);
    s.push_str("{\"f\": \"");
    json_escape(f_field, &mut s);
    s.push_str("\", \"k\": ");
    s.push_str(&r.k.to_string());
    s.push_str(", \"name\": ");
    match &r.name {
        Some(names) => {
            s.push('"');
            json_escape(&names[0], &mut s);
            s.push('"');
        }
        None => s.push_str("null"),
    }
    s.push_str(", \"alt\": ");
    match &r.name {
        Some(names) if names.len() > 1 => {
            s.push('[');
            for (i, n) in names[1..].iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push('"');
                json_escape(n, &mut s);
                s.push('"');
            }
            s.push(']');
        }
        _ => s.push_str("null"),
    }
    s.push_str(&format!(
        ", \"rc\": {}, \"nc\": {}, \"rt\": {}, \"link\": {}, \"sec\": {}, \"fw\": {}",
        r.rc, r.nc, r.rt, r.link, r.sec, r.fw
    ));
    s.push_str(", \"pts\": ");
    match &r.pts {
        Some(pts) => {
            s.push('[');
            for (i, &(x, y)) in pts.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push('[');
                s.push_str(&format!("{}", x as f64 / PAU));
                s.push_str(", ");
                s.push_str(&format!("{}", y as f64 / PAU));
                s.push(']');
            }
            s.push(']');
        }
        None => s.push_str("null"),
    }
    s.push('}');
    s.push('\n');
    fh.write_all(s.as_bytes())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut bbox_spec = "-30,30,60,75".to_string(); // whole EUR dataset
    let mut positional: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "-b" && i + 1 < args.len() {
            bbox_spec = args[i + 1].clone();
            i += 2;
        } else if let Some(spec) = args[i].strip_prefix("-b=") {
            bbox_spec = spec.to_string();
            i += 1;
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    let base = positional.get(0).cloned().unwrap_or_else(|| {
        "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/"
            .to_string()
            + "CRYPTNAV/DATA/DATA/RNW/CCP"
    });
    let outp = positional
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/tmp/opencode/rnw_roads.jsonl".to_string());
    let bbox = match BBox::parse(&bbox_spec) {
        Some(b) => Some(b),
        None if bbox_spec.eq_ignore_ascii_case("none") => None,
        None => {
            eprintln!(
                "invalid -b '{}' (expected W,S,E,N in degrees or 'none')",
                bbox_spec
            );
            exit(1);
        }
    };
    eprintln!(
        "bbox: {}",
        match &bbox {
            Some(b) => format!("{},{},{},{}", b.west, b.south, b.east, b.north),
            None => "none".to_string(),
        }
    );

    let base_path = PathBuf::from(&base);
    let entries = match fs::read_dir(&base_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("cannot read {}: {}", base, e);
            exit(1);
        }
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let has_dat = names.iter().any(|f| f.ends_with(".DAT"));
    let mut dirs: Vec<PathBuf> = if has_dat {
        vec![base_path.clone()]
    } else {
        names.retain(|n| base_path.join(n).is_dir());
        names.sort();
        names.iter().map(|n| base_path.join(n)).collect()
    };
    dirs.sort();

    let mut files: Vec<(PathBuf, String)> = Vec::new();
    for dd in &dirs {
        let list = match fs::read_dir(dd) {
            Ok(l) => l,
            Err(_) => continue,
        };
        let mut fns: Vec<String> = list
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|f| f.starts_with("NAV") && f.ends_with(".DAT"))
            .collect();
        fns.sort();
        for fn_ in fns {
            files.push((dd.clone(), fn_));
        }
    }

    let file = match fs::File::create(&outp) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot create {}: {}", outp, e);
            exit(1);
        }
    };
    let mut fh = BufWriter::new(file);
    let (mut total, mut named, mut geom) = (0u64, 0u64, 0u64);
    for (dd, fn_) in &files {
        let roads = extract_file(&dd.join(fn_), &bbox);
        let f_field = format!(
            "{}/{}",
            dd.file_name().unwrap().to_string_lossy(),
            fn_
        );
        for r in &roads {
            total += 1;
            if r.name.is_some() {
                named += 1;
            }
            if r.pts.as_ref().map_or(false, |p| !p.is_empty()) {
                geom += 1;
            }
            if let Err(e) = write_line(&mut fh, &f_field, r) {
                eprintln!("write error: {}", e);
                exit(1);
            }
        }
    }
    if let Err(e) = fh.flush() {
        eprintln!("write error: {}", e);
        exit(1);
    }
    println!(
        "files={} roads={} named={} with_geom={}",
        files.len(),
        total,
        named,
        geom
    );
}
