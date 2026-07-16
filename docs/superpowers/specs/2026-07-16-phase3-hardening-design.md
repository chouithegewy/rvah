# Phase 3: Hardening — Design

Per SDD §13 Phase 3: signing/repository, quotas, CI matrix, fuzz/property
tests, security review, packaging. Quotas and package signing shipped in
Phase 1; this phase adds the repository layer (FR-008), portability and CI,
property-based tests for the security-critical parsers, a documented security
review against SDD §9, and release packaging settings.

## rvah-repo (FR-008)

Single-trust-root MVP: one ed25519 repo key signs both the index and the
packages it lists.

- **Index format** (JSON): `RepoIndex { name, generated_at, addons: [RepoEntry] }`,
  `RepoEntry { id, version, channel ("stable"|"beta"), url, sha256, sig, deprecated? }`.
- **Fetch + verify**: index fetched over a `Transport` (host-level, no add-on
  allowlist involved); detached signature at `<index-url>.sig` verified
  (ed25519 over sha256 of the index bytes) before parsing — same scheme as
  packages. Tampered index → `RepoError::BadSignature`.
- **Update checks**: compare installed versions against index entries in the
  selected channel; report available upgrades and deprecation notices
  (deprecation satisfies the FR-008 "notices" clause; channels serve Phase 4).
- **Install/update**: download package bytes, check sha256 against the entry,
  write to a temp file, then hand to `AddonManager::install`/`update` which
  re-verifies the package signature and permission diffs (TC-ADD-005 path
  unchanged).

## Portability + CI

- `rvah-player-mpv` is Unix-only (Unix-socket IPC): gate the crate contents
  with `#[cfg(unix)]` so Windows builds an empty crate; `rvah-app` falls back
  to the mock player off-Unix.
- GitHub Actions matrix: {ubuntu, macos, windows} × stable — fmt, clippy
  `-D warnings`, tests; a separate ubuntu job runs `--all-features` (wasm).

## Property tests (proptest)

Chosen over cargo-fuzz (needs nightly + libFuzzer); deterministic in CI.

- `Manifest::parse` never panics on arbitrary input.
- Allowlist wildcard safety: `*.ex.com` permits only hosts that end with
  `.ex.com` on a label boundary and never IP literals.
- Redactor: output never contains a registered secret (len ≥ 4).
- `Protocol::from_url` never panics.

## Security review + packaging

- `docs/security-review.md`: SDD §9 risk table → implementation → covering
  tests → known gaps (SBOM tooling, OS keychain, wasm host-call breadth).
- Release profile in the workspace root (`thin` LTO, strip, 1 codegen unit)
  and `docs/packaging.md` build instructions.
