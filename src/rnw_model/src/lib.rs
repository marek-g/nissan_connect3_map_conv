//! RNW one-cell model — THE shared authority for cluster identity.
//!
//! Why this crate exists: a GenAttr house-number cell row carries a `global_id`
//! (column `0x004`, `LID_format.md` §11.6b) that the destination path hands to the
//! routing layer to locate the road `onecell`/cluster inside the RNW tiles
//! (`NAV*.DAT`). `osm2rnw` (RNW writer) and `osm2lid` (GenAttr writer) must emit the
//! *same* cluster number for the *same* road. They do — not by the RNW writer telling
//! the LID writer anything, but because BOTH compute ids from identical code over the
//! identical OSM input. Concretely, a same-source pair needs no cross-file consultation
//! as long as all of the following shared decisions apply:
//!
//! 1. [`routable_way`] — the ONE road-inclusion predicate (= `osm2rnw`'s tag filter +
//!    `classify()` acceptance); `osm2rnw` remains the RNW-semantics authority.
//! 2. [`walk_onecells`] — the segment walk out of way node lists: parse order,
//!    `windows(2)`, skip pairs with missing nodes, bbox test on the FIRST node only,
//!    skip collapsed (equal-node) pairs.
//! 3. [`build_clusters`] + the push-order numbering (`cluster id = group index + 1`,
//!    as `osm2rnw`'s `write_nav` assigns) over the root [`extent`] of used endpoints.
//!
//! Changing ANY of these in one consumer and not the other silently shifts cluster
//! ids; the GenAttr bounds check (`LID_format.md` §11.6b) will not catch a *valid but
//! wrong* row, so the device would happily route each address to its own wrong segment.
//! Keep both binaries calling THIS crate, and re-run the byte-level cross-check
//! (`trials`-style golden runs of `osm2rnw` + `osm2lid` over one OSM) whenever you edit
//! anything here. `osm2lid`'s PA cell id (`PA cell` -> `bGetPACellIDs 00b898dc` -> the
//! street block's `enGetCells`) inherits the same identity through the `0xc11` row.

/// Default onecells-per-cluster target of the bbox quad-split (`osm2rnw --target`).
pub const TARGET_ONECELLS: usize = 700;
/// Quad-split depth cap (`osm2rnw build_clusters` stop condition).
pub const MAX_SPLIT_DEPTH: u32 = 16;

/// The ONE routable-road predicate, kept byte-identical to the `osm2rnw` pipeline
/// (`add_way` blacklist ∧ `classify()` acceptance ⇒ this whitelist). Input: the highway
/// tag value. Anything else (paths, cycleways, constructions, …) is not a segment.
pub fn routable_way(hw: &str) -> bool {
    matches!(
        hw.strip_suffix("_link").unwrap_or(hw),
        "motorway"
            | "trunk"
            | "primary"
            | "secondary"
            | "tertiary"
            | "unclassified"
            | "road"
            | "residential"
            | "living_street"
            | "service"
    )
}

/// PAU midpoint of two endpoints — the clustering key. Integer ÷2 exactly as `osm2rnw`
/// builds `Seg::mid` (both binaries must round the same way).
pub fn midpoint(a: (i64, i64), b: (i64, i64)) -> (i64, i64) {
    ((a.0 + b.0) / 2, (a.1 + b.1) / 2)
}

/// Axis-aligned extent `(w, s, e, n)` of a point set — argument order matches
/// `osm2rnw::extent`; the split root consumes `(w, s, e, n)` as-is.
pub fn extent(pts: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    let (mut w, mut s, mut e, mut n) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for &(x, y) in pts {
        if x < w {
            w = x;
        }
        if x > e {
            e = x;
        }
        if y < s {
            s = y;
        }
        if y > n {
            n = y;
        }
    }
    (w, s, e, n)
}

/// Walk onecells out of parse-ordered ways. Input items are `(node_ids, payload)` —
/// the payload (street name, RNW attrs, …) rides along in `OneCell::extra` and MUST NOT
/// feed back into inclusion or order. `coords` resolves a node id to its PAU position
/// (missing ⇒ the pair is skipped). `bbox` is the optional PAU window `(w, s, e, n)`,
/// tested on the FIRST node of each window only (`osm2rnw` rule). Returns segments in
/// canonical order with ids and `mid` precomputed for [`build_clusters`].
pub fn walk_onecells<'a, E: Clone + 'a>(
    ways: impl IntoIterator<Item = (Vec<i64>, E)>,
    coords: impl Fn(i64) -> Option<(i64, i64)>,
    bbox: Option<(i64, i64, i64, i64)>,
) -> Vec<OneCell<E>> {
    let mut out: Vec<OneCell<E>> = Vec::new();
    for (ids, extra) in ways {
        for pair in ids.windows(2) {
            let (ca, cb) = match (coords(pair[0]), coords(pair[1])) {
                (Some(a), Some(b)) => (a, b),
                _ => continue,
            };
            if let Some((w, s, e, n)) = bbox {
                if ca.0 < w || ca.0 > e || ca.1 < s || ca.1 > n {
                    continue;
                }
            }
            if pair[0] == pair[1] {
                continue;
            }
            out.push(OneCell { ca, cb, ids: (pair[0], pair[1]), mid: midpoint(ca, cb), extra: extra.clone() });
        }
    }
    out
}

/// One routable road segment (RNW onecell): node ids, endpoints, the clustering key
/// `mid`, and a consumer payload. Geometry/ids/mid are what cluster identity depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct OneCell<E = ()> {
    pub ids: (i64, i64),
    pub ca: (i64, i64),
    pub cb: (i64, i64),
    pub mid: (i64, i64),
    pub extra: E,
}

/// Root box for the split: extent of every used segment endpoint
/// (`osm2rnw`: `extent(&gnodes)` over the segments that actually made it into `segs`).
pub fn root_box(segs: &[OneCell<impl Clone>]) -> (i64, i64, i64, i64) {
    let mut pts = Vec::with_capacity(segs.len() * 2);
    for s in segs {
        pts.push(s.ca);
        pts.push(s.cb);
    }
    extent(&pts)
}

/// Cluster segment groups from the split root box (convenience over [`build_clusters`]).
pub fn clusters_of(segs: &[OneCell<impl Clone>], target: usize) -> Vec<Vec<usize>> {
    let mids: Vec<(i64, i64)> = segs.iter().map(|s| s.mid).collect();
    build_clusters(&mids, root_box(segs), target.max(1))
}

/// Recursive bbox quad-split, target `target` mids per group, stack DFS — copied
/// op-for-op from `osm2rnw build_clusters`, the ONLY clusterer. Returns segment-index
/// groups in pop order; **cluster id = group index + 1** (see [`cluster_ids`]).
pub fn build_clusters(mids: &[(i64, i64)], root: (i64, i64, i64, i64), target: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let all: Vec<usize> = (0..mids.len()).collect();
    let (rw, rs, re, rn) = root; // extent order (w, s, e, n) -> walk order (w, e, s, n)
    let mut stack = vec![(0u32, rw, re, rs, rn, all)];
    while let Some((d, w, e, s, n, mem)) = stack.pop() {
        if mem.len() <= target || d >= MAX_SPLIT_DEPTH || w >= e || s >= n {
            if !mem.is_empty() {
                out.push(mem);
            }
            continue;
        }
        let mw = w + (e - w) / 2;
        let mh = s + (n - s) / 2;
        let mut q: [Vec<usize>; 4] = [vec![], vec![], vec![], vec![]];
        for &i in &mem {
            let (x, y) = mids[i];
            q[(if x >= mw { 1 } else { 0 }) + if y >= mh { 2 } else { 0 }].push(i);
        }
        let cells = [(w, mw, s, mh), (mw, e, s, mh), (w, mw, mh, n), (mw, e, mh, n)];
        for (cell, (cw, ce, cs, cn)) in q.into_iter().zip(cells) {
            if !cell.is_empty() {
                stack.push((d + 1, cw, ce, cs, cn, cell));
            }
        }
    }
    out
}

/// Per-segment cluster ids (`0x004` values): `group position + 1` exactly as
/// `osm2rnw`'s `write_nav` numbers the blobs it is handed in this same order.
pub fn cluster_ids(groups: &[Vec<usize>], nseg: usize) -> Vec<u32> {
    let mut ids = vec![0u32; nseg];
    for (gi, g) in groups.iter().enumerate() {
        for &i in g {
            ids[i] = gi as u32 + 1;
        }
    }
    ids
}

/// Squared PAU distance from point `p` to segment `ab` (i128, clamped projection).
/// Used by `osm2lid` to snap an address to its nearest segment of the street.
pub fn pt_seg_d2(p: (i64, i64), a: (i64, i64), b: (i64, i64)) -> i128 {
    let (px0, py0) = (p.0 as i128, p.1 as i128);
    let (ax, ay) = (a.0 as i128, a.1 as i128);
    let (bx, by) = (b.0 as i128, b.1 as i128);
    let (vx, vy) = (bx - ax, by - ay);
    let len2 = (vx * vx + vy * vy).max(1);
    let t = ((px0 - ax) * vx + (py0 - ay) * vy).clamp(0, len2);
    let dx = px0 - (ax + vx * t / len2);
    let dy = py0 - (ay + vy * t / len2);
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    // A way 1: (0,0)->(10,0)->(20,0); way 2 uses a missing node id (9) mid-way.
    #[test]
    fn walk_rules_and_determinism() {
        let coords = |id: i64| -> Option<(i64, i64)> {
            match id {
                0 => Some((0, 0)),
                1 => Some((10, 0)),
                2 => Some((20, 0)),
                3 => Some((0, 10)),
                _ => None,
            }
        };
        let ways = vec![
            (vec![0i64, 1, 2], "A".to_string()),
            (vec![1, 9, 3], "B".to_string()), // pair (1,9) and (9,3) have missing coords
            (vec![2, 2], "C".to_string()),    // collapsed pair
        ];
        let cells = walk_onecells(ways, coords, None);
        assert_eq!(cells.len(), 2); // (0,1) and (1,2) only
        assert_eq!(cells[0].ids, (0, 1));
        assert_eq!(cells[1].ids, (1, 2));
        assert_eq!(cells[0].mid, (5, 0)); // midpoint rounds like osm2rnw integer div
        let again = walk_onecells(
            vec![
                (vec![0i64, 1, 2], "A".to_string()),
                (vec![1, 9, 3], "B".to_string()),
                (vec![2, 2], "C".to_string()),
            ],
            coords,
            None,
        );
        assert_eq!(cells, again);
    }

    #[test]
    fn bbox_hits_first_node_only() {
        let coords = |id: i64| -> Option<(i64, i64)> {
            match id {
                0 => Some((0, 0)),
                1 => Some((100, 0)), // outside window
                2 => Some((200, 0)),
                _ => None,
            }
        };
        // bbox covers (0,0) but not (100,0): window (0,1) kept (first node in bbox),
        // window (1,2) dropped (first node outside).
        let cells = walk_onecells(vec![(vec![0i64, 1, 2], ())], coords, Some((0, 0, 50, 0).into_wsen()));
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].ids, (0, 1));
    }

    trait Wsen {
        fn into_wsen(self) -> (i64, i64, i64, i64);
    }
    impl Wsen for (i64, i64, i64, i64) {
        fn into_wsen(self) -> (i64, i64, i64, i64) {
            self
        }
    }

    #[test]
    fn cluster_ids_match_group_plus_one_and_are_deterministic() {
        // 3 points in a 100x100 box, target 1 -> split until singleton groups.
        let mids = vec![(10i64, 10), (90, 10), (60, 90)];
        let a = build_clusters(&mids, (0, 0, 100, 100), 1);
        let b = build_clusters(&mids, (0, 0, 100, 100), 1);
        assert_eq!(a, b);
        let ids = cluster_ids(&a, mids.len());
        assert_eq!(a.len(), 3);
        assert!(ids.iter().all(|&c| c >= 1 && c <= 3));
        for (gi, g) in a.iter().enumerate() {
            for &i in g {
                assert_eq!(ids[i], gi as u32 + 1);
            }
        }
    }
}
