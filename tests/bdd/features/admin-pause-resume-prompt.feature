@rust @rust-4 @admin-pause-reconnect
Feature: RESUME unblocks waiting clients promptly
  Verify that RESUME notification reaches blocked clients quickly,
  not after the full query_wait_timeout expires.

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
      query_wait_timeout = 5000
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

  @pause-resume-prompt-unblock
  Scenario: RESUME unblocks waiting clients promptly (not after full timeout)
    # Establish a working connection first
    When we create session "s1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "s1" and store backend_pid
    # PAUSE the pool via admin
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "PAUSE example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "PAUSE"
    # Send a query that will block on PAUSE (pg_doorman receives but cannot get a backend connection)
    When we send SimpleQuery "SELECT 1" to session "s1" without waiting
    And we sleep 200ms
    # RESUME — should unblock the waiting client promptly
    When we execute "RESUME example_db" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "RESUME"
    # Response must arrive within 2s — well before the 5s query_wait_timeout
    Then we read SimpleQuery response from session "s1" within 2000ms
