@rust @rust-3 @cache @anonymous @lru
Feature: Per-client Anonymous LRU keeps Named entries safe
  The per-client prepared statement cache is split into a Named part (unbounded)
  and an Anonymous part (LRU bounded by client_anonymous_prepared_cache_size).
  Anonymous LRU pressure must never evict a Named entry, sibling clients sharing
  the same query hash must remain healthy when one of them evicts, and admin
  consoles must surface the named/anonymous breakdown.

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
      client_anonymous_prepared_cache_size = 2

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  Scenario: Named statement survives anonymous LRU pressure
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Prepare a Named statement that must survive any anonymous LRU pressure.
    And we send Parse "stmt_keep" with query "SELECT 1" to session "one"
    And we send Sync to session "one"
    # Push three distinct anonymous queries through a cache sized for two,
    # forcing at least one anonymous eviction.
    And we send Parse "" with query "SELECT 11" to session "one"
    And we send Sync to session "one"
    And we send Parse "" with query "SELECT 22" to session "one"
    And we send Sync to session "one"
    And we send Parse "" with query "SELECT 33" to session "one"
    And we send Sync to session "one"
    # The Named statement must still resolve after the anonymous churn.
    And we send Bind "" to "stmt_keep" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "1"

  Scenario: Anonymous LRU eviction in one client does not break a sibling sharing the same hash
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Both clients parse the same anonymous query. The pool cache entry is shared
    # by hash; per-client caches are independent.
    And we send Parse "" with query "SELECT 'shared'::text" to session "client_a"
    And we send Sync to session "client_a"
    And we send Parse "" with query "SELECT 'shared'::text" to session "client_b"
    And we send Sync to session "client_b"
    # Drive client_a past its anonymous LRU limit so its local entry for
    # "SELECT 'shared'" is evicted.
    And we send Parse "" with query "SELECT 'one'::text" to session "client_a"
    And we send Sync to session "client_a"
    And we send Parse "" with query "SELECT 'two'::text" to session "client_a"
    And we send Sync to session "client_a"
    # client_b never grew its cache past one entry, so its mapping is intact and
    # the Bind+Execute on the previously parsed anonymous statement must succeed.
    And we send Bind "" to "" with params "" to session "client_b"
    And we send Execute "" to session "client_b"
    And we send Sync to session "client_b"
    Then session "client_b" should receive DataRow with "shared"

  Scenario: SHOW PREPARED_STATEMENTS classifies entries by kind
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Prepare a Named entry, then an anonymous one with a distinct query.
    And we send Parse "stmt_named" with query "SELECT 'A'::text" to session "one"
    And we send Sync to session "one"
    And we send Parse "" with query "SELECT 'B'::text" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show prepared_statements" on admin session "admin" and store response
    # Both queries must appear with their respective kinds.
    Then admin session "admin" response should contain "SELECT 'A'"
    And admin session "admin" response should contain "named"
    And admin session "admin" response should contain "SELECT 'B'"
    And admin session "admin" response should contain "anonymous"

  Scenario: SHOW POOLS_MEMORY exposes named and anonymous breakdown columns
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # One Named statement and one anonymous statement so both counters are non-zero.
    And we send Parse "stmt_one" with query "SELECT 1" to session "one"
    And we send Sync to session "one"
    And we send Parse "" with query "SELECT 2" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_memory" on admin session "admin" and store response
    Then admin session "admin" response should contain "client_named_count"
    And admin session "admin" response should contain "client_anonymous_count"
    And admin session "admin" response should contain "client_anonymous_evictions"
