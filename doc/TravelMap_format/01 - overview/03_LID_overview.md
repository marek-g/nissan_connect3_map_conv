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
| `LID0nnnnn.DAT` | **The address gazetteer.** Lists of *names* grouped by kind (cities, streets, crossings…) and by country, each name tied to a location. **This is what address search reads.** | The phone book: "name → where". |
| `LID2/3/4/5nnnn.DAT` | **Map objects.** The actual landmarks and shapes drawn/anchored on the map (points, lines, areas) with their labels. | The illustrated entries next to the phone-book entry. |
| `GLOB_POI.DAT` | **POI search index** — a small SQLite database, a searchable table of named points (fuel, hotels, churches, airports…). | The yellow-pages search box. |
| `DB_CITY.DAT` | *(optional)* **Global city index** — a SQLite table of every city with its position and ZIP. Used for country-wide city/ZIP lookup when present. | The nationwide city directory. |
| `RELnnnnn.DAT` | **Relations.** Links like *"this street belongs to that city"* or *"this is a district of that city."* | The cross-references between phone-book entries. |
| `PA_nnnnn.DAT` | *(optional)* **Point addresses.** Exact coordinates of specific house numbers (a "portal"), used instead of guessing. | The precise door location. |
| `META0000/9999.DAT`, `CONNECT.DAT` | **Index / connective tissue.** Region roots, type tables, and how regions cross-reference each other. | The library's catalogue. |

The relationship: **address search fans out across these files.** The name you type is matched against
a `LID0` name-list; `REL` files say which city a matched street sits in; `PA`/interpolation pinpoints
the house number; the result is a coordinate handed to the router.

---

## 3. How the car uses LID: the journey of one address search

Best way to see *why* every file exists. The user types **city → street → house number**:

1. **Type the words.** The search engine (internally "OSDE/LISA") splits your text into *city*, *street*,
   *house number* parts using tag rules from a config file (`CONF.XML`).
2. **Find the region.** It locates which country/region folder(s) could contain that name.
3. **Match names.** It opens the region's `LID0` name-lists and string-matches — the city list (kind 2),
   the street list (kind 3). A fuzzy match is allowed for typos (the config lists characters that can be
   replaced without penalty).
4. **Tie street to city.** Via `REL` relation files it confirms *"this street is in that city"* (and folds
   in city *districts*).
5. **Resolve the house number.** For a number it reads the street's **general-attribute** list (in `LID0`
   at a high file-number band) to see the valid numbers and even/odd sides, then gets an exact point from
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

Technically a name-list is a *sequence* of records inside a `LID0` file, with a small header stating
that sequence's kind and country and how many entries it holds. The car filters by kind when searching
("only look in the street list").

### 4.3 Names live in a shared text pool (interning)

Names are long and repeat constantly across languages, so they are **not** stored inside each record.
Each record carries a short **text-id**, and the strings themselves sit once in a shared **text pool**
at the end of the file. Place entries are stored as one composite string that looks like
`NAME/CODE/COUNTRY` (e.g. `BOBOSZOW/33/CZECHY`), and streets simply as `ULICA …`. A city shown in Polish,
German and English is the *same* entry with the country word translated — cheap, because only the pool
differs.

### 4.4 The three "containers" of LID files

Open a file and you notice two different first-bytes signatures:

- `LID0…` (and `CONNECT.DAT`) begin with `0d 00` — the container that **address search** walks.
- `LID2/3/4/5…` begin with the **region id** (POL = `04 02` = 1026) — the container for **map objects**.

Both wrap the same underlying "block" structure (§4.5); they just carry a different kind of payload. The
`04 02` region id is the same `0x402` "regionIdent" used elsewhere in the system — one region, one id.

### 4.5 Blocks, sequences, records (LID's three sizes)

LID content is nested three levels deep:

- a **block** = one geographic chunk of content, starting with a fixed 40-byte header: a version, a
  unique id, a **bounding box** (the lat/lon rectangle it covers, in PAU), the dataset/region id, and a
  content-type tag. (So the car can skip a whole block if it is outside the area you are searching.)
- a **sequence** = a run of same-kind, same-country entries (a name-list, §4.2), with a 12-byte header.
- a **record** = one entry: for a point it is a handful of bytes holding a display scale, a position
  (PAU) and a **text-id**; lines/areas add a coordinate list.

### 4.6 POI records and categories

A **POI** (point of interest) record is the same idea — position + text-id + attributes. Its **category**
(fuel, hotel, church, city, …) maps to a `CAT_ID` in the SQLite `GLOB_POI.DAT`, translated through the
`POI_MAPPING.DAT` database. There are ~16 broad categories (Fuel, Hotel, Restaurant, City, Landmark,
Transport, Sanctuary, …). Note: **streets and house numbers are NOT POI rows** — they only exist in the
`LID0` name-lists.

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

## 5. Walking through a `LID0` file (top to bottom)

1. **Container opening** (`0d 00`, then counts/offsets and the region id `04 02`): tells the reader how
   the file is sectioned and which region it is. *(The precise framing of this section table is the one
   byte-level detail still being confirmed — see `LID_format.md` §5/§10.6.)*
2. **One or more blocks** (each 40-byte header → bounding box + region + type).
3. Inside a block, **sequences** = the name-lists, each labelled with its **kind** (city/street/…) and
   **country** and a record count.
4. The **records**: positions + text-ids.
5. The **text pool**: every name string once, indexed by text-id (`NAME/CODE/COUNTRY`, `ULICA …`).

A search that wants "cities" reads only the kind-2 sequences; "streets" reads kind-3; a house number also
pulls the general-attribute records at the +20000 file band.

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
- **Text pool / text-id** — names stored once and referenced by a small number (interning).
- **Block / sequence / record** — LID's three nesting sizes (chunk → name-list → one entry).
- **Bounding box** — a block's lat/lon rectangle; lets the car skip irrelevant blocks.
- **PAU** — the whole-number coordinate scale (`deg = PAU × 180 / 2³¹`), shared by MAP/RNW/LID.
- **`GLOB_POI.DAT` / `DB_CITY.DAT`** — plain-SQLite search tables (POIs / cities+ZIP).
- **`CAT_ID`** — a POI's category number, translated through `POI_MAPPING.DAT`.
- **`REL` / `PA` file** — relations (street-in-city) / exact house-number coordinates.
- **regionIdent / REGION_ID** — a region's id (`0x402` = POL); the same number used across the system.
- **Interpolation** — guessing a house-number position along a street when no exact point exists.

> **Converter takeaway:** OSM → LID means writing the city/street name-lists (with `NAME/code/COUNTRY`
> and `ULICA …` text), the `REL` city links, the SQLite `GLOB_POI`/`DB_CITY` tables, and (for exact house
> numbers) `PA`/general-attribute records. Roads come from RNW/MAP — **addresses always come from LID.**
