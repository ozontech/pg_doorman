@rust @rust-2 @auth-query @auth-query-init-race
Feature: Dynamic pool survives GC sweeps that race its first connection
  Issue #209: a dynamic pool is inserted into POOLS before its first
  server connection exists. If GC sweeps the pool map while
  get_server_parameters is still establishing that connection, the
  pool is removed and the next client sees "No pool configured".

  Until the RAII PoolInitGuard landed, GC relied on a `created_at`
  field with a 2-second grace period — closing the race only when
  the first connection completes inside that window. With slow CI
  runners and the SSLRequest round-trip in `prefer` mode the window
  closes early enough to drop pools mid-initialization. The new flow
  flips `init_complete` from `false` to `true` exactly once
  `get_server_parameters` succeeds, and the GC sweep keys off that
  flag instead of timing.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_passthrough_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """
    And pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        idle_timeout: 500
        server_lifetime: 1000
        retain_connections_time: 200
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """

  Scenario: Concurrent first logins for two users with GC sweeping every 200 ms
    # retain_connections_time drives the GC tick. 200 ms means the sweep
    # fires roughly five times per second — every dynamic pool creation
    # is observed by at least one sweep while its first connection is
    # being established. Without the init_complete check the pool can be
    # reaped before get_server_parameters returns; the next client sees
    # `Connection refused` because pg_doorman lost the pool entry.
    When I run shell command:
      """
      set -e
      PGPASSWORD=md5_pass  psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U pt_md5_user  -d postgres -c "select 1" >/dev/null &
      PGPASSWORD=md5_pass2 psql -h 127.0.0.1 -p ${DOORMAN_PORT} -U pt_md5_user2 -d postgres -c "select 1" >/dev/null &
      wait
      """
    Then the command should succeed
    # After a quiet stretch the connections drain (idle_timeout=500ms,
    # server_lifetime=1000ms) and the regular GC sweep reaps the pools.
    # This confirms the init_complete flag does NOT permanently disable
    # GC — it only delays it until the first connection lands.
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And we sleep for 5000 milliseconds
    And we execute "SHOW POOLS" on admin session "adm" and store response
    Then admin session "adm" response should not contain "pt_md5_user"
    And admin session "adm" response should not contain "pt_md5_user2"
