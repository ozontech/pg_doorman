@rust @rust-3 @prepared-cache @bug
Feature: Server-side LRU eviction during batch breaks already-buffered Bind

  When a client sends a batch like Parse(A), Bind(A), Parse(C), Bind(C), Sync:
  1. Parse(A) is skipped (already on server) — no Parse bytes in buffer
  2. Bind(A) is added to client buffer
  3. Parse(C) triggers register_prepared_statement → add_to_cache evicts A
     from server LRU → Close(A)+Sync sent to PostgreSQL out-of-band → A deleted
  4. Parse(C) bytes added to client buffer
  5. Sync flushes buffer: Bind(A) fails — "prepared statement does not exist"

  The root cause is two-fold:
  - has_prepared_statement() uses LruCache::contains() which does NOT promote
    entries in the LRU, so actively-used statements can still be evicted
  - Eviction Close is sent out-of-band (bypassing client buffer ordering)

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  Scenario: Bind for statement evicted by subsequent Parse in same batch
    # Server-side LRU cache_size=2 means 3rd statement triggers eviction
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

    # Step 1: Fill server LRU to capacity with statements A and B
    When we create session "setup" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select $1::int" to session "setup"
    And we send Bind "" to "a" with params "1" to session "setup"
    And we send Execute "" to session "setup"
    And we send Sync to session "setup"
    Then session "setup" should receive DataRow with "1"

    And we send Parse "b" with query "select $1::text" to session "setup"
    And we send Bind "" to "b" with params "hello" to session "setup"
    And we send Execute "" to session "setup"
    And we send Sync to session "setup"
    Then session "setup" should receive DataRow with "hello"

    When we close session "setup"
    And we sleep 100ms

    # Step 2: New session sends batch where Parse(C) evicts A from server LRU
    # while Bind(A) is already in the client buffer
    #
    # Server LRU = [A, B] (A is LRU because has() uses contains() — no promotion)
    # Parse(A) → server has A → skip (no Parse bytes in buffer)
    # Bind(A) → ensure_on_server → has(A)=true → skip → Bind(A) in buffer
    # Parse(C) → new → register → add_to_cache(C) → evicts A → Close(A) sent!
    # Bind(C) → Bind(C) in buffer
    # Sync → flush: Bind(A) hits PostgreSQL where A no longer exists → ERROR
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select $1::int" to session "test"
    And we send Bind "" to "a" with params "42" to session "test"
    And we send Parse "c" with query "select $1::int + 1" to session "test"
    And we send Bind "" to "c" with params "99" to session "test"
    And we send Execute "" to session "test"
    And we send Execute "" to session "test"
    And we send Sync to session "test"
    # This SHOULD work — both statements should execute correctly.
    # Currently fails because eviction Close(A) is sent out-of-band.
    Then session "test" should receive DataRow with "42"

  Scenario: Same bug with three statements filling cache progressively
    # Variant: fill cache one-by-one across transactions, then trigger in batch
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

    # Fill server LRU: [A, B]
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select $1::int as val" to session "s1"
    And we send Bind "" to "a" with params "10" to session "s1"
    And we send Execute "" to session "s1"
    And we send Parse "b" with query "select $1::int * 2 as val" to session "s1"
    And we send Bind "" to "b" with params "10" to session "s1"
    And we send Execute "" to session "s1"
    And we send Sync to session "s1"
    Then session "s1" should receive DataRow with "10"
    When we close session "s1"
    And we sleep 100ms

    # Reuse A, then introduce C which evicts A
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select $1::int as val" to session "s2"
    And we send Bind "" to "a" with params "77" to session "s2"
    And we send Execute "" to session "s2"
    And we send Parse "c" with query "select $1::int - 1 as val" to session "s2"
    And we send Bind "" to "c" with params "50" to session "s2"
    And we send Execute "" to session "s2"
    And we send Sync to session "s2"
    Then session "s2" should receive DataRow with "77"

  Scenario: No eviction when batch stays within cache capacity
    # Sanity check: with cache_size=2 and only 2 statements, no eviction occurs
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

    When we create session "setup" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select $1::int" to session "setup"
    And we send Bind "" to "a" with params "1" to session "setup"
    And we send Execute "" to session "setup"
    And we send Parse "b" with query "select $1::text" to session "setup"
    And we send Bind "" to "b" with params "x" to session "setup"
    And we send Execute "" to session "setup"
    And we send Sync to session "setup"
    Then session "setup" should receive DataRow with "1"
    When we close session "setup"
    And we sleep 100ms

    # Reuse both A and B — no new statement, no eviction, should work
    When we create session "test" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "a" with query "select $1::int" to session "test"
    And we send Bind "" to "a" with params "42" to session "test"
    And we send Execute "" to session "test"
    And we send Parse "b" with query "select $1::text" to session "test"
    And we send Bind "" to "b" with params "ok" to session "test"
    And we send Execute "" to session "test"
    And we send Sync to session "test"
    Then session "test" should receive DataRow with "42"
