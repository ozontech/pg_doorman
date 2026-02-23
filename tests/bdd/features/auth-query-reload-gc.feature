@auth-query @auth-query-reload-gc
Feature: Auth query RELOAD and idle pool GC

  Tests for dynamic pool lifecycle: RELOAD preserves/removes dynamic pools,
  and garbage collector cleans up idle dynamic pools.

  Scenario: RELOAD removes auth_query — dynamic pools destroyed
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
            pool_size: 1
            default_pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Connect dynamic user — creates dynamic pool
    Then psql query "SELECT 1" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "1"
    # Verify pool exists
    When we create admin session "adm1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "adm1" and store response
    Then admin session "adm1" response should contain "pt_md5_user"
    # Overwrite config: remove auth_query
    When we overwrite pg_doorman config file with:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
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
      """
    And we execute "RELOAD" on admin session "adm1" and store response
    And we sleep for 500 milliseconds
    And we execute "SHOW POOLS" on admin session "adm1" and store response
    Then admin session "adm1" response should not contain "pt_md5_user"
    # Confirm: connection without auth_query fails (no static user either)
    Then psql connection to pg_doorman as user "pt_md5_user" to database "postgres" with password "md5_pass" fails

  Scenario: RELOAD adds static user — overrides dynamic pool
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
            pool_size: 1
            default_pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Connect dynamic user — creates dynamic pool
    Then psql query "SELECT 1" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "1"
    # Overwrite config: add static user with same name, keep auth_query
    When we overwrite pg_doorman config file with:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
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
            - username: "pt_md5_user"
              password: "md5_pass"
              pool_size: 10
              server_username: "pt_md5_user"
              server_password: "md5_pass"
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
    When we create admin session "adm1" to pg_doorman as "admin" with password "admin"
    And we execute "RELOAD" on admin session "adm1" and store response
    And we sleep for 500 milliseconds
    # Connection should still work (via static user now)
    Then psql query "SELECT current_user" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "pt_md5_user"

  Scenario: Idle dynamic pool GC removes empty pools
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
        idle_timeout: 1000
        server_lifetime: 2000
        retain_connections_time: 1000
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
            pool_size: 1
            default_pool_size: 5
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Connect dynamic user — creates dynamic pool with server connection
    Then psql query "SELECT 1" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "1"
    # Verify pool exists
    When we create admin session "adm1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "adm1" and store response
    Then admin session "adm1" response should contain "pt_md5_user"
    # Wait for idle timeout + server lifetime + GC interval to clean up
    When we sleep for 5000 milliseconds
    # Pool should be GC'd (all connections expired, pool size == 0)
    When we execute "SHOW POOLS" on admin session "adm1" and store response
    Then admin session "adm1" response should not contain "pt_md5_user"
    # Reconnect — new pool should be created, query succeeds
    Then psql query "SELECT 1" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "1"
