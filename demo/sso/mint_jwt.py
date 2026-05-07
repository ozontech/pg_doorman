"""Mint a short-lived RS256 JWT signed with demo/sso/keys/sso-private.pem.

Argv:
    mint_jwt.py [username] [audience] [ttl_seconds]

Environment:
    SSO_GROUPS         Comma-separated group names to put into the
                       JWT (default: empty, no group claim).
    SSO_GROUPS_CLAIM   Claim name for the group list (default:
                       "groups"). Mirror this in pg_doorman.toml's
                       [web].sso_groups_claim.

Prints the JWT on stdout. This script is for the demo only; the
private key in this directory is throwaway and exists in version
control so the demo runs without external setup. Never deploy this
key.
"""

from __future__ import annotations

import os
import sys
import time
from pathlib import Path

import jwt


def main() -> int:
    username = sys.argv[1] if len(sys.argv) > 1 else "alice"
    audience = sys.argv[2] if len(sys.argv) > 2 else "pg_doorman"
    ttl = int(sys.argv[3]) if len(sys.argv) > 3 else 3600

    private_key = Path("/keys/sso-private.pem").read_text()
    payload = {
        "preferred_username": username,
        "sub": f"demo-{username}",
        "aud": audience,
        "exp": int(time.time()) + ttl,
        "iat": int(time.time()),
    }

    groups_csv = os.environ.get("SSO_GROUPS", "").strip()
    if groups_csv:
        claim_name = os.environ.get("SSO_GROUPS_CLAIM", "groups")
        payload[claim_name] = [g.strip() for g in groups_csv.split(",") if g.strip()]

    token = jwt.encode(payload, private_key, algorithm="RS256")
    sys.stdout.write(token)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
