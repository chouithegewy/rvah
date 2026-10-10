# TMDB Discover (wasm add-on)

Browse what is trending, in theaters, coming soon and popular on
[TMDB](https://www.themoviedb.org), search films and series, and open a
details page with genres, tagline, trailers and ratings. Nothing here is
playable: the TV's details page finds the title in your other add-ons
(such as Real-Debrid).

- **catalog** — eight rows at the root (trending this week, in theaters,
  coming soon, popular and top rated films, new episodes this week,
  popular and top rated series), paged by TMDB's page numbers. Only those
  rows can be listed. "In theaters" and "Coming soon" follow the `region`
  setting (default `US`); everything follows `language` (default `en-US`).
- **genres** — "Films by genre" and "Series by genre" close the root,
  each a folder of folders (`of_directories`), so the TV shows one column
  per TMDB genre; a genre lists its popular titles (`/discover`, at least
  100 votes for films and 50 for series).
- **search** — TMDB's multi search, films and series only.
- **seasons and episodes** — a series (`tv:<id>`) lists its seasons,
  specials last; a season (`tv:<id>/season/<n>`) lists its episodes
  (`tv:<id>:s<n>e<m>`), whose details page names the series and carries
  its IMDb id, so a sources add-on can find releases of the episode.
- **details** — `/movie/{id}` or `/tv/{id}` with videos: genres, runtime,
  tagline, release date, IMDb id, and up to three YouTube trailers or
  teasers (official trailers first). The TV builds the YouTube link from
  the video key. Ratings: TMDB's own score, plus Rotten Tomatoes (critics
  and audience), IMDb, Metacritic and Letterboxd from
  [MDBList](https://mdblist.com) when `mdblist_key` is set. MDBList's free
  tier allows 1,000 lookups a day, so ratings are looked up only for an
  opened details page, and an MDBList failure only drops its ratings.
- **secrets** — `tmdb_key`: an API key or a read access token from
  <https://www.themoviedb.org/settings/api> (a token, `eyJ…`, is sent as a
  bearer header, a key as `api_key`). `mdblist_key`: optional, from
  <https://mdblist.com/preferences>.

The permission grant covers `api.themoviedb.org`, `image.tmdb.org` (the TV
loads artwork only from hosts an add-on declares) and `api.mdblist.com`.

## Build & install

```sh
./build.sh                                 # → dist/{manifest.toml,module.wasm}
rvah package dist -o tmdb.rvah --key keys/signing.key
```

On Android TV debug builds the keys come from the repository's `.env`
(`TMDB_READ_ACCESS_TOKEN` or `TMDB_API_KEY`, and `MDBLIST_API_KEY`).

## Tests

`cargo test` here runs the logic natively against the SDK's scripted host.
After `./build.sh`, `cargo test -p rvah-android --test tmdb_guest --
--ignored` in the repository root runs the built module under the Android
sandbox with its default fuel and time limits.

This product uses the TMDB API but is not endorsed or certified by TMDB.
