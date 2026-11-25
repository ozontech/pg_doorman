@rust @rust-3 @cleanup-review @cleanup-prepare
Feature: Adaptive cleanup removes SQL PREPARE statements left by a client
  SQL PREPARE and an extended-protocol Parse share one server-side
  statement namespace. An untracked PREPARE made the next client on the
  same backend fail with 42P05 duplicate_prepared_statement.
  The checkin DEALLOCATE ALL also removes every pooler-cached DOORMAN_*
  statement of that backend. A broken cache drop surfaces on the next
  client as 26000 prepared statement does not exist on a cached Bind.
  The scenarios cover single statements, pipelined batches, multi-statement
  Query batches, and the two error paths that arm the same flag.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      web:
        enabled: true
        host: "127.0.0.1"
        port: 9129
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
        example_db_session:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          server_database: "example_db"
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """

  @cleanup-prepare-statement
  Scenario: A SQL PREPARE statement does not survive a checkin
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "PREPARE leak_stmt AS SELECT 42" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    # The same name must be preparable again: 42P05 means the statement leaked.
    And we send SimpleQuery "PREPARE leak_stmt AS SELECT 42" to session "next" and store response
    Then session "next" should receive CommandComplete "PREPARE"

  @cleanup-prepare-deallocate-all
  Scenario: A client DEALLOCATE ALL suppresses the checkin DEALLOCATE ALL
    # Session pooling has a single checkin at disconnect, so PREPARE and the
    # client's DEALLOCATE ALL share one lease.
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "PREPARE keep_stmt AS SELECT 42" to session "old"
    And we send SimpleQuery "DEALLOCATE ALL" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "next" and store response
    Then session "next" should receive DataRow with "0"
    # The client's own DEALLOCATE ALL already emptied the view, so the counter
    # is what pins suppression. Its label is the pool name, and creating the
    # next lease waits for the previous checkin to finish.
    When I run shell command:
      """
      python3 - <<'PY'
      import time
      import urllib.request
      deadline = time.monotonic() + 5
      while True:
          body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
          lines = body.splitlines()
          ok = 'pg_doorman_server_cleanup_total{database="example_db_session",result="ok",user="example_user_1"} 0' in lines
          err = 'pg_doorman_server_cleanup_total{database="example_db_session",result="error",user="example_user_1"} 0' in lines
          if ok and err:
              break
          assert time.monotonic() < deadline, body
          time.sleep(0.05)
      PY
      """
    Then the command should succeed

  @cleanup-prepare-mixed-cache
  Scenario: A SQL PREPARE checkin invalidates the cached extended statement
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    And we send Parse "mixed" with query "select $1::int + $1::int" to session "one"
    And we send Bind "" to "mixed" with params "20" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "40"
    # The SQL PREPARE tag arms the flag; the next checkin sends DEALLOCATE ALL,
    # which removes both the manual statement and the cached DOORMAN_* name.
    When we send SimpleQuery "PREPARE mixed_manual AS SELECT 1" to session "one" and store response
    Then session "one" should receive CommandComplete "PREPARE"
    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    # If the checkin left the stale LRU entry behind, this Parse would be
    # skipped as a cache hit and the Bind would fail with 26000.
    And we send Parse "mixed" with query "select $1::int + $1::int" to session "next"
    And we send Bind "" to "mixed" with params "5" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "10"
    When we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements WHERE name = 'mixed_manual'" to session "next" and store response
    Then session "next" should receive DataRow with "0"

  @cleanup-prepare-batch
  Scenario: A pipelined batch and a multi-statement Query batch invalidate the cache together
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    # One pipelined extended batch: two Parses, two Binds, two Executes, one Sync.
    And we send Parse "batch_a" with query "select $1::int + 1" to session "one"
    And we send Parse "batch_b" with query "select $1::int * 2" to session "one"
    And we send Bind "" to "batch_a" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Bind "" to "batch_b" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "11"
    # A multi-statement Query: the PREPARE tag arms the flag mid-batch.
    When we send SimpleQuery "SELECT 1; PREPARE batch_manual AS SELECT 2; SELECT 3" to session "one" and store response
    Then session "one" should receive CommandComplete "PREPARE"
    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    # Both cached entries must be gone: each Bind re-parses instead of
    # failing with 26000.
    And we send Parse "batch_a" with query "select $1::int + 1" to session "next"
    And we send Bind "" to "batch_a" with params "100" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "101"
    And we send Parse "batch_b" with query "select $1::int * 2" to session "next"
    And we send Bind "" to "batch_b" with params "100" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "200"

  @cleanup-prepare-query-error
  Scenario: A failed query checkin invalidates the cached extended statement
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    And we send Parse "err_cached" with query "select $1::int * 3" to session "one"
    And we send Bind "" to "err_cached" with params "14" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "42"
    # Any ErrorResponse arms prepare-cleanup while the cache is enabled.
    When we send SimpleQuery "SELECT 1 / 0" to session "one" expecting error
    Then session "one" should receive ErrorResponse with SQLSTATE "22012"
    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    And we send Parse "err_cached" with query "select $1::int * 3" to session "next"
    And we send Bind "" to "err_cached" with params "7" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "21"

  @cleanup-prepare-parse-error
  Scenario: A failed Parse in a batch still invalidates the earlier cached statement
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    And we send Parse "good_early" with query "select $1::int + 100" to session "one"
    And we send Bind "" to "good_early" with params "1" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "101"
    # The broken Parse fails at Sync. Its registration rolls back, but the
    # error still arms prepare-cleanup for the surviving cached entry.
    When we send Parse "broken" with query "selec broken syntax here" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive ErrorResponse with SQLSTATE "42601"
    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    And we send Parse "good_early" with query "select $1::int + 100" to session "next"
    And we send Bind "" to "good_early" with params "23" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "123"

  @cleanup-prepare-session-named-parse
  Scenario: A session-mode named Parse does not leak to the next client
    # Session pooling forwards Parse as-is: the client owns the backend for
    # the whole connection. The forwarded name must not outlive the lease.
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid
    And we send Parse "forwarded" with query "select $1::int + 5" to session "one"
    And we send Bind "" to "forwarded" with params "1" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "6"
    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    # 42P05 here would mean the forwarded statement leaked across clients.
    And we send Parse "forwarded" with query "select $1::int + 5" to session "next"
    And we send Bind "" to "forwarded" with params "2" to session "next"
    And we send Execute "" to session "next"
    And we send Sync to session "next"
    Then session "next" should receive DataRow with "7"
