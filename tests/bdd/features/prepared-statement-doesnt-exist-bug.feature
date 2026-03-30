@rust @prepared-cache @bug
Feature: Prepared statement cache desync on client disconnect before Sync

  When a client sends Parse but disconnects before Sync/Flush,
  pg_doorman registers the statement in the server-side LRU cache
  (via register_parse_to_server_cache with should_send_parse_to_server=false)
  but the actual Parse message is never sent to PostgreSQL (it stays in the buffer
  which is dropped on disconnect). The server cache now thinks the statement exists
  on PostgreSQL, but it doesn't.

  The next client that gets the same server and sends Parse for the same query
  will see has_prepared_statement() = true, skip sending Parse to PostgreSQL,
  and send only Bind — which fails with "prepared statement does not exist".

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: Client disconnect after Parse without Sync causes stale server cache
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "one"
    And we close session "one"
    And we sleep 200ms

    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "two"
    And we send Bind "" to "s1" with params "10, 20" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "30"

  Scenario: TCP abort after Parse without Sync causes stale server cache
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select 42" to session "one"
    And we abort TCP connection for session "one"
    And we sleep 200ms

    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select 42" to session "two"
    And we send Bind "" to "s1" with params "" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "42"

  Scenario: Parse with Sync before disconnect does NOT cause stale cache
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "one"
    And we send Bind "" to "s1" with params "1, 2" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "3"
    When we close session "one"
    And we sleep 200ms

    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "two"
    And we send Bind "" to "s1" with params "10, 20" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "30"

  # --- Edge cases ---

  Scenario: Multiple stale Parse messages from a single client disconnect
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    # Client sends multiple Parse messages for different queries, then disconnects.
    # All of them end up in the server cache but none reach PostgreSQL.
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "add" with query "select $1::int + $2::int" to session "one"
    And we send Parse "mul" with query "select $1::int * $2::int" to session "one"
    And we send Parse "sub" with query "select $1::int - $2::int" to session "one"
    And we close session "one"
    And we sleep 200ms

    # Next client uses all three queries — each must work
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "add" with query "select $1::int + $2::int" to session "two"
    And we send Bind "" to "add" with params "10, 5" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "15"

    And we send Parse "mul" with query "select $1::int * $2::int" to session "two"
    And we send Bind "" to "mul" with params "10, 5" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "50"

    And we send Parse "sub" with query "select $1::int - $2::int" to session "two"
    And we send Bind "" to "sub" with params "10, 5" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "5"

  Scenario: Parse+Bind+Execute without Sync then disconnect
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    # Full batch without Sync — everything is in client buffer, nothing sent to PG
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "one"
    And we send Bind "" to "s1" with params "1, 2" to session "one"
    And we send Execute "" to session "one"
    # No Sync!
    And we close session "one"
    And we sleep 200ms

    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "two"
    And we send Bind "" to "s1" with params "10, 20" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "30"

  Scenario: Rapid disconnect-reconnect cycle preserves cache consistency
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    # Three rapid disconnect cycles with Parse-only, same server
    When we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select 1" to session "c1"
    And we close session "c1"
    And we sleep 100ms

    When we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select 1" to session "c2"
    And we close session "c2"
    And we sleep 100ms

    When we create session "c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select 1" to session "c3"
    And we close session "c3"
    And we sleep 100ms

    # After three stale cycles, the fourth client must still work
    When we create session "ok" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select 1" to session "ok"
    And we send Bind "" to "s1" with params "" to session "ok"
    And we send Execute "" to session "ok"
    And we send Sync to session "ok"
    Then session "ok" should receive DataRow with "1"

  Scenario: Successful transaction after stale Parse clears the flag
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    # Client 1: stale Parse
    When we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to session "c1"
    And we close session "c1"
    And we sleep 200ms

    # Client 2: successful full transaction re-establishes the statement
    When we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to session "c2"
    And we send Bind "" to "s1" with params "42" to session "c2"
    And we send Execute "" to session "c2"
    And we send Sync to session "c2"
    Then session "c2" should receive DataRow with "42"
    When we close session "c2"
    And we sleep 100ms

    # Client 3: reuses the now-valid cached statement — must work without re-parse on server
    When we create session "c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int" to session "c3"
    And we send Bind "" to "s1" with params "99" to session "c3"
    And we send Execute "" to session "c3"
    And we send Sync to session "c3"
    Then session "c3" should receive DataRow with "99"

  Scenario: Different query after stale Parse does not collide
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    # Client 1: stale Parse for query A
    When we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s1" with query "select $1::int + $2::int" to session "c1"
    And we close session "c1"
    And we sleep 200ms

    # Client 2: uses a completely different query — should not be affected
    When we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "s2" with query "select $1::text || ' hello'" to session "c2"
    And we send Bind "" to "s2" with params "world" to session "c2"
    And we send Execute "" to session "c2"
    And we send Sync to session "c2"
    Then session "c2" should receive DataRow with "world hello"

  Scenario: Cache eviction during stale Parse does not break cleanup
    # With cache_size=2, the third Parse triggers eviction.
    # All three are stale (no Sync). Cleanup must still work.
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 2

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select 1" to session "one"
    And we send Parse "b" with query "select 2" to session "one"
    And we send Parse "c" with query "select 3" to session "one"
    And we close session "one"
    And we sleep 200ms

    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select 1" to session "two"
    And we send Bind "" to "a" with params "" to session "two"
    And we send Execute "" to session "two"
    And we send Sync to session "two"
    Then session "two" should receive DataRow with "1"
