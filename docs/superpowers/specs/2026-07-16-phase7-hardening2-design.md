# Phase 7: Hardening II — Design

Continuation from the Phase 6 backlog (README "Deferred" + security-review
gaps). Three items, all verifiable on this machine; the Windows mpv transport
stays deferred (still no way to test named pipes here).

## A. DNS pinning + non-global address policy for add-on HTTP

Closes the security-review gap: "a permitted name re-resolving to a different
address between checks is not detected", and the DNS-rebinding corollary
(allowlisted name resolving to 127.0.0.1/RFC-1918).

- `Resolve` trait in `rvah-net`: `resolve(&host) -> Result<Vec<IpAddr>,
  NetError>`; `SystemResolver` uses `std::net::ToSocketAddrs`. Tests use a
  scripted `MockResolver`.
- `PermissionedClient::with_resolver(Box<dyn Resolve>)` enables the stage;
  `new` keeps today's behavior so existing tests stand. Production wiring
  passes `SystemResolver`.
- Per hop, after the allowlist check: resolve the host, then reject any
  non-global address (loopback, RFC-1918, link-local, ULA, unspecified,
  multicast, IPv4-mapped forms) with new `NetError::AddressDenied(host, ip)`.
  Exception: a URL host that is an IP literal and passed the allowlist by
  exact match is an explicit user grant and is permitted as-is.
  Empty resolution → `NetError::ResolveFailed(host)`.
- Pinning: `Transport::execute_pinned(req, addrs)` (default: delegate to
  `execute`). `UreqTransport` installs a `ureq::Resolver` backed by a shared
  host→addrs map that `execute_pinned` populates, so the connection goes to
  exactly the checked addresses — no second resolution. `MockTransport`
  records pinned addrs for assertions.
- Docs: security-review gap marked closed; README deferred list updated.

## B. D-pad focus navigation (FR-045 completion)

All list screens render vertical rows, so focus is a 1-D model per screen:

- `UiState.focus: Option<usize>`; pure `focus_targets(state) ->
  Vec<FocusEntry { label, intent }>` for Home (open per enabled add-on),
  Browse (directories, media, next-page), Search results.
- `UiIntent::{FocusNext, FocusPrev, Activate}`: focus moves clamp to the
  list; `Activate` dispatches the focused entry's intent through `intent()`.
- `NavKey::{Up, Down, Select}` map to those in `key_intent` (only on screens
  with targets); `reduce` clears focus whenever the screen's data changes.
- egui: ArrowUp/ArrowDown/Enter translate when not typing; the focused row
  renders highlighted. Unit tests cover targets, movement, activation, reset.

## C. Cookie redaction patterns (security-review note)

`Redactor`'s built-in pattern set grows `Cookie`/`Set-Cookie` values (and
generic `cookie=`-style pairs), pinned by a test, so the log surface stays
clean if the bridge ever grows cookie support.
