@auth-query @auth-query-hba
Feature: Auth query HBA integration — edge cases for HBA rules with auth_query

  Tests that pg_doorman HBA rules are correctly enforced for auth_query users:
  Phase 1 gate (reject before auth_query), Phase 2 post-check (password type vs HBA),
  trust mode (skip password, verify user exists), and SCRAM upgrade behavior.

  Scenario: HBA reject — connection rejected before auth_query (dedicated mode)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 reject
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
            server_user: "postgres"
            server_password: ""
            default_pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "hba_md5_user" to database "postgres" with password "hba_md5_pass" fails

  Scenario: HBA trust — user exists, no password needed (dedicated mode)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 trust
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
            server_user: "postgres"
            server_password: ""
            default_pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "hba_trust_user" to database "postgres" without password succeeds

  Scenario: HBA trust — nonexistent user rejected (dedicated mode)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 trust
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
            server_user: "postgres"
            server_password: ""
            default_pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "nonexistent_user" to database "postgres" without password fails

  Scenario: HBA scram-only — user has MD5 password — denied (dedicated mode)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 scram-sha-256
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
            server_user: "postgres"
            server_password: ""
            default_pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "hba_md5_user" to database "postgres" with password "hba_md5_pass" fails

  Scenario: HBA md5 — user has SCRAM password — SCRAM upgrade succeeds (dedicated mode)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
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
            server_user: "postgres"
            server_password: ""
            default_pool_size: 10
            cache_ttl: "1h"
            cache_failure_ttl: "30s"
            min_interval: "0s"
      """
    Then psql connection to pg_doorman as user "hba_scram_user" to database "postgres" with password "hba_scram_pass" succeeds

  Scenario: HBA reject — connection rejected before auth_query (passthrough mode)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 reject
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
    Then psql connection to pg_doorman as user "hba_md5_user" to database "postgres" with password "hba_md5_pass" fails

  Scenario: HBA trust — user exists, no password (passthrough mode, backend trust)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             hba_pt_trust_user 127.0.0.1/32          trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_hba_fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 trust
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
    Then psql connection to pg_doorman as user "hba_pt_trust_user" to database "postgres" without password succeeds
