# TravelMap `.RNW` (NAV) — a plain-language, detailed overview

This document explains the Bosch "TravelMap" **road-network** file format (as used in the Nissan
LCN2KAI) in words a non-programmer can follow. It is intentionally verbose: every technical term is
defined the first time it appears, and each part of the file is explained not just for *what* it is
but for *why* it exists and *what job* it does.

It is the companion to [`01_MAP_overview.md`](./01_MAP_overview.md), which covers the `.IDX`/`.MAP`
files (the *drawing* side of the map). This one covers the **road network** — the part the car uses
to actually *calculate a route*.

For the precise byte-level layout (offsets, sizes, bit fields) see
[`RNW_format.md`](../02%20-%20details/RNW_format.md). For how to build your own files, see
[`writer_guide.md`](../03%20-%20writer%20guide/writer_guide.md). This document is the "understanding" layer underneath both.

---

## 1. What are these files, and why do they exist?

A car's navigation system does two fundamentally different jobs:

1. **Draw the map** on the screen — roads as lines, rivers, cities, labels. (This is what the
   `.IDX`/`.MAP` files in the other document are for.)
2. **Work out how to get there** — given "I am here and I want to be there," find the best sequence
   of roads to drive.

The `.RNW` files exist entirely for job 2. They store the road network as a **graph**: a web of
**roads** (the lines) joined at **intersections** (the dots), so the navigation engine can "walk"
from one place to another, road by road, and choose the shortest or fastest path.

Think of it this way: the `.MAP` files are the *picture* of the map; the `.RNW` files are the
*underlying wiring diagram* that shows which roads connect to which. You could look at a beautiful
map and still not know how to route across it — but given the wiring diagram (which road touches
which junction), routing is just a matter of following connections.

Two design goals shaped everything about the format, exactly as for `.MAP`:

1. **Fast random access.** The car cannot load "the whole road network of Europe." It must be able to
   say *"give me just the roads around here"* and read only the relevant chunks. So the network is
   chopped into many small, independently-locatable pieces called **clusters**, and an index file
   tells the car exactly where every cluster lives.
2. **Small size.** Coordinates and attributes are stored in the most compact form possible (short
   whole numbers, offsets relative to a nearby reference point, shared/reused text).

The format achieves both by being **hierarchical**: the world is split into regions, each region's
road network is split into clusters, and each cluster packs its roads and intersections tightly. A
table-of-contents file (`NAV_ROOT.DAT`) tells the car exactly where every cluster lives.

---

## 2. The files and how they relate

A road-network "region" is a folder (for example `POL` for Poland) that holds three kinds of file:

| File | Plain-language role | Analogy |
|------|---------------------|---------|
| `NAV_ROOT.DAT` | **Table of contents.** For the whole region, an index saying "the roads around *this place* are in file X, starting here, this many bytes long." | The index at the back of a book: "Chapter 5 — page 87". |
| `NAVnnnnn.DAT` | **The content.** Chunks of the actual road network (intersections + roads), several per region. Each file holds one or more *clusters* back to back. | The actual pages of the book. |
| `AEX/AEXnnnnn.DAT` | **Optional extras.** Extra annotation/attribute data for a cluster, loaded only when the car's configuration asks for it. | A set of optional "reader's notes" you can skip. |

The relationship is one-directional, just like `.IDX`→`.MAP`: **`NAV_ROOT.DAT` points at the
`NAVnnnnn.DAT` files.** The car reads `NAV_ROOT.DAT` first to learn *where* a cluster is, then jumps
into the right `NAVnnnnn.DAT` file to read *what* is there. You never need a data file without its
root index, and the root index alone contains no roads — just pointers.

### How `.RNW` differs from `.MAP` (and how they fit together)

This is the single most important idea, so it gets its own short table:

| | **`.RNW` (this document)** | **`.MAP` (the other document)** |
|---|---|---|
| Job | **Routing** — work out the sequence of roads to drive | **Drawing** — paint the map on the screen |
| Shape of the data | A **graph**: intersections (dots) + roads (lines) that explicitly connect to each other | **Independent pictures**: roads/water/POIs drawn as lines and shapes, not wired together |
| Cut up by | **Clusters** — chunks of connected road network | **Tiles** — a regular square grid at several zoom levels |
| Region names | Country/group codes: `POL`, `DEU`, `FRM`, `CHS`, `EEU`, … | Atlas-sheet codes: `N6E2`, `N6E1`, … |

Both describe **the same physical roads** — just in two different shapes for two different jobs. The
car computes a route on the `.RNW` graph, and then *draws* that route as a line on top of the
`.MAP` picture. (That is also why a tool can match an RNW road to a MAP road purely by comparing
their shapes: they are the same street, described twice.)

A small naming note so you are not confused: the `.MAP` "region" (`N6E2`) and the `.RNW` region
folder (`POL`) do **not** use the same names. `.MAP` divides the world into a fixed grid of atlas
sheets; `.RNW` groups the data by country or region (`POL` = Poland, `DEU` = Germany, `FRM` =
France, `CHS` = Austria/Czechia/Slovakia, `EEU` = eastern Europe, and so on). The two partitions
overlap in the real world but are organised differently. Also, all the `.RNW` data for this car sits
under a folder named after its **profile** (here, `CCP`) — think of the profile as "which edition /
configuration of the map set this car has."

---

## 3. How the car actually uses these files (the journey)

Imagine the driver enters a destination and the car must find a route. Here is the sequence, which is
the best way to understand *why* each file part exists:

1. **Where am I, and where am I going?** The car knows its own longitude/latitude from GPS, and it has
   the destination's position too.
2. **Which region folder?** It works out which road-network regions the route passes through — e.g.
   `POL`.
3. **Open the table of contents.** It opens `NAV_ROOT.DAT` for that region.
4. **Which clusters cover my corridor?** The root index is a lookup: *"the roads around this place
   are in file `NAV20200.DAT`, starting at byte 0x4000, 1,328,044 bytes long."* For a route it may
   need several clusters (one for each stretch of the journey).
5. **Load each cluster.** It opens the named `NAVnnnnn.DAT`, jumps to the given byte, reads that many
   bytes, and parses them into intersections + roads, turning the compact offsets back into real
   positions.
6. **Stitch the pieces together.** Where two clusters meet, they share the same border intersections
   (the same junction appears in both). The car joins the clusters at those shared dots so the graph
   is continuous across cluster boundaries.
7. **Find the route.** Now it has one big connected web of roads covering the journey. It searches
   that web for the best path from start to finish — this is just "find the shortest chain of roads."
8. **Draw it.** The chosen sequence of roads is drawn as a line on top of the `.MAP` picture, and
   turn-by-turn directions are read off the same graph.

Every structure in these files exists to make some step of that journey fast. Keep that journey in
mind and the "why" of each field becomes obvious.

---

## 4. The building blocks, one by one

### 4.1 Coordinates and "PAU"

A place on Earth is given by **longitude** (east–west) and **latitude** (north–south), measured in
degrees. But the files do not store degrees as decimal numbers like `52.23`. Instead they store a
large whole number called **PAU** ("Private Angular Unit").

Why? Whole numbers are easier and cheaper to store and compare than decimals, and they avoid rounding
errors. The conversion is fixed:

```
degrees = PAU × 180 / 2^31        (equivalently: PAU = degrees × 2^31 / 180)
```

So a longitude of 19° becomes the whole number `19 × 2^31 / 180`. You never need to do this by hand —
it is just how the numbers are scaled. Think of PAU as "degrees, but expressed in tiny whole-number
ticks instead of decimals." (This is exactly the same scheme used by `.MAP`.)

### 4.2 Storing a point as an offset, not a full coordinate

Even PAU numbers are big. To save space, points *inside a cluster* are not stored as full
coordinates at all. They are stored as a small **offset (delta)** from the **cluster's reference
point** — one fixed anchor position that every cluster carries in its header.

Analogy: instead of writing "this junction is at 52°13'N, 21°01'E", the file writes "this junction
is a little north and a little east of *the reference corner of this chunk*." The offset is a small
number (often just two bytes), because everything in one cluster lies close to that cluster's
reference point.

To turn an offset back into a real position, you do:

```
real_position = cluster_reference + (offset × 2^shift)
```

The `× 2^shift` part **scales** the small offset up to the right size. `shift` is a number stored in
the cluster header — a coarser cluster uses a smaller `shift`, a denser one a bigger one, so the same
tiny integers can represent positions more or less precisely. This "scale factor" is what lets tiny
numbers encode precise locations. (In `.MAP` the anchor was the *tile centre*; here it is the
*cluster reference point* — same idea, different anchor.)

Depending on a flag in the cluster header, these offsets are stored as either **16-bit** or **24-bit**
whole numbers. 16-bit is smaller and is used where the cluster is sparse; 24-bit is more precise and
is used where it is dense.

### 4.3 Clusters (the spatial unit)

A **cluster** is a self-contained chunk of the road network: all the intersections and roads for one
patch of the area, packed together, with its own reference point and scale. It is the unit that
`NAV_ROOT.DAT` points at ("the roads around here are *this* cluster").

Why clusters? Because they are the smallest piece you can jump straight to by a single byte offset,
and because grouping *connected* roads together means a route that stays inside one cluster never has
to touch any other. A `NAVnnnnn.DAT` file simply holds several clusters laid end to end; the root
index records each cluster's starting byte and length so any one of them can be loaded on its own.

A useful mental model: if `.MAP` tiles are *squares* cut from a regular grid, then RNW clusters are
*patches* cut so that each one holds a coherent piece of the road web. The exact patching is decided
by the map-build tools; what matters to the reader is only "this byte range = one cluster."

### 4.4 The cluster's own table of contents (the descriptor list)

Right after a cluster's header there is a tiny **directory** that says which sections this cluster
actually contains, and where each one starts. Not every cluster has everything — a small rural
cluster might have roads and intersections but no names, while a big city cluster has all of it.

The directory works like a checklist. There are a fixed set of possible sections (in a fixed order):

| # | Section | What it holds |
|---|---------|---------------|
| 0 | **Annotations** | The cluster's own extra attributes / notes |
| 2 | **Neighbour list A** | Pointers to *adjacent* clusters (see 4.11) |
| 3 | **Neighbour list B** | A second set of adjacent-cluster pointers |
| 4 | **Intersections** | The cluster's nodes (see 4.5) |
| 5 | **Roads** | The cluster's road segments (see 4.6) |
| 8 | **Positions** | Where each intersection actually is (see 4.7) |

(Numbers 1, 6, 7, 9, 10 are reserved slots that may carry a pointer but whose contents the reader
ignores — they exist for other features and can safely be empty.)

For each section that *is* present, the directory stores just two small numbers: **where it starts**
(an offset from the cluster beginning) and **how many items it has**. That is all — a compact
"section 4 starts here and has 512 items." The reader walks this checklist in order, jumping to each
listed section. (The fixed ordering matters: if a section is missing from the checklist, every later
one shifts up, so the order can never be scrambled.)

### 4.5 Intersections — the nodes

An **intersection** (called a *node*, or technically a "zerocell") is a dot in the graph: a place
where roads meet or turn. A crossroads, a T-junction, a roundabout entry, even just a bend where one
road segment ends and the next begins — each is a node.

Nodes are the **vertices** of the routing graph. On their own a node has no position and no shape —
it is really just an *identifier* ("junction #37 in this cluster"). Its actual location lives in the
**position list** (4.7), and its connections to roads live on the road side (4.6). Think of a node as
a labelled peg; the map of where the pegs are and which sticks join them is built from the other
lists.

### 4.6 Roads — the edges

A **road** (called a *segment*, or technically an "onecell") is a line in the graph: a stretch of
pavement that runs **from one node to another**. It is the **edge** of the routing graph.

Each road stores:

- **Its two ends.** Which node it starts at (the **from**-node) and which node it finishes at (the
  **to**-node). This is what makes the network a *directed* web — you know, for each road, which end
  is "in" and which is "out," which matters for one-way streets.
- **Optional bends in between.** If the road is not straight, it can carry a few extra points (its
  *shape*) so the line follows the real curve of the street rather than cutting across blocks. Many
  short roads have no extra points — just the two end nodes.
- **Its attributes** (4.10): what kind of road it is, whether it is a link/ramp, and its name(s).

So a complete road is: *start-node → [optional bend points] → end-node*, plus what kind of road it is
and its name (4.10). That single line is one edge the routing engine can travel.

### 4.7 The position list — where the nodes really are

Remember that a node (4.5) is only an identifier. The cluster keeps a separate **position list** —
one coordinate per node, in the same order as the nodes themselves. Position #0 belongs to node #0,
position #1 to node #1, and so on. (The two lists always have exactly the same length.)

Each position is stored compactly as an offset from the cluster reference point, scaled by `shift`
(4.2). When the reader pairs node #37 with position #37 and applies the scale, it knows exactly where
that junction sits on Earth. This separation — "here are the nodes" in one list, "here is where each
one is" in another — keeps both lists as small as possible.

### 4.8 How a road knows which way is "from" and which is "to"

A road needs to name its two end-nodes, but the file stores each end very compactly: a small number
(the node's index within the cluster) with a single flag bit attached. That one bit is the whole
direction rule:

- **bit clear** → this number is the road's **from**-node (its start),
- **bit set**   → this number is the road's **to**-node (its end).

Both ends are stored this way, so a road carries two such little numbers and the reader can tell
which is which from the flag. The node numbers are counted starting at 1 (not 0), and they always
refer to nodes *inside the same cluster* — there is no global "junction number" across the whole map.
(This last point matters: it is why clusters stitch together by *matching positions*, not by shared
IDs — see 4.11.)

### 4.9 Names and attributes (annotations)

A road is more than a line — it has a name, a type, and other facts. These are stored as
**annotations**, a compact packed list where each item is `{ size (2 bytes), type (2 bytes), payload }`.
The `type` says what the annotation is (a name reference, a road number, …) and the payload holds its
value.

**Names and other text** are stored separately in **text records**, so that the same string used by
many roads is stored only once (this "interning" is a big space-saver, since street names repeat). A
name can be held in **several languages at once** — for example the local spelling plus a
transliteration ("ULICA KOBIERZYŃSKA" and "ULICA KOBIERZYNSKA"). An annotation that references a name
stores just an index into the text section, not the whole string again.

### 4.10 What kind of road is it? (class → highway type)

Each road carries a small **class** code (a couple of bits for "road class" and "network class," plus
a "road type"). The navigation software turns that combination into a **display class** — an ordered
rank where smaller means more important. That rank is what ultimately decides how the road is drawn
and how it is described, and it lines up with the familiar road categories:

| Rank | Meaning | Everyday example |
|------|---------|------------------|
| (lowest) | Motorway / expressway | An interchange ("węzeł") |
| … | Trunk, then primary, secondary, tertiary | Main highways down to town links |
| (highest) | Residential / unclassified | Ordinary side streets |

So the raw class bits in the file are what let the car know "this is a motorway" versus "this is a
quiet street," and they also drive the routing priorities (the engine weighs roads by this rank).

### 4.11 Neighbouring clusters, and stitching across boundaries

A single cluster only covers one patch of the area. To route *across* patches, clusters need to know
about their neighbours. That is what the **neighbour lists** (sections 2 and 3 in 4.4) are for: each
entry describes an adjacent cluster — which file it is in, where it starts, how big it is, and its own
reference point — so the car can load it when a route runs into the boundary.

Here is the subtle but important part. Because node numbers are only meaningful *within* one cluster
(4.8), two clusters that share a border **do not** share node IDs. The shared junction appears in
*both* clusters as distinct nodes, and the runtime relates them **logically — never by position**:

- An **explicit link** (the main path): a crossing road segment carries an *Overlaps* list naming the
  exact neighbour segment in the adjacent cluster (`RNW_format.md` §3b).
- A **border-marker test** (fallback, only if no link resolves): the runtime checks whether the
  onecell's end node is a "border/crossing" zerocell — rim / cpx-crossing flag or a `0x31` annotation
  (`RNW_format.md` §3c/§5). This too is non-positional.

The runtime does **not** match the two copies by coordinate. (The two stored copies do differ by a few
PAU, ~0.06–0.08 m, but that is not how the app identifies them.) `rnw2osm_rs` must emit one shared OSM
`<node>` for roads to connect, so it performs that unification itself in the same order — links → marker
→ proximity — where the **proximity** step (`-s`, default 1.0 m) is an OSM-side necessity with no
counterpart in the runtime. `--no-snap` drops it (marker + links only); `-s 0` drops both (links only).

A corollary worth knowing: **clusters overlap by design.** (a) A road that crosses a boundary is stored
in *both* neighbouring clusters, so their road/node footprints bleed into each other near the edge.
(b) Some clusters are **major-road overlays** — they contain only high-priority roads
(motorway / trunk / secondary, no residential or unclassified) and therefore span many ordinary tiles,
appearing as one large cluster that overlaps a lot of neighbours. Seeing overlapping outlines is normal,
not an error; the overlap is the stitching mechanism plus these overlay layers.

### 4.12 Two copies of a road: primary and secondary (a level-of-detail pair)

Some roads are stored **twice** — as two copies sitting on top of each other, one more detailed than the
other. The map tags them **primary** and **secondary** (a *level-of-detail* pair). The **primary** is the
full, precise shape — usually the copy that carries the road's name — and it is what the car actually draws.
The **secondary** is a coarser companion copy of the same stretch of road; it often has no name of its own.

You can tell a secondary apart because a flag in the road's header (`bIsSecundary`, header bit 15) marks it
as such. The two copies are not in different places: they cover the *same* stretch of road, so on a map they
overlap exactly. That is why, if you decode everything raw, a busy interchange can look "doubled" — each
motorway segment appearing once as its primary and again as a coincident secondary copy. The Balice I
interchange is a clean example: the shaped, named primary in one cluster, a 2-point unnamed secondary in its
neighbour, with an identical bounding box.

The car never draws the secondary by itself. Before rendering it resolves every road to its **primary** — if
the copy in hand is a secondary, it follows that copy's overlap links (4.11) to the matching primary and uses
that instead (`RNW_format.md` §6). From the driver's point of view there is only ever one road; the pair is an
internal detail of how the map is stored.

The practical consequence for a converter: emit the **primaries** and you reproduce exactly the map the car
shows — each road once, at full detail. The secondaries can be set aside (or emitted on their own, to inspect
the raw duplicates), but they add nothing the primaries do not already cover. No cluster is secondary-only, so
dropping them never blanks an area.

### 4.13 Two tiers of clusters: coarse and fine (a second, independent detail axis)

Primary/secondary (4.12) is one way the map holds two levels of detail — *two copies of the same road*. There
is a **second, different** axis: the clusters themselves come in **two interleaved tiers**.

- The **coarse tier** — larger patches, the main roads. Its clusters have a non-zero value in the header's
  `flags` word (the `u16@2` field: `0x0001`, `0x0008`, and a few other combinations).
- The **fine tier** — smaller patches laid over the same ground, carrying the **dense residential grid**
  (the little streets you only see when zoomed in). Its clusters have `flags = 0x0000`.

The two tiers sit on top of each other geographically. A coarse road "refines" into the fine tier through its
*down-cells* — but that refinement is only **one step deep** (a coarse onecell points at fine onecells; the
fine onecells have no down-cells of their own). So the map has exactly two tiers, not a deep pyramid.

Why this matters: `flags = 0x0000` is a *valid* cluster, not a marker for "no cluster here." A reader that
skips any header whose `flags` word is zero silently throws away the entire fine tier — which in a city is
roughly **half the roads**, and specifically the residential streets. (Symptom: motorways and main roads are
all present, but the small streets of a housing estate vanish.) The real test for "is this a cluster?" is the
`listFlags` descriptor pattern plus plausible outline fields — not the `flags` word. The converter exposes
this as `--level 0` (coarse tier only) / `--level 1` (coarse + fine, the default).

### 4.14 Putting the layers together: two independent "detail" axes (and which option you want)

The two mechanisms above are easy to mix up — both are "a level of detail" — but they act on *different
things*. Keep them separate and the whole layered structure becomes simple:

| | **Tier** — coarse / fine (§4.13) | **Twin** — primary / secondary (§4.12) |
|---|---|---|
| What it is | two interleaved sets of *clusters* covering the same ground at different density | two *copies of one single road*, one detailed, one simplified |
| How to see it in the data | cluster header `flags` word: fine tier = `0x0000`, coarse ≠ 0 | onecell header bit 15 (`bIsSecundary`) |
| Same roads or different? | mostly **different** — the fine tier *adds* the residential grid; a minority of main roads appear in both | **exactly the same** stretch, drawn twice |
| Converter switch | `--level 0` / `--level 1` | default = primary only; `--secondary` = the twin layer |

A picture. Ask two independent questions:

1. **How finely should the road web be drawn over this territory?** Coarse strokes (main roads) or the fine
   strokes too (the little streets)? → that is the *tier*, chosen with `--level`.
2. **For each stroke, keep the crisp original or its rough backup copy?** → that is the *twin*, chosen with
   `--secondary`.

Because the questions are independent, the options combine freely.

> Don't confuse either of these with the `.MAP` file's own zoom levels (**L0–L3**, see
> `01_MAP_overview.md`): that is a *third*, separate mechanism belonging to the pre-built tile store. The RNW
> `--level` in this document is only about the coarse/fine **cluster** tier.

**Which combination to use**

| goal | command |
|---|---|
| a complete, detailed street map — the normal case | *(default)* = `--level 1`, primary layer |
| a lightweight overview: main roads only, smaller and faster | `--level 0` |
| inspect the simplified twin copies in isolation (diagnostics, not a usable map) | `--secondary` |
| the twin copies of both tiers (pure experimentation) | `--level 1 --secondary` |

For everyday use just take the default. `--secondary` is a magnifying glass for poking at the storage, not a
map you would navigate.

**Why `--level 1` does not double-draw the roads.** The two tiers are ~90% complementary — the fine tier
mostly *adds* streets rather than redrawing the coarse ones — but a minority of main roads exist in both
(measured: in one housing-estate box, 2167 fine roads were added to 1706 coarse ones, and only ~9% of the
coarse roads had a fine counterpart nearby). To keep the output clean, when you select `--level 1` the
converter drops a coarse road **only** if it is fully refined into fine sub-segments that are present in the
run — decided by the data's own down-cell links (§4.13), not by a geometry guess. That means it never removes
a road that has no fine counterpart and never loses a name; the summary line reports how many were dropped as
`refined_dropped=N` (e.g. 1010 for the Kraków box).

---

## 5. Walking through a cluster (top to bottom)

Now that the concepts are clear, here is what you actually find when you open one cluster inside a
`NAVnnnnn.DAT`, in order:

1. **The header** — a short fixed block. The important parts are:
   - A **flags** field (one bit of which chooses 16-bit vs 24-bit coordinate offsets).
   - The cluster's **reference point** (its anchor longitude/latitude, in PAU) and its **scale
     (`shift`)** — together these let you decode every relative coordinate in the cluster (4.2).
   - The **outline**: a small ring of points describing roughly where this cluster sits on the map
     (a boundary polygon). It is not needed to route, only as a geographic "fence" for the patch.
   - The **descriptor list** (4.4): the checklist saying which sections are present and where each
     starts.

2. **The sections themselves**, at the offsets the descriptor list named — typically:
   - **Annotations** (the cluster's own notes).
   - **Neighbour lists** (pointers to adjacent clusters, 4.11).
   - **Intersections** — the nodes, one short record each (4.5).
   - **Roads** — the segments, each with its from/to nodes, optional shape, and attributes (4.6, 4.8).
   - **Positions** — one coordinate per node, index-aligned with the intersections (4.7).

3. **The text** — the shared strings (names in their various languages) that annotations point into
   (4.9).

That is a complete cluster: read it and you have a self-contained patch of the road web — every
junction, every road, which roads join which junctions, where everything is, and what each road is
called. Load enough of them and stitch them at shared positions, and you have the whole network the
route can travel over.

---

## 6. Tying it together: computing one route, end to end

Let's follow a single journey from "driver enters destination" to "a line on the screen," using every
concept above:

1. The driver sets a destination. The car knows its own GPS position and the destination's position.
2. It decides the route lies in region **`POL`** and opens **`NAV_ROOT.DAT`**.
3. Using the root index (a lookup of "place → cluster"), it finds the clusters covering the corridor:
   say *"cluster in `NAV20200.DAT`, byte 0x4000, 1,328,044 bytes"* and a couple more further along.
4. It loads each cluster: reads the header (getting the **reference point** and **shift**), walks the
   **descriptor list**, and pulls out the **intersections**, the **roads**, and the **positions**.
5. It pairs each node with its position (same index) and applies `reference + offset × 2^shift` to get
   real coordinates — now every junction has a place on Earth, and every road knows its two end-junctions.
6. Where two loaded clusters meet, it **stitches** them by matching the shared border junctions at
   their identical positions (4.11), so the graph is continuous.
7. It searches the combined graph for the best path from start to finish, weighing roads by their
   class rank (4.10). The result is an ordered list of roads: *junction A → B → C → … → destination*.
8. It reads each chosen road's name from the **text section** for turn-by-turn instructions ("turn onto
   ULICA SOBIESKIEGO"), and hands the resulting line to the drawing side, which paints it over the
   `.MAP` picture.

Every file structure you read about exists to make one of those steps fast and compact.

---

## 7. A surprise: the car also draws roads straight from the road network

You might expect the car to draw everything on screen from the `.MAP` picture files, and only use
the `.RNW` for working out routes. Mostly true — but there is a twist, and it is worth understanding
because it explains why both file types are needed at once.

The drawing side does not read map data from just one place. When it asks for "the pieces I need to
draw this bit of the screen," each piece carries a little **type tag**, and the tag decides *where*
that piece is fetched from:

- Most background pieces (water, areas, points-of-interest) come straight from the pre-built **`.MAP`**
  files on the card.
- But some **road** pieces are instead pulled **live from the `.RNW` road network** and turned into
  drawing pieces on the spot, in memory.

So there is a small **converter running inside the car** that takes a chunk of the road network
(intersections + roads) and rewrites it into the "drawing piece" form that the screen side expects —
including translating each road's class into how it should be drawn (a thick motorway line versus a
thin side-street line).

Why bother, when the roads are already in the `.MAP` files? Two reasons:

1. **The drawn roads then match the routed roads exactly.** The route is computed on the `.RNW`
   network; by drawing those same roads (converted from the very same data) there is no chance of the
   line on screen drifting away from the path the car actually follows.
2. **It saves space** — the road layer does not have to be stored a second time in the picture files.

This internal converter is part of what the engineers call the **"FastMap"** drawing path.

One important thing this *does not* mean: the `.RNW` file does **not** contain everything needed to
build the whole `.MAP`. The converter only ever makes *road* pieces — it has no water, lakes, areas,
or points-of-interest to work from (the road network simply does not store those). So the full
picture (roads *and* rivers *and* cities) still comes from the `.MAP` files; the converter just
supplies one of the layers, live, from the road network.

---

## 8. What about `AEX`?

The `AEX/AEXnnnnn.DAT` files sit alongside the data files and use the **same numbers** (an `AEX` file
with the same `nnnnn` as a `NAV` file holds extra information for that same cluster). They are loaded
**only when** the car's configuration switches them on; otherwise they are ignored.

Conceptually they hold **extra annotation / attribute data beyond the core road network** — the kind
of "reader's notes" that some configurations want and others do not. They are *not* needed for the
basic job of routing over the road web: a car can load and route on the `NAV` files alone. If you are
generating your own data (see `writer_guide.md`), you can safely omit the `AEX` files entirely and
leave the switch off.

### If you peek inside an `AEX` file

Even though you do not need them, here is what one actually contains (it has been worked out from the
bytes). Think of it as a small self-contained booklet:

1. **A cover page** (16 numbers): the file's own number, how big the whole file is, and where the
   "table of contents" and the first real entry begin.
2. **A few notes in plain text**: a copyright line ("(c) 2006 Blaupunkt GmbH, Hildesheim"), the date
   and time it was built, and a tag naming the tool that made it — `TPNAV_ANNEXPORT` (in other words,
   "an export of map annotations"). This confirms what the file is for: **annotation data**, produced
   by the map supplier, not road geometry.
3. **A table of contents**: a short list where each line says "entry number *n* starts here and is this
   many bytes long." The lines line up back-to-back with no gaps, so together they exactly fill the
   rest of the file.
4. **The entries themselves**: a variable-length collection of annotation records (from a few dozen
   bytes up to a few thousand). Each bundles together a set of small labelled facts — the same
   "label + value" style used for names and attributes elsewhere in these files. In practice (POL)
    each record is a list of **speed-limit zones**: `{direction, limit in km/h, start along the road
    in metres, length in metres}` — e.g. *"this stretch runs 90 km/h from 0 to 1202 m, then 50 km/h
    for the next 909 m."* The `direction` field is usually a single value meaning "both directions";
    on a few roads it splits the limit per travel direction (one value each way).

So an `AEX` file is, in plain terms: *"here are some extra notes about the places and features in this
patch of map"* (for POL, speed limits per road stretch) — optional garnish on top of the core road
network, which is why it can be switched off without breaking navigation.

---

## 9. Quick glossary

- **PAU** — "Private Angular Unit": a whole-number scaling of degrees (`deg = PAU × 180 / 2^31`).
- **Region (RNW)** — a country/group folder holding one area's road network: `POL`, `DEU`, `FRM`,
  `CHS`, `EEU`, … (different naming from the `.MAP` atlas-sheet regions).
- **Profile** — the edition/configuration of the map set; all RNW data for this car sits under a folder
  named after it (here, `CCP`).
- **Cluster** — the smallest independently-loadable chunk of road network: a patch of intersections +
  roads with its own reference point and scale. Several clusters live in each `NAVnnnnn.DAT`.
- **Reference point + shift** — how a position is stored compactly:
  `real = cluster_reference + offset × 2^shift` (offsets are 16-bit or 24-bit whole numbers).
- **Node / intersection (zerocell)** — a dot in the graph: a junction. Just an identifier inside its
  cluster; its position comes from the position list.
- **Road / segment (onecell)** — a line in the graph: a stretch of road running from one node to
  another, with optional bend points and attributes.
- **Primary / secondary** — the *twin* axis: the two level-of-detail copies some roads are stored as: a
  full, named primary (what the car draws) and a coarser companion secondary covering the same stretch. A
  header flag (`bIsSecundary`, bit 15) marks the secondary; the runtime resolves every road to its primary
  before rendering (4.12). Converter: default = primary only, `--secondary` = the twin layer.
- **Coarse / fine tier** — the *tier* axis: two interleaved sets of clusters covering the same ground at
  different density (4.13). A coarse tier (main roads; cluster header `flags` word ≠ 0) and a fine tier
  (the dense residential grid; `flags = 0x0000`). A coarse road refines into the fine tier via down-cells,
  one step deep. Converter: `--level 0` = coarse only, `--level 1` = both (default). The two axes are
  independent — see the combined picture and option table in 4.14.
- **From / to node** — a road's two ends; each is stored as a node index plus one flag bit
  (clear = from, set = to), counted from 1, always local to the cluster.
- **Position list** — one coordinate per node, in the same order as the nodes; pairing them gives each
  junction its real location.
- **Descriptor list** — the cluster's internal checklist: which sections are present and where each
  starts (a fixed-order table of `{offset, count}` pairs).
- **Neighbour list** — pointers to adjacent clusters (file, offset, length, reference point), used to
  stitch the network across cluster boundaries.
- **Stitching by position** — how two clusters join at a shared border: the same junction appears in
  both at the same coordinates, so they are matched by position, not by ID.
- **Annotation** — a small `{size, type, payload}` attribute attached to a road or cluster (name ref,
  road number, …).
- **Text record** — a stored string (possibly multi-language); shared strings are stored once and
  referenced by index.
- **Class → display class → highway type** — a road's class bits are ranked into a "display class"
  that maps to familiar categories (motorway, trunk, primary, …, residential).
- **`NAV_ROOT.DAT` / TCI** — the region's table of contents: an index turning "this place" into
  "file X, byte Y, Z bytes long." (TCI = "Tile Cluster Index.")
- **`AEX`** — optional per-cluster extra annotation data, loaded only when configured; not required for
  routing.
- **Block type tag** — a small number on each "draw this piece" request that selects its source:
  background layers from `.MAP`, some road layers converted live from `.RNW`.
- **FastMap** — the run-time drawing path; includes the internal converter that turns RNW road
  network chunks into drawing pieces (roads only).
