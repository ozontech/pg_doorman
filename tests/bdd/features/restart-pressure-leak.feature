@rust @rust-3 @restart-pressure-leak
Feature: Client restart under full pool pressure should release active counters

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
      prepared_statements_cache_size = 300
      worker_threads = 3
      query_wait_timeout = 5000
      # Cap proxy timeout so cleanup after client close completes within
      # the test's wait window instead of relying on the 15s default.
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

  @restart-pressure-leak-40-longwait
  Scenario: 40 active clients restart, counters must return to zero
    When we create 40 sessions with prefix "rpl" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT repeat('X', 4000000)::text, pg_sleep(8)" to 40 sessions with prefix "rpl" without waiting
    And we sleep 700ms

    When we create admin session "admin-pre" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-pre" and store response
    Then admin session "admin-pre" column "cl_active" for row with "user" = "example_user_1" should be between 40 and 40
    And admin session "admin-pre" column "sv_active" for row with "user" = "example_user_1" should be between 40 and 40

    When we close 40 sessions with prefix "rpl"
    # 8s pg_sleep finishes + 2s proxy timeout + cleanup margin.
    And we sleep 12000ms

    When we create admin session "admin-post" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin-post" and store response
    Then admin session "admin-post" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin-post" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
