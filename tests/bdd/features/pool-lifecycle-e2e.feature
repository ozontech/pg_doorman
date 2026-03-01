@rust @rust-3 @pool-lifecycle-e2e
Feature: Pool lifecycle end-to-end (prewarm → scale up → shrink)
  Comprehensive tests verifying the full connection pool lifecycle:
  1. Prewarm at startup (min_pool_size connections created)
  2. Scale up to pool_size under load
  3. Shrink back to min_pool_size after retain closes expired connections

  @e2e-dedicated-lifecycle
  Scenario: Dedicated pool full lifecycle: prewarm → scale up → shrink to min_pool_size
    # pool_size=5, min_pool_size=2, pool-level server_lifetime=1000ms.
    # Phase 1: After startup, prewarm should create min_pool_size=2 connections.
    # Phase 2: Open 5 concurrent transactions → pool scales to pool_size=5.
    # Phase 3: Release all, wait for lifetime+retain → pool shrinks to min_pool_size=2.
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      retain_connections_time = 500
      server_lifetime = 60000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_lifetime = 1000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      min_pool_size = 2
      """
    ## Phase 1: Prewarm — verify min_pool_size connections created at startup
    When we sleep for 1500 milliseconds
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
    ## Phase 2: Scale up — open 5 concurrent transactions, pin all backends
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "s4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "s5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s1"
    And we send SimpleQuery "BEGIN" to session "s2"
    And we send SimpleQuery "BEGIN" to session "s3"
    And we send SimpleQuery "BEGIN" to session "s4"
    And we send SimpleQuery "BEGIN" to session "s5"
    # All 5 backends should be in use
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be 5
    ## Phase 3: Release and shrink — COMMIT all, wait for lifetime + retain
    When we send SimpleQuery "COMMIT" to session "s1"
    And we send SimpleQuery "COMMIT" to session "s2"
    And we send SimpleQuery "COMMIT" to session "s3"
    And we send SimpleQuery "COMMIT" to session "s4"
    And we send SimpleQuery "COMMIT" to session "s5"
    # Wait for pool-level server_lifetime (1000ms ±20%) + retain cycles
    When we sleep for 4000 milliseconds
    # Pool should shrink to min_pool_size=2 (expired connections closed, replenished to min)
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be 2

  @e2e-auth-query-lifecycle
  Scenario: Auth_query pool lifecycle: scale up under load → shrink to 0 after retain
    # auth_query dynamic pool with default_pool_size=5, pool-level server_lifetime=1000ms.
    # min_pool_size is not supported for dynamic pools, so no prewarm.
    # Phase 1: Open 5 concurrent transactions via auth_query user → pool scales to 5.
    # Phase 2: Release all, wait for lifetime+retain → pool shrinks to 0.
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
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        admin_username: "admin"
        admin_password: "admin"
        server_lifetime: 60000
        retain_connections_time: 500
        server_idle_check_timeout: 0
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          server_lifetime: 1000
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 1
            default_pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    ## Phase 1: Scale up — open 5 concurrent transactions via auth_query user
    When we create session "s1" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we create session "s2" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we create session "s3" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we create session "s4" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we create session "s5" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we send SimpleQuery "BEGIN" to session "s1"
    And we send SimpleQuery "BEGIN" to session "s2"
    And we send SimpleQuery "BEGIN" to session "s3"
    And we send SimpleQuery "BEGIN" to session "s4"
    And we send SimpleQuery "BEGIN" to session "s5"
    # All 5 data backends should be in use (+ possibly 1 executor connection)
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 5
    ## Phase 2: Release and shrink — COMMIT all, wait for lifetime + retain
    When we send SimpleQuery "COMMIT" to session "s1"
    And we send SimpleQuery "COMMIT" to session "s2"
    And we send SimpleQuery "COMMIT" to session "s3"
    And we send SimpleQuery "COMMIT" to session "s4"
    And we send SimpleQuery "COMMIT" to session "s5"
    # Wait for pool-level server_lifetime (1000ms ±20%) + retain cycles
    When we sleep for 4000 milliseconds
    # All connections should be closed (no min_pool_size for dynamic pools)
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be 0
