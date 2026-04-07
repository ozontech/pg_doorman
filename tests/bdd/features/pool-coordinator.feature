@rust @rust-4 @pool-coordinator
Feature: Pool Coordinator — database-level connection limit
  Verify that max_db_connections enforces a hard cap on total server connections
  across all user pools for the same database. Test eviction, reserve pool,
  and error handling under connection pressure.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @coordinator-disabled
  Scenario: Coordinator disabled by default — pools work independently
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" without waiting
    Then we read SimpleQuery response from session "s1" within 2000ms
    Then session "s1" should receive DataRow with "1"

  @coordinator-single-user-within-limit
  Scenario: Single user within max_db_connections — no interference
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 5

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" without waiting
    Then we read SimpleQuery response from session "s1" within 2000ms
    Then session "s1" should receive DataRow with "1"
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 2" to session "s2" without waiting
    Then we read SimpleQuery response from session "s2" within 2000ms
    Then session "s2" should receive DataRow with "2"

  @coordinator-single-user-pool-full-no-reserve
  Scenario: Single user, pool full, no reserve — query reports error
    # max_db_connections = 2, pool_size = 2, reserve = 0
    # Two active transactions pin both coordinator slots.
    # Third query on a new session: coordinator has 0 permits, reserve=0 → error.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      reserve_pool_size = 0
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    # Fill both coordinator slots with active transactions
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s1"
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s2"
    # Third session: pool_size=3 allows the per-user semaphore, but coordinator
    # has 0 permits and reserve=0 → connection creation fails.
    When we create session "s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s3" without waiting
    Then we read SimpleQuery response from session "s3" within 5000ms
    Then session "s3" should receive error containing "all server connections"

  @coordinator-reserve-for-second-user
  Scenario: Pool full for user1, second user gets connection from reserve
    # max_db_connections = 2, reserve = 1
    # user1 fills 2 main slots with active transactions,
    # user2 gets a connection from the reserve pool.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      reserve_pool_size = 1
      reserve_pool_timeout = 1000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      """
    # user1 fills both main coordinator slots (holds transactions open)
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s2"
    # user2 arrives — main permits exhausted, eviction impossible (all active),
    # but reserve_pool_size=1 so user2 gets a reserve connection
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s1" without waiting
    Then we read SimpleQuery response from session "u2_s1" within 5000ms
    Then session "u2_s1" should receive DataRow with "1"

  @coordinator-reserve-exhausted
  Scenario: Both main and reserve exhausted — error with reserve info
    # max_db_connections = 2, reserve = 1. Three active transactions consume everything.
    # Fourth query gets error mentioning reserve status.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      reserve_pool_size = 1
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 4
      """
    # Fill main (2) + reserve (1) = 3 active transactions
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s1"
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s2"
    When we create session "s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s3"
    # Fourth: everything exhausted
    When we create session "s4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s4" without waiting
    Then we read SimpleQuery response from session "s4" within 5000ms
    Then session "s4" should receive error containing "reserve"

  @coordinator-error-mentions-database
  Scenario: Error message includes database name
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 1
      reserve_pool_size = 0
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s1"
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s2" without waiting
    Then we read SimpleQuery response from session "s2" within 5000ms
    Then session "s2" should receive error containing "example_db"

  @coordinator-eviction-by-lifetime
  Scenario: Eviction frees idle connection older than min_connection_lifetime
    # max_db_connections = 2, min_connection_lifetime = 500ms, reserve = 0
    # user1 opens 2 connections (fills limit), then releases them to pool.
    # After 500ms the idle connections become evictable.
    # user2 arrives → coordinator evicts user1's oldest idle connection.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      min_connection_lifetime = 500
      reserve_pool_size = 0
      reserve_pool_timeout = 2000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      min_pool_size = 0

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      min_pool_size = 0
      """
    # user1 creates 2 connections (fills the coordinator limit)
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    # In transaction mode, connections are returned to pool after each query.
    # Now user1 has 2 server connections in idle state.
    # Wait for min_connection_lifetime (500ms) so idle connections become evictable.
    When we sleep for 800 milliseconds
    # user2 arrives — coordinator evicts user1's oldest idle connection
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT current_user" to session "u2_s1" without waiting
    Then we read SimpleQuery response from session "u2_s1" within 5000ms
    Then session "u2_s1" should receive DataRow with "postgres"

  @coordinator-min-guaranteed-protects
  Scenario: min_guaranteed_pool_size prevents eviction below minimum
    # user1 has 2 connections, min_guaranteed_pool_size=2.
    # spare_above_min = 0, so eviction cannot take from user1.
    # user2 falls through to reserve.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      min_guaranteed_pool_size = 2
      min_connection_lifetime = 500
      reserve_pool_size = 1
      reserve_pool_timeout = 1000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      """
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    When we sleep for 800 milliseconds
    # user2: eviction impossible (user1 at guaranteed minimum), must use reserve
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT current_user" to session "u2_s1" without waiting
    Then we read SimpleQuery response from session "u2_s1" within 5000ms
    Then session "u2_s1" should receive DataRow with "postgres"

  @coordinator-session-mode-reserve
  Scenario: Session mode — no idle connections to evict, reserve is the only option
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      pool_mode = "session"
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      min_connection_lifetime = 500
      reserve_pool_size = 1
      reserve_pool_timeout = 1000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      """
    # Session mode: connections held for entire client session (never returned to pool)
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    When we sleep for 800 milliseconds
    # user2: no idle connections to evict (session mode), falls to reserve
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT current_user" to session "u2_s1" without waiting
    Then we read SimpleQuery response from session "u2_s1" within 5000ms
    Then session "u2_s1" should receive DataRow with "postgres"

  @coordinator-sustained-eviction
  Scenario: Multiple evictions in rapid succession under sustained load
    # user1 fills 5 slots, all go idle. 3 user2 clients arrive — each triggers eviction.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 5
      min_connection_lifetime = 500
      reserve_pool_size = 0
      reserve_pool_timeout = 2000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 5
      """
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    When we create session "u1_s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s3"
    When we create session "u1_s4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s4"
    When we create session "u1_s5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s5"
    When we sleep for 800 milliseconds
    # 3 user2 clients — each triggers an eviction of user1's idle connections
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s1" without waiting
    When we create session "u2_s2" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 2" to session "u2_s2" without waiting
    When we create session "u2_s3" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 3" to session "u2_s3" without waiting
    Then we read SimpleQuery response from session "u2_s1" within 5000ms
    Then session "u2_s1" should receive DataRow with "1"
    Then we read SimpleQuery response from session "u2_s2" within 5000ms
    Then session "u2_s2" should receive DataRow with "2"
    Then we read SimpleQuery response from session "u2_s3" within 5000ms
    Then session "u2_s3" should receive DataRow with "3"

  @coordinator-show-admin
  Scenario: SHOW POOL_COORDINATOR returns coordinator state
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 5
      reserve_pool_size = 2

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1"
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    Then admin session "admin" column "max_db_conn" should be between 5 and 5
    Then admin session "admin" column "reserve_size" should be between 2 and 2

  @coordinator-reload-unchanged
  Scenario: RELOAD with unchanged config — coordinator reused, connections survive
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 3
      reserve_pool_size = 1
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    # Create a connection to populate the pool
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1"
    # RELOAD with same config — coordinator should be reused
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "RELOAD" on admin session "admin" and store response
    And we sleep for 300 milliseconds
    # Verify coordinator still works after RELOAD
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 2" to session "s2" without waiting
    Then we read SimpleQuery response from session "s2" within 2000ms
    Then session "s2" should receive DataRow with "2"
    # Verify SHOW POOL_COORDINATOR still shows coordinator
    When we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    Then admin session "admin" column "max_db_conn" should be between 3 and 3

  @coordinator-reload-changed
  Scenario: RELOAD with changed max_db_connections — new coordinator applies
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 1
      reserve_pool_size = 0
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    # With max_db_connections=1, only 1 server connection allowed.
    # Fill it with an active transaction.
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "s1"
    # Second query should fail (limit=1, reserve=0)
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s2" without waiting
    Then we read SimpleQuery response from session "s2" within 5000ms
    Then session "s2" should receive error containing "all server connections"
    # Now RELOAD with higher limit: max_db_connections=5
    When we overwrite pg_doorman config file with:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 5
      reserve_pool_size = 0
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "RELOAD" on admin session "admin" and store response
    And we sleep for 300 milliseconds
    # Now second query should succeed (new coordinator has limit=5)
    When we create session "s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 42" to session "s3" without waiting
    Then we read SimpleQuery response from session "s3" within 2000ms
    Then session "s3" should receive DataRow with "42"
    # Verify SHOW reflects new limit
    When we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    Then admin session "admin" column "max_db_conn" should be between 5 and 5

  @coordinator-reload-add-coordinator
  Scenario: RELOAD adds coordinator to pool that had none
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    # No coordinator — SHOW POOL_COORDINATOR should return empty
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    Then admin session "admin" response should not contain "example_db"
    # Add coordinator via RELOAD
    When we overwrite pg_doorman config file with:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 3
      reserve_pool_size = 1

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """
    And we execute "RELOAD" on admin session "admin" and store response
    And we sleep for 300 milliseconds
    # Now coordinator should be active
    When we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    Then admin session "admin" column "max_db_conn" should be between 3 and 3
    Then admin session "admin" column "reserve_size" should be between 1 and 1

  @coordinator-reserve-pressure-relief
  Scenario: Reserve connections are released after idle time exceeds min_connection_lifetime
    # Reserve connections should be closed by the retain cycle once idle long
    # enough, freeing reserve permits for future use.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      retain_connections_time = 500

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      min_connection_lifetime = 500
      reserve_pool_size = 1
      reserve_pool_timeout = 1000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      """
    # user1 fills 2 main coordinator slots
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s2"
    # user2 gets a reserve connection
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s1"
    # user2's transaction-mode query completes → reserve connection goes idle.
    # Wait for retain cycle (500ms interval) + min_connection_lifetime (500ms)
    # to allow reserve pressure relief to close the idle reserve connection.
    When we sleep for 1500 milliseconds
    # Reserve should be freed — another user can get a reserve again
    When we create session "u2_s2" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 2" to session "u2_s2" without waiting
    Then we read SimpleQuery response from session "u2_s2" within 5000ms
    Then session "u2_s2" should receive DataRow with "2"

  @coordinator-eviction-too-young
  Scenario: Eviction skipped for connections younger than min_connection_lifetime — error without reserve
    # max_db_connections = 3, min_connection_lifetime = 30s, reserve = 0.
    # user2 warms up first (caches server params, pins 1 coordinator slot).
    # user1 opens 2 CONCURRENT transactions to force 2 separate backend connections,
    # then commits both so the connections go idle (but keep coordinator permits).
    # user2 tries a second connection — coordinator is full (3/3).
    # Eviction scans user1's 2 idle connections but they are < 30s old → skipped.
    # No reserve → error.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 3
      min_connection_lifetime = 30000
      reserve_pool_size = 0
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      min_pool_size = 0

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      min_pool_size = 0
      """
    # user2 warmup: cache server params and pin 1 coordinator slot with active transaction
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u2_s1"
    And we send SimpleQuery "SELECT 1" to session "u2_s1"
    # user1 opens 2 concurrent transactions — forces 2 separate backend connections
    # (second session can't reuse first session's connection because it's active)
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s1"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s2"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    # Commit both → connections return to idle pool (coordinator permits stay in ObjectInner)
    And we send SimpleQuery "COMMIT" to session "u1_s1"
    And we send SimpleQuery "COMMIT" to session "u1_s2"
    # Coordinator: 3/3 (user2 active + user1 x2 idle). All connections < 30s old.
    # user2 second session: server params cached, pool has room (pool_size=2),
    # but needs new backend connection → coordinator full →
    # eviction: user1 has 2 idle but age < 30s → skipped → no reserve → error
    When we create session "u2_s2" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s2" without waiting
    Then we read SimpleQuery response from session "u2_s2" within 5000ms
    Then session "u2_s2" should receive error containing "all server connections"

  @coordinator-fair-eviction
  Scenario: Eviction targets user with largest surplus, not the one with fewest connections
    # 3 users share max_db_connections = 6.
    # user1 fills 4 slots, user2 fills 2 slots. All go idle.
    # user3 arrives — eviction should close user1's idle (surplus=4 > user2's surplus=2).
    # Verify user3 gets a connection (eviction worked) and total is still ≤ 6.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 6
      min_connection_lifetime = 500
      reserve_pool_size = 0
      reserve_pool_timeout = 2000

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 6
      min_pool_size = 0

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 6
      min_pool_size = 0

      [[pools.example_db.users]]
      username = "example_user_2"
      password = ""
      pool_size = 6
      min_pool_size = 0
      """
    # user1 fills 4 slots
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    When we create session "u1_s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s3"
    When we create session "u1_s4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u1_s4"
    # user2 fills 2 slots
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s1"
    When we create session "u2_s2" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s2"
    # All 6 slots filled. Wait for min_connection_lifetime.
    When we sleep for 800 milliseconds
    # user3 arrives — should evict from user1 (surplus=4 > user2's surplus=2)
    When we create session "u3_s1" to pg_doorman as "example_user_2" with password "" and database "example_db"
    And we send SimpleQuery "SELECT current_user" to session "u3_s1" without waiting
    Then we read SimpleQuery response from session "u3_s1" within 5000ms
    Then session "u3_s1" should receive DataRow with "example_user_2"

  @coordinator-replenish-respects-limit
  Scenario: Replenish (min_pool_size prewarm) respects coordinator limit
    # min_pool_size=2, max_db_connections=2, pool_size=5.
    # Replenish creates exactly min_pool_size connections; coordinator allows them
    # (sum(min_pool_size)=2 ≤ max_db_connections=2).
    # Verify total connections ≤ max_db_connections after prewarm.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      reserve_pool_size = 0
      reserve_pool_timeout = 500

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      min_pool_size = 2
      """
    # Wait for prewarm (retain cycle creates connections up to min_pool_size)
    When we sleep for 1500 milliseconds
    # Coordinator should have at most 2 active connections
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    Then admin session "admin" column "current" should be between 0 and 2

  @coordinator-phase-c-idle-return-wake
  Scenario: Phase C waiter wakes on peer idle return and acquires via eviction
    # Regression for the case where Phase C slept through peer idle-return events.
    #
    # Setup:
    #   - max_db_connections = 2, reserve_pool_size = 0 (no fallback).
    #   - min_connection_lifetime = 50 ms (so returned connections are evictable
    #     almost immediately).
    #   - reserve_pool_timeout = 800 ms (Phase C wait budget). Short enough that
    #     this scenario fails fast if the fix regresses.
    #
    # Before the fix:
    #   1. user1 pins both coordinator slots with open transactions.
    #   2. user2 sends a query → Phase B finds nothing in user1's vec (both
    #      connections are checked out, not in the idle queue) → Phase C waits
    #      on connection_returned. That Notify only fires on permit drop.
    #   3. user1 commits one transaction → Pool::return_object pushes the
    #      backend into user1's slots.vec — no permit drop, no Notify.
    #   4. user2 sleeps until reserve_pool_timeout (800 ms) → reserve_pool_size
    #      is 0 → client receives "all server connections to database ... in
    #      use" error. Test fails.
    #
    # After the fix:
    #   1-2 as above.
    #   3. user1 commits one transaction → Pool::return_object calls
    #      coordinator.notify_idle_returned() → Phase C wakes, re-runs
    #      try_evict_one against user1, finds the freshly returned idle in
    #      user1's vec (older than min_connection_lifetime), drops its permit.
    #   4. user2's try_acquire succeeds, the SELECT completes well within
    #      reserve_pool_timeout. Test passes.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      max_db_connections = 2
      min_connection_lifetime = 50
      reserve_pool_size = 0
      reserve_pool_timeout = 800

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      min_pool_size = 0

      [[pools.example_db.users]]
      username = "postgres"
      password = ""
      pool_size = 2
      min_pool_size = 0
      """
    # Warm-up: postgres user has never authenticated before, so its
    # original_server_parameters cache is empty. The first auth has to call
    # `database.get()` to fetch parameters from a real backend session,
    # which itself goes through the coordinator. With reserve_pool_size = 0
    # and the slots about to be pinned by user1, that fetch would fail.
    # Run a throwaway query as postgres now to populate the cache; the
    # cache persists across the warm-up session's drop and survives the
    # later eviction of the postgres backend.
    When we create session "u2_warmup" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_warmup"
    When we close session "u2_warmup"
    # user1 pins both coordinator slots via open transactions (conn is checked
    # out to the client session, not in slots.vec). The first user1 session
    # may need to evict the postgres warm-up connection — sleep 80 ms first
    # so it is past min_connection_lifetime and eligible.
    When we sleep for 80 milliseconds
    When we create session "u1_s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s1"
    And we send SimpleQuery "SELECT 1" to session "u1_s1"
    When we create session "u1_s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "u1_s2"
    And we send SimpleQuery "SELECT 1" to session "u1_s2"
    # Wait past min_connection_lifetime so the user1 connections become
    # evictable as soon as they hit the idle queue.
    When we sleep for 80 milliseconds
    # user2 starts a query — Phase A fails (no idle in postgres user),
    # Phase B finds nothing evictable in user1's vec, Phase C begins to wait.
    When we create session "u2_s1" to pg_doorman as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2_s1" without waiting
    # Give user2 a head start so it is parked in Phase C before user1 commits.
    When we sleep for 100 milliseconds
    # user1 commits one transaction → Pool::return_object fires
    # notify_idle_returned → Phase C wakes → eviction succeeds → user2 gets
    # the slot. Total wall time user2 → response should be well under the
    # 800 ms reserve_pool_timeout.
    When we send SimpleQuery "COMMIT" to session "u1_s1"
    Then we read SimpleQuery response from session "u2_s1" within 600ms
    Then session "u2_s1" should receive DataRow with "1"
    # Verify observability — exactly the expected counter changes.
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOL_COORDINATOR" on admin session "admin" and store response
    # At least one eviction happened (Phase C retry). Upper bound is loose
    # because the BDD step only supports "between A and B".
    Then admin session "admin" column "evictions" should be between 1 and 999999
    # No reserve grants, no client exhaustion errors.
    And admin session "admin" column "reserve_acq" should be between 0 and 0
    And admin session "admin" column "exhaustions" should be between 0 and 0
