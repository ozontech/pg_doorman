# JWT Authentication

Authenticate clients with a JSON Web Token signed by an external identity provider. PgDoorman verifies the token's RSA-SHA256 signature using a public key from disk, checks the `preferred_username` claim, and forwards the connection to PostgreSQL with a configured backend identity.

This fits service-to-database access where short-lived tokens are issued by an OIDC provider, Vault, or an internal token service.

## Configuration

Generate (or obtain) an RSA public key and reference it in the user's `password` field with the `jwt-pkey-fpath:` prefix:

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "billing-service"
        password: "jwt-pkey-fpath:/etc/pg_doorman/jwt-public.pem"
        server_username: "billing"
        server_password: "md5..."
        pool_size: 40
```

Whatever the client sends as a password is treated as a JWT and verified against `/etc/pg_doorman/jwt-public.pem`. The token must:

- Be signed with RS256 (RSA-SHA256). HS256 and EC variants are not supported.
- Have a `preferred_username` claim equal to the configured `username` (`billing-service` in the example above).
- Pass standard `exp` and `nbf` validation.

The backend connection is opened as `billing` with the `server_password` hash. The client's identity (`billing-service`) is decoupled from the database identity (`billing`).

## Generating a key pair

```bash
openssl genrsa -out jwt-private.pem 2048
openssl rsa -in jwt-private.pem -pubout -out jwt-public.pem
```

Keep `jwt-private.pem` on the token issuer. Distribute `jwt-public.pem` to PgDoorman.

## Issuing a token

Any RS256 JWT library works. Example with Python (`PyJWT`):

```python
import jwt
import time

private_key = open("jwt-private.pem").read()

token = jwt.encode(
    {
        "preferred_username": "billing-service",
        "iat": int(time.time()),
        "exp": int(time.time()) + 300,  # 5 minutes
    },
    private_key,
    algorithm="RS256",
)
```

The client connects to PgDoorman with `user=billing-service` and `password=<token>`. Most PostgreSQL drivers accept any string in the password field.

## Token rotation

PgDoorman reads the public key file once at startup and on `SIGHUP`. Rotate the key by:

1. Add the new public key to a second user entry with a parallel name.
2. Reload (`kill -HUP`).
3. Switch the issuer to the new key.
4. Remove the old user entry after the grace period.

Or, simpler, replace the file in place and `SIGHUP`. There is no support for multiple keys per user.

## Dispatch order

JWT is the lowest-priority password format: PgDoorman checks `SCRAM-SHA-256$` and `md5` prefixes first, then `jwt-pkey-fpath:`. In practice this only matters if you use a placeholder password — set `auth_pam_service` for PAM, or use the `jwt-pkey-fpath:` prefix exclusively for JWT users.

If the same user has both `auth_pam_service` and a `jwt-pkey-fpath:` password, PAM wins.

See [Overview](overview.md#dispatch-order).

## Caveats

- The `preferred_username` claim must match exactly. There is no claim mapping or aliasing.
- No JWKS endpoint support: the public key must be on disk.
- No issuer (`iss`) or audience (`aud`) checks. If you need them, terminate JWT at a sidecar and translate to passthrough.
- For client identity carrying database role information (e.g., `read_only` vs `read_write`), see [Talos](talos.md).
