@rust @rust-1 @server-stats-registration-leak
Feature: Cancelled backend startup attempts remove their SERVER_STATS rows

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And TCP blackhole listener registered as 'blackhole'
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      query_wait_timeout = "200ms"
      connect_timeout = "500ms"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${BLACKHOLE_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  @cancellation-leak
  Scenario: Backend startup timeout leaves no SHOW SERVERS row
    # The blackhole accepts TCP and never sends a PostgreSQL startup response.
    # connect_timeout cancels pool checkout while pg_doorman has a login row
    # for the backend attempt. After cancellation, SHOW SERVERS returns no rows.
    When we attempt session to pg_doorman as "example_user_1" with password "" and database "example_db" expecting startup failure
    When we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    Then we poll "SHOW SERVERS" on admin session "admin1" until row count is 0 within 2000ms
