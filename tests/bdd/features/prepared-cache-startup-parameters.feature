@rust @rust-3 @prepared-cache @prepared-cache-startup-parameters
Feature: Prepared statement cache and startup_parameters
  Three independent failing scenarios that pin each of the bugs the
  prepared-statement path has when a client uses
  `startup_parameters` to set planner-visible GUCs. Pool size of one
  and transaction mode share a single backend across sessions, which
  is what surfaces the cache-side collisions without race
  conditions.

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
  # Bug 1 — search_path pinned in the client StartupMessage never
  # reaches the backend because
  # `ServerParameters::set_param(_, _, false)` (called from
  # `client/startup.rs:399,404`) drops every name that is not in
  # `TRACKED_PARAMETERS` (`server/parameters.rs:7-16`). The backend
  # starts with the role-default `search_path` and a single-table
  # query against `t` cannot resolve, so PostgreSQL returns
  # `42P01 relation "t" does not exist`.
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
  # Bug 2 — `Parse::get_hash` (`messages/extended.rs:174-193`)
  # folds only `query` + `num_params` + `param_types`, so two
  # clients of the same `user@db` pool sending the same Parse text
  # but pinning different `search_path` values collide on the cache
  # key. The pool hands the second client the `DOORMAN_N` name
  # allocated for the first; the cached plan was built under the
  # other client's GUCs.
  #
  # This scenario depends on Bug 1's fix to even reach the planner —
  # before that, the first Parse fails on the backend and the second
  # client never gets the chance to receive a wrong row. Once both
  # fixes are in, session B must read `2`, not `1`.
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
  # Bug 3 — `Server::register_prepared_statement`
  # (`server/server_backend.rs:556-632`) inserts the freshly
  # rewritten `DOORMAN_N` name into the per-backend LRU **before**
  # the Parse byte stream is flushed to PostgreSQL. When PG replies
  # with an `ErrorResponse` (in this scenario: 42P01 because the
  # table doesn't exist), the LRU entry is left in place. The next
  # `Bind` against the same client-given statement name sees a
  # phantom hit, skips re-emitting Parse, and PG answers with
  # `prepared statement "DOORMAN_0" does not exist`.
  #
  # The scenario walks that path explicitly: one client, one
  # backend, a Parse against a non-existent table, then a Bind that
  # should — after the fix — re-Parse against the backend instead
  # of relying on the poisoned LRU entry. The post-fix expectation
  # is the same `42P01` from PostgreSQL on the second attempt, not
  # `"prepared statement … does not exist"` originating from a
  # stale cache.
  # ----------------------------------------------------------------
  @bug3 @bug3-parse-error-poisons-lru-cache
  Scenario: Bug 3 — Parse error does not poison the backend prepared-statement LRU
    When we create session "C" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "broken" with query "select val from nonexistent_t" to session "C"
    And we send Sync to session "C"
    And we send Bind "" to "broken" with params "" to session "C"
    And we send Execute "" to session "C"
    And we send Sync to session "C"
    Then session "C" should not receive ErrorResponse with SQLSTATE "26000"
