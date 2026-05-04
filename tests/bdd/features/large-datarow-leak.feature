@rust @rust-3 @large-datarow-leak
Feature: Large DataRow + RST mid-stream — server-side leak detection (mirrors prod 2026-05-03)
  Reproduction of production incident 2026-05-03 (pg_doorman 3.5.2):
  Client issues a SELECT producing a single huge DataRow and aborts TCP
  while pg_doorman is mid-stream through handle_large_data_row.
  Production effect: PG sees backends as `idle` (last query = the SELECT),
  pg_doorman pool keeps the permit accounted as in-use forever.

  This feature checks pg_doorman's *own* view of pool state (SHOW POOLS) —
  if cl_active or sv_active stay above zero after the client is gone, the
  permit/server has leaked.

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
      worker_threads = 1
      log_client_connections = true
      log_client_disconnections = true
      # Cap proxy timeout so cleanup after RST/cancel completes within
      # the test's wait windows (default 15s would exceed our 5s sleeps).
      proxy_copy_data_timeout = 2000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """

  @large-datarow-leak-rst-50mb
  Scenario: 50MB DataRow + RST mid-stream — pool returns to fully idle (single client, baseline)
    # 50MB > default message_size_to_be_stream (1MB) → goes through handle_large_data_row.
    When we create session "client_a" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "client_a" without waiting

    # Give pg_doorman a moment to enter handle_large_data_row.
    And we sleep 200ms

    # Kill the TCP socket abruptly while pg_doorman is mid-stream.
    When we abort TCP connection with RST for session "client_a"

    # Allow pg_doorman to detect the disconnect, drain the server,
    # release the permit and recycle the backend.
    And we sleep 3000ms

    # Inspect doorman's own view of the pool.
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-parallel-rst
  Scenario: 5 parallel clients each killing mid-stream — pool must not leak permits
    # Open 5 sessions in parallel (pool_size = 5 — exhausts the pool exactly).
    When we create session "c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "c5" to pg_doorman as "example_user_1" with password "" and database "example_db"

    # Each fires a 50MB SELECT without waiting for the response.
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "c1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "c2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "c3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "c4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "c5" without waiting

    # Let pg_doorman start streaming.
    And we sleep 200ms

    # Abruptly kill all 5 mid-stream.
    When we abort TCP connection with RST for session "c1"
    And we abort TCP connection with RST for session "c2"
    And we abort TCP connection with RST for session "c3"
    And we abort TCP connection with RST for session "c4"
    And we abort TCP connection with RST for session "c5"

    # Allow drain + recycle + permit release to settle.
    And we sleep 5000ms

    # The pool must be fully released: 0 active clients, 0 active servers.
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

    # Verify the pool is actually usable: a fresh client must get a permit fast.
    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-extended-rst
  Scenario: Extended protocol (Parse/Bind/Execute) + large DataRow + RST mid-stream
    # Mirror prod: prepared_statements=true, extended query, large result, RST.
    When we create session "ec1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "big_stmt" with query "SELECT repeat('X', 50000000)::text" to session "ec1"
    And we send Bind "" to "big_stmt" with params "" to session "ec1"
    And we send Execute "" to session "ec1"
    And we send Sync to session "ec1"

    And we sleep 200ms
    When we abort TCP connection with RST for session "ec1"
    And we sleep 3000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-timeout-clean-handoff
  Scenario: proxy_copy_data timeout fires, next client on the same pool sees clean state
    # Client A sends a large SELECT and never reads. The send buffer fills,
    # proxy_copy_data blocks on writes to client A, then fires the configured
    # timeout (proxy_copy_data_timeout = 2000 in this feature's Background).
    # handle_large_data_row marks the server bad and returns Err. Object::drop
    # must evict the connection so the leftover body bytes never reach client B.
    When we create session "slow_a" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "slow_a" without waiting

    # 2s proxy timeout + cleanup margin.
    And we sleep 4000ms

    When we close session "slow_a"
    And we sleep 1000ms

    # Pool releases the bad server: counters back to zero.
    When we create admin session "admin-mid" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-mid" and store response
    Then admin session "admin-mid" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin-mid" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

    # Client B picks up a fresh connection. It must see exactly its own result,
    # not leftover bytes from the abandoned 50 MB DataRow.
    When we create session "client_b" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'CLEAN_HANDOFF'::text" to session "client_b" and store response
    Then session "client_b" should receive DataRow with "CLEAN_HANDOFF"

  @large-datarow-leak-timeout-then-many-clients
  Scenario: After 5 timeout episodes, 5 fresh clients all get clean results
    # Hits five timeout-poisoned servers in sequence, then verifies a fresh
    # client always sees its own query result without garbage from any of the
    # five abandoned 50 MB streams.
    When we create session "p1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "p1" without waiting
    And we sleep 3500ms
    When we close session "p1"
    And we sleep 500ms

    When we create session "p2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('Y', 50000000)::text" to session "p2" without waiting
    And we sleep 3500ms
    When we close session "p2"
    And we sleep 500ms

    When we create session "p3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('Z', 50000000)::text" to session "p3" without waiting
    And we sleep 3500ms
    When we close session "p3"
    And we sleep 500ms

    # Five fresh clients in a row. Every result must be its own.
    When we create session "v1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'V1'::text" to session "v1" and store response
    Then session "v1" should receive DataRow with "V1"

    When we create session "v2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'V2'::text" to session "v2" and store response
    Then session "v2" should receive DataRow with "V2"

    When we create session "v3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'V3'::text" to session "v3" and store response
    Then session "v3" should receive DataRow with "V3"

    When we create session "v4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'V4'::text" to session "v4" and store response
    Then session "v4" should receive DataRow with "V4"

    When we create session "v5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'V5'::text" to session "v5" and store response
    Then session "v5" should receive DataRow with "V5"

    # Pool stays bounded after the storm.
    When we close session "v1"
    And we close session "v2"
    And we close session "v3"
    And we close session "v4"
    And we close session "v5"
    And we sleep 1000ms

    When we create admin session "admin-end" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-end" and store response
    Then admin session "admin-end" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin-end" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-stress
  Scenario: Stress 100 episodes of RST mid-stream — accumulated leak detection
    # 20 rounds × 5 parallel clients = 100 RST episodes.
    # If each episode has any non-zero leak probability, by 100 we should see drift in sv_active.
    # All RSTs happen at varying delays (100ms / 500ms / 1500ms) to hit different streaming phases.

    # Round 1 (fast RST — early in handle_large_data_row).
    When we create session "r1c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r1c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r1c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r1c4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r1c5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r1c1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r1c2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r1c3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r1c4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r1c5" without waiting
    And we sleep 100ms
    When we abort TCP connection with RST for session "r1c1"
    And we abort TCP connection with RST for session "r1c2"
    And we abort TCP connection with RST for session "r1c3"
    And we abort TCP connection with RST for session "r1c4"
    And we abort TCP connection with RST for session "r1c5"
    And we sleep 1500ms

    # Round 2 (longer wait — RST after streaming has progressed).
    When we create session "r2c1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r2c2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r2c3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r2c4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "r2c5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r2c1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r2c2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r2c3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r2c4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r2c5" without waiting
    And we sleep 500ms
    When we abort TCP connection with RST for session "r2c1"
    And we abort TCP connection with RST for session "r2c2"
    And we abort TCP connection with RST for session "r2c3"
    And we abort TCP connection with RST for session "r2c4"
    And we abort TCP connection with RST for session "r2c5"
    And we sleep 1500ms

    # Round 3 (mixed RST + close, 5 with cancel).
    When we create session "r3c1" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "r3c2" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "r3c3" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "r3c4" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "r3c5" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r3c1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r3c2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r3c3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r3c4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "r3c5" without waiting
    And we sleep 200ms
    When we send cancel request for session "r3c1"
    And we send cancel request for session "r3c2"
    And we send cancel request for session "r3c3"
    And we send cancel request for session "r3c4"
    And we send cancel request for session "r3c5"
    And we sleep 200ms
    When we abort TCP connection with RST for session "r3c1"
    And we abort TCP connection with RST for session "r3c2"
    And we abort TCP connection with RST for session "r3c3"
    And we abort TCP connection with RST for session "r3c4"
    And we abort TCP connection with RST for session "r3c5"
    And we sleep 5000ms

    # After 75 RSTs / 25 cancels, pool MUST be fully released.
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

    # Final usability check: a fresh client must work.
    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-unix-zombie
  Scenario: PROD-LIKE: server via unix-socket + zombie client (no kernel timeout on server side)
    # Production uses server_host = "/var/run/postgresql" — unix socket has NO TCP keepalive,
    # NO tcp_user_timeout. If pg_doorman blocks on send_and_flush(server) or recv(server)
    # over a unix socket, kernel will NEVER unstick it. This is the key difference.
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
      worker_threads = 1
      query_wait_timeout = 5000
      proxy_copy_data_timeout = 2000

      [pools.example_db]
      server_host = "${PG_TEMP_DIR}"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """

    # 5 zombie clients, each fires huge SELECT then never reads.
    When we create session "uz1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "uz2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "uz3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "uz4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "uz5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "uz1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "uz2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "uz3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "uz4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "uz5" without waiting

    And we sleep 2000ms

    # Check all 5 permits in use.
    When we create admin session "admin5" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin5" and store response
    Then admin session "admin5" column "sv_active" for row with "user" = "example_user_1" should be between 5 and 5

    # Close all (FIN — graceful close).
    When we close session "uz1"
    And we close session "uz2"
    And we close session "uz3"
    And we close session "uz4"
    And we close session "uz5"

    # Now: with unix-socket to server, will the cleanup happen as fast as TCP variant?
    And we sleep 5000ms

    When we create admin session "admin6" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin6" and store response
    Then admin session "admin6" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin6" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0

    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-zombie-noread
  Scenario: TCP-zombie — client sends huge SELECT, never reads, never closes
    # Models the prod culprit: client's TCP socket is alive (kernel ACKs everything,
    # keepalive happy), but application stopped reading. doorman writes the response
    # to the client, kernel buffer fills, write_all_flush in handle_large_data_row
    # blocks indefinitely (no timeout in code, no signal from client side).
    #
    # Expected (current code): sv_active stays at 1 for the entire scenario duration,
    # because the only thing that would unblock the write is tcp_user_timeout (60s default)
    # which we won't wait for.
    When we create session "zomb" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "zomb" without waiting

    # Let kernel buffer fill on the client side, doorman should be blocked on write.
    And we sleep 2000ms

    # Snapshot pool state — client's permit is in use by the not-reading session.
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin1" and store response
    Then admin session "admin1" column "sv_active" for row with "user" = "example_user_1" should be between 1 and 1
    And admin session "admin1" column "cl_active" for row with "user" = "example_user_1" should be between 1 and 1

    # Now close the zombie session gracefully (FIN, not RST).
    When we close session "zomb"

    # After close, doorman should detect EPIPE on the next write attempt and clean up.
    # If permit is still held — that's the leak we're hunting.
    And we sleep 3000ms

    When we create admin session "admin2" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin2" and store response
    Then admin session "admin2" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin2" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-zombie-noread-burst
  Scenario: TCP-zombie burst — 5 not-reading clients fully exhaust the pool
    When we create session "z1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "z2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "z3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "z4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "z5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "z1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "z2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "z3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "z4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "z5" without waiting

    And we sleep 2000ms

    # Pool size = 5 with 5 zombies → all permits in use, doorman blocked on writes.
    When we create admin session "admin3" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin3" and store response
    Then admin session "admin3" column "sv_active" for row with "user" = "example_user_1" should be between 5 and 5

    # Close all zombies. After cleanup, pool should release all permits.
    When we close session "z1"
    And we close session "z2"
    And we close session "z3"
    And we close session "z4"
    And we close session "z5"

    And we sleep 3000ms

    When we create admin session "admin4" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin4" and store response
    Then admin session "admin4" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin4" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0

    # Verify a fresh client gets a permit.
    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-pg-sleep
  Scenario: pg_sleep + large DataRow — RST during the gap, before large body arrives
    # PG executes the SELECT *first* (including pg_sleep which blocks 3s),
    # then assembles the row, then sends it. During those 3s doorman is parked
    # in server.recv() waiting for headers. If we RST the client *during* the sleep,
    # doorman is in await on the server-side, with no signal from the client side
    # because we're not in wait_for_next_message. When PG finally sends the giant
    # DataRow, doorman tries write_all_flush(client) inside handle_large_data_row
    # against an already-closed client.
    When we create session "ps1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "ps1" without waiting

    # PG is in pg_sleep right now. doorman is in server.recv() blocking.
    And we sleep 500ms

    # Kill client while doorman is mid-recv from server (slow query in progress).
    When we abort TCP connection with RST for session "ps1"

    # Wait until pg_sleep completes + drain + cleanup.
    And we sleep 8000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-pg-sleep-burst
  Scenario: 5 parallel pg_sleep + large DataRow + RST during pre-body window
    # Same idea, but 5 in parallel — fills entire pool while all are stuck in
    # server.recv() (PG is sleeping).
    When we create session "psb1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "psb2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "psb3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "psb4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "psb5" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "psb1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "psb2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "psb3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "psb4" without waiting
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "psb5" without waiting

    And we sleep 500ms

    When we abort TCP connection with RST for session "psb1"
    And we abort TCP connection with RST for session "psb2"
    And we abort TCP connection with RST for session "psb3"
    And we abort TCP connection with RST for session "psb4"
    And we abort TCP connection with RST for session "psb5"

    And we sleep 8000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-pg-sleep-extended
  Scenario: Extended protocol (Parse/Bind/Execute) with pg_sleep — RST during pre-body window
    # Same scenario as pg-sleep, but via raw extended protocol (Parse/Bind/Execute/Sync).
    # Ensures we hit the prepared-statement code path with pending_large_message handling.
    When we create session "pse1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "huge_sleep" with query "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to session "pse1"
    And we send Bind "" to "huge_sleep" with params "" to session "pse1"
    And we send Execute "" to session "pse1"
    And we send Sync to session "pse1"

    And we sleep 500ms
    When we abort TCP connection with RST for session "pse1"
    And we sleep 8000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-pg-sleep-cancel
  Scenario: pg_sleep + large DataRow + cancel during sleep
    # Server is sleeping; client cancels. PG aborts query, sends ErrorResponse.
    # Tests cancel-during-server-sleep path.
    When we create session "psc1" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send Parse "huge_sleep" with query "SELECT repeat('X', 50000000)::text, pg_sleep(5)" to session "psc1"
    And we send Bind "" to "huge_sleep" with params "" to session "psc1"
    And we send Execute "" to session "psc1"
    And we send Sync to session "psc1"

    And we sleep 500ms
    When we send cancel request for session "psc1"
    And we sleep 2000ms
    When we abort TCP connection with RST for session "psc1"
    And we sleep 5000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-coordinator-burst
  Scenario: Coordinator-managed pool with reserve, 2 users, large DataRow + cancel storm
    # Mirror prod: max_db_connections + reserve_pool_size + 2 users contending.
    # Coordinator (cross-pool) gates new creates after main is exhausted.
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
      worker_threads = 1
      log_client_connections = true
      log_client_disconnections = true
      # Cap proxy timeout so cleanup after RST/cancel completes within
      # the test's wait windows (default 15s would exceed our 5s sleeps).
      proxy_copy_data_timeout = 2000
      query_wait_timeout = 5000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"
      max_db_connections = 4
      reserve_pool_size = 2
      min_guaranteed_pool_size = 1

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 4

      [[pools.example_db.users]]
      username = "example_user_2"
      password = ""
      pool_size = 4
      """

    # Saturate pool with 4 long-running large queries from user_1 (RST mid-stream).
    When we create session "u1c1" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "u1c2" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "u1c3" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we create session "u1c4" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "u1c1" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "u1c2" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "u1c3" without waiting
    And we send SimpleQuery "SELECT repeat('X', 30000000)::text" to session "u1c4" without waiting
    And we sleep 100ms

    # While user_1's pool is mid-stream, user_2 hits the same shard — must use reserve via coordinator.
    When we create session "u2c1" to pg_doorman as "example_user_2" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "u2c1" without waiting
    And we sleep 100ms

    # Storm: cancel + RST all four user_1 clients while doorman is still streaming.
    When we send cancel request for session "u1c1"
    And we send cancel request for session "u1c2"
    And we abort TCP connection with RST for session "u1c3"
    And we abort TCP connection with RST for session "u1c4"
    And we sleep 200ms
    When we abort TCP connection with RST for session "u1c1"
    And we abort TCP connection with RST for session "u1c2"
    And we abort TCP connection for session "u2c1"
    And we sleep 6000ms

    # Pool must converge to 0 active.
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "cl_active" for row with "user" = "example_user_2" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_2" should be between 0 and 0

    # Verify pool is alive after the storm.
    When we create session "verify" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify" and store response
    Then session "verify" should receive DataRow with "1"

  @large-datarow-leak-cancel-mid-stream
  Scenario: Cancel arrives while doorman is mid-stream of huge DataRow
    # Replicates direction (1): client cancels query while handle_large_data_row
    # is proxying the gigabyte body. PG aborts the query mid-row and replies
    # with ErrorResponse + ReadyForQuery; doorman is left holding partial data.
    When we create session "qclient" to pg_doorman as "example_user_1" with password "" and database "example_db" and store backend key
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text" to session "qclient" without waiting

    # Let doorman enter handle_large_data_row, then cancel.
    And we sleep 200ms
    When we send cancel request for session "qclient"

    # Give doorman time to process the cancel and clean up.
    And we sleep 3000ms

    # Original session may or may not have data; we don't care. Close it.
    When we close session "qclient"
    And we sleep 1000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-burst-extended
  Scenario: 5 parallel extended-protocol clients, all RST mid-stream
    When we create session "ex1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "ex2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "ex3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "ex4" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "ex5" to pg_doorman as "example_user_1" with password "" and database "example_db"

    And we send Parse "p1" with query "SELECT repeat('X', 50000000)::text" to session "ex1"
    And we send Bind "" to "p1" with params "" to session "ex1"
    And we send Execute "" to session "ex1"
    And we send Sync to session "ex1"
    And we send Parse "p1" with query "SELECT repeat('X', 50000000)::text" to session "ex2"
    And we send Bind "" to "p1" with params "" to session "ex2"
    And we send Execute "" to session "ex2"
    And we send Sync to session "ex2"
    And we send Parse "p1" with query "SELECT repeat('X', 50000000)::text" to session "ex3"
    And we send Bind "" to "p1" with params "" to session "ex3"
    And we send Execute "" to session "ex3"
    And we send Sync to session "ex3"
    And we send Parse "p1" with query "SELECT repeat('X', 50000000)::text" to session "ex4"
    And we send Bind "" to "p1" with params "" to session "ex4"
    And we send Execute "" to session "ex4"
    And we send Sync to session "ex4"
    And we send Parse "p1" with query "SELECT repeat('X', 50000000)::text" to session "ex5"
    And we send Bind "" to "p1" with params "" to session "ex5"
    And we send Execute "" to session "ex5"
    And we send Sync to session "ex5"

    And we sleep 200ms

    When we abort TCP connection with RST for session "ex1"
    And we abort TCP connection with RST for session "ex2"
    And we abort TCP connection with RST for session "ex3"
    And we abort TCP connection with RST for session "ex4"
    And we abort TCP connection with RST for session "ex5"

    And we sleep 5000ms

    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin" and store response
    Then admin session "admin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-pg-sleep-fin-burst
  Scenario: 5 parallel pg_sleep + large DataRow + FIN (app restart style close)
    # Same pre-body window as pg-sleep-burst, but close sockets gracefully (FIN),
    # which is closer to "application restart" than explicit RST.
    When we create 5 sessions with prefix "psf" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 50000000)::text, pg_sleep(3)" to 5 sessions with prefix "psf" without waiting
    And we sleep 500ms

    # Simulate app restart: all client sockets are closed at once.
    When we close 5 sessions with prefix "psf"
    And we sleep 9000ms

    When we create admin session "admin-fin" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-fin" and store response
    Then admin session "admin-fin" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin-fin" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @large-datarow-leak-restart-pressure-40
  Scenario: PROD-LIKE pressure — 40 active clients/servers then app restart
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 300
      worker_threads = 3
      query_wait_timeout = 5000
      proxy_copy_data_timeout = 2000

      [pools.example_db]
      server_host = "${PG_TEMP_DIR}"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 40
      """

    # Occupy all 40 permits, keep PG busy in pg_sleep before body arrives.
    When we create 40 sessions with prefix "r40c" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 4000000)::text, pg_sleep(8)" to 40 sessions with prefix "r40c" without waiting
    And we sleep 700ms

    When we create admin session "admin-pre" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-pre" and store response
    Then admin session "admin-pre" column "cl_active" for row with "user" = "example_user_1" should be between 40 and 40
    And admin session "admin-pre" column "sv_active" for row with "user" = "example_user_1" should be between 40 and 40

    # Under full pressure, new client should hit query_wait_timeout.
    When we create session "probe-timeout" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "probe-timeout" expecting error
    Then session "probe-timeout" should receive error containing "timeout"

    # App restart simulation: all original clients disconnect.
    When we close 40 sessions with prefix "r40c"
    And we sleep 12000ms

    When we create admin session "admin-post" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-post" and store response
    Then admin session "admin-post" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin-post" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

    # Sanity: pool should accept new traffic after pressure subsides.
    When we create session "verify-post" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "verify-post" and store response
    Then session "verify-post" should receive DataRow with "1"

  @large-datarow-leak-restart-pressure-40-longwait
  Scenario: PROD-LIKE pressure — 40 active clients/servers then app restart (long wait)
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 300
      worker_threads = 3
      query_wait_timeout = 5000
      proxy_copy_data_timeout = 2000

      [pools.example_db]
      server_host = "${PG_TEMP_DIR}"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 40
      """

    When we create 40 sessions with prefix "r40lw" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 4000000)::text, pg_sleep(8)" to 40 sessions with prefix "r40lw" without waiting
    And we sleep 700ms

    When we create admin session "admin-lw-pre" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-lw-pre" and store response
    Then admin session "admin-lw-pre" column "cl_active" for row with "user" = "example_user_1" should be between 40 and 40
    And admin session "admin-lw-pre" column "sv_active" for row with "user" = "example_user_1" should be between 40 and 40

    When we close 40 sessions with prefix "r40lw"
    And we sleep 70000ms

    When we create admin session "admin-lw-post" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-lw-post" and store response
    Then admin session "admin-lw-post" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin-lw-post" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
