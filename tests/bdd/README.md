# BDD Tests for pg_doorman

This directory contains Behavior-Driven Development (BDD) tests for pg_doorman using Cucumber framework.

## Running Tests

To run all BDD tests:

```bash
make test-bdd
```

## Debug Mode

To enable verbose output during test execution, set the `DEBUG` environment variable:

```bash
DEBUG=1 make test-bdd
```

When DEBUG mode is enabled, the following verbose output will be streamed to the console in real-time:

- **pg_doorman output**: stdout and stderr from the pg_doorman process will be streamed directly to the console
- **PostgreSQL logs**: The PostgreSQL log file (pg.log) will be streamed to the console with `[PG_LOG]` prefix in real-time (tail -f behavior)
- **Shell command output**: Any shell commands executed during tests will have their output streamed immediately (instead of waiting for the streaming threshold)
- **Rust debug logs**: Debug-level tracing logs from the Rust code will be displayed with detailed information including:
  - Target module
  - Thread IDs
  - Line numbers

### Example

```bash
# Run tests with debug output
DEBUG=1 make test-bdd

# Run tests normally (quiet mode)
make test-bdd
```

## Test Structure

- `features/` - Gherkin feature files describing test scenarios
- `main.rs` - Test runner entry point
- `world.rs` - Shared state structure for test scenarios
- `doorman_helper.rs` - Helper functions for starting/stopping pg_doorman
- `postgres_helper.rs` - Helper functions for managing PostgreSQL instances
- `shell_helper.rs` - Helper functions for executing shell commands
- `pg_connection.rs` - PostgreSQL connection utilities
- `extended.rs` - Extended query protocol test steps

## Benchmarks

Benchmarks are located in `tests/bdd/features/bench.feature` and can be run using the `@bench` tag:

```bash
make test-bdd TAGS=@bench
```

### Parameterization

You can parameterize benchmarks using environment variables:

- `BENCH_DOORMAN_WORKERS`: Number of worker threads for `pg_doorman` (default: 12)
- `BENCH_ODYSSEY_WORKERS`: Number of workers for `odyssey` (default: 12)
- `BENCH_PGBENCH_JOBS`: Global number of threads (`-j`) for `pgbench`. If set, it overrides all specific job settings.
- `BENCH_PGBENCH_JOBS_C1`: Number of threads for 1-client tests (default: 1)
- `BENCH_PGBENCH_JOBS_C40`: Number of threads for 40-client tests (default: 4)
- `BENCH_PGBENCH_JOBS_C120`: Number of threads for 120-client tests (default: 4)
- `BENCH_PGBENCH_JOBS_C500`: Number of threads for 500-client tests (default: 4)
- `BENCH_PGBENCH_JOBS_C10000`: Number of threads for 10,000-client tests (default: 4)
- `FARGATE_CPU`: AWS Fargate CPU units (optional, for reporting)
- `FARGATE_MEMORY`: AWS Fargate memory in MB (optional, for reporting)

## Requirements

- PostgreSQL installed and available in PATH
- Rust toolchain
- Sufficient shared memory available (for PostgreSQL instances)

## LDAP infrastructure

`make test-bdd TAGS=@ldap` runs real OpenLDAP fixtures directly through the
OpenLDAP CLI. This suite tests the directory infrastructure only: it neither
starts pg_doorman nor adds LDAP authentication to it.

Each named server owns a foreground `slapd`, two nonprivileged loopback ports,
an MDB database, the scenario's inline LDIF, a CA and server certificate. Instances are created
only by the LDAP startup steps below. Separate names have independent data and
certificates. The scenario cleanup hook stops and reaps every child
before deleting its files; ownership also covers errors or cancellation during
startup. Restart creates fresh certificates and restores the same initial LDIF
and base DN, discarding changes made since startup.
Ports are reserved together and a diagnosed bind collision triggers a bounded
retry; the suite deliberately exercises this race. The child has a file
descriptor limit of at most 4096 to bound slapd's startup allocation even when
Docker inherits a very large host limit.

The suite covers LDAP, LDAPS and mandatory StartTLS (`-ZZ`), exact
`invalidCredentials (49)` errors, bind identity, search results, rejection of an
unrelated CA, independent directories and clean restarts. The CA failure check
requires a certificate trust diagnostic and successful trusted binds immediately
before and after; an unrelated connection failure cannot satisfy it.

The Nix image and development shell provide `LDAP_SLAPD_BIN` and
`LDAP_SCHEMA_DIR`, plus `slaptest`, `slapadd`, `slapdn`, `ldapwhoami`, `ldapsearch` and
`ldapmodify` on PATH. No host daemon, privileged port, Docker socket or system
trust-store changes are needed. Setup and CLI commands have deadlines. Passwords
are synthetic fixture data; CLI credentials use private files, never argv or
shell interpolation, and captured diagnostics redact them. Bind steps take
literal synthetic passwords with `with password "..."`. Inline LDIF and
literal passwords are visible in scenario output, so use test data only. LDAP
client defaults from the host are disabled; TLS verification uses the scenario
CA explicitly.

After changing `tests/nix/flake.nix`, build the matching local image:

```bash
LDAP_IMAGE_TAG="flake-$(cat tests/nix/flake.nix tests/nix/flake.lock | sha256sum | cut -c1-16)"
make local-build IMAGE_TAG="$LDAP_IMAGE_TAG"
make test-bdd TAGS=@ldap
```

Run these commands from the repository root. The local build uses a path flake
so an isolated Git worktree does not need its parent `.git` mounted in Docker.
It preserves `flake.lock`. CI runs a separate `@ldap` matrix entry.

`world.vars` exposes these placeholders for the example name `primary`:

- `${LDAP_PRIMARY_URL}` and `${LDAP_PRIMARY_LDAPS_URL}`
- `${LDAP_PRIMARY_PORT}` and `${LDAP_PRIMARY_LDAPS_PORT}`
- `${LDAP_PRIMARY_BASE_DN}` and `${LDAP_PRIMARY_ADMIN_DN}`
- `${LDAP_PRIMARY_CA_CERT}`

### Inline directory data

Every startup requires a nonempty inline LDIF docstring containing the complete
initial database, including its base entry. `slapadd` validates and imports
exactly these records:

```gherkin
Given LDAP server "primary" is started with base DN "dc=team,dc=test" and LDIF:
  """
  dn: dc=team,dc=test
  objectClass: dcObject
  objectClass: organization
  dc: team
  o: Team directory

  dn: uid=ren,dc=team,dc=test
  objectClass: inetOrgPerson
  uid: ren
  cn: Ren
  sn: Example
  mail: ren@team.test
  userPassword: test-password
  """
Then LDAP server "primary" accepts bind over "LDAPS" as "uid=ren,${LDAP_PRIMARY_BASE_DN}" with password "test-password"
And LDAP server "primary" search over "StartTLS" at "${LDAP_PRIMARY_BASE_DN}" for "(&(uid=ren)(mail=ren@team.test))" returns DN "uid=ren,${LDAP_PRIMARY_BASE_DN}"
When LDAP server "primary" password for "uid=ren,${LDAP_PRIMARY_BASE_DN}" becomes "temporary-password"
And LDAP server "primary" is restarted with fresh data
Then LDAP server "primary" accepts bind over "LDAP" as "uid=ren,${LDAP_PRIMARY_BASE_DN}" with password "test-password"
```

`Given LDAP server "primary" is started with LDIF:` uses the default base DN
`dc=example,dc=test`; this config default creates no database records.
`Given LDAP server "primary" is started after a port collision with LDIF:`
requires the same inline docstring and exercises the port retry path.
Inline LDIF, the base DN and bind/search parameters support existing
`${VARIABLE}` placeholders. The new server's own placeholders become available
after startup. Restart reloads the original expanded values even if their source
variables have since disappeared. The fixture includes the core, cosine and
inetOrgPerson schemas. Its root bind DN is `cn=admin,<base DN>` with the synthetic
password `fixture-admin-password`. This is slapd's technical `rootdn`/`rootpw`
configuration, not an imported database record.

The imported LDIF is stored in a private file. Missing or empty docstrings fail
explicitly; invalid LDIF is rejected by `slapadd` before launch. Import failure
diagnostics do not echo LDIF lines, which may contain plaintext passwords.
Base DNs preserve LDAP escaping when quoted in `slapd.conf`; DN arguments reject
actual CR, LF and NUL characters. DN identity checks use OpenLDAP's `slapdn`
representation, including escaped quotes and backslashes.

Names use lowercase ASCII letters, digits and underscores. Stop removes the
placeholders; restart replaces them. The configuration template lives in
`fixtures/ldap/slapd.conf`; all directory records live inline in their scenarios.
Lifecycle, bounded command execution and reusable steps live in
`ldap_helper.rs`. Keep directory assertions in shared steps. A future production
LDAP client or pg_doorman auth feature belongs in a separate change.
