# pg_doorman SSO demo

A self-contained docker-compose stack that boots pg_doorman with SSO
enabled, a throwaway RSA keypair, and a helper container that mints
short-lived JWTs. There is no real OIDC provider here — standing up
oauth2-proxy or Keycloak is out of scope for a demo. The commands
below exercise the pg_doorman side: the server validates the JWT,
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

The keypair in `keys/` is checked into version control on purpose so
the demo runs without out-of-band setup. It is throwaway. Do not copy
it into a real deployment.

## Boot

```bash
cd demo/sso
docker compose up -d
docker compose logs -f pg_doorman   # optional: watch the access log
```

Each request emits one line on the `pg_doorman::web::access` target.
Tail the log to confirm the demo is live.

## Prove the SSO path

Mint a JWT for `alice` (read-only `Sso` role), then probe the API:

```bash
JWT=$(docker compose run --rm mint-jwt)
echo "$JWT" | head -c 80; echo "..."
curl -s -H "Authorization: Bearer $JWT" http://localhost:9127/api/auth/config | jq
# {
#   "sso_enabled": true,
#   "sso_proxy_url": "https://sso.example.com/oauth2/start",
#   "sso_admin_groups_configured": true,
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

Basic still works and grants Admin:

```bash
curl -i -X POST -u admin:admin-demo \
     http://localhost:9127/api/admin/reload
# HTTP/1.1 200 OK
```

## Promote an SSO user to Admin via group claim

`pg_doorman.toml` in this demo sets:

```toml
sso_groups_claim = "groups"
sso_admin_groups = ["pg-doorman-admins"]
```

Mint a JWT that carries the group; the same `Authorization: Bearer`
flow now resolves to `Admin`:

```bash
ADMIN_JWT=$(SSO_GROUPS=pg-doorman-admins docker compose run --rm -e SSO_GROUPS mint-jwt)
curl -s -H "Authorization: Bearer $ADMIN_JWT" http://localhost:9127/api/auth/config | jq '.current_user'
# {
#   "username": "alice",
#   "source": "sso",
#   "role": "admin"
# }

curl -i -X POST -H "Authorization: Bearer $ADMIN_JWT" \
     http://localhost:9127/api/admin/reload
# HTTP/1.1 200 OK
```

The access log records the same user with the role flip:

```
INFO pg_doorman::web::access method=POST path=/api/admin/reload status=200 auth_role=admin auth_source=sso auth_user=alice
```

Removing `pg-doorman-admins` from `[web].sso_admin_groups` (or minting
without `SSO_GROUPS`) drops the same user back to `Sso` on the next
request.

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
   `admin` / `admin-demo` via the Basic form. Or repeat steps 1–4
   with a token minted via `SSO_GROUPS=pg-doorman-admins`; the SPA
   then renders the admin surface and pool action buttons reappear.

## Tear down

```bash
docker compose down -v
```

## Adapting to a real SSO proxy

Replace the throwaway key with the public key your SSO provider uses
to sign tokens, point the audience at whatever the provider puts into
the `aud` claim, and (optionally) tighten the allowlist:

```toml
[web]
sso_enabled = true
sso_proxy_url = "https://sso.example.com/oauth2/start"
sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
sso_audience = ["pg_doorman"]
sso_allowed_users = ["alice", "bob"]
sso_groups_claim = "groups"
sso_admin_groups = ["pg-doorman-admins"]
```

The proxy must redirect back to pg_doorman with the JWT in
`?token=<jwt>` (or set the `sso_access_token` cookie on the same
domain). The SPA captures the parameter into `localStorage` and
continues with the existing flow.

### Keycloak

The Keycloak realm signs JWTs with the realm's RSA key. Export the
public half once per realm into a PEM file pg_doorman can read:

1. In the Keycloak admin UI: select the realm → **Realm settings** →
   **Keys** → row with `Algorithm = RS256` and `Use = SIG` →
   **Public key** → **Copy** (Keycloak shows the base64-DER body
   without the PEM header).
2. Wrap the copied body into a real PEM file:

   ```bash
   {
     echo "-----BEGIN PUBLIC KEY-----"
     fold -w 64    # paste the body, then Ctrl-D
     echo "-----END PUBLIC KEY-----"
   } > /etc/pg_doorman/sso-public.pem
   ```

3. Or fetch the key non-interactively from the JWKS endpoint and
   convert it to PEM with `jq` and `openssl`:

   ```bash
   REALM=https://kc.example.com/realms/operators
   curl -s "$REALM/protocol/openid-connect/certs" \
     | jq -r '.keys[] | select(.alg=="RS256") | "-----BEGIN CERTIFICATE-----\n" + .x5c[0] + "\n-----END CERTIFICATE-----"' \
     | openssl x509 -pubkey -noout \
     > /etc/pg_doorman/sso-public.pem
   ```

4. In `pg_doorman.toml`:

   ```toml
   [web]
   sso_enabled = true
   sso_proxy_url = "https://kc.example.com/realms/operators/protocol/openid-connect/auth"
   sso_public_key_file = "/etc/pg_doorman/sso-public.pem"
   sso_audience = ["pg_doorman"]   # the client_id you configured on Keycloak
   sso_groups_claim = "groups"     # Keycloak default with the "groups" mapper enabled
   sso_admin_groups = ["pg-doorman-admins"]
   ```

5. On the Keycloak side, add a **Group Membership** mapper to the
   client (Clients → your client → **Mappers**) so the access token
   carries `"groups": [...]`. Without that mapper Keycloak issues
   tokens without the claim and every operator stays on `Sso`.

When Keycloak rotates the realm signing key, repeat step 1 (or 3) and
issue `RELOAD` to make pg_doorman pick up the new PEM without a
restart.

The complete reference is in `documentation/en/src/guides/web-ui.md`.
