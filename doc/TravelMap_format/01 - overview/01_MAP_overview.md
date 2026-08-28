# TravelMap `.IDX` / `.MAP` — a plain-language, detailed overview

This document explains the Bosch "TravelMap" navigation file format (as used in the Nissan
LCN2KAI) in words a non-programmer can follow. It is intentionally verbose: every technical term is
defined the first time it appears, and each part of the file is explained not just for *what* it is
but for *why* it exists and *what job* it does.

For the precise byte-level layout (offsets, sizes, bit fields) see
[`MAP_format.md`](../02%20-%20details/MAP_format.md). For how to build your own files, see
[`writer_guide.md`](../03%20-%20writer%20guide/writer_guide.md). This document is the "understanding" layer underneath both.

---

## 1. What are these files, and why do they exist?

A car's navigation system needs a map of the world — roads, rivers, lakes, cities, street names —
stored on its internal memory so it can work without an internet connection. The `.IDX` and `.MAP`
files **are** that stored map, in a compact form designed for one specific job: let the car quickly
pull up and draw only the small piece of the world right around where the car currently is.

Two design goals shaped everything about the format:

1. **Fast random access.** The car cannot load "the whole map of Europe" into memory. It must be
   able to say *"give me just this one square-kilometre-ish patch at this zoom level"* and read only
   a few kilobytes. So the data is chopped into many small, independently-locatable pieces.
2. **Small size.** Car storage and load times are limited, so coordinates and attributes are stored
   in the most compact form possible (short integers, offsets relative to a nearby reference point,
   shared/reused text, etc.).

The format achieves both by being **hierarchical**: the world is split into big regions, each region
into a grid of tiles, each tile's data packed into small "blocks". A table-of-contents file tells the
car exactly where every piece lives.

---

## 2. The two (plus two) files and how they relate

| File | Plain-language role | Analogy |
|------|---------------------|---------|
| `<REGION>AA.IDX` | **Table of contents.** For each region, a list saying "tile #123 at zoom level 2 is in file X, starting here, this many bytes long." | The index at the back of a book: "Chapter 5 — page 87". |
| `<REGION>1XX.MAP` | **The content.** The actual map data (roads, water, POIs) for one *profile* of one region. A region has several of these (one per profile). | The actual pages of the book. |
| `MAPWORLD.MAP` | **World layout.** A tiny file describing how the whole world is divided into regions and how the tile grids are structured. | The "how this atlas is organised" page. |
| `<REGION>1XX.TCI` | Per-MAP-file sub-index. Not needed to decode the geometry itself. | A fine-grained bookmark list (optional). |

The relationship is one-directional: **the `.IDX` points at the `.MAP` files.** The car reads the
`.IDX` first to learn *where* things are, then jumps into the right `.MAP` file to read *what* is
there. You never need a `.MAP` without its `.IDX`, and the `.IDX` alone contains no drawing data —
just pointers.

A "region" is named like `N6E2`. Think of the world as a set of large rectangular atlas sheets;
`N6E2` is one such sheet (roughly a chunk of central Europe). Each sheet has its own `.IDX` and its
own set of `.MAP` files.

---

## 3. How the car actually uses these files (the journey)

Imagine the car is driving and needs to redraw the map on the screen. Here is the sequence, which is
the best way to understand *why* each file part exists:

1. **Where am I?** The car knows its own longitude/latitude from GPS.
2. **Which sheet?** It works out which region (atlas sheet) contains that position — e.g. `N6E2`.
3. **Open the table of contents.** It opens `N6E2AA.IDX`.
4. **Which square, at which zoom?** The region is a grid of tiles at several zoom levels. The car
   picks the tile(s) covering its position at the current zoom (e.g. "level 2, tile #417").
5. **Where is that data?** It looks up level-2 / tile-#417 in the `.IDX`. The entry says: *"profile
   '0A', in file `N6E210A.MAP`, starting at byte 0x3d374, 8085 words long."*
6. **Read and draw.** It opens `N6E210A.MAP`, jumps to that byte, reads the block (which contains
   the roads/water/POIs for that tile), converts the compact coordinates back into real positions,
   and draws them.

Every structure in these files exists to make some step of that journey fast. Keep that journey in
mind and the "why" of each field becomes obvious.

---

## 4. The building blocks, one by one

### 4.1 Coordinates and "PAU"

A place on Earth is given by **longitude** (east–west) and **latitude** (north–south), measured in
degrees. But the files do not store degrees as decimal numbers like `52.23`. Instead they store a
large whole number called **PAU** ("Private Angular Unit").

Why? Whole numbers are easier and cheaper to store and compare than decimals, and they avoid
rounding errors. The conversion is fixed:

```
degrees = PAU × 180 / 2^31        (equivalently: PAU = degrees × 2^31 / 180)
```

So a longitude of 18° becomes the whole number `18 × 2^31 / 180`. You never need to do this by hand —
it is just how the numbers are scaled. Think of PAU as "degrees, but expressed in tiny whole-number
ticks instead of decimals."

### 4.2 Storing a point as an offset, not a full coordinate

Even PAU numbers are big. To save space, points *inside a tile* are not stored as full coordinates at
all. They are stored as a small **offset (delta)** from the **centre of that tile**.

Analogy: instead of writing "the shop is at 52°13'N, 21°01'E", you write "the shop is 40 metres
north and 10 metres east of the middle of this block." The offset is a small number (often just a
couple of bytes), because everything in one tile is close to that tile's centre.

To turn an offset back into a real position, you do:

```
real_position = tile_centre + (offset × 2^shift)
```

The `× 2^shift` part **scales** the small offset up to the right size. `shift` is a number that
depends on the zoom level (see tiles below) — finer zoom levels use a bigger `shift`, so the same
small integers can represent positions more precisely. This "scale factor" is what lets tiny numbers
encode precise locations.

### 4.3 Regions and the world grid

The world is divided into **regions** named `N<row>E<column>`:

- The **row** (`N1`…`N8`) tells you how far north — rows are horizontal latitude bands, each about
  9.45° tall, stacked from the equator toward the poles.
- The **column** (`E1`…) tells you how far east — columns are longitude bands, side by side.

So `N6E2` means "row 6 (a certain latitude band), column 2 (a certain longitude band)." Each region
is a fixed rectangle, and its exact four corner coordinates are stored right in its `.IDX` header —
so you never have to recompute the grid, just read the corners.

### 4.4 Tiles and zoom levels

Inside each region there is a **grid of tiles** at **four zoom levels**:

| Level | Grid size | What it looks like | `shift` (scale) |
|-------|-----------|--------------------|-----------------|
| L0 | 1 × 1 | The whole region as one tile (very coarse overview) | 13 |
| L1 | 5 × 5 = 25 tiles | The region split into a small grid | 10 |
| L2 | 50 × 50 = 2,500 tiles | A much finer grid (town-level detail) | 7 |
| L3 | 500 × 500 = 250,000 tiles | The finest grid (street-level detail) | 4 |

Think of it like zooming into a map app: L0 is "see the whole country", L3 is "see individual
streets." The same real-world area appears at every level, just with more or less detail and in
bigger or smaller tiles.

Each tile has a **number** (called `K`). That number encodes the tile's row and column in the grid
using a fixed arithmetic formula (higher levels use a nested base-100 / base-5 scheme — see the
format reference §6 for the exact math). The important idea: *the number tells you exactly where in
the grid the tile sits, and therefore its geographic rectangle.* From that rectangle you get the tile
centre, which is the reference point for all the offsets in 4.2.

### 4.5 Profiles (why one area has several `.MAP` files)

Here is a subtle but important idea. The same geographic area is stored **multiple times**, once per
**profile**. A profile is a *category of content*. Roughly: one profile file holds roads, another
holds water features, another holds areas/polygons, another holds points-of-interest, and so on.

Analogy: imagine one transparent sheet of paper can only hold one "layer" — say, just the roads. To
show roads *and* rivers *and* buildings you need several transparent sheets stacked in the same
place. Each sheet is a profile; stacking them all gives the complete picture.

That is why a region has **several** `.MAP` files (typically around 8–9), named `<REGION>1<XX>.MAP`
where `<XX>` identifies the profile. When the car wants to draw a tile, it may need to read from
*several* of these profile files at once and composite them — which is exactly what the `multi` slot
in the `.IDX` (see §5.4) is for.

### 4.6 Blocks (the smallest independently-locatable chunk)

A **block** is a self-contained chunk of map data: all the features for one tile in one profile,
packed together. It is the unit that the `.IDX` points at ("this tile's data is this block").

Why blocks? Because they are the smallest piece you can jump straight to by a single byte offset. The
car reads one block = gets everything it needs to draw one tile/layer. No block depends on reading any
other block first.

Every block begins with a **marker** — a 4-byte number whose top half is always `0xFFFF` and whose
bottom half is the block's length (in 4-byte words). The marker serves two purposes: it acts as a
"this is the start of a block" signature, and it tells the reader exactly where the block ends (so it
can find the next one). Think of it as both a nameplate and a "this section is N long" label.

### 4.7 Cells (a single feature)

Inside a block are **cells** — each cell describes exactly **one feature**: one road segment, one
river stretch, one lake, one city marker, etc. A cell is always 12 bytes. It has two parts:

- A small header saying *what kind* of feature it is (its "feature code" and a "display scale" — the
  zoom level at which it should appear).
- An 8-byte body that differs by feature type (see 4.8).

The block groups its cells into **three lists** by feature type:

| List | Feature type | What it holds | Analogy |
|------|--------------|---------------|---------|
| 0 | **Polygons** | Areas — lakes, forests, country borders, building footprints. A *closed loop* of points. | A drawn shape with an interior (a filled-in blob). |
| 1 | **Lines** | Roads, rivers, coastlines, railways. An *open chain* of points. | A stroke / a path you could trace with a pen. |
| 2 | **POI** (points of interest) | Cities, gas stations, restaurants, parking. A *single location* plus attributes. | A pin dropped on the map. |

### 4.8 The two ways a cell stores its shape

- **Polygons and lines (lists 0 & 1):** the cell does **not** store the coordinates directly. It
  stores a **pointer** (`pointIdx`) and a **count** into a shared **point pool** (see 4.9). So the
  cell says "my shape is points #500 through #507 in the pool." This avoids repeating coordinates and
  lets the pool be packed tightly.
- **POIs (list 2):** there is only one point, so the cell stores its offset (`dlon`, `dlat`)
  *directly* in those two bytes — no pointer needed for a single location.

Every cell also carries an **annotation descriptor** (a start + count) pointing at the cell's extra
attributes (names, road class, water type, …) — see 4.10.

### 4.9 The point pool

The **point pool** is a single tightly-packed list of coordinate offsets that all the polygons and
lines in the block share. If a road is points #500–#507 and a lake is points #600–#620, both just
reference ranges in this one pool rather than each carrying their own coordinates. It is like a
shared "coordinate scratchpad" that many features point into. Each entry is a small pair of offsets
(relative to the tile centre, scaled by `shift`, per 4.2).

### 4.10 Annotations and text (the attributes and names)

A feature is more than its shape — a road has a name and a surface type; a river has a class; a city
has a name, an importance level, etc. These extra facts are stored as **annotations**.

Annotations are a compact packed list. Each one is: `{ size (1 byte), type (1 byte), payload }`, where
`size` counts the header + payload. The `type` byte says what the annotation is (road number, water
class, city info, name reference, …) and the payload holds its value. A feature's cell points at its
slice of this list via the annotation descriptor.

**Names and other text** are stored separately in **text records**, so that the same string used by
many features is stored only once (this is called *interning* — a big space saver, since street names
repeat). A text record for a name can hold the name in **several languages** at once. Annotations that
reference a name store just an index into the text section, not the whole string again.

---

## 5. Walking through a `.IDX` file (top to bottom)

Now that the concepts are clear, here is what you actually find when you open a `.IDX`, in order:

### 5.1 The header (first 32 bytes)

The header is the file's "front cover" and tells the reader where everything else is:

- **Where the tile tables begin** (`binOff`) — the offset of the first (L0) table.
- A small fixed value (always `32`).
- **The region's four corner coordinates** (west, south, east, north) in PAU. This is the geographic
  rectangle this `.IDX` covers — read these and you know exactly what area of the world the file is
  about.
- **Where the partition table is** (`partOff`).

### 5.2 The info-string region (optional notes)

A short block of ordinary human-readable text: a copyright line, the build date/time, who created the
configuration, the product name ("TpMap2 (Map-Data) for TravelMap"), the project name, and a list of
"default files." This is pure **metadata** — it is there for humans and diagnostics, and the map
rendering does not use it. A generator can omit or shrink this freely.

### 5.3 The partition table (4 × 12 bytes)

One 12-byte entry per zoom level (L0…L3). Each says: "for level *i*, there are `tileCnt` tiles, the
scale is `shift[i]`, and that level's tile table starts at this offset." It is a small directory that
lets the reader jump directly to any level's full list of tiles. (Fixed for the EUR dataset: 1 / 25 /
2,500 / 250,000 tiles and shifts 13 / 10 / 7 / 4.)

### 5.4 The tile tables (the heart of the `.IDX`)

For each level there is a long list — **one 8-byte entry per tile**. Each entry answers: *"for this
tile, in which profile file is the data, how big is it, and where does it start?"* The 8 bytes are:
`{ profile, length-in-words, offset-in-that-MAP-file }`.

Two special cases:

- **Empty tile** — a flag says "no data here," so the car skips it (e.g. open sea at a detail level
  that has no features).
- **Multi-slot** — when one tile needs data from *several* profile files (common at low zoom levels),
  the entry is instead a pointer to a short list of several such 8-byte entries. This is how a single
  "draw this tile" request fans out to roads + water + areas + POIs across multiple `.MAP` files.

This is the direct answer to step 5 of the journey in §3: the tile table *is* the lookup that turns
"level 2, tile #417" into "file `N6E210A.MAP`, byte 0x3d374, 8085 words."

---

## 6. Walking through a `.MAP` file (top to bottom)

### 6.1 The header (first 32 bytes)

Like the `.IDX` header, it is a front cover:

- **Where the first data block begins** (`binOff`).
- Where the info-string notes are.
- **The total file size** in bytes.
- **The region's four corner coordinates** (identical to the `.IDX` — they must match).
- A few small bookkeeping numbers (a couple of which are constant, and one that encodes the profile).
  These are internal housekeeping; the geometry does not depend on them.

### 6.2 The info-string region (optional notes)

Same idea as in the `.IDX`: human-readable build/copyright/project metadata. Not used for rendering.

### 6.3 The data blocks (the actual map content)

From `binOff` to the end of the file is a **sequence of blocks**, placed back-to-back with no gaps,
each aligned to a 4-byte boundary. Recall from §4: a block = all the features for one tile in one
profile. Each block, in order, contains:

1. **The marker** (4 bytes): `0xFFFF` + the block's length in words — both a "start of block" tag and
   an "ends here" signal.
2. **A 3-list table** (3 × `{start, count}`): how many polygons, lines, and POIs this block has, and
   where each list begins. The lists are laid out one after another, so each start is derived from the
   previous (`start of list n+1 = start of list n + (count of list n × 3)`).
3. **The cells** themselves: 12 bytes each, in the three lists (§4.7–4.8). Each says what feature it
   is and where its shape/attributes are.
4. **The point pool**: all the shared coordinate offsets the polygons and lines point into (§4.9).
5. **The annotations and text**: the attributes and names, packed compactly, with shared strings
   stored once (§4.10).

That is a complete block: read it and you have everything needed to draw one tile/layer — shapes,
their precise positions (once you apply the tile-centre + offset×2^shift rule), and all their labels
and attributes.

---

## 7. Tying it together: drawing one road, end to end

Let's follow a single road from "the car is here" to "a line on the screen," using every concept above:

1. The car is at some GPS position → it determines it is in region **`N6E2`**.
2. It opens **`N6E2AA.IDX`**, reads the header, and learns the region's corner coordinates.
3. At the current zoom (say level 2), it computes which **tile** covers its position.
4. In the IDX **partition table** it finds where the level-2 **tile table** is; in that table it reads
   the entry for its tile. The entry is a **multi-slot**, pointing to several profile entries — one of
   which is the *roads* profile, "file `N6E210A.MAP`, block at byte 0x3d374, 8085 words."
5. It opens **`N6E210A.MAP`**, jumps to byte 0x3d374, reads the **marker** (confirming size), then the
   3-list table: "this block has 0 polygons, 235 lines, 22 POIs."
6. It walks the **lines list**. One **cell** is a road: feature code = "road," display scale says it
   should show at this zoom, and its body points to **points #500–#507 in the point pool** plus an
   annotation descriptor.
7. It reads those 8 offsets from the **point pool** and converts each with
   `tile_centre + offset × 2^shift` (shift = 7 for level 2) into real PAU coordinates, then to degrees.
   Now it has 8 real-world positions — the shape of the road.
8. It follows the **annotation descriptor** to find the road's attributes: a surface type, and a name
   reference pointing into the **text section**, which yields (say) "ULICA JANA III SOBIESKIEGO."
9. It draws the 8-point line on screen and labels it. Done.

Every file structure you read about exists to make one of those nine steps fast and compact.

---

## 8. Quick glossary

- **PAU** — "Private Angular Unit": a whole-number scaling of degrees (`deg = PAU × 180 / 2^31`).
- **Region** — a large rectangular area of the world, named `N<row>E<column>` (e.g. `N6E2`); one
  `.IDX` + several `.MAP` files per region.
- **Tile** — one square of a region's grid at a given zoom level; identified by a number `K`.
- **Level (L0–L3)** — a zoom/detail tier; L0 = whole region, L3 = street detail. Each has its own
  `shift` scale factor and tile count.
- **Profile (`regProf`)** — a content category; the same area is stored once per profile in separate
  `.MAP` files (roads, water, areas, POIs, …).
- **Block** — the smallest independently-locatable chunk: all features for one tile in one profile.
- **Marker** — a block's first 4 bytes: `0xFFFF` + length-in-words; a start tag and an end signal.
- **Cell** — a 12-byte record describing one feature (a road, river, lake, POI).
- **Point pool** — the shared list of coordinate offsets that polygon/line cells point into.
- **Delta / offset + shift** — how a position is stored compactly: `real = tile_centre + offset × 2^shift`.
- **Annotation** — a small `{size, type, payload}` attribute attached to a feature (name ref, road
  class, water type, city info, …).
- **Text record** — a stored string (possibly multi-language); shared strings are stored once and
  referenced by index.
- **Multi-slot** — an IDX tile entry that points to several profile entries, used when one tile needs
  data from multiple `.MAP` files.
