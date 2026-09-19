#!/usr/bin/env python3
"""Inject a road-code LEGEND comb into an OSM extract for an on-car visual test.

The comb is 9 short (~100 m) straight segments pointing north, evenly spaced
west->east just north of a reference street, each way tagged
`highway=residential` + `legend_code=0xNN` so osm2map emits that exact feature
byte. Order west->east is fixed and printed so the user can map position->code.

Usage: gen_roadlegend.py <in.osm> <out.osm> [base_lat] [base_lon] [teeth_csv]
"""
import re, sys

in_osm, out_osm = sys.argv[1], sys.argv[2]
base_lat = float(sys.argv[3]) if len(sys.argv) > 3 else 50.14105
base_lon = float(sys.argv[4]) if len(sys.argv) > 4 else 19.65400
teeth = (sys.argv[5].split(",") if len(sys.argv) > 5
         else ["0x21","0x30","0x31","0x32","0x33","0x34","0x35","0x36","0x37"])
# each tooth is "code" or "code:nc" (nc = 0x11 netclass 0..7, default derived from residential=6)

TOOTH_LAT = 0.00090      # ~100 m north
LON_STEP  = 0.00075      # ~53 m spacing between teeth
DLEN      = 0.00012      # split each tooth into ~2 segments (equal parts)

nid = -900000            # negative ids, won't collide with real OSM ids
wid = -900000
extra_nodes, extra_ways = [], []

for i, spec in enumerate(teeth):
    code, _, nc = spec.partition(":")
    lon = base_lon + i * LON_STEP
    lats = [base_lat + k * (TOOTH_LAT / 2.0) for k in range(3)]  # 3 pts = 2 equal segments
    ids = []
    for la in lats:
        nid -= 1
        extra_nodes.append(f'  <node id="{nid}" lat="{la:.7f}" lon="{lon:.7f}" />')
        ids.append(nid)
    wid -= 1
    nds = "".join(f'    <nd ref="{x}" />\n' for x in ids)
    nctag = f'    <tag k="legend_nc" v="{nc}" />\n' if nc else ""
    extra_ways.append(
        f'  <way id="{wid}">\n{nds}'
        f'    <tag k="highway" v="residential" />\n'
        f'    <tag k="legend_code" v="{code}" />\n'
        f'{nctag}'
        f'    <tag k="name" v="LEG{code}{"_"+nc if nc else ""}" />\n'
        f'  </way>\n'
    )

d = open(in_osm, encoding="utf-8", errors="replace").read()
d = d.replace("</osm>", "".join(extra_nodes) + "".join(extra_ways) + "</osm>\n")
open(out_osm, "w", encoding="utf-8").write(d)

print(f"wrote {out_osm} with {len(teeth)} legend teeth")
print("west -> east order (count teeth from the WEST end):")
for i, spec in enumerate(teeth):
    lon = base_lon + i * LON_STEP
    print(f"  #{i}  lon={lon:.5f}  spec={spec}")
print(f"base lat {base_lat} (south end) -> {base_lat+TOOTH_LAT:.5f} (north tip), teeth ~{TOOTH_LAT*111320:.0f} m")
