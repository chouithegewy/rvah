# Prowlarr Sources (wasm add-on)

Finds releases of a film or an episode through **your own**
[Prowlarr](https://prowlarr.com), so a debrid add-on (Real-Debrid 0.4.0+)
can play titles you don't already have. Which indexers Prowlarr searches is
up to you; this add-on ships none.

- **sources** — `GET /api/v1/search` with Prowlarr's `X-Api-Key`: by IMDb
  id first (`{ImdbId:tt…}`, type `movie`, category 2000; for an episode
  `{ImdbId:tt…}{Season:n}`, type `tvsearch`, category 5000), then by title
  if that finds nothing, since not every indexer searches by id. For an
  episode the whole season is asked for, keeping that episode's releases
  (`S01E02`, `1x02`, `S01E01E02`) and season packs (`S01`, `Season 1`),
  since a debrid add-on plays an episode out of a pack. Only torrents with
  an info hash (or a real `magnet:` link) are kept, best resolution first,
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
