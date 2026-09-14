# TravelMap LID (`.DAT`) — a plain-language, detailed overview

This document explains the Bosch "LID" data category (as used in the Nissan LCN2KAI / Connect 3)
in words a non-programmer can follow, in the same spirit as
[`01_MAP_overview.md`](01_MAP_overview.md) explains the rendered map. It says what each part is, *why*
it exists, and *what job* it does.

For the precise byte-level layout see [`LID_format.md`](../02%20-%20details/LID_format.md). This is the
"understanding" layer underneath it.

---

## 1. What LID is, and why it exists

A navigation system stores three kinds of data on the card:

| Layer | Folder | Job |
|-------|--------|-----|
| **MAP** | `DATA/MAP` | *Drawing* — the pictures of roads, rivers, lakes you see on screen. |
| **RNW** | `DATA/RNW` | *Driving* — the road network the router computes a route over. |
| **LID** | `DATA/LID` | *Finding* — the searchable content: **cities, streets, house numbers, points of interest, place names, landmarks**. |

The `.MAP` and `.DAT` road files answer *"how do I get there"* and *"what does it look like"*. **LID
answers *"where is it""** — the whole experience of typing *"Kraków, ul. Wiślna, 12"* and the car
understanding you, plus drawing a church or a petrol station icon on the map.

LID is the **largest** category on the card (~2.1 GB across 16 country folders — bigger than MAP),
because names and addresses take a lot more room than lines and polygons.

Why is it separate from MAP/RNW? Because *drawing a road* and *knowing that a road is called "ul.
Wiślna" and belongs to Kraków and has house numbers 2, 4 … 88 on the even side* are different problems
with different data shapes. Keeping them apart lets the car stream only what it needs for the current
task (draw, drive, or search).

---

## 2. The files and how they relate

All under `DATA/LID/CCP/<REGION>/`, where `<REGION>` is one of the 16 content regions (`POL`, `DEU`,
`EEU`, …). Each region is a self-contained set of files:

| File(s) | Plain-language role | Analogy |
|---------|---------------------|---------|
| `LID2nnnn.DAT` | **The address name-lists — this is what address search reads.** Batched *names* by kind (cities in one file, streets in another), each tied to a location. | The phone book: "name → where". |
| `LID(n+10000).DAT` (same slot, +10000) | **Crossings** for that list — where a street crosses/meets another (the intersection points used when you search for a crossing). A *different* container from the name-list (§4.4). *(Our reader/writer treats these as out of scope.)* | Street-corner map in the back of the phone book. |
| `LID(n+20000).DAT` (same slot, +20000) | **House-number attributes (GenAttr).** For each street: which house numbers exist, their even/odd sides, and — keyed by an address *element* — the exact number, its name and a *record→street* join. This is what lets *"Wiślna 12"* resolve. | The number-range sidebar next to the street entry ("4–14 even, 1–9 odd"). |
| `LID0nnnn.DAT` | **Landmark / map-object content** (`fm_tcl` blocks). Named POIs (churches, monuments…) and drawn shapes with a shared text pool. *Not* the city/street search source. | The illustrated entries next to the phone-book entry. |
| other `LID3/4/5nnnn.DAT` | **Map objects.** The actual shapes drawn/anchored on the map (points, lines, areas) with their labels — i.e. `LID3/4/5` whose number is *not* a name-list slot shifted by +10000/+20000. | More illustrated entries. |
| `GLOB_POI.DAT` | **POI search index** — a small SQLite database, a searchable table of named points (fuel, hotels, churches, airports…). | The yellow-pages search box. |
| `DB_CITY.DAT` | *(optional)* **Global city index** — a SQLite table of every city with its position and ZIP. Used for country-wide city/ZIP lookup when present. *(Absent on the reviewed EUR card, so the regional name-lists carry all city lookup.)* | The nationwide city directory. |
| `RELnnnnn.DAT` | **Relations.** Links like *"this street belongs to that city"* or *"this is a district of that city."* | The cross-references between phone-book entries. |
| `PA_nnnnn.DAT` | *(optional)* **Point addresses.** Exact coordinates of specific house numbers (a "portal"), used instead of guessing. | The precise door location. |
| `META0000/9999.DAT`, `CONNECT.DAT` | **Index / connective tissue.** Region roots, type tables, cross-region thesaurus. *Not needed for regional address search* — only for cross-region / multi-language fuzzy matching. | The library's catalogue. |

The relationship: **address search fans out across these files.** The name you type is matched against
a `LID2` name-list; `REL` files say which city a matched street sits in; the house-number attributes live
in the **+20000** file and a `PA`/interpolation pinpoints the number; the result is a coordinate handed to
the router.

---

## 3. How the car uses LID: the journey of one address search

Best way to see *why* every file exists. The user types **city → street → house number**:

1. **Type the words.** The search engine (internally "OSDE/LISA") splits your text into *city*, *street*,
   *house number* parts using tag rules from a config file (`CONF.XML`).
2. **Find the region.** It locates which country/region folder(s) could contain that name.
3. **Match names.** It opens the region's `LID2` name-lists and string-matches — the city list, the street
   list. A fuzzy match is allowed for typos (the config lists characters that can be replaced without
   penalty).
4. **Tie street to city.** Via `REL` relation files it confirms *"this street is in that city"* (and folds
   in city *districts*).
5. **Resolve the house number.** For a number it reads the street's **general-attribute** columns (the
   **+20000** file) to see the valid numbers and even/odd sides, then gets an exact point from
   a `PA` file, or else **interpolates** the position along the street between neighbours.
6. **Show results / go.** Each result becomes a coordinate. That coordinate is handed to the router —
   the rest is RNW's job. The road network itself is *never consulted for the address text*.

That is why a converter that writes only MAP + RNW makes the map look and drive right but leaves the
address search on the **old** data — to refresh it you must also write LID.

---

## 4. The building blocks, one by one

### 4.1 Coordinates and "PAU" (same as MAP/RNW)

Every position in LID is stored the same way everywhere in this system: **PAU** —
degrees scaled to a whole number (`degrees = PAU × 180 / 2³¹`). Think "degrees, in tiny whole-number
ticks." (MAP/RNW use the identical rule, so a coordinate is comparable across all three layers.)

### 4.2 Name-lists: a *kind* and a *country*

The heart of address search is the **name-list**: a batch of names that all share:

- a **kind** — city (2), street (3), crossing (4), house number (5), ZIP (61), … — and
- a **country** (an ISO code), so the same place can appear under each language's spelling.

Technically a name-list is a *sequence* of records inside a `LID2` file, with a small header stating
that sequence's kind and country and how many entries it holds. The car filters by kind when searching
("only look in the street list") — in practice the city list and the street list are separate `LID2`
files, so "filter by kind" is mostly "pick the right file".

### 4.3 Names are stored as a *trie*, not a per-record text-id

It is tempting to picture a name-list as "record → id → string in a pool". For **landmark POIs** (`LID0`,
§4.6) that is roughly right — they use a shared text pool and a small `text-id`. But the **address
name-lists** (`LID2`, the ones address search reads) store names completely differently, and this is the
single most important thing to get right in a converter:

- The names of a block live in a **trie** (a character tree): the letters are shared along common
  prefixes, so `WIŚLN` is stored once and `…A` / `…B` branch off it. Each entry (a *street*, a *city*) is
  one **leaf** of that tree, and reading it walks root→leaf spelling the name. The on-device code confirms
  it is a *plain* tree — every node has exactly one parent, no sharing between branches.
- Everything *about* an entry — its position, which city it belongs to, whether it has house numbers or
  crossings — is **not** in the record either. It sits in **separate parallel columns**, one value per
  entry, each independently compressed. So a name-list block is really *a trie of names* laid side-by-side
  with *a small table of per-entry attributes*.
- Where a name has two spellings (a folded ASCII form and a proper accented form) they are stored together
  in the trie separated by a tab byte; the car shows the accented one.

### 4.4 The container is shared; the *sub*-header is not (corrected)

All the address files share the same **outer header** (the region-id / opening bytes); what *differs* is the
sub-header read immediately after it, so the *same* file-opening signature hides **three unrelated layouts**.
This is the most converter-relevant subtlety in LID:

- **Name-lists** (the `LID2` cities + streets a search reads) carry, after the header, a *section table* —
  a list of block positions — and each block is the **trie + parallel columns** of §4.3.
- **Crossings** and **GenAttr house-number attributes** (a name-list's own slot shifted **+10000** / **+20000**)
  share that outer header too, but the car reads them with **different sub-parsers**:
  - crossings: a compact *street-pair index* (which street meets which);
  - GenAttr: a block TOC (`elem_start / elem_end / block_off` entries that *tile* the element range) whose blocks
    hold **attribute-vector columns** keyed by an *address* element (see §4.4b).
- **Landmark / `fm_tcl` content** (`LID0…`, and `CONNECT.DAT`, open differently) uses an entirely distinct
  block→sequence→record nesting (§4.5) — *not* the address search source.

So "it opens like a name-list" ≠ "it *is* a name-list": opening a **+20000** GenAttr file with the name-list
reader returns garbage, because the *second* header uses a different format. The region id (`04 02` = 1026
for POL) at the very start is the same `0x402` "regionIdent" used everywhere — one region, one id.

### 4.4b How the **+20000** GenAttr file answers "this street has numbers 2, 4…88, even"

Inside the GenAttr file sits a **block TOC** (`start / end / offset` entries that *tile without gaps* the
element range 0…) — the reliable "this is a GenAttr, not a name-list" signal. Each block then describes a run
of **address elements** (one record per house-numbered point, *not* per street) as attribute columns; the car
pulls a street's numbers through a small **fixed join**:

- **Which street owns each number (reverse, the browse path):** a per-street column lists the **address-element
  ids** that street owns (existence bitmap + per-street count + the concatenated ids) → "give me Wiślna's
  numbers" = the street's slice of that list.
- **The number range (forward, each address):** a per-address *range* column holds the actual house **number**
  (and its even/odd + interpolation flags).
- **Which street each address belongs to (the record→street join):** `address → owner-descr index → list of
  street elements`; the UI then resolves each street element back to its `LID2` name.

Each column is a *triple* of byte-streams with an author-convention `param`: an existence bitmap (over a domain
of elements), a *cumulative-count* stream, and the values — a tiny per-element "does it exist / how many bytes /
what are they". Our `src/lid_format/` reader decodes all of the above against `POL/LID40006`; and the **byte
encoders are proven device-faithful** — re-encoding every read block reproduces the authoring tool's bytes
exactly (133 blocks), so the `write.rs` writer's output is byte-valid for the device.

### 4.5 Blocks, entries, and columns

Address name-list content is nested a little differently from the landmark content:

- a **block** = one chunk of a name-list — a trie plus its attribute columns. A big file is split into many
  small blocks (a city file like POL's is a few dozen; a street file ~a hundred).
- an **entry** = one name (one trie leaf) — a city or a street — given a sequential number.
- a **column** = one attribute laid across all entries in the block (position X, position Y, belonging-city,
  "has house numbers"…), each column compressed on its own (run-length, delta, bitmap, byte-packing…).

(Landmark `LID0` files instead use the block → sequence → record nesting of §4.6.)

### 4.6 POI records and categories

A **POI** (point of interest) record is the same idea — position + text-id + attributes. Its **category**
(fuel, hotel, church, city, …) maps to a `CAT_ID` in the SQLite `GLOB_POI.DAT`, translated through the
`POI_MAPPING.DAT` database. There are ~16 broad categories (Fuel, Hotel, Restaurant, City, Landmark,
Transport, Sanctuary, …). Note: **streets and house numbers are NOT POI rows** — they only exist in the
`LID2` address name-lists (§4.3), never in the POI tables.

### 4.7 The SQLite helper files (`GLOB_POI.DAT`, `DB_CITY.DAT`)

These are ordinary SQLite databases — no proprietary encoding, trivially readable or writable.

- `GLOB_POI.DAT` = one virtual FTS (full-text-search) table `GLOBAL_POIS` with columns roughly
  *(id, normalised-name, display-name, language, …, longitude, latitude, category, region, flags)*.
  `normalised-name` is the ASCII-folded form the search matches on; the `region` column equals the
  region's id (POL = 1026).
- `DB_CITY.DAT` = a `GlobalCityList` table *(id, name, normalised, lon, lat, province, category)* used for
  country-wide city and ZIP lookups. On the reviewed European card it is **absent**, so regional name-lists
  carry the load.

---

## 5. Walking through a `LID2` name-list file (top to bottom)

1. **Container opening** (region id `04 02`, then counts and a table of block positions): tells the reader
   how the file is split into blocks and which region it is.
2. **One or more blocks** (a city file: a few dozen; a street file: ~a hundred). Each block is independent.
3. Inside a block: a small **header** (how many names, how many attribute columns), a **table describing the
   columns**, then the columns themselves.
4. The **name trie** — spell each entry root→leaf to get its street/city name (accents come from the tab-
   separated variant, §4.3).
5. The **attribute columns** — each entry's position, its belonging city, and its flags (has house numbers,
   has crossings, …), one compressed column per attribute.

Positions are stored as small **deltas from a per-block origin point**, not as absolute coordinates. The origin
is now pinned (`LID_format.md` §12.5): for street tiles it is the **position of the city being searched** —
supplied by the lookup flow when it loads the block, never stored in the name-list — while each file's header
carries one file-level anchor (the region's corner, `-1/-1` = none, `LID_format.md` §12.5). For a converter
this means street blocks must be grouped **one city per block**, with positions relative to that city's
coordinate, so they agree with what the device will assume at query time.

A search that wants "cities" reads the city `LID2` file; "streets" reads the street `LID2` file; a house
number also walks the **+20000** GenAttr file's record→street join + number-range columns (§4.4b) — that file is a
*different* container, so a name-list reader cannot read it.

---

## 6. Tying it together: "Wiślna 12, Kraków", end to end

1. Split into city=Kraków, street=Wiślna, number=12 (tag rules).
2. Open the POL region. Match **Kraków** in the kind-2 city name-list → a text-id and the city centre
   position (or, if `DB_CITY.DAT` existed, its position there).
3. Match **Wiślna** in the kind-3 street name-list → a street element.
4. Use `REL` (relation type "street-in-city") to confirm Wiślna really belongs to Kraków.
5. Read the street's **GenAttr house-number attributes** (the **+20000** file, §4.4b): the street's *address-*
   element list yields the range/even-odd; pick **12**; take the exact point from a `PA` file, or interpolate it
   along Wiślna between 10 and 14. (The same file also carries the *record→street* join the result list uses to
   name/address each point.)
6. Hand the resulting PAU coordinate to the router (RNW). Search done.

Every LID file you read about above exists to make one of those six steps work.

---

## 7. Quick glossary

- **LID** — the *finding* data layer: cities, streets, house numbers, POIs, place names.
- **OSDE / LISA** — the on-device engine that runs address search over LID.
- **Name-list** — a batch of names sharing a *kind* (city/street/…) and *country*; the unit search scans.
- **Kind / category id** — what a name-list holds: 2=city, 3=street, 4=crossing, 5=house-number, 61=ZIP.
- **Text pool / text-id** — landmark-POI names (`LID0`) stored once and referenced by a small number. The
  *address* name-lists do **not** use this — they store names as a **trie** (§4.3).
- **Trie (name-list)** — the tree of shared name prefixes that address search reads; one leaf = one entry.
- **Attribute column** — one per-entry attribute (position, belonging city, "has house numbers"…) laid
  across a block, compressed on its own.
- **+10000 / +20000 file band** — the *crossing* / *house-number-attribute(GenAttr)* variants of a name-list
  file *slot* (same slot number, file-number shifted). Same outer header, **different sub-header**, so each
  needs its own reader (§4.4).
- **Block / sequence / record** — the *landmark* `LID0` nesting (chunk → typed list → one entry). Address
  name-lists nest as block → trie + columns (§4.5).
- **Bounding box** — a block's lat/lon rectangle; lets the car skip irrelevant blocks.
- **PAU** — the whole-number coordinate scale (`deg = PAU × 180 / 2³¹`), shared by MAP/RNW/LID.
- **`GLOB_POI.DAT` / `DB_CITY.DAT`** — plain-SQLite search tables (POIs / cities+ZIP).
- **`CAT_ID`** — a POI's category number, translated through `POI_MAPPING.DAT`.
- **`REL` / `PA` file** — relations (street-in-city) / exact house-number coordinates.
- **regionIdent / REGION_ID** — a region's id (`0x402` = POL); the same number used across the system.
- **Interpolation** — guessing a house-number position along a street when no exact point exists.

> **Converter takeaway:** OSM → LID means writing the city and street `LID2` name-lists as **tries** with
> their position/belonging columns (reader + writer in `src/lid_format/`, round-trip validated), the `REL` city
> links, the SQLite `GLOB_POI` table, and — for house numbers — **writing the correct **+20000** GenAttr file
> (§4.4b)**. The GenAttr *format* is solved both ways: the reader decodes it from `POL/LID40006` and the
> **writer (`src/lid_format/src/write.rs`) is now byte-valid** — its column encoders reproduce the authoring
> tool's bytes exactly over 27 MB (133 blocks), and a synthetic write round-trips through the car's member-lookup
> path. What's left for a *refreshed* house-number path is the OSM→file **data join**: mapping each
> `addr:housenumber` object to a street element + a LID3 crossing/address element so the block can actually be
> filled. Roads come from RNW/MAP — **addresses always come from LID.** On the reviewed card `DB_CITY.DAT` is
> absent, so the regional `LID2` name-lists are the *only* thing that makes city/street search work: skip them
> and the address field keeps using stale stock data.
