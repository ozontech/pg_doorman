Feature: pg_doorman stays within its fd budget
  Two failure modes for the same root issue — pg_doorman must keep its
  file-descriptor table inside the configured `LimitNOFILE` and reject
  clients gracefully when load exceeds `max_connections`, rather than
  exhaust the fd table and start spamming EMFILE on every new socket
  (including Patroni-fallback discovery, which then dominates the log).

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @client-migration @migration-fd-budget
  Scenario: Binary upgrade with 100 idle clients under NOFILE=50 stays out of EMFILE
    Given pg_doorman log capture enabled
    And pg_doorman started with NOFILE limit 50 and config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we attempt to create 100 idle sessions to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Under NOFILE=50 the listener cannot service 100 clients, but the
    # pool itself is sized for 10 backends, so we expect at least the
    # first 10 clients to have settled into the pool. Anything below
    # would mean the accept loop or the auth path is dropping clients
    # the pool could otherwise serve — a regression we must not ship.
    Then at least 10 idle sessions should be open from the last batch attempt
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # accept-loop must be rate-limited: a single backoff line every 5 s is
    # acceptable, but tight-loop spam (thousands of lines per ms) is the bug
    # this scenario guards against.
    Then pg_doorman log contains "Failed to accept new connection" at most 5 times
    # Patroni fallback must not be triggered on local fd exhaustion.
    And pg_doorman log does not contain "fallback discovery failed"
    # Binary upgrade must not be aborted by the pre-flight validator just
    # because the local fd table is full — the upgrade is the recovery path.
    And pg_doorman log does not contain "BINARY UPGRADE ABORTED"
    # Note: at NOFILE=50 the post-upgrade fresh-connection check is
    # intentionally omitted — under that tight a budget the new process
    # is still draining migration RX during the assertion window, so a
    # bare connect/auth round-trip is racy. The fresher-window assertion
    # lives in the NOFILE=200 scenario below where the budget is wide
    # enough for the new process to settle.

  @client-migration @migration-fd-budget @binary-upgrade-fd-cloexec @linux-only
  Scenario: Chained binary upgrade with 50 idle clients keeps fd table stable (FD_CLOEXEC + cleanup)
    # Catches the production failure mode that the migration channel
    # reproduces: each migrated client fd was inherited by the next
    # child generation, doubling the socket fd count on every SIGUSR2.
    # Without FD_CLOEXEC on the SCM_RIGHTS-received fds the table
    # grows by ~N each upgrade; with the fix it stays roughly
    # constant. The assertion that catches the regression is the
    # non-listener socket fd count *delta* between two consecutive
    # generations.
    Given pg_doorman log capture enabled
    And pg_doorman started with NOFILE limit 200 and config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we attempt to create 50 idle sessions to pg_doorman as "example_user_1" with password "" and database "example_db"
    # NOFILE=200 has comfortable headroom for 50 clients, so the
    # accept-side bottleneck does not apply here — we expect every
    # client to have settled.
    Then at least 50 idle sessions should be open from the last batch attempt

    # ----- First SIGUSR2: existing-process → child generation 1 -----
    When we store foreground pg_doorman PID as "parent"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then pg_doorman log does not contain "Too many open files"
    And pg_doorman log does not contain "fallback discovery failed"
    And pg_doorman log does not contain "BINARY UPGRADE ABORTED"

    # Pin the new child PID independent of the original spawn handle.
    # /api/process is intentionally NOT used here: it depends on web
    # admin being on and would only see whichever copy of the listener
    # the kernel hashed our request to under SO_REUSEPORT.
    When we discover the current pg_doorman PID externally and store as "gen1"
    And we capture the fd inventory for stored PID "gen1" as "gen1"
    Then every non-listener socket fd of stored PID "gen1" has FD_CLOEXEC set

    # Migrated sessions must be able to round-trip a query through
    # whichever process is now servicing them. The MIGRATION_TX path
    # is only useful if the protocol state survived intact; a healthy
    # `SELECT 1` is the cheapest end-to-end check of that.
    When we send SimpleQuery "SELECT 1" to the first open idle session and store response as "post-upgrade-1"
    Then session "post-upgrade-1" should receive DataRow with "1"

    # ----- Second SIGUSR2: generation 1 → generation 2 -----
    # If the child of the first upgrade kept the migrated client fds
    # inheritable, the child of the second upgrade will end up with
    # both the fork-inherited and SCM_RIGHTS-received copies. The
    # non-listener socket count would grow by ~50 (one extra fd per
    # active client).
    When we send SIGUSR2 to pg_doorman process at stored PID "gen1"
    And we wait for foreground binary upgrade to complete
    And we discover the current pg_doorman PID externally and store as "gen2"
    And we capture the fd inventory for stored PID "gen2" as "gen2"
    Then stored PID "gen2" should be different from stored PID "gen1"
    # Slack of 5: tokio and jemalloc occasionally adjust their internal
    # fd usage during a process replacement; the FD_CLOEXEC bug doubled
    # the count by ~50, ten times this bound.
    And the non-listener socket fd count delta from "gen1" to "gen2" should be at most 5
    And every non-listener socket fd of stored PID "gen2" has FD_CLOEXEC set
    # The fd delta on its own can pass vacuously: a regression that
    # drops migrated clients during the second upgrade also shrinks
    # the socket count. Round-trip the original sessions to make
    # sure session continuity actually survived, then assert a
    # high-watermark of successes. Without this assertion the test
    # would call "delete every migrated client" a successful fix.
    When we send SimpleQuery "SELECT 1" to every open idle session and count successes as "survived_after_gen2"
    Then the stored count "survived_after_gen2" should be at least 45
    And a fresh PostgreSQL session to pg_doorman as "example_user_1" with password "" and database "example_db" succeeds

  @client-migration @migration-fd-budget @binary-upgrade-fd-cloexec @linux-only
  Scenario: SIGUSR2 child cleans up inheritable fds leaked by a polluted parent
    # Models the production recovery case for `c891054`: a buggy old
    # parent has been running long enough to accumulate non-CLOEXEC
    # inheritable fds (16 here, simulated via `pipe(2)` pairs in
    # `pre_exec`). On the next SIGUSR2 the new child sees those fds
    # via `--inherit-fd`-triggered allowlist cleanup, walks the
    # numeric range up to `RLIMIT_NOFILE`, and closes everything not
    # in the allowlist. Without that cleanup the inherited pipes
    # ride along forever in every subsequent child generation.
    Given pg_doorman log capture enabled
    And pg_doorman started with NOFILE limit 200 and 16 extra inheritable pipes and config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we discover the current pg_doorman PID externally and store as "polluted_parent"
    # 16 pairs × 2 ends = 32 inheritable pipe fds, plus whatever
    # tokio/jemalloc opened natively. The 30 lower bound leaves room
    # for the seeded fds to have been seen even if the runtime
    # consolidated a few of its own internal pipes.
    Then the pipe fd count for stored PID "polluted_parent" should be at least 30

    When we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we discover the current pg_doorman PID externally and store as "clean_child"
    # The cleanup announces itself on stderr; pin that before the
    # numeric pipe-count assertion so a regression that silently
    # disables the cleanup pass is reported as "the log line never
    # showed up" rather than "we still see N pipes".
    Then pg_doorman log contains "unexpected inherited file descriptor"
    # The cleanup runs in `main()` before config load, so by the time
    # the new process is reachable on the listener the seeded pipes
    # are gone. Anything tokio/jemalloc opens for its own use stays;
    # the bound of 12 is comfortably above the native pipe count seen
    # in practice and well below the 30+ a leak would produce.
    And the pipe fd count for stored PID "clean_child" should be at most 12

  @client-migration @migration-fd-budget @fd-overload
  Scenario: 1000 clients on a 50-client cap reject cleanly, accepted clients keep working
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      pool_mode = "transaction"
      max_connections = 50
      shutdown_timeout = 5000
      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """
    When we sleep 1000ms
    And we attempt to create 1000 idle sessions to pg_doorman as "example_user_1" with password "" and database "example_db"
    # `max_connections = 50` is the documented cap: the listener must
    # admit the first 50 cleanly and reject the rest at the protocol
    # layer ("too many clients already"). Anything lower means accept
    # itself is the bottleneck — that's the failure mode we're catching.
    Then at least 50 idle sessions should be open from the last batch attempt
    And pg_doorman log does not contain "Too many open files"
    And pg_doorman log does not contain "fallback discovery failed"
    When we send SimpleQuery "SELECT 1" to the first open idle session and store response as "after-cap"
    Then session "after-cap" should receive DataRow with "1"
