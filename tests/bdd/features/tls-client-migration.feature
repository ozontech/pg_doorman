@client-migration @tls-migration @linux-only
Feature: TLS client migration during binary upgrade
  On SIGUSR2, idle TLS clients should migrate from the old process to the
  new one. The encrypted session continues on the same TCP socket — the
  client never re-handshakes or reconnects.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And self-signed SSL certificates are generated

  @tls-transparent
  Scenario: TLS session migrates transparently after binary upgrade
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create TLS session "tls1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "tls1" and store backend_pid as "before_upgrade"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "tls1" and store backend_pid as "after_upgrade"
    Then stored PID "after_upgrade" should be different from "before_upgrade"
    And stored foreground PID "old_doorman" should not exist
    When we close session "tls1"

  @tls-integrity
  Scenario: TLS encrypted traffic remains intact after migration
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we sleep 1000ms
    And we create TLS session "integrity" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'BEFORE_OK'" to session "integrity" and store response
    Then session "integrity" should receive DataRow with "BEFORE_OK"
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Long response tests cipher state: keys, IVs, sequence numbers
    And we send SimpleQuery "SELECT md5(repeat('migration_test', 1000))" to session "integrity" and store response
    Then session "integrity" should receive DataRow with "e3e1c648e2bccd97c3baf3dab0e4b8a8"
    And stored foreground PID "old_doorman" should not exist
    When we close session "integrity"

  @tls-prepared
  Scenario: Prepared statements over TLS survive migration
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      prepared_statements_cache_size = 100
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create TLS session "ps_tls" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "tls_stmt" with query "SELECT 1 AS val" to session "ps_tls"
    And we send Bind "" to "tls_stmt" with params "" to session "ps_tls"
    And we send Execute "" to session "ps_tls"
    And we send Sync to session "ps_tls"
    Then session "ps_tls" should receive DataRow with "1"
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we send Bind "" to "tls_stmt" with params "" to session "ps_tls"
    And we send Execute "" to session "ps_tls"
    And we send Sync to session "ps_tls"
    Then session "ps_tls" should receive DataRow with "1"
    And stored foreground PID "old_doorman" should not exist
    When we close session "ps_tls"

  @tls-concurrent
  Scenario: Multiple TLS clients migrate concurrently
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    When we sleep 1000ms
    And we create TLS session "t1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create TLS session "t2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create TLS session "t3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "t1"
    And we send SimpleQuery "SELECT 1" to session "t2"
    And we send SimpleQuery "SELECT 1" to session "t3"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then we send SimpleQuery "SELECT 't1_alive'" to session "t1" and store response
    And session "t1" should receive DataRow with "t1_alive"
    And we send SimpleQuery "SELECT 't2_alive'" to session "t2" and store response
    And session "t2" should receive DataRow with "t2_alive"
    And we send SimpleQuery "SELECT 't3_alive'" to session "t3" and store response
    And session "t3" should receive DataRow with "t3_alive"
    And stored foreground PID "old_doorman" should not exist
    When we close session "t1"
    And we close session "t2"
    And we close session "t3"
