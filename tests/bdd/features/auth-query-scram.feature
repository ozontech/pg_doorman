@auth-query @auth-query-scram
Feature: Auth query SCRAM-SHA-256 — end-to-end authentication

  Tests for SCRAM-SHA-256 authentication via auth_query: client authenticates
  using SCRAM with a verifier fetched from auth_users table, then gets a
  connection from the shared server_user pool.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_scram_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 scram-sha-256
      """

  Scenario: SCRAM auth succeeds with correct password via auth_query
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
    Then psql connection to pg_doorman as user "scram_aq_user" to database "postgres" with password "scram_secret" succeeds

  Scenario: SCRAM auth fails with wrong password via auth_query
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
    Then psql connection to pg_doorman as user "scram_aq_user" to database "postgres" with password "wrong_password" fails

  Scenario: SCRAM password rotation — cache invalidated, reconnect succeeds
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
    Then psql connection to pg_doorman as user "scram_rotate_user" to database "postgres" with password "old_scram_pass" succeeds
    # Change password in PG and update auth_users with new SCRAM verifier
    When I run shell command:
      """
      psql -h 127.0.0.1 -p ${PG_PORT} -U postgres -d postgres -c "SET password_encryption = 'scram-sha-256'; ALTER USER scram_rotate_user PASSWORD 'new_scram_pass'; UPDATE auth_users SET password = (SELECT rolpassword FROM pg_authid WHERE rolname = 'scram_rotate_user') WHERE username = 'scram_rotate_user';"
      """
    Then the command should succeed
    # First attempt with new password fails (old salt cached), but cache is invalidated
    And psql connection to pg_doorman as user "scram_rotate_user" to database "postgres" with password "new_scram_pass" fails
    # Second attempt succeeds — fresh verifier fetched from PG
    And psql connection to pg_doorman as user "scram_rotate_user" to database "postgres" with password "new_scram_pass" succeeds
