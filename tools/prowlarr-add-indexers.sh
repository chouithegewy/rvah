#!/bin/sh
# Add the Prowlarr indexers you name, with their default settings.
#
#   tools/prowlarr-add-indexers.sh [--dry-run] "Indexer name" ["Another" ...]
#
# Names are as Prowlarr lists them under Indexers > Add (matched without
# regard to case). Reads PROWLARR_URL and PROWLARR_API_KEY from the
# environment or the repository's .env. Prowlarr tests each indexer as it
# is added; one that fails (needs an account, or sits behind a Cloudflare
# challenge) is reported and skipped. --dry-run only shows what would be
# added.
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
from_env() { sed -n "s/^$1=//p" "$repo/.env" 2>/dev/null | tr -d '"'"'" | tail -n 1; }
url=${PROWLARR_URL:-$(from_env PROWLARR_URL)}
key=${PROWLARR_API_KEY:-$(from_env PROWLARR_API_KEY)}
[ -n "$url" ] && [ -n "$key" ] || { echo "Set PROWLARR_URL and PROWLARR_API_KEY (or put them in .env)." >&2; exit 1; }

dry=0
if [ "${1:-}" = "--dry-run" ]; then dry=1; shift; fi
[ $# -gt 0 ] || { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

schema=$(mktemp)
trap 'rm -f "$schema"' EXIT
curl -fsS -H "X-Api-Key: $key" "$url/api/v1/indexer/schema" > "$schema"

PROWLARR_URL=$url PROWLARR_API_KEY=$key DRY=$dry SCHEMA=$schema python3 - "$@" <<'EOF'
import json, os, sys, urllib.request, urllib.error

url, key = os.environ["PROWLARR_URL"].rstrip("/"), os.environ["PROWLARR_API_KEY"]
dry = os.environ["DRY"] == "1"
schema = json.load(open(os.environ["SCHEMA"]))
by_name = {d["name"].lower(): d for d in schema}

def call(method, path, body=None):
    req = urllib.request.Request(
        url + path, method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"X-Api-Key": key, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        return json.load(resp)

have = {i["name"].lower() for i in call("GET", "/api/v1/indexer")}
failed = 0
for wanted in sys.argv[1:]:
    definition = by_name.get(wanted.lower())
    if definition is None:
        print(f"? {wanted}: no indexer by that name in this Prowlarr")
        failed += 1
        continue
    name = definition["name"]
    if name.lower() in have:
        print(f"= {name}: already added")
        continue
    body = dict(definition, enable=True, appProfileId=1)
    if dry:
        print(f"~ {name}: would add ({definition.get('privacy', '?')}, {definition.get('protocol', '?')})")
        continue
    try:
        call("POST", "/api/v1/indexer", body)
        print(f"+ {name}: added")
    except urllib.error.HTTPError as e:
        failed += 1
        try:
            problems = json.load(e)
            reason = "; ".join(p.get("errorMessage", "") for p in problems) if isinstance(problems, list) else str(problems)
        except Exception:
            reason = f"HTTP {e.code}"
        print(f"! {name}: not added: {reason}")
sys.exit(1 if failed else 0)
EOF
