@auth-query @auth-query-min-pool-size
Feature: Auth query min_pool_size for dynamic passthrough pools

  Tests that min_pool_size prewarmed connections are created and maintained
  for dynamic auth_query passthrough pools.

  Scenario: Dynamic pool prewarm with min_pool_size
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
      host all postgres 127.0.0.1/32 trust
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        server_lifetime: 60000
        retain_connections_time: 500
        server_idle_check_timeout: 0
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
          users:
            - username: "postgres"
              password: ""
              pool_size: 2
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 1
            default_pool_size: 5
            min_pool_size: 2
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Connect as dynamic user to trigger pool creation + prewarm spawn
    When we create session "md5_session" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we send SimpleQuery "SELECT 1" to session "md5_session"
    # Wait for prewarm + retain cycles to establish min_pool_size connections
    When we sleep for 2000 milliseconds
    # Check server connections — should have at least 2 (min_pool_size=2)
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2

  Scenario: Dynamic pool maintains min_pool_size after server_lifetime expiry
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
      host all postgres 127.0.0.1/32 trust
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        server_lifetime: 60000
        retain_connections_time: 500
        server_idle_check_timeout: 0
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
          server_lifetime: 1000
          users:
            - username: "postgres"
              password: ""
              pool_size: 2
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            pool_size: 1
            default_pool_size: 5
            min_pool_size: 2
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Connect as dynamic user to trigger pool creation + prewarm
    When we create session "md5_session" to pg_doorman as "pt_md5_user" with password "md5_pass" and database "postgres"
    And we send SimpleQuery "SELECT 1" to session "md5_session"
    # Wait for prewarm + retain cycles
    When we sleep for 2000 milliseconds
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
    # Wait for server_lifetime (1s ±20% jitter) to expire + retain to close and replenish
    When we sleep for 4000 milliseconds
    # Pool should still have at least 2 connections (replenished by retain cycle)
    And we execute "SHOW SERVERS" on admin session "admin1" and store row count
    Then admin session "admin1" row count should be greater than or equal to 2
