# Phase 4: Ecosystem — Design

Per SDD §13 Phase 4: developer CLI, docs, example add-ons, repository policy,
update channels, telemetry opt-in if desired.

## Scope decisions

- **Developer CLI**: extend `rvah` with the publisher/consumer repo workflow:
  - `rvah repo-index <packages-dir> -o index.json --key signing.key
    [--base-url URL]` — scan `.rvah` packages + their `.sig` files, emit a
    signed `RepoIndex` (channel from each package's manifest? No — channel is
    repo metadata: default `stable`, override per package via
    `--channel <id>=<channel>`).
  - `rvah repo-check --index <url-or-path> --pubkey verify.pub --data-dir DIR
    [--channel stable]` — list available updates + deprecation notices.
  - `rvah repo-install <addon-id> --index … --pubkey … --data-dir DIR
    [--channel stable] [--approve]` — fetch, verify, install/update.
  - File-based indexes are served through a `FileTransport` (a `Transport`
    impl reading `file://`/plain paths) so the whole flow works offline and
    in tests.
- **Example add-ons**: `examples/addons/hello-catalog/` and
  `examples/addons/needs-auth/` — complete manifests + READMEs usable with
  `rvah validate/package/install` against the simulator.
- **Docs**: `docs/developer-guide.md` (SDK model, manifest reference,
  packaging/signing, publishing, simulator workflow) and
  `docs/repository-policy.md` (what a repo operator must enforce: signing,
  review, deprecation, channels, revocation).
- **Update channels**: already modeled in `RepoEntry.channel` (Phase 3);
  exposed via `--channel` flags here.
- **Telemetry**: explicitly **not implemented**. SDD says "if desired" — the
  decision, recorded in the developer guide, is no telemetry in RVAH; the
  diagnostics export (user-initiated, redacted) is the only data egress.

## Testing

CLI tests spawn the binary: `repo_index_and_install_flow` (keygen → package →
repo-index → repo-check shows nothing → bump version → re-index →
repo-check shows update → repo-install --approve upgrades), and
`repo_install_rejects_wrong_pubkey`.
