# SubDL Subtitles (wasm add-on)

Subtitles from [SubDL](https://subdl.com) for what you play from a details
page, through the `subtitles` capability.

- **Search** — one call to `api.subdl.com/api/v1/subtitles` by IMDb id (else
  by title and year), in the languages asked for (`en` → `EN`, `pt-BR` →
  `BR_PT`), with `unpack=1`: SubDL then lists each archive's files with a
  link to each on its own, so the player loads a plain `.srt` (or `.vtt`,
  `.ass`, `.ssa`) from `dl.subdl.com`; no unzipping.
- **Episodes** — a season pack's file is picked out by its episode number;
  other seasons' packs are left out. **Films** split over several files
  (CD1, CD2) are left out.
- **Order** — those made for the release playing first: the words they share
  with its name (resolution, source, codec), and its release group most;
  among equals, plain before SDH (hearing impaired, labelled so). At most 20.
- **Your key** — `api_key` (free: 2,000 searches a day). SubDL puts the key
  in every download link it sends; the add-on cuts it off (the files
  download without it), so it never reaches the player or its logs.

The TV asks for English subtitles by itself when the file playing has none,
and Subtitles → Find online lists the rest.

```sh
examples/addons/subdl/build.sh
rvah package examples/addons/subdl/dist -o subdl.rvah --key keys/signing.key
```
