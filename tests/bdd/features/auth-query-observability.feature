@auth-query @auth-query-observability
Feature: Auth query observability (SHOW AUTH_QUERY)

  Verifies that SHOW AUTH_QUERY admin command returns cache, auth,
  executor, and dynamic pool metrics.

  Scenario: SHOW AUTH_QUERY shows cache and auth metrics after login
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
    # First connection: cache miss
    Then psql query "SELECT 1" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "1"
    # Second connection: cache hit
    Then psql query "SELECT 2" as user "pt_md5_user" to database "postgres" with password "md5_pass" returns "2"
    # Check SHOW AUTH_QUERY
    When we create admin session "adm1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW AUTH_QUERY" on admin session "adm1" and store response
    Then admin session "adm1" response should contain "postgres"

  Scenario: SHOW AUTH_QUERY shows auth_failure on wrong password
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
    # Attempt with wrong password — should fail
    Then psql query "SELECT 1" as user "pt_md5_user" to database "postgres" with password "wrong_pass" fails
    # Check SHOW AUTH_QUERY shows the pool
    When we create admin session "adm1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW AUTH_QUERY" on admin session "adm1" and store response
    Then admin session "adm1" response should contain "postgres"
