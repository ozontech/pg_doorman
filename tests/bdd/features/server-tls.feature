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
      host all postgres 127.0.0.1/32 trust
      hostssl all all 127.0.0.1/32 cert
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
