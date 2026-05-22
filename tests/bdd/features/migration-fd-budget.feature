Feature: pg_doorman stays within its fd budget
  pg_doorman must stay inside `LimitNOFILE`, reject overload at the
  protocol layer, and avoid routing local fd exhaustion into fallback.

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
    # NOFILE=50 cannot accept all 100 clients, but pool_size=10 should settle.
    Then at least 10 idle sessions should be open from the last batch attempt
    When we store foreground pg_doorman PID as "old_doorman"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    # EMFILE in the accept loop must be rate-limited.
    Then pg_doorman log contains "Failed to accept new connection" at most 5 times
    # Patroni fallback must not be triggered on local fd exhaustion.
    And pg_doorman log does not contain "fallback discovery failed"
    # Local fd exhaustion must not abort the recovery upgrade.
    And pg_doorman log does not contain "BINARY UPGRADE ABORTED"
    # The fresh-session check lives in the NOFILE=200 scenario; NOFILE=50 is
    # too tight while the child is still draining migration RX.

  @client-migration @migration-fd-budget @binary-upgrade-fd-cloexec @linux-only
  Scenario: Chained binary upgrade with 50 idle clients keeps fd table stable (FD_CLOEXEC + cleanup)
    # Regression target: a second SIGUSR2 must not inherit duplicate
    # migrated client fds from the previous child.
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
    # NOFILE=200 leaves enough headroom for all 50 clients.
    Then at least 50 idle sessions should be open from the last batch attempt

    # First upgrade.
    When we store foreground pg_doorman PID as "parent"
    And we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    Then pg_doorman log does not contain "Too many open files"
    And pg_doorman log does not contain "fallback discovery failed"
    And pg_doorman log does not contain "BINARY UPGRADE ABORTED"

    # Discover PID externally; /api/process is not a stable oracle
    # while upgrade generations overlap.
    When we discover the current pg_doorman PID externally and store as "gen1"
    And we capture the fd inventory for stored PID "gen1" as "gen1"
    Then every non-listener socket fd of stored PID "gen1" has FD_CLOEXEC set

    # One migrated session must still round-trip after gen1.
    When we send SimpleQuery "SELECT 1" to the first open idle session and store response as "post-upgrade-1"
    Then session "post-upgrade-1" should receive DataRow with "1"

    # Second upgrade. Missing CLOEXEC would add roughly one extra
    # socket per active migrated client.
    When we send SIGUSR2 to pg_doorman process at stored PID "gen1"
    And we wait for foreground binary upgrade to complete
    And we discover the current pg_doorman PID externally and store as "gen2"
    And we capture the fd inventory for stored PID "gen2" as "gen2"
    Then stored PID "gen2" should be different from stored PID "gen1"
    # Slack 5 covers runtime fd churn; the bug adds about 50.
    And the non-listener socket fd count delta from "gen1" to "gen2" should be at most 5
    And every non-listener socket fd of stored PID "gen2" has FD_CLOEXEC set
    # Socket stability alone can pass if clients were dropped.
    # Verify that most original sessions still work.
    When we send SimpleQuery "SELECT 1" to every open idle session and count successes as "survived_after_gen2"
    Then the stored count "survived_after_gen2" should be at least 45
    And a fresh PostgreSQL session to pg_doorman as "example_user_1" with password "" and database "example_db" succeeds

  @client-migration @migration-fd-budget @binary-upgrade-fd-cloexec @linux-only
  Scenario: SIGUSR2 child cleans up inheritable fds leaked by a polluted parent
    # Start with 32 inheritable pipe fds. The SIGUSR2 child must close
    # unexpected fds before config load.
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
    And we capture the fd inventory for stored PID "polluted_parent" as "polluted_parent"
    # 16 pipe pairs produce 32 seeded fds; allow a little runtime drift.
    Then the pipe fd count for stored PID "polluted_parent" should be at least 30

    When we send SIGUSR2 to foreground pg_doorman
    And we wait for foreground binary upgrade to complete
    And we discover the current pg_doorman PID externally and store as "clean_child"
    And we capture the fd inventory for stored PID "clean_child" as "clean_child"
    # Log proves the startup cleanup ran.
    Then pg_doorman log contains "unexpected inherited file descriptor"
    # Assert a drop instead of an absolute pipe count; runtime pipe
    # usage can drift.
    And the pipe fd count drop from "polluted_parent" to "clean_child" should be at least 20

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
    # max_connections=50 should reject at the protocol layer, not in accept.
    Then at least 50 idle sessions should be open from the last batch attempt
    And pg_doorman log does not contain "Too many open files"
    And pg_doorman log does not contain "fallback discovery failed"
    When we send SimpleQuery "SELECT 1" to the first open idle session and store response as "after-cap"
    Then session "after-cap" should receive DataRow with "1"
