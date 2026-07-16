# Phase 2: MVP UX — Design

Per SDD §13 Phase 2: desktop UI, settings, permission prompts, auth flow,
player controls, diagnostics export. Covers FR-040..044, FR-046 (FR-045
remote-first focus stays post-MVP).

## Decisions

- **Toolkit: egui/eframe** (user choice), hidden behind `rvah-ui` per SDD §3.
  UI code is split into pure view-model logic (testable) and thin egui views.
- **Player: mpv via JSON IPC** (user chose "include mpv now"). `rvah-player-mpv`
  spawns system `mpv --input-ipc-server=<socket>` and speaks the JSON IPC
  protocol over a Unix socket, implementing `PlayerBackend`. No libmpv link:
  compiles everywhere, tested against a fake IPC server (mpv is not installed
  on the dev machine). Video renders in mpv's own window; RVAH drives it.
  Windows named pipes are out of scope (unix only, documented).
- **Runtime in the app:** Phase 2 keeps the Phase 1 runtime story — installed
  add-ons run on the in-process fixture runtime (simulator). Wiring the wasm
  runtime into the app is Phase 3+ work.

## Architecture

```
rvah-app (bin)          rvah-ui (lib)                    rvah-core
┌──────────────┐   ┌───────────────────────────┐   ┌────────────────┐
│ bootstrap:   │   │ views (egui, thin)        │   │ AppCore<B>     │
│ data dir, db,│──▶│   ▼ UiIntent              │   │ runs on a      │
│ addons,      │   │ state.rs (pure reducers)  │   │ dedicated      │
│ backend pick │   │   ▼ CoreRequest  ▲ CoreEvent │ core thread    │
└──────────────┘   │ service.rs (channels) ────┼──▶│                │
                   └───────────────────────────┘   └────────────────┘
```

- `rvah-ui::service`: `CoreService` owns `AppCore<B>` on its own thread;
  UI sends `CoreRequest`, receives `CoreEvent` over mpsc channels. This keeps
  the UI responsive and the domain model toolkit-agnostic.
- `rvah-ui::state`: `UiState` + pure `reduce(state, CoreEvent)` and
  `intent(state, UiIntent) -> Option<CoreRequest>` functions — unit-tested
  without egui.
- `rvah-ui::views`: one egui pass rendering the current `Screen`; emits
  `UiIntent`s only. Untested (thin by construction).
- `rvah-app`: arg parsing (`--data-dir`), db/manager bootstrap, runtime
  registration for installed add-ons, backend selection (mpv if present on
  PATH, else MockPlayer with an on-screen notice), eframe launch.
- `rvah-player-mpv`: `MpvIpc` protocol client (connect, request/response with
  request_id, observed properties → `PlayerEvent`s) + `MpvBackend`
  (spawn + connect + `PlayerBackend` impl). Header policy: only
  user-agent/referer are forwarded (already validated upstream).

## Screens (FR map)

| Screen | Contents | FRs |
|---|---|---|
| Home | installed add-ons, favorites, recent (watch state), settings/diagnostics entries | FR-040 |
| Browse | directory/media grid from `Page`, badges/labels; loading/empty/error/partial states; back/forward | FR-041, 042, 043 |
| Search | per-add-on query + aggregated search-all | FR-044 |
| Player | controls: play/pause/seek bar/volume/stop; position/duration; error banner | FR-062, 066 |
| Permissions | per-add-on grant view: network/storage/secrets; revoke + re-approve | FR-046 |
| Auth | device-code flow: user code + URL display, poll status | UC-04 |
| Settings | per-add-on settings from manifest schema (string/enum/bool) | FR-102 |
| Diagnostics | recent redacted events + "export bundle" (JSON file) | FR-103, Phase 2 deliverable |

Install-time permission prompts stay in the CLI for now (UC-01 CLI flow);
the app's Permissions screen covers inspect/revoke (FR-046).

## Error handling

`CoreEvent::Failed { class: ErrorClass, message, retryable }` renders as: 
UserActionable → inline banner with action hint; Recoverable → banner with
Retry button (re-issues last request); Developer → banner + "see diagnostics";
FatalHost → modal. Loading states are explicit in each screen's data enum
(`Loading | Loaded(_) | Failed(_)`), satisfying FR-043.

## Testing

- `rvah-player-mpv`: fake IPC server (Unix socket in tempdir) asserting
  protocol: loadfile with start/headers, pause toggles, seek, quit; property
  change events map to `PlayerEvent::{Loaded, Progress, Ended}`.
- `rvah-ui`: reducer/intent unit tests: browse loading→loaded, error classes
  render state, nav back/forward, search flow, permission revoke round-trip
  (with a real in-memory `AppCore<MockPlayer>` behind `CoreService`).
- `rvah-app`: bootstrap smoke test (build core service headless from a temp
  data dir).
- egui rendering: not unit-tested; kept intentionally thin.

## Out of scope

FR-045 (remote focus), install-permission prompt in GUI, wasm runtime in the
app, Windows IPC, subtitle rendering, DASH.
