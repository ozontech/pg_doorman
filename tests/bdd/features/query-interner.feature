@rust @rust-3 @cache @interner
Feature: Query interner admin surface and missing-anonymous SQLSTATE
  pg_doorman keeps two global interners that deduplicate Parse query texts:
  NAMED entries are bounded by passive Arc::strong_count GC; ANON entries
  are bounded by query_interner_anon_idle_ttl_seconds. The admin SHOW
  INTERNER family exposes the live state without scraping Prometheus, and
  RESET INTERNER clears both halves for diagnostics. A Bind that references
  an anonymous prepared statement which is not in any cache must return
  SQLSTATE 26000 — the same code native PostgreSQL emits.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
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
      query_interner_gc_interval_seconds = 2
      query_interner_anon_idle_ttl_seconds = 4

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  Scenario: SHOW INTERNER reports both named and anonymous kinds
    When we create admin session "admin0" to pg_doorman as "admin" with password "admin"
    And we execute "reset interner" on admin session "admin0" and store response
    And we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt_named" with query "SELECT 11" to session "one"
    And we send Sync to session "one"
    And we send Parse "" with query "SELECT 22" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show interner" on admin session "admin" and store response
    Then admin session "admin" response should contain "named"
    And admin session "admin" response should contain "anonymous"

  Scenario: SHOW INTERNER N orders by interned text length
    When we create admin session "admin0" to pg_doorman as "admin" with password "admin"
    And we execute "reset interner" on admin session "admin0" and store response
    And we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "tiny_stmt" with query "SELECT 1" to session "one"
    And we send Sync to session "one"
    And we send Parse "huge_stmt" with query "SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'::text" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show interner 5" on admin session "admin" and store response
    Then admin session "admin" response should contain "aaaaaaaaaaaaaaaa"
    And admin session "admin" response should contain "named"

  Scenario: RESET INTERNER returns CommandComplete RESET
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "to_be_cleared" with query "SELECT 99" to session "one"
    And we send Sync to session "one"
    And we send Parse "" with query "SELECT 100" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "reset interner" on admin session "admin" and store response
    Then admin session "admin" response should contain "RESET"

  Scenario: Bind without prior anonymous Parse returns SQLSTATE 26000
    When we create session "no_anon" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Bind "" to "" with params "" to session "no_anon"
    Then session "no_anon" should receive ErrorResponse with SQLSTATE "26000"
