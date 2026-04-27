# TLS

PgDoorman terminates TLS on the client side (clients → PgDoorman) and originates TLS on the server side (PgDoorman → PostgreSQL). The two sides are configured independently.

## Client-side TLS

Encrypt connections between client applications and PgDoorman.

### Modes

| Mode | Behavior |
| --- | --- |
| `disable` | Do not advertise TLS. Clients sending `SSLRequest` get `'N'` (rejected). |
| `allow` | Advertise TLS but accept plain TCP. |
| `require` | Require TLS. Plain connections are dropped after `SSLRequest` fails. |
| `verify-full` | Require TLS and a valid client certificate. Used for mTLS. |

`verify-full` is mTLS — the server verifies the client's certificate. Set up a client CA bundle with `tls_ca_cert`.

### Configuration

```yaml
general:
  tls_mode: "require"
  tls_certificate: "/etc/pg_doorman/tls/server.crt"
  tls_private_key: "/etc/pg_doorman/tls/server.key"
  tls_ca_cert: "/etc/pg_doorman/tls/client_ca.pem"   # only for verify-full
  tls_rate_limit_per_second: 100                       # optional handshake throttle
```

The certificate may be self-signed for development; production deployments typically use Let's Encrypt or an internal CA.

### Reload (client side)

Client-side certificates are loaded at startup. Changing them requires a process restart. There is no `SIGHUP` reload for client-side TLS.

For zero-downtime certificate rotation, see [Binary Upgrade](../tutorials/binary-upgrade.md).

### Cipher policy

Minimum TLS 1.2 enforced in the handshake. PgDoorman does **not** set an explicit cipher list — the effective ciphers come from the system OpenSSL build. If you need a hardened cipher list, configure it system-wide (`/etc/ssl/openssl.cnf`) or build OpenSSL with the policy you want.

Direct TLS handshake (PG17, no `SSLRequest`) is not supported. For TLS 1.3 cipher control or PG17 direct TLS, use PgBouncer 1.25+.

## Server-side TLS

Encrypt connections between PgDoorman and PostgreSQL backends. Added in 3.6.0.

### Modes

| Mode | Behavior |
| --- | --- |
| `disable` | Plain TCP. |
| `allow` (default) | Try plain first; if the server rejects, retry on a new socket with TLS. Matches `libpq sslmode=allow`. |
| `prefer` | Send `SSLRequest`; if the server says `'N'`, fall back to plain. |
| `require` | Require TLS. Fail if the server does not support it. |
| `verify-ca` | Require TLS and verify the server certificate against the configured CA. |
| `verify-full` | Require TLS, verify CA, and verify the server hostname against the certificate. |

`allow` is the default to keep backward compatibility — existing deployments where PostgreSQL has TLS configured automatically upgrade without config changes. New deployments wanting explicit guarantees should use `require` or `verify-full`.

### Configuration

```yaml
general:
  server_tls_mode: "verify-full"
  server_tls_ca_cert: "/etc/pg_doorman/tls/pg_ca_bundle.pem"

# Optional: client certificate for mTLS to PostgreSQL
  server_tls_certificate: "/etc/pg_doorman/tls/pg_client.crt"
  server_tls_private_key: "/etc/pg_doorman/tls/pg_client.key"
```

`server_tls_ca_cert` accepts a PEM bundle (multiple CA certificates concatenated). All are loaded.

### Hot reload

On `SIGHUP`, server-side certificates are re-read from disk. Existing connections keep using their original TLS context; new connections use the reloaded certificates. The reload is lock-free via `Arc<ArcSwap<...>>` — no connection drop, no handshake stall.

```bash
kill -HUP $(pidof pg_doorman)
```

This is the only TLS reload path. Client-side certificates do not reload on `SIGHUP`.

### mTLS to PostgreSQL

Set `server_tls_certificate` and `server_tls_private_key`. PostgreSQL must be configured with `ssl_ca_file` matching the client cert's signer, and the role must have `clientcert=verify-ca` (or `verify-full`) in `pg_hba.conf` on the PostgreSQL side.

## Observability

Three Prometheus series cover server-side TLS:

| Metric | Type | Purpose |
| --- | --- | --- |
| `pg_doorman_server_tls_connections` | gauge per pool | Number of active TLS connections to PostgreSQL. |
| `pg_doorman_server_tls_handshake_duration_seconds` | histogram per pool | Handshake duration buckets. |
| `pg_doorman_server_tls_handshake_errors_total` | counter per pool | Failed handshakes. Alert if non-zero rate. |

See [Prometheus reference](../reference/prometheus.md).

## Known limitations

- The `COPY` protocol over server TLS is not exercised by the BDD test suite. Behavior is expected to work but unverified.
- Cancel requests to the backend bypass server TLS — they use a fresh plain TCP connection. This matches PostgreSQL's protocol design (cancel is sent on a separate socket).
- Direct TLS handshake (PG17 fast handshake without `SSLRequest`) is not supported on either side.

## Where to next

- New cluster setup? See [Installation](../tutorials/installation.md).
- Rotating certificates? See [Binary Upgrade](../tutorials/binary-upgrade.md) and [Signals](../operations/signals.md).
- Hardening an existing deployment? Combine with [pg_hba.conf](../authentication/hba.md): force `hostssl` for non-local connections.
