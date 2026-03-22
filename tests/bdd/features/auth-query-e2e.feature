@auth-query @auth-query-e2e
Feature: Auth query end-to-end — MD5 auth with server_user mode

  Tests for the full auth_query flow: client authenticates via MD5 using
  a password hash fetched from a custom PostgreSQL table, then gets a
  connection from the shared server_user pool.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_e2e_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """

  Scenario: MD5 auth succeeds with correct password via auth_query
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
            workers: 1
            server_user: "postgres"
            server_password: ""
            pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "auth_user1" to database "postgres" with password "secret1" succeeds

  Scenario: MD5 auth fails with wrong password via auth_query
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
            workers: 1
            server_user: "postgres"
            server_password: ""
            pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "auth_user1" to database "postgres" with password "wrong_pass" fails

  Scenario: Auth query user not found
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
            workers: 1
            server_user: "postgres"
            server_password: ""
            pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "nonexistent_user" to database "postgres" with password "any_pass" fails

  Scenario: Static user takes priority over auth_query user
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
          users:
            - username: "static_user"
              password: "md55260eead4132209ce419175f9de0d570"
              server_username: "postgres"
              server_password: ""
              pool_size: 5
          auth_query:
            query: "SELECT username, password FROM auth_users WHERE username = $1"
            user: "postgres"
            password: ""
            workers: 1
            server_user: "postgres"
            server_password: ""
            pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # Static password "static_pass" should work (static user takes priority)
    Then psql connection to pg_doorman as user "static_user" to database "postgres" with password "static_pass" succeeds
    # Auth query password "dynamic_pass" should NOT work (static user wins)
    And psql connection to pg_doorman as user "static_user" to database "postgres" with password "dynamic_pass" fails

  Scenario: Password rotation via refetch on failure
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
            workers: 1
            server_user: "postgres"
            server_password: ""
            pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    # First, connect with the initial password to warm the cache
    Then psql connection to pg_doorman as user "rotate_user" to database "postgres" with password "old_pass" succeeds
    # Update the password in auth_users (simulating password rotation)
    When I run shell command:
      """
      psql -h 127.0.0.1 -p ${PG_PORT} -U postgres -d postgres -c "UPDATE auth_users SET password = 'md5' || md5('new_pass' || 'rotate_user') WHERE username = 'rotate_user'"
      """
    Then the command should succeed
    # Connect with the new password — should succeed via refetch_on_failure
    And psql connection to pg_doorman as user "rotate_user" to database "postgres" with password "new_pass" succeeds
    # Old password should no longer work (cache was updated by refetch)
    And psql connection to pg_doorman as user "rotate_user" to database "postgres" with password "old_pass" fails
