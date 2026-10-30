# Web player for add-ons — design

Date: 2026-10-29. Status: draft for approval.

## Intent

David wants to watch videos that only exist as embeds on web pages, on the
Android TV app, the way RVAH shows everything else: browse a directory with
the remote, pick an item, and the video fills the screen. Such pages come
with pop-up ads, click hijackers and trackers. RVAH should show only the
video, block the ads, and never let the page wander off to another site.

RVAH needs three new things:

- an add-on source type that means "open this web page" rather than "play
  this stream";
- a locked-down, ad-blocking, full-screen web player on Android TV;
- a way for such an add-on to appear on Home.

The first user is a private catalog add-on for a live-sports site. **It is
kept outside this repository** (David's decision). Its design lives with it.
Nothing in this repo names the site or its hosts.

## Goals

- An add-on can return a **web source**: an https page the TV opens full
  screen in a web view, with nothing else on screen.
- Ads and trackers are blocked by a filter engine built into RVAH, using
  standard filter lists (EasyList, EasyPrivacy). No browser extension is used.
  David chose built-in blocking over a real uBlock Origin; what matters is
  that the ads are gone.
- The web view stays on its one page: it opens no pop-ups or new windows and
  follows no navigation to another site.
- The remote works: OK starts or pauses the video, and Back returns to where
  it was opened from.
- An add-on must declare that it opens web pages, and the install screen
  says so.
- A catalog add-on that fits none of Home's sections gets its own Home tile,
  which opens its folders as a list.

## Non-goals

- A general web browser: there's no address bar, tabs, or bookmarks.
- Playing web-source videos in RVAH's own player (Media3).
- Cosmetic filtering (hiding page elements). Network blocking is enough
  because only the video page is shown.
- Web sources on desktop, web, and CLI hosts. They reject them with a clear
  "unsupported" error.
- Progress, resume, and scrobbling for web sources.

## Anti-goals (a failure even if it technically works)

- Anything around the video: page chrome, chat, ads.
- A click or OK press opens a pop-up, a new window, or another site.
- The blocker breaks playback.
- Any add-on can open any page without having declared it.
- This repo names the private add-on's site or its hosts.

## What a spike on the Shield established (2026-10-29)

A throwaway app opened a real embed page full screen in a plain Android
WebView (Chromium 156) on the Shield. It logged requests, refused pop-ups
and navigation away, and turned OK into a tap at the centre of the screen.
A live video played; David confirmed it. Generic lessons:

- **Embed pages are often several iframes deep.** Their players check the
  Referer, refuse to run outside an iframe, and refuse to run in a
  **sandboxed** iframe. Loading the outer embed URL as the WebView's page
  satisfies all of that, because the nested iframes send the right
  referrers. So the player can't sandbox anything; the lock-down has to come
  from WebView settings.
- **Autoplay is often off.** A synthetic touch (down, then up) at the centre
  of the WebView reaches the player's play button, even across cross-origin
  frames, where injected JavaScript can't. One OK press started playback.
- **Video segments can come from third-party CDNs.** That includes big
  social-media CDNs that a privacy list might block. The add-on knows which
  hosts its video needs, so it is the one to say "never block these".

## Design

### 1. The `web` source

- **Protocol:** add `Protocol::Web` (serialized `"web"`) to
  `rvah-addon-sdk`: "an https web page the host opens in its locked-down web
  player". `Protocol::from_url` never infers it; an add-on must set it
  explicitly.
- **Shape:** a `Web` `PlayableSource` must be `https://` and carries **no
  headers**; the validator rejects headers on it, failing closed. Subtitles,
  quality, and DRM don't apply.
- **Never-block hosts:** `PlayableSource` gains `never_block: Vec<String>`
  (serde default, skipped when empty). These are host suffixes the web
  player's blocker must let through. It's only valid on `Web` sources, each
  entry must be a plain hostname, and there are at most 16.
- **Per-add-on permission:** the manifest gets `[permissions] web = true`.
  `rvah-core` rejects a `Web` source from an add-on without it, with a
  clear error. The install preview lists it ("Opens web pages full screen").
  The security docs get a line about it.
- **Host capability:** `PlaybackCapabilities` gets `web: bool`. `media3()`
  keeps `web: false`. The Android TV host passes a set with `web: true`.
  Every other host rejects `Web` through the existing `UnsupportedProtocol`
  path, as DASH does without a backend.
- **Validation and mapping:**
  - `rvah-player`'s validator accepts `Web` only when that capability is
    on.
  - `rvah-core`'s `PlaybackCapabilities::supports` gains the arm.
  - `rvah-android` adds `NativeProtocol::Web` and passes `never_block`
    through to Kotlin.
- **Tests and docs:** update the SDK property tests (`tests/prop.rs`) and
  the docs that list protocols.

### 2. Android: the web player screen

When `prepare_playback` returns `NativeProtocol.Web`, `Screen.Player` shows a
new `WebPlayerScreen` in place of the Media3 surface. `TrailerScreen` is the
model: an `AndroidView` WebView, black background, destroyed on dispose.

- **Settings:** JavaScript and DOM storage on; media playback without a user
  gesture; third-party cookies on (the spike ran with them on). File and
  content access off. `javaScriptCanOpenWindowsAutomatically = false`.
- **Lock-down:**
  - `setSupportMultipleWindows(true)` with `onCreateWindow` returning false,
    so pop-ups are refused.
  - `shouldOverrideUrlLoading` refuses main-frame navigation to any host
    other than the source URL's. Sub-frames load freely, because they are the
    player chain.
  - A download listener that does nothing.
  - Permission requests (camera, location and so on) denied.
- **Ad blocking:** `shouldInterceptRequest` asks the blocker (section 3)
  about each request and returns an empty response for blocked ones.
  - Never blocked: the main-frame request, and any host matching the
    source's `never_block`.
- **Fullscreen:** if the page asks for fullscreen (`onShowCustomView`), show
  that view full screen over the WebView, and remove it on
  `onHideCustomView`.
- **Remote:**
  - OK, Enter, and Play/Pause send a synthetic tap (down, then up) at the
    centre of the WebView, or of the fullscreen view if one is showing.
  - Back leaves fullscreen first if it's on, and otherwise returns to where
    the player was opened from.
  - D-pad arrows are consumed so focus can't wander into the page.
- **Session:** keep the screen on while it's shown, and call `stopPlayback`
  for the session on exit, like the Media3 path.
- **Errors:** an error or HTTP error status on the main frame shows RVAH's
  usual error text, and Back still returns.

### 3. The ad blocker

- **Engine:** Brave's `adblock` crate (adblock-rust, MPL-2.0) in
  `rvah-android`, exposed through UniFFI as a small object Kotlin asks for
  each request: `should_block(url, source_url, request_type) -> bool`.
  Network rules only.
- **Lists:** EasyList and EasyPrivacy, downloaded by the app from their
  official URLs. The app caches them in its files directory and refreshes
  them weekly. They're downloaded rather than bundled, so the repo and APK
  don't redistribute them. The engine is built on a background thread at
  app start, or before the first web playback.
- **No lists yet:** when the lists have never downloaded, the player still
  works. Pop-ups and navigation stay locked, and the trackers get through.
- **No built-in host exceptions:** site-specific exceptions come only from
  the source's `never_block`.

### 4. Add-on HTTP: reading only the start of a big page

- **The problem:** a page body crosses into an add-on as a JSON array of
  numbers, which the add-on walks byte by byte. A 1 MB page can use up most
  of a call's 50 M fuel before any parsing starts. Some servers ignore
  `Range`, and the useful part of a page is often near the top.
- **The fix:** `HttpRequest` gains `max_bytes: Option<u64>` (serde default).
  - The host stops reading the body after that many bytes and closes the
    response.
  - `HttpResponse` gains `truncated: bool` (serde default false), so the
    add-on knows it has only part of the page.
  - The SDK `guest` helpers get a variant that takes the cap.
  - This is generic: any add-on can use it.
- **Deadline:** if the Shield measurements show the 2 s `list` deadline is
  still too tight for an add-on that fetches a page and scans it, add a
  manifest-declared longer `list` deadline. It's capped (10 s) and works
  like the existing longer `sources` and release deadlines. Build it only if
  the measurement calls for it.

### 5. Home: a tile for add-ons that fit no section

- **The problem:** today every top-level folder of every catalog add-on
  becomes a Home column. Columns are sorted into sections by keyword, and
  whatever matches nothing goes to "More to explore", whose board merges all
  its columns into one grid. An add-on whose folders match no keyword is
  scattered there, and its own structure is lost.
- **New rule in `homeLayout`/`homeSections`:** an enabled catalog add-on
  whose top-level folders **all** land in "More to explore" is taken out of
  it. It gets its **own Home tile**, named after the add-on.
  - Opening the tile goes to `Screen.Browse(addonId, addonName)` at the
    add-on's top level.
  - That screen already lists folders, keeps its own back stack, and opens
    media items in the player.
  - David chose this list over a section of columns.
- **Effect on other add-ons:** add-ons with some folders matching a section
  keep today's behaviour. RVAH Discover and Trakt are unaffected.
- **Tests:** JVM tests in `HomeLayoutTest.kt` for the new rule.
- **SVG artwork:** add Coil's SVG decoder (`coil-svg`, the same 3.2.0 as
  `coil-compose`) and register it with the app's image loader, so add-on
  artwork can be SVG (team crests, for example).

### 6. Public repo

- **Nothing site-specific:** no site name, host, markup or fixture of the
  private add-on in tracked files. That includes test data and docs.
- **No sample pages:** the two saved sample pages in the repo root
  (`example_site_rvah_*.html`, untracked) move to the private add-on's
  folder before any commit.
- **Generic tests only:** tests in this repo use made-up hosts and pages.

### 7. Where the work starts

David's decision:

1. Commit the current uncommitted work on `feat/web-wasm-portability` as-is,
   in one commit, with nothing else changed. That work includes
   `Trailer.kt`, the Home sections, and the Veilid source; this design builds
   on all three.
2. Then branch `feat/web-player` from it.
3. Merge locally as usual when done.

## Testing

- **SDK, player, and core:**
  - `Web` round-trips through serde.
  - The validator accepts it only with the capability on, with no headers,
    over https, and with a well-formed `never_block`.
  - Hosts without web support reject it with `UnsupportedProtocol`.
  - An add-on without `web = true` is rejected.
- **Install preview:** shows the web permission.
- **Host HTTP:** `max_bytes` truncates and sets `truncated`; it's unset by
  default; it's enforced on redirect hops too.
- **`rvah-android`:**
  - The `NativeProtocol::Web` mapping and `never_block` pass-through.
  - Blocker decisions on a small inline rule set: tracker blocked,
    `never_block` host allowed, main frame never blocked.
- **Kotlin JVM tests:** the request-type inference, the navigation-allowed
  rule, and the Home tile rule.
- **On the Shield (manual, with the private add-on):**
  - Its Home tile opens the list; the folders work.
  - A server plays after one OK, and Back returns.
  - The logs show trackers blocked and no pop-up windows.
  - Run it once with the lists downloaded, to confirm they don't break
    playback.

## Decided by David

- A built-in blocker with standard filter lists, not a real uBlock Origin.
- Just the video, filling the screen.
- OK starts or pauses, Back returns, everything else is locked.
- The site add-on lives outside this public repo.
- The add-on appears as its own Home tile that opens a list.
- Commit the current work in progress first, then branch.

## My calls (check these)

- `Protocol::Web`: https-only, no headers, gated by a host `web`
  capability and a per-add-on `web` permission.
- Never-block hosts come from the add-on, on the source, with no built-in
  exceptions.
- adblock-rust in the native library; lists downloaded and refreshed weekly,
  not bundled; network blocking only.
- Third-party cookies on in the web player.
- `max_bytes` on add-on HTTP fetches, plus a longer `list` deadline only if
  the Shield measurements need it.
- The Home tile rule: "all of the add-on's folders would land in More".
- SVG artwork support via `coil-svg`.
- Arrows consumed in the web player.

## Left to the builder

- How the request type is inferred, and the blocker object's exact UniFFI
  shape.
- Where list downloading lives (Kotlin or Rust) and its retry policy.
- Whether the SDK version range needs bumping (all hosts build from this
  repo).
- Error and permission wording.
