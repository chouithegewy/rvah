# needs-auth

Demonstrates the authenticated-provider shape: declares the `auth` and
`account_status` capabilities, one named secret (`oauth_token`) that the
device-code flow stores in the host vault, a CDN wildcard domain, and typed
settings (enum + bool).

Key points to copy:

- `secrets = ["oauth_token"]` — `secret_get("oauth_token")` works only
  because it is declared here; any other name is `PermissionDenied`.
- `resolve` should return `ResolveResult::AuthRequired { provider_hint }`
  when the token is missing/expired; the UI then routes the user into the
  sign-in flow (UC-04).
- `*.cdn.example.com` matches `a.cdn.example.com`, never `cdn.example.com`
  itself — list both if you need both.
