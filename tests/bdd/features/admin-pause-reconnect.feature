@rust @rust-4 @admin-pause-reconnect
Feature: Admin PAUSE, RESUME, RECONNECT commands
  Test that PAUSE blocks new connections, RESUME unblocks them,
  and RECONNECT rotates all backend connections.

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
      query_wait_timeout = 1000
      server_lifetime = 60000
      server_idle_check_timeout = 0

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """

  @pause-resume-basic
  Scenario: PAUSE blocks new connections, RESUME unblocks
    # Establish a working connection first
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store backend_pid
    # PAUSE via admin
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "PAUSE example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    # New query on a different session should timeout (pool is paused, query_wait_timeout=1s)
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s2" expecting error
    Then session "s2" should receive error containing "timeout"
    # RESUME via admin
    When we execute "RESUME example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RESUME"
    # New query should now succeed
    When we create session "s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s3" and store backend_pid

  @reconnect-rotates-connections
  Scenario: RECONNECT rotates all backend connections
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "pid_before"
    # RECONNECT via admin — bumps epoch, drains idle
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "RECONNECT example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RECONNECT"
    # Should get a new backend PID (old connection recycled due to epoch mismatch)
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "new_pid_reconnect"
    Then named backend_pid "new_pid_reconnect" from session "s1" is different from "pid_before"

  @pause-reconnect-full-rotation
  Scenario: PAUSE + RECONNECT ensures full connection rotation
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "old_pid"
    # PAUSE — block new connections
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "PAUSE example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    # RECONNECT — bump epoch + drain idle
    When we execute "RECONNECT example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RECONNECT"
    # RESUME — unblock clients
    When we execute "RESUME example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RESUME"
    # New query should get a new backend PID
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "new_pid_pause_reconnect"
    Then named backend_pid "new_pid_pause_reconnect" from session "s1" is different from "old_pid"

  @pause-idempotent
  Scenario: PAUSE and RESUME are idempotent
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    # Double PAUSE should not cause issues
    When we execute "PAUSE example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    When we execute "PAUSE example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    # Double RESUME should not cause issues
    When we execute "RESUME example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RESUME"
    When we execute "RESUME example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RESUME"
    # Pool should be working normally
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store backend_pid

  @pause-nonexistent-db
  Scenario: PAUSE/RESUME/RECONNECT nonexistent database returns error
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    When we execute "PAUSE nonexistent_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "No pool for database"
    When we execute "RESUME nonexistent_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "No pool for database"
    When we execute "RECONNECT nonexistent_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "No pool for database"
    # Existing pools should still be working normally
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store backend_pid

  @show-help-includes-new-commands
  Scenario: SHOW HELP includes PAUSE, RESUME, RECONNECT
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "show help" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    And admin session "admin1" response should contain "RESUME"
    And admin session "admin1" response should contain "RECONNECT"

  @show-pools-paused-column
  Scenario: SHOW POOLS reflects paused state
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    # Initially pool is not paused
    And we execute "SHOW POOLS" on admin session "admin1" and store response
    Then admin session "admin1" column "paused" should be between 0 and 0
    # PAUSE — paused column becomes 1
    When we execute "PAUSE example_db" on admin session "admin1" and store response
    And we execute "SHOW POOLS" on admin session "admin1" and store response
    Then admin session "admin1" column "paused" should be between 1 and 1
    # RESUME — paused column returns to 0
    When we execute "RESUME example_db" on admin session "admin1" and store response
    And we execute "SHOW POOLS" on admin session "admin1" and store response
    Then admin session "admin1" column "paused" should be between 0 and 0

  @global-pause-resume
  Scenario: Global PAUSE and RESUME without database argument
    # Establish a working connection first
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store backend_pid
    # Global PAUSE via admin
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "PAUSE" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    # Verify paused state via SHOW POOLS
    When we execute "SHOW POOLS" on admin session "admin1" and store response
    Then admin session "admin1" column "paused" should be between 1 and 1
    # New query on a different session should timeout (pool is paused)
    When we create session "s2" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s2" expecting error
    Then session "s2" should receive error containing "timeout"
    # Global RESUME
    When we execute "RESUME" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RESUME"
    # Verify unpaused state via SHOW POOLS
    When we execute "SHOW POOLS" on admin session "admin1" and store response
    Then admin session "admin1" column "paused" should be between 0 and 0
    # New query should succeed after resume
    When we create session "s3" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s3" and store backend_pid

  @global-reconnect
  Scenario: Global RECONNECT without database argument rotates connections
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "pid_before"
    # Global RECONNECT via admin
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "RECONNECT" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RECONNECT"
    # Should get a new backend PID
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "s1" and store backend_pid as "new_pid_global_reconnect"
    Then named backend_pid "new_pid_global_reconnect" from session "s1" is different from "pid_before"
