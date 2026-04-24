@server-tls
Feature: Server-side TLS connections
  pg_doorman connects to PostgreSQL over TLS when configured.

  Background:
    Given PostgreSQL SSL certificates are generated

  @server-tls-prefer
  Scenario: prefer mode connects via TLS when server supports it
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "prefer"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-prefer-fallback
  Scenario: prefer mode falls back to plain TCP when server has ssl=off
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "prefer"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-require
  Scenario: require mode connects via TLS when server supports it
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "require"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-require-no-ssl
  Scenario: require mode fails when server has ssl=off
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "require"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails

  @server-tls-verify-ca
  Scenario: verify-ca with correct CA succeeds
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "verify-ca"
      server_tls_ca_cert = "${PG_SSL_CA_CERT}"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-verify-ca-wrong
  Scenario: verify-ca with wrong CA fails
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "verify-ca"
      server_tls_ca_cert = "${PG_SSL_WRONG_CA_CERT}"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    Then psql connection to pg_doorman as user "example_user_1" to database "example_db" with password "" fails

  @server-tls-verify-full
  Scenario: verify-full with correct CA and hostname succeeds
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "verify-full"
      server_tls_ca_cert = "${PG_SSL_CA_CERT}"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "localhost"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-disable
  Scenario: disable mode uses plain TCP
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "disable"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-mtls
  Scenario: mTLS — pg_doorman presents client certificate to server
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY} -c ssl_ca_file=${PG_SSL_CA_CERT}" and pg_hba.conf:
      """
      hostnossl all postgres 127.0.0.1/32 trust
      hostssl all all 127.0.0.1/32 trust clientcert=verify-ca
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "verify-ca"
      server_tls_ca_cert = "${PG_SSL_CA_CERT}"
      server_tls_certificate = "${PG_SSL_CLIENT_CERT}"
      server_tls_private_key = "${PG_SSL_CLIENT_KEY}"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-show-servers-tls
  Scenario: SHOW SERVERS shows tls=true for TLS backend connection
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "require"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "adm" and store response
    Then admin session "adm" response should contain "true"
    When we close session "s1"

  @server-tls-show-servers-plain
  Scenario: SHOW SERVERS shows tls=false for plain backend connection
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "disable"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "adm" and store response
    Then admin session "adm" response should contain "false"
    When we close session "s1"

  @server-tls-cancel
  Scenario: cancel request uses TLS when main connection uses TLS
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "require"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "main" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT pg_sleep(10)" to session "main" without waiting
    And we sleep 500ms
    And we send cancel request for session "main"
    Then session "main" should receive cancel error containing "canceling"

  @server-tls-per-pool
  Scenario: per-pool TLS override
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      hostnossl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "disable"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.tls_pool]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_database = "example_db"
      server_tls_mode = "require"

      [[pools.tls_pool.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1

      [pools.plain_pool]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_database = "example_db"

      [[pools.plain_pool.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "tls_pool"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "plain_pool"
    And we send SimpleQuery "SELECT 1" to session "s2" and store response
    Then session "s2" should receive DataRow with "1"
    When we close session "s1"
    And we close session "s2"

  @server-tls-allow-retry
  Scenario: allow mode retries with TLS when server requires encryption
    Given PostgreSQL started with options "-c ssl=on -c ssl_cert_file=${PG_SSL_CERT} -c ssl_key_file=${PG_SSL_KEY}" and pg_hba.conf:
      """
      hostssl all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "allow"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"

  @server-tls-allow-plain
  Scenario: allow mode uses plain TCP when server accepts unencrypted
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      server_tls_mode = "allow"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store response
    Then session "s1" should receive DataRow with "1"
    When we close session "s1"
