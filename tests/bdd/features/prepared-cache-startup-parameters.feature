@rust @rust-3 @prepared-cache @prepared-cache-startup-parameters
Feature: Prepared statement cache and StartupMessage parameters
  These scenarios cover prepared-statement cache bugs that appear when
  sessions in the same transaction pool use different startup-time
  planner GUCs. With pool_size=1 all sessions reuse the same backend,
  so cache collisions and leaked session state are deterministic.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And fixtures from "tests/fixture-search-path.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100
      sync_server_parameters = true

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  # ----------------------------------------------------------------
  # Bug 1: search_path sent in StartupMessage must reach PostgreSQL
  # before the first Parse. Otherwise unqualified table `t` resolves
  # against the role default and PostgreSQL returns 42P01.
  # ----------------------------------------------------------------
  @bug1 @bug1-startup-search-path-not-reaching-backend
  Scenario: Bug 1 — search_path from StartupMessage reaches the backend
    When we create session "A" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "search_path=schema_a"
    And we send Parse "lookup" with query "select val from t" to session "A"
    And we send Sync to session "A"
    And we send Bind "" to "lookup" with params "" to session "A"
    And we send Execute "" to session "A"
    And we send Sync to session "A"
    Then session "A" should receive DataRow with "1"

  # ----------------------------------------------------------------
  # Bug 2: the prepared-cache key must include startup-time planner
  # state. The same query text under schema_a and schema_b needs two
  # server-side prepared statements, not one shared plan.
  # ----------------------------------------------------------------
  @bug2 @bug2-hash-collision-across-startup-parameters @blocked-by-bug1
  Scenario: Bug 2 — distinct startup_parameters yield distinct prepared statements
    When we create session "A" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "search_path=schema_a"
    And we send Parse "lookup" with query "select val from t" to session "A"
    And we send Sync to session "A"
    And we send Bind "" to "lookup" with params "" to session "A"
    And we send Execute "" to session "A"
    And we send Sync to session "A"
    Then session "A" should receive DataRow with "1"
    When we create session "B" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "search_path=schema_b"
    And we send Parse "lookup" with query "select val from t" to session "B"
    And we send Sync to session "B"
    And we send Bind "" to "lookup" with params "" to session "B"
    And we send Execute "" to session "B"
    And we send Sync to session "B"
    Then session "B" should receive DataRow with "2"

  # ----------------------------------------------------------------
  # Bug 2, sticky variant: when the next client does not pin
  # search_path, pg_doorman must RESET the backend. PLAIN should read
  # public.t (3), not schema_a.t (1) left by the previous client.
  # ----------------------------------------------------------------
  @bug2-sticky-search-path @blocked-by-bug1
  Scenario: Bug 2 (sticky) — RESET fires when next client lacks the pinned GUC
    When we create session "PIN" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "search_path=schema_a"
    And we send Parse "lookup_a" with query "select val from t" to session "PIN"
    And we send Sync to session "PIN"
    And we send Bind "" to "lookup_a" with params "" to session "PIN"
    And we send Execute "" to session "PIN"
    And we send Sync to session "PIN"
    Then session "PIN" should receive DataRow with "1"
    When we close session "PIN"
    And we create session "PLAIN" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "lookup_default" with query "select val from t" to session "PLAIN"
    And we send Sync to session "PLAIN"
    And we send Bind "" to "lookup_default" with params "" to session "PLAIN"
    And we send Execute "" to session "PLAIN"
    And we send Sync to session "PLAIN"
    Then session "PLAIN" should receive DataRow with "3"

  # ----------------------------------------------------------------
  # Bug 3: a failed Parse must not leave a stale DOORMAN_N entry in
  # the backend LRU. Reusing the same client statement name should
  # force a fresh Parse and then execute normally.
  # ----------------------------------------------------------------
  @bug3 @bug3-parse-error-poisons-lru-cache
  Scenario: Bug 3 — Parse error does not poison the backend prepared-statement LRU
    When we create session "C" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "broken" with query "select val from nonexistent_t" to session "C"
    And we send Sync to session "C"
    Then session "C" should receive ErrorResponse with SQLSTATE "42P01"
    # Reuse the same client name. A missing rollback would skip
    # re-Parse and fail with 26000; the fixed path reads schema_a.t.
    When we send Parse "broken" with query "select val from schema_a.t" to session "C"
    And we send Bind "" to "broken" with params "" to session "C"
    And we send Execute "" to session "C"
    And we send Sync to session "C"
    Then session "C" should receive DataRow with "1"
