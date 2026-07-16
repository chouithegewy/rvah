# RVAH Repository Policy

Requirements for operating an add-on repository that RVAH installs from.

## Signing and integrity

- The repository holds one ed25519 signing key (the trust root users pin via
  `--pubkey`). Protect it accordingly; rotation means users must re-pin.
- `index.json` must be served alongside `index.json.sig` (hex ed25519 over
  the sha256 of the index bytes). Clients refuse unsigned or mismatched
  indexes before parsing them.
- Every listed package must carry its sha256 and package signature in the
  index. Re-sign and re-index whenever a package changes; never reuse a
  version number for different bytes (immutable version records, SDD §9).

## Review requirements

Before listing an add-on, the operator must verify:

1. `rvah validate` passes and the SDK range is honest.
2. The permission block is minimal: only domains the add-on actually
   contacts, a storage quota it needs, and secrets it uses. Wildcards must be
   justified.
3. No hidden data collection: add-ons must not phone home beyond their
   declared provider domains. Violations are removed and deprecated with a
   notice.
4. Licensing is clean: no copied Kodi (or other GPL-incompatible) code or
   assets in the package.

## Channels

- `stable` is the default channel and must only receive reviewed builds.
- `beta` (or other named channels) may receive faster builds; clients opt in
  per check/install with `--channel`.
- A version must never be listed in `stable` before `beta` if both are used.

## Deprecation and revocation

- To retire an add-on, keep its entry and set `deprecated` to a
  human-readable notice (ideally naming a replacement). Clients surface the
  notice in update checks even without a newer version.
- For a malicious or compromised add-on, remove the entry *and* publish a
  deprecated final entry so installed users see the warning. Clients refuse
  future installs automatically once the entry is gone (nothing to resolve).

## Update expectations

- Permission expansions in an update always require the user's re-approval
  client-side; write release notes accordingly.
- Keep `generated_at` accurate; clients may eventually use it for staleness
  warnings.
