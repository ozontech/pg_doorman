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
    # Establish a connection and get backend PID via Extended Protocol
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT pg_backend_pid()" to session "s1"
    And we send Bind "" to "" with params "" to session "s1"
    And we send Execute "" to session "s1"
    And we send Sync to session "s1"
    Then we remember backend_pid from session "s1" as "pid_before"
    # RECONNECT via admin — bumps epoch, drains idle
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "RECONNECT example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RECONNECT"
    # Query again — should get a new backend PID (old connection recycled due to epoch mismatch)
    When we send Parse "" with query "SELECT pg_backend_pid()" to session "s1"
    And we send Bind "" to "" with params "" to session "s1"
    And we send Execute "" to session "s1"
    And we send Sync to session "s1"
    Then we verify backend_pid from session "s1" is different from "pid_before"

  @pause-reconnect-full-rotation
  Scenario: PAUSE + RECONNECT ensures full connection rotation
    # Establish connection and get backend PID
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "SELECT pg_backend_pid()" to session "s1"
    And we send Bind "" to "" with params "" to session "s1"
    And we send Execute "" to session "s1"
    And we send Sync to session "s1"
    Then we remember backend_pid from session "s1" as "old_pid"
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
    When we send Parse "" with query "SELECT pg_backend_pid()" to session "s1"
    And we send Bind "" to "" with params "" to session "s1"
    And we send Execute "" to session "s1"
    And we send Sync to session "s1"
    Then we verify backend_pid from session "s1" is different from "old_pid"

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
