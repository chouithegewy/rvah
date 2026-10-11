# RVAH Discover (wasm add-on)

One Discover board from two add-ons **nested as components**: the TMDB
add-on and the Trakt add-on are compiled in as libraries (their `standalone`
feature, which exports their own wasm entry points, is off). Each stays a
separate add-on too.

Columns, left to right:

1. **Up next** — the series you are watching (only once linked to Trakt):
   first those with an episode out that you haven't seen, the newest release
   first, each opening on that episode; then those with a new episode in the
   week ahead, the soonest first, each opening on the series. Each card says
   which episode and when ("S2E5 · 3 to watch", "S3E2 · in 2 days").
2. **New to streaming** — films released digitally in the last 60 days in
   your region, most popular first (TMDB `with_release_type=4`); never
   theatrical-only or upcoming.
3. **Popular to stream or rent** — popular films already out digitally or on
   disc that some streaming, rental or purchase service in the region has.
4. **You might like** — your Trakt recommendations, films and series
   alternating (only once linked).
5. **Your genres** — the `genres` setting, in its order: each column mixes
   that genre's popular films and series you can stream or rent. Anime is
   animation in Japanese. The TV toggles genres; turning one on puts it last.

Search, details, seasons and episodes are TMDB's (ratings through MDBList
with an `mdblist_key`); Link account, account status and scrobbling are
Trakt's. Trakt paths are mounted under `trakt/`. "Today" comes from the
host's `now` call.

Secrets: `tmdb_key`, `mdblist_key`, `client_id`, `access_token`,
`refresh_token` (debug Android builds store the first three from `.env`).

```sh
./build.sh      # → dist/, redirect URI from TRAKT_REDIRECT_URI
rvah package dist -o discover.rvah --key keys/signing.key
```

This product uses the TMDB API but is not endorsed or certified by TMDB.
