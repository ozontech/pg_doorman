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
