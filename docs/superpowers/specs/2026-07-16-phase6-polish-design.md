# Phase 6: Polish — Design

User-directed continuation from the Phase 5 backlog. Three items, all
verifiable on this machine; the Windows mpv transport stays deferred (no way
to test named pipes here).

## A. Subtitles (FR-064 direction)

`PlayableSource.subtitles` finally reaches the player:

- `PlayerBackend::select_subtitle(index: Option<usize>)` (default `Ok(())`);
  `None` = off. `MockPlayer` records the command.
- `MpvBackend::load` issues `sub-add <url> auto <label> <lang>` per track
  after `loadfile`; `select_subtitle` sets `sid` to the 1-based track index
  or `"no"`.
- `PlayerController::select_subtitle` passes through (illegal from Idle).
- Service: `PlayerCommand::Subtitle(Option<usize>)`;
  `CoreEvent::PlaybackStarted` gains `subtitles: Vec<String>` (labels) so the
  player screen can render an off/track selector.

## B. Redirect policy for add-on HTTP (security-review gap)

Today ureq follows redirects internally, so a 30x could hop to a disallowed
host inside one `fetch`. Fix: the transport never follows redirects
(`redirects(0)`); `PermissionedClient` follows them itself, up to 5 hops,
re-running scheme + allowlist checks and consuming request quota on every
hop. New `NetError::TooManyRedirects`; a denied hop reports `DomainDenied`.
`MockTransport` grows `route_redirect` (Location header support) for tests.

## C. Remote-friendly key navigation (FR-045 direction)

A pure key→intent mapping in `rvah-ui::state` (`key_intent`), unit-tested:
Escape → Home, Backspace → NavBack (except while typing in text fields),
Alt+Left/Right → history, Space → play/pause toggle and `S` → stop on the
player screen. The egui layer just translates key events and stays thin.
Full spatial focus (D-pad grid movement) remains future work.
