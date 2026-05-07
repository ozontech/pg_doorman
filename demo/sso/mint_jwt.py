"""Mint a short-lived RS256 JWT signed with demo/sso/keys/sso-private.pem.

Argv:
    mint_jwt.py [username] [audience] [ttl_seconds]

Prints the JWT on stdout. This script is for the demo only; the
private key in this directory is throwaway and exists in version
control so the demo runs without external setup. Never deploy this
key.
"""

from __future__ import annotations

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
    token = jwt.encode(payload, private_key, algorithm="RS256")
    sys.stdout.write(token)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
