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
| `LID2nnnn.DAT` | **The address name-lists — this is what address search reads.** Batched *names* by kind (cities in one file, streets in another), each tied to a location. The *same* file slot is reused, offset by file-number, for the extras: **+10000 = crossings**, **+20000 = house-number attributes** for that street list. | The phone book: "name → where". |
| `LID0nnnn.DAT` | **Landmark / map-object content** (`fm_tcl` blocks). Named POIs (churches, monuments…) and drawn shapes with a shared text pool. *Not* the city/street search source. | The illustrated entries next to the phone-book entry. |
| `LID3/4/5nnnn.DAT` | **Map objects.** The actual shapes drawn/anchored on the map (points, lines, areas) with their labels. | More illustrated entries. |
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

### 4.4 The two containers, corrected

Open a file and you notice two different first-bytes signatures:

- `LID2/3/4/5…` open with the **region id** (`04 02` = 1026 for POL) — this is the container **address
  search** walks (the trie/column name-lists live in the `LID2` files here).
- `LID0…` (and `CONNECT.DAT`) open with `0d 00` — the container for the **landmark-POI / `fm_tcl`** content
  (§4.6), *not* the address search source.

The `04 02` region id is the same `0x402` "regionIdent" used elsewhere in the system — one region, one id.
So although an older note called `LID0` "the gazetteer", **address search actually reads the `LID2` files**.

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

Positions are stored as small **deltas from a block's origin point**, not as absolute coordinates; the origin
is looked up from a separate position index at run time. (That per-block origin anchor is the one byte-level
detail the reader has *not* yet pinned — see `LID_format.md` §12.5. It cancels out in a converter's own
write→read round-trip, so relative positions are exact.)

A search that wants "cities" reads the city `LID2` file; "streets" reads the street `LID2` file; a house
number also pulls the general-attribute columns in the **+20000** file.

---

## 6. Tying it together: "Wiślna 12, Kraków", end to end

1. Split into city=Kraków, street=Wiślna, number=12 (tag rules).
2. Open the POL region. Match **Kraków** in the kind-2 city name-list → a text-id and the city centre
   position (or, if `DB_CITY.DAT` existed, its position there).
3. Match **Wiślna** in the kind-3 street name-list → a street element.
4. Use `REL` (relation type "street-in-city") to confirm Wiślna really belongs to Kraków.
5. Read the street's house-number attributes: even/odd rule and range; pick **12**; take the exact point
   from a `PA` file, or interpolate it along Wiślna between 10 and 14.
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
- **+10000 / +20000 file band** — the *crossing* / *house-number-attribute* variants of a name-list file
  (same file slot, file-number shifted).
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
> their position/belonging columns (the reader and writer in `src/lid_format/` already do this and are
> round-trip validated), the `REL` city links, the SQLite `GLOB_POI` table, and — for house numbers — the
> **+20000** general-attribute columns plus optional `PA` exact points (the reader in `src/lid_format/` now
> decodes the **+20000** file's block/column structure too; only its *writer* and `PA`/`REL` remain). Roads
> come from RNW/MAP —
> **addresses always come from LID.** On the reviewed card `DB_CITY.DAT` is absent, so the regional `LID2`
> name-lists are the *only* thing that makes city/street search work: skip them and the address field keeps
> using the stale stock data.
