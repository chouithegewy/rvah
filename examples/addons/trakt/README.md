# Trakt (wasm add-on)

Rows from [Trakt](https://trakt.tv) on the TV, and your playback in your
Trakt history.

- **catalog** — once linked: Up next (the series you are watching, by
  their next episode: Trakt's `/sync/progress/up_next`, then your calendar
  for the week ahead), recommended films and series, your watchlist;
  always: trending and most anticipated films and series (`extended=full`,
  20 per page, Trakt's `X-Pagination-Page-Count`). Items are TMDB ids
  (`movie:<id>`, `tv:<id>`), so the TV opens them on the TMDB add-on's
  details page, with its ratings, trailers, Episodes and Find sources;
  items Trakt knows no TMDB id for are left out. Posters are Trakt's own
  (WebP from media.trakt.tv), which the TV caches, as Trakt asks.
- **auth** — Link account with Trakt's device code: a code to enter at
  trakt.tv/activate. Trakt apps have no client secret since 2026-10-01, so
  only the client id is sent. The access token lasts 7 days; when it is
  rejected the host renews it with the single-use refresh token, sending the
  app's redirect URI (the `redirect_uri` setting).
- **account_status** — `GET /users/settings`.
- **scrobble** — start, pause and stop with progress for a film or an
  episode (by the series' TMDB id, season and number). Not linked: nothing
  is sent. Only an expired sign-in is an error (so it gets renewed); any
  other answer, such as "already scrobbled", is logged, never shown.
- **secrets** — `client_id` (your app's, from app.trakt.tv/settings/apps;
  Android TV debug builds store `.env`'s `TRAKT_CLIENT_ID`),
  `access_token` and `refresh_token` (written by Link account).

```sh
./build.sh       # → dist/, redirect URI from TRAKT_REDIRECT_URI
rvah package dist -o trakt.rvah --key keys/signing.key
```

`cargo test` here runs the logic against the SDK's scripted host.
