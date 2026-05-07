# pg_doorman SSO demo

A self-contained docker-compose stack that boots pg_doorman with SSO
enabled, a throwaway RSA keypair, and a helper container that mints
short-lived JWTs. There is no real OIDC provider here. Standing up
oauth2-proxy or Keycloak is out of scope for a demo. The following
commands exercise the pg_doorman side: the server validates the JWT,
the SPA respects the role, and the access log records the user.

## Layout

```
demo/sso/
├── docker-compose.yml    # postgres + pg_doorman + mint-jwt helper
├── pg_doorman.toml       # SSO config; admin user is admin / admin-demo
├── mint_jwt.py           # PyJWT helper that runs inside `mint-jwt`
└── keys/
    ├── sso-public.pem    # injected into pg_doorman as sso_public_key_file
    └── sso-private.pem   # used only by mint_jwt; never deploy this
```

The keypair in `keys/` is checked into version control on purpose
so the demo runs without out-of-band setup. It is throwaway. Do not
copy it into a real deployment.

## Boot

```bash
cd demo/sso
docker compose up -d
docker compose logs -f pg_doorman   # optional: watch the access log
```

Each request emits one line on the `pg_doorman::web::access` target.
Tail the log to confirm the demo is live.

## Prove the SSO path

Mint a JWT for `alice`, then probe the API:

```bash
JWT=$(docker compose run --rm mint-jwt)
echo "$JWT" | head -c 80; echo "..."
curl -s -H "Authorization: Bearer $JWT" http://localhost:9127/api/auth/config | jq
# {
#   "sso_enabled": true,
#   "sso_proxy_url": "https://sso.example.com/oauth2/start",
#   "current_user": { "username": "alice", "source": "sso", "role": "sso" }
# }
```

Read-only personal data works:

```bash
curl -s -H "Authorization: Bearer $JWT" http://localhost:9127/api/logs | jq '.entries | length'
```

Writes are denied with 403, not 401:

```bash
curl -i -X POST -H "Authorization: Bearer $JWT" \
     http://localhost:9127/api/admin/reload
# HTTP/1.1 403 Forbidden
# {"error":"forbidden","message":"admin role required"}
```

Anonymous access to a personal-data path is rejected with 401:

```bash
curl -i http://localhost:9127/api/logs
# HTTP/1.1 401 Unauthorized
```

Basic still works and grants admin:

```bash
curl -i -X POST -u admin:admin-demo \
     http://localhost:9127/api/admin/reload
# HTTP/1.1 200 OK
```

## Browser flow

The demo lacks a real SSO proxy, so the redirect step is simulated:

1. Mint a JWT: `docker compose run --rm mint-jwt > /tmp/jwt`.
2. Open `http://localhost:9127/`.
3. In the browser devtools console, run:

   ```js
   localStorage.setItem("pgdoorman.sso-token", "<paste contents of /tmp/jwt>");
   ```

4. Reload the page. The sidebar shows `sso: alice`. Logs and Caches
   are visible. Pool action buttons are hidden because the SSO role
   does not have admin privileges. Forcing a request to one (devtools
   or curl) returns the `403 admin role required` banner.

5. To get full access, sign out (sidebar footer) and sign in with
   `admin` / `admin-demo` via the Basic form.

## Tear down

```bash
docker compose down -v
```

## Adapting to a real SSO proxy

In production:

1. Replace `keys/sso-public.pem` with the public key your SSO proxy
   uses to sign tokens. `oauth2-proxy --signing-key`, Keycloak realm
   keys, and Authelia all provide this.
2. Set `[web].sso_proxy_url` to the actual sign-in URL the proxy
   serves (`https://sso.example.com/oauth2/start` for oauth2-proxy).
3. Set `[web].sso_audience` to the audience the proxy puts into the
   `aud` claim. The demo uses `["pg_doorman"]`.
4. Optionally restrict `[web].sso_allowed_users` from `["*"]` to the
   list of operators allowed to read.
5. The proxy must redirect back to pg_doorman with the JWT in
   `?token=<jwt>` (or set the `sso_access_token` cookie on the same
   domain). The SPA captures the parameter into localStorage and
   continues with the existing flow.

The complete reference is in `documentation/en/src/guides/web-ui.md`.
