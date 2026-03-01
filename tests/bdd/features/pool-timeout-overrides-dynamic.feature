@auth-query @pool-timeout-override-dynamic
Feature: Pool-level timeout overrides for auth_query dynamic pools and min_pool_size
  Verify that pool-level idle_timeout and server_lifetime overrides work
  correctly for dynamically created pools via auth_query passthrough,
  and that min_pool_size is properly maintained with pool-level server_lifetime.

  @dynamic-pool-idle-timeout
  Scenario: Dynamic pool inherits pool-level idle_timeout override
    # General idle_timeout=60s, pool idle_timeout=500ms.
    # After a dynamic user connects via auth_query passthrough,
    # the idle server connection should be closed by retain within 2s.
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
        idle_timeout: 60000
        server_lifetime: 60000
        retain_connections_time: 200
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
          idle_timeout: 500
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
    # Connect dynamic user — creates dynamic pool with server connection
    Then psql query "SELECT 1" via pg_doorman as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "1"
    # Verify server connection exists
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than 0
    # Wait for pool-level idle_timeout (500ms) + several retain cycles (200ms each)
    When we sleep for 2000 milliseconds
    # Server connection should be closed by retain (pool idle_timeout=500ms)
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be 0

  @dynamic-pool-server-lifetime
  Scenario: Dynamic pool inherits pool-level server_lifetime override
    # General server_lifetime=60s, pool server_lifetime=500ms.
    # After a dynamic user connects and the connection ages past 500ms,
    # the next query should get a different backend (recycled).
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
          server_lifetime: 500
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
    # Connect via extended protocol with MD5 auth (triggers auth_query → dynamic pool)
    When we create session "one" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we remember backend_pid from session "one" as "first_pid"
    # Wait for pool server_lifetime (500ms) to expire
    When we sleep for 1500 milliseconds
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    # Backend should be recycled — different PID
    Then we verify backend_pid from session "one" is different from "first_pid"

  @min-pool-size-with-pool-lifetime
  Scenario: min_pool_size maintained after pool-level server_lifetime expiry
    # Pool-level server_lifetime=1000ms (general=60s), min_pool_size=2.
    # We verify that:
    # 1. Prewarm creates min_pool_size connections
    # 2. Pool-level server_lifetime causes them to expire (not general=60s)
    # 3. Retain replenishes back to min_pool_size
    # 4. Backend PID changes — proving connections were actually replaced
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
    # Get initial backend PID right after prewarm
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we remember backend_pid from session "one" as "initial_pid"
    # Verify min_pool_size connections exist
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
    # Wait for pool-level server_lifetime (1000ms ±20%) to expire
    # and retain to close + replenish. 3s is enough for multiple cycles.
    When we sleep for 3000 milliseconds
    # Connections should be replenished to min_pool_size
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
    # Verify backend was replaced: the original connection expired
    # and a new one was created by replenish, so PID must be different.
    # This proves pool-level server_lifetime=1000ms was applied (not general=60s).
    When we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we verify backend_pid from session "one" is different from "initial_pid"
