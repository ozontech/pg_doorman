# PAM Authentication

PgDoorman delegates client authentication to a PAM service on the host. Use this for OS-integrated authentication (LDAP via `pam_ldap`, Kerberos, local PAM modules) without putting per-user credentials in the pool config.

PAM is Linux-only. The pre-built binaries ship with PAM support enabled.

## Configuration

```yaml
pools:
  mydb:
    server_host: "127.0.0.1"
    server_port: 5432
    pool_mode: "transaction"
    users:
      - username: "alice"
        auth_pam_service: "pg_doorman"
        server_username: "alice"
        server_password: "md5..."
        pool_size: 20
```

`auth_pam_service` is the name of the PAM service file under `/etc/pam.d/`. PgDoorman does not validate the service name at startup — make sure the file exists.

The `password` field is omitted because PAM does the verification. `server_username` and `server_password` are required: PAM only authenticates the client to PgDoorman; PgDoorman still needs credentials for the backend connection.

## Example PAM service

`/etc/pam.d/pg_doorman`:

```
auth     required pam_unix.so
account  required pam_unix.so
```

For LDAP-backed authentication:

```
auth     required pam_ldap.so
account  required pam_ldap.so
```

Configure `pam_ldap` in `/etc/ldap.conf` (or `/etc/nslcd.conf`) per your environment.

## Dispatch order

PAM is checked after Talos and HBA Trust, but before any password-based method. If a user has both `auth_pam_service` and a static `password` (MD5, SCRAM, or JWT prefix), PAM wins.

See [Overview](overview.md#dispatch-order).

## Caveats

- PAM blocks the worker thread during the authentication call. If your PAM stack does network calls (LDAP, Kerberos), expect occasional latency spikes.
- `pam_unix.so` requires read access to `/etc/shadow` — usually only `root`. Run PgDoorman as a user with the right group membership, or use a different PAM module.
- PAM does not support SCRAM passthrough. The backend connection always uses `server_username` and `server_password`.
- For LDAP without PAM machinery, PgDoorman has no native LDAP support. Use Odyssey or PgBouncer 1.25+ for that.
