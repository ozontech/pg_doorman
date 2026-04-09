@rust @rust-3 @client-session-reset-cleanup
Feature: Client session reset batch suppresses doorman-side cleanup
  When a client (e.g. jackc/pgx on an internal context deadline) keeps the
  server connection alive by running a self-cleanup batch such as

      SET SESSION AUTHORIZATION DEFAULT;
      RESET ALL;
      CLOSE ALL;
      UNLISTEN *;
      SELECT pg_advisory_unlock_all();
      DISCARD PLANS;
      DISCARD SEQUENCES;
      DISCARD TEMP;

  pg_doorman must recognise that the session is already clean and skip the
  checkin-time `RESET ROLE; RESET ALL; ...` round-trip it would otherwise send.
  Without this recognition every dangling client query triples the traffic to
  PostgreSQL: the original (stuck) query, the client's self-reset batch, and a
  redundant pg_doorman reset.

  Each scenario starts PostgreSQL with `log_statement = 'all'` and asserts the
  absence or presence of pg_doorman's cleanup statements in the server log.
  The `RESET ROLE` prefix is used as a marker because pg_doorman always prefixes
  its cleanup batch with `RESET ROLE;` and clients from the real world do not
  issue it.

  Background:
    Given PostgreSQL started with options "-c log_statement=all -c logging_collector=off" and pg_hba.conf:
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

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2

      [pools.example_db_session]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      server_database = "example_db"
      pool_mode = "session"

      [[pools.example_db_session.users]]
      username = "example_user_1"
      password = ""
      pool_size = 2
      """

  @client-session-reset-cleanup-pgx-batch
  Scenario: pgx-style session reset batch does not trigger a second server cleanup
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Warm the pool with a trivial query so that server auth and any startup
    # chatter is already in the log before we start asserting on it.
    And we send SimpleQuery "SELECT 1" to session "one"
    And we sleep 100ms
    When we truncate PostgreSQL log
    # Exactly the batch jackc/pgx emits on an internal context deadline.
    And we send SimpleQuery "SET SESSION AUTHORIZATION DEFAULT; RESET ALL; CLOSE ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); DISCARD PLANS; DISCARD SEQUENCES; DISCARD TEMP" to session "one"
    And we sleep 300ms
    # Client batch itself still shows up once — that is the one we expect.
    Then PostgreSQL log should contain exactly 1 occurrences of "RESET ALL"
    # pg_doorman's checkin cleanup would have prefixed its batch with RESET ROLE.
    # Its absence proves the second cleanup was suppressed.
    And PostgreSQL log should not contain "RESET ROLE"

  @client-session-reset-cleanup-real-set-still-cleans
  Scenario: a genuine SET still arms the checkin cleanup
    # Baseline: if the client actually mutates session state and does not
    # follow up with RESET/DISCARD, pg_doorman must still clean up on checkin.
    # This guards against the fix over-correcting and swallowing real cleanups.
    When we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "two"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "SET statement_timeout = 1000" to session "two"
    And we sleep 300ms
    # Client sent `RESET ALL` zero times — the one we expect is pg_doorman's.
    Then PostgreSQL log should contain exactly 1 occurrences of "RESET ALL"
    And PostgreSQL log should contain "RESET ROLE"

  @client-session-reset-cleanup-discard-all
  Scenario: DISCARD ALL after SET suppresses doorman-side cleanup (session mode)
    # DISCARD ALL cannot run inside an implicit transaction block, so it has to
    # be sent as a standalone SimpleQuery. That only works with session mode,
    # where pg_doorman keeps the same server connection across multiple client
    # queries and defers checkin_cleanup until the client disconnects.
    When we create session "three" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT 1" to session "three"
    And we sleep 100ms
    When we truncate PostgreSQL log
    # SET arms set-cleanup; a subsequent DISCARD ALL in the same session must
    # disarm every cleanup flag because DISCARD ALL is semantically
    # `RESET ALL; DEALLOCATE ALL; CLOSE ALL; UNLISTEN *; ...`.
    And we send SimpleQuery "SET statement_timeout = 1000" to session "three"
    And we send SimpleQuery "DISCARD ALL" to session "three"
    # Close the session so pg_doorman returns the server connection and runs
    # checkin_cleanup — which must be a no-op thanks to the DISCARD ALL disarm.
    And we close session "three"
    And we sleep 300ms
    Then PostgreSQL log should contain "DISCARD ALL"
    # No `RESET ALL` from either side: client did not issue one, and pg_doorman
    # learned from the DISCARD ALL tag that the session is already clean.
    And PostgreSQL log should not contain "RESET ROLE"
    And PostgreSQL log should contain exactly 0 occurrences of "RESET ALL"

  @client-session-reset-cleanup-close-all-disarms-declare
  Scenario: CLOSE ALL in the same batch as DECLARE suppresses declare cleanup
    When we create session "four" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "four"
    And we sleep 100ms
    When we truncate PostgreSQL log
    # DECLARE CURSOR arms declare cleanup (CLOSE ALL on checkin); the same batch
    # explicitly closes every cursor, so pg_doorman must recognise the server
    # state is already clean and skip its own CLOSE ALL.
    And we send SimpleQuery "BEGIN; DECLARE doorman_cur CURSOR FOR SELECT 1; CLOSE ALL; COMMIT" to session "four"
    And we sleep 300ms
    # Sanity-check the client batch is actually in the log (PostgreSQL logs the
    # whole simple-query string on one line when log_statement = 'all').
    Then PostgreSQL log should contain "DECLARE doorman_cur"
    # And the marker for pg_doorman's own checkin cleanup is absent.
    And PostgreSQL log should not contain "RESET ROLE"

  @client-session-reset-cleanup-per-guc-reset
  Scenario: Per-GUC RESET disarms set-cleanup
    # PostgreSQL returns the same `RESET` tag for `RESET ALL` and `RESET foo`,
    # so a per-GUC RESET after a SET on the same GUC leaves the session clean
    # as far as pg_doorman is concerned. Documents the intentional trade-off:
    # `SET a=1; SET b=2; RESET a;` would also be treated as clean, because
    # pg_doorman only tracks a single cleanup bit and cannot distinguish which
    # GUCs are still modified.
    When we create session "five" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT 1" to session "five"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "SET statement_timeout = 1000" to session "five"
    And we send SimpleQuery "RESET statement_timeout" to session "five"
    And we close session "five"
    And we sleep 300ms
    Then PostgreSQL log should contain "RESET statement_timeout"
    And PostgreSQL log should not contain "RESET ROLE"

  @client-session-reset-cleanup-single-close-keeps-armed
  Scenario: Closing one named cursor does not disarm declare-cleanup
    # Only `CLOSE CURSOR ALL` carries the disarm semantics. Closing a single
    # named cursor emits `CLOSE CURSOR` (no ALL), which leaves other cursors
    # open and must not clear the cleanup flag. pg_doorman has to follow up
    # with its own `CLOSE ALL` on checkin.
    When we create session "six" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "six"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "BEGIN; DECLARE doorman_c1 CURSOR FOR SELECT 1; DECLARE doorman_c2 CURSOR FOR SELECT 2; CLOSE doorman_c1; COMMIT" to session "six"
    And we sleep 300ms
    # The client closed only c1 — c2 is still defined on the server until the
    # implicit transaction ended via COMMIT, so pg_doorman stayed armed and
    # issued its own cleanup batch.
    Then PostgreSQL log should contain "RESET ROLE"
    And PostgreSQL log should contain "CLOSE ALL"

  @client-session-reset-cleanup-error-arms-prepare
  Scenario: PostgreSQL error arms prepare-cleanup, forcing DEALLOCATE ALL on checkin
    # Baseline for the prepare-cleanup path: an ErrorResponse while the
    # prepared-statement cache is enabled sets `needs_cleanup_prepare`, and
    # pg_doorman must still issue `DEALLOCATE ALL` on checkin.
    When we create session "seven" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT 1" to session "seven"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "SELECT 1/0" to session "seven" expecting error
    And we close session "seven"
    And we sleep 300ms
    Then PostgreSQL log should contain "DEALLOCATE ALL"

  @client-session-reset-cleanup-discard-after-error
  Scenario: DISCARD ALL after a PostgreSQL error disarms prepare-cleanup
    # After the error arms `needs_cleanup_prepare`, a subsequent DISCARD ALL
    # clears every cleanup flag. No redundant `DEALLOCATE ALL` on checkin.
    When we create session "eight" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT 1" to session "eight"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "SELECT 1/0" to session "eight" expecting error
    And we send SimpleQuery "DISCARD ALL" to session "eight"
    And we close session "eight"
    And we sleep 300ms
    Then PostgreSQL log should contain "DISCARD ALL"
    # Neither the client nor pg_doorman issued DEALLOCATE ALL.
    And PostgreSQL log should contain exactly 0 occurrences of "DEALLOCATE ALL"
    And PostgreSQL log should not contain "RESET ROLE"
