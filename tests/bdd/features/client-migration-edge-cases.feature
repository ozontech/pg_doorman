@client-migration
Feature: Client migration edge cases during binary upgrade
  Edge-case behaviors during SIGUSR2 binary upgrade that verify
  correctness of migration in non-trivial scenarios.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: DEALLOCATE ALL clears migrated prepared statement cache
    # After migration, DEALLOCATE ALL must clear the transferred cache
    # so that re-Parse with the same name uses the new query text.
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "da" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "my_stmt" with query "SELECT 42 AS val" to session "da"
    And we send Bind "" to "my_stmt" with params "" to session "da"
    And we send Execute "" to session "da"
    And we send Sync to session "da"
    Then session "da" should receive DataRow with "42"
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # DEALLOCATE ALL clears migrated cache
    And we send SimpleQuery "DEALLOCATE ALL" to session "da"
    # Re-Parse same name with different query
    And we send Parse "my_stmt" with query "SELECT 99 AS val" to session "da"
    And we send Bind "" to "my_stmt" with params "" to session "da"
    And we send Execute "" to session "da"
    And we send Sync to session "da"
    Then session "da" should receive DataRow with "99"
    And stored foreground PID "old_doorman" should not exist
    When we close session "da"

  Scenario: Deferred BEGIN blocks migration until COMMIT
    # A standalone BEGIN is deferred by the pooler (no server checkout).
    # Migration explicitly skips clients with pending_begin. The client
    # must complete the transaction before migrating.
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we sleep 1000ms
    And we create session "def" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Deferred BEGIN — no server checked out yet
    And we send SimpleQuery "BEGIN" to session "def"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Query flushes the deferred BEGIN + executes on old process
    And we send SimpleQuery "SELECT 1" to session "def"
    # COMMIT releases server, client becomes idle and migrates
    And we send SimpleQuery "COMMIT" to session "def"
    # Verify session works on new process
    And we send SimpleQuery "SELECT 'after_deferred'" to session "def" and store response
    Then session "def" should receive DataRow with "after_deferred"
    When we sleep 2000ms
    Then stored foreground PID "old_doorman" should not exist
    When we close session "def"

  Scenario: Clients with different users migrate and preserve identity
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      [[pools.example_db.users]]
      username = "example_user_2"
      password = ""
      pool_size = 2
      """
    When we sleep 1000ms
    And we create session "u1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "u2" to pg_doorman as "example_user_2" with password "" and database "example_db"
    And we send SimpleQuery "SELECT current_user" to session "u1" and store response
    Then session "u1" should receive DataRow with "example_user_1"
    When we send SimpleQuery "SELECT current_user" to session "u2" and store response
    Then session "u2" should receive DataRow with "example_user_2"
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Both users must land in correct pool after migration
    Then we send SimpleQuery "SELECT current_user" to session "u1" and store response
    And session "u1" should receive DataRow with "example_user_1"
    And we send SimpleQuery "SELECT current_user" to session "u2" and store response
    And session "u2" should receive DataRow with "example_user_2"
    And stored foreground PID "old_doorman" should not exist
    When we close session "u1"
    And we close session "u2"

  Scenario: Double SIGUSR2 does not corrupt plain TCP migration
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    When we sleep 1000ms
    And we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "c1"
    And we send SimpleQuery "SELECT 1" to session "c2"
    And we store foreground pg_doorman PID as "old_doorman"
    # Two SIGUSR2 in immediate succession
    And we send SIGUSR2 to foreground pg_doorman
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then we send SimpleQuery "SELECT 'c1_ok'" to session "c1" and store response
    And session "c1" should receive DataRow with "c1_ok"
    And we send SimpleQuery "SELECT 'c2_ok'" to session "c2" and store response
    And session "c2" should receive DataRow with "c2_ok"
    And stored foreground PID "old_doorman" should not exist
    When we close session "c1"
    And we close session "c2"

  Scenario: Client survives two consecutive plain TCP binary upgrades
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "survivor" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'gen0'" to session "survivor" and store response
    Then session "survivor" should receive DataRow with "gen0"
    # First upgrade
    When we store foreground pg_doorman PID as "gen1_pid"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we send SimpleQuery "SELECT 'gen1'" to session "survivor" and store response
    Then session "survivor" should receive DataRow with "gen1"
    And stored foreground PID "gen1_pid" should not exist
    # Second upgrade
    When we sleep 1000ms
    And we store foreground pg_doorman PID as "gen2_pid"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we send SimpleQuery "SELECT 'gen2'" to session "survivor" and store response
    Then session "survivor" should receive DataRow with "gen2"
    And stored foreground PID "gen2_pid" should not exist
    When we close session "survivor"

  Scenario: Idle and in-transaction clients both migrate correctly
    # Idle client migrates immediately. In-transaction client finishes
    # on old process, then migrates on next idle iteration.
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    When we sleep 1000ms
    And we create session "idle" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "idle"
    And we create session "tx" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "tx"
    And we send SimpleQuery "SELECT 1" to session "tx"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Idle client already migrated
    Then we send SimpleQuery "SELECT 'idle_ok'" to session "idle" and store response
    And session "idle" should receive DataRow with "idle_ok"
    # In-tx client still works on old process
    When we send SimpleQuery "SELECT 'still_in_tx'" to session "tx" and store response
    Then session "tx" should receive DataRow with "still_in_tx"
    # COMMIT releases server, client migrates
    When we send SimpleQuery "COMMIT" to session "tx"
    And we send SimpleQuery "SELECT 'tx_migrated'" to session "tx" and store response
    Then session "tx" should receive DataRow with "tx_migrated"
    When we sleep 2000ms
    Then stored foreground PID "old_doorman" should not exist
    When we close session "idle"
    And we close session "tx"

  Scenario: Shutdown timeout expires and force-closes stuck transaction
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "stuck" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Hold transaction with advisory lock so it never commits
    And we send SimpleQuery "BEGIN" to session "stuck"
    And we send SimpleQuery "SELECT pg_advisory_lock(888)" to session "stuck"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Old process must exit after shutdown_timeout (3s) + setup overhead (~2s)
    # Budget: ~2s readiness handshake + 3s shutdown_timeout = ~5s max.
    # Poll for 10s to have margin.
    And we sleep 6000ms
    Then stored foreground PID "old_doorman" should not exist

  Scenario: Admin console works on new process during migration
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
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we sleep 1000ms
    # Hold transaction to keep old process alive
    And we create session "tx" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "tx"
    And we send SimpleQuery "SELECT 1" to session "tx"
    And we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # Admin console on new process should work
    When we create admin session "adm" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "adm" and store response
    Then admin session "adm" response should contain "example_db"
    # Cleanup: let old process drain
    When we send SimpleQuery "COMMIT" to session "tx"
    And we sleep 2000ms
    Then stored foreground PID "old_doorman" should not exist
    When we close session "tx"

  Scenario: Full prepared statement cache migrates and eviction works after
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
      client_prepared_statements_cache_size = 3
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we sleep 1000ms
    And we create session "full" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Fill cache to limit (3 entries)
    And we send Parse "s1" with query "SELECT 1 AS val" to session "full"
    And we send Bind "" to "s1" with params "" to session "full"
    And we send Execute "" to session "full"
    And we send Sync to session "full"
    Then session "full" should receive DataRow with "1"
    When we send Parse "s2" with query "SELECT 2 AS val" to session "full"
    And we send Bind "" to "s2" with params "" to session "full"
    And we send Execute "" to session "full"
    And we send Sync to session "full"
    Then session "full" should receive DataRow with "2"
    When we send Parse "s3" with query "SELECT 3 AS val" to session "full"
    And we send Bind "" to "s3" with params "" to session "full"
    And we send Execute "" to session "full"
    And we send Sync to session "full"
    Then session "full" should receive DataRow with "3"
    # Migrate with full cache
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # All 3 statements should work after migration
    And we send Bind "" to "s3" with params "" to session "full"
    And we send Execute "" to session "full"
    And we send Sync to session "full"
    Then session "full" should receive DataRow with "3"
    # Add s4 — should evict oldest (s1) from LRU
    When we send Parse "s4" with query "SELECT 4 AS val" to session "full"
    And we send Bind "" to "s4" with params "" to session "full"
    And we send Execute "" to session "full"
    And we send Sync to session "full"
    Then session "full" should receive DataRow with "4"
    # s2 should still work (not evicted)
    When we send Bind "" to "s2" with params "" to session "full"
    And we send Execute "" to session "full"
    And we send Sync to session "full"
    Then session "full" should receive DataRow with "2"
    And stored foreground PID "old_doorman" should not exist
    When we close session "full"
