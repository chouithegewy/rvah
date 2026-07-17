# Phase 8: Navigation Depth — Design

Continuation from the Phase 7 backlog (README "Deferred" + packaging.md's
open CI item). Three items, all verifiable on this machine; the Windows mpv
transport stays deferred.

## A. Full 2-D spatial focus (FR-045 completion)

Phase 7's focus model was 1-D: one target per row, Up/Down only. But rows
have multiple actions (an add-on row has Open / Disable / Permissions /
Settings / Sign in; a media row has play + favorite). Full D-pad coverage:

- `focus_grid(state) -> Vec<Vec<FocusEntry>>` replaces `focus_targets`:
  outer index = row (render order), inner = the row's actions left-to-right.
  Home: add-on rows (Open, Enable/Disable, Permissions, Settings, Sign in),
  favorite rows (play, remove), recent rows (play). Browse: directory rows
  (open), media rows (play, favorite), plus a Load-more row (item B).
  Search: result rows (play).
- `UiState.focus: Option<(usize, usize)>` (row, col). `FocusNext`/`FocusPrev`
  move rows, clamping the column into the new row; new
  `FocusLeft`/`FocusRight` move within the row, clamping at the edges.
  `Activate` dispatches the focused entry's intent.
- `NavKey::{Left, Right}`: plain ArrowLeft/ArrowRight (no Alt, not typing)
  map to column movement — Alt+arrows keep meaning history.
- Views: every action button in a focusable row goes through `nav_button`;
  disabled add-ons' Enable button is focusable too (it was skipped in 1-D).

## B. Browse pagination in the GUI (FR-024)

`Page.cursor_next` flows end-to-end (provider → core → service) but the GUI
renders only a hint label. Fix:

- `UiIntent::LoadMore`: when Browse is `Loaded` with `cursor_next`, issues
  `CoreRequest::Browse` with the cursor and sets `state.loading_more`.
- `reduce` on `Page` for the current location: if `loading_more`, *append*
  items to the existing page and take the new cursor (existing focus and
  scroll stay valid — indices only grow); otherwise replace as today.
  A `Failed` while loading more clears the flag and shows the banner
  instead of nuking the loaded page.
- The hint label becomes a real "Load more" button, which is also a focus
  row; while `loading_more`, it renders as a spinner label.

## C. SBOM in CI (packaging.md item)

- `cargo cyclonedx` generates a CycloneDX SBOM per crate; a new `sbom` CI
  job runs it on ubuntu and uploads the JSON files as a build artifact.
- packaging.md's "SBOM / license audit" bullet updated: cargo-deny (already
  wired) covers licenses/advisories, cargo-cyclonedx covers the SBOM.
- Verified locally by installing cargo-cyclonedx and generating the SBOM.
