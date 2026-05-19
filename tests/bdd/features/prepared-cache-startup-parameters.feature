@rust @rust-3 @prepared-cache @prepared-cache-startup-parameters
Feature: Prepared statement cache splits clients by their startup_parameters
  Two clients connect to the same user@db pool but pin different
  search_path values in their StartupMessage. Each schema holds a
  table named `t` with a different value. Without a fix, the pool's
  prepared-statement cache keys by Parse text only — the second client
  receives the DOORMAN_N allocated for the first and reads the wrong
  row because the backend plan was built under the first client's
  search_path. Pool size of one and transaction mode force the two
  clients to share a single backend, surfacing the collision in a
  single test instead of relying on race conditions.

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

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  Scenario: search_path in startup packet must not be shadowed by the cache
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
