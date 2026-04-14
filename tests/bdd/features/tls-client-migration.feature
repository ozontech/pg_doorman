@tls-migration @linux-only
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
    Then session "integrity" should receive DataRow with "9d52ca440b32ab282c96b3d7f152f0cb"
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

  @tls-transaction-drain
  Scenario: TLS client in active transaction drains then migrates
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 10000
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
    And we create TLS session "tx_tls" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "tx_tls"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "tx_tls" and store backend_pid as "in_tx"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Transaction still works on old process — cipher state intact
    And we send SimpleQuery "SELECT 1" to session "tx_tls"
    And we send SimpleQuery "COMMIT" to session "tx_tls"
    # After COMMIT the client becomes idle, TLS state exported, fd migrated
    And we send SimpleQuery "SELECT 42" to session "tx_tls" and store response
    Then session "tx_tls" should receive DataRow with "42"
    When we sleep 2000ms
    Then stored foreground PID "old_doorman" should not exist
    When we close session "tx_tls"

  @tls-mixed-migration
  Scenario: TLS and plain TCP clients migrate together
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
    And we create TLS session "tls_c" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "plain_c" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'tls_before'" to session "tls_c" and store response
    Then session "tls_c" should receive DataRow with "tls_before"
    When we send SimpleQuery "SELECT 'plain_before'" to session "plain_c" and store response
    Then session "plain_c" should receive DataRow with "plain_before"
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then we send SimpleQuery "SELECT 'tls_after'" to session "tls_c" and store response
    And session "tls_c" should receive DataRow with "tls_after"
    And we send SimpleQuery "SELECT 'plain_after'" to session "plain_c" and store response
    And session "plain_c" should receive DataRow with "plain_after"
    And stored foreground PID "old_doorman" should not exist
    When we close session "tls_c"
    And we close session "plain_c"

  @tls-seqnum-stress
  Scenario: TLS sequence numbers remain correct over many post-migration queries
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
    And we create TLS session "seq" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Push sequence numbers up before migration
    And we send SimpleQuery "SELECT generate_series(1, 200)" to session "seq"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Advance sequence numbers significantly after migration
    And we send SimpleQuery "SELECT generate_series(1, 500)" to session "seq"
    # Verify final integrity with a deterministic checksum
    And we send SimpleQuery "SELECT md5(repeat('seqcheck', 2000))" to session "seq" and store response
    Then session "seq" should receive DataRow with "3463a427fd767320b475327df734d8a9"
    And stored foreground PID "old_doorman" should not exist
    When we close session "seq"

  @tls-double-upgrade
  Scenario: TLS client survives two consecutive binary upgrades
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
    And we create TLS session "double" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'gen0'" to session "double" and store response
    Then session "double" should receive DataRow with "gen0"
    # First upgrade
    When we store foreground pg_doorman PID as "gen1_pid"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we send SimpleQuery "SELECT 'gen1'" to session "double" and store response
    Then session "double" should receive DataRow with "gen1"
    And stored foreground PID "gen1_pid" should not exist
    # Second upgrade — exports from a reconstructed SSL object, not a real handshake
    When we sleep 1000ms
    And we store foreground pg_doorman PID as "gen2_pid"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we send SimpleQuery "SELECT 'gen2'" to session "double" and store response
    Then session "double" should receive DataRow with "gen2"
    And we send SimpleQuery "SELECT md5(repeat('double_check', 500))" to session "double" and store response
    Then session "double" should receive DataRow with "40b93f812a7cee41df1234ea8c340c72"
    And stored foreground PID "gen2_pid" should not exist
    When we close session "double"

  @tls-shutdown-timeout
  Scenario: Shutdown timeout force-closes TLS client stuck in transaction
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 3000
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
    And we create TLS session "stuck" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "stuck"
    And we send SimpleQuery "SELECT pg_advisory_lock(999)" to session "stuck"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Wait for shutdown_timeout (3s) to expire
    And we sleep 4000ms
    Then stored foreground PID "old_doorman" should not exist

  @tls-double-sigusr2
  Scenario: Double SIGUSR2 does not corrupt TLS migration in progress
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 10000
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    When we sleep 1000ms
    And we create TLS session "t1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create TLS session "t2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "t1"
    And we send SimpleQuery "SELECT 1" to session "t2"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then we send SimpleQuery "SELECT 't1_ok'" to session "t1" and store response
    And session "t1" should receive DataRow with "t1_ok"
    And we send SimpleQuery "SELECT 't2_ok'" to session "t2" and store response
    And session "t2" should receive DataRow with "t2_ok"
    And stored foreground PID "old_doorman" should not exist
    When we close session "t1"
    And we close session "t2"
