# Security Policy

RVAH treats package verification, add-on isolation, permission enforcement,
secret handling, networking, update integrity, and player input validation as
security boundaries.

## Supported versions

RVAH is currently pre-1.0. Security fixes are made on the latest revision of
the default branch. Older commits and locally modified builds are not
supported release lines.

## Reporting a vulnerability

Please use the repository's **Security** tab to open a private vulnerability
report. Do not open a public issue, discussion, or pull request containing
exploit details, credentials, private user data, or an unpatched proof of
concept.

Include the affected component and revision, impact, reproduction steps, and
any suggested mitigation. Remove real service tokens, signing keys, account
identifiers, and media URLs from the report. If GitHub private vulnerability
reporting is not yet enabled, contact the repository owner privately and ask
for a secure reporting channel before sending details.

Reports will be acknowledged when received, reproduced where possible, and
coordinated toward a fix and disclosure. No response-time or bounty guarantee
is offered.

## Scope

Examples of in-scope issues include sandbox escapes, signature or consent
bypasses, archive extraction attacks, secret or diagnostic leakage, network
allowlist bypasses, unsafe native-player input, and unauthorized update paths.
That includes an add-on opening a web page without declaring `web = true` in
its manifest (shown at install), and the web player leaving the page it was
given (pop-ups, new windows, navigation to another site).
Third-party service outages, account disputes, unsupported DRM, and findings
that require committing a user's own secrets are generally out of scope.
