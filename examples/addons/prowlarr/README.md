# Prowlarr Sources (wasm add-on)

Finds releases of a film or an episode through **your own**
[Prowlarr](https://prowlarr.com), so a debrid add-on (Real-Debrid 0.4.0+)
can play titles you don't already have. Which indexers Prowlarr searches is
up to you; this add-on ships none.

- **sources** — `GET /api/v1/search` with Prowlarr's `X-Api-Key`, twice:
  by IMDb id (`{ImdbId:tt…}`, type `movie`, category 2000; for an episode
  `{ImdbId:tt…}{Season:n}`, type `tvsearch`, category 5000), and by title
  (`Title 2008`, or `Title S01`), since only some indexers search by id
  (of the common public ones, YTS). The title search finds other titles
  too, so its results must name this one: the title's words in a row, and
  for a film its year (give or take one) or no year at all. Each answer is
  read only so far (32 KB by id, 96 KB by title): a call's fuel reads
  about 190 KB, and a popular film's title search can answer a megabyte.
  Prowlarr lists results indexer by indexer, so the cut drops the last
  indexers it lists, typically leaving 20–40 releases from the first.
  For an episode the whole season is asked for, keeping that episode's releases
  (`S01E02`, `1x02`, `S01E01E02`) and season packs (`S01`, `Season 1`),
  since a debrid add-on plays an episode out of a pack. Only torrents with
  an info hash (or a real `magnet:` link) are kept, each once, best resolution first,
  then most seeders, at most 40.
- **address** — `build.sh` writes your Prowlarr's host into the manifest's
  network permission, and its URL into the `url` setting's default, from
  `PROWLARR_URL` (environment or the repository's `.env`). It must be https.
- **secret** — `api_key`: Prowlarr's, from Settings > General > Security.
  Android TV debug builds store it from `.env`'s `PROWLARR_API_KEY`.

A search waits on every indexer, so the host gives `sources` 45 s rather
than the usual 2 s.

```sh
./build.sh                                  # → dist/, for PROWLARR_URL
rvah package dist -o prowlarr.rvah --key keys/signing.key
```

`cargo test` here runs the logic against the SDK's scripted host.
