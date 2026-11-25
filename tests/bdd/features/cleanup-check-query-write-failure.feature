@rust @rust-3 @cleanup-review @cleanup-review-check-query-write-failure
Feature: Cleanup runs even when the check query response cannot be written to the client
  recycle() does not clean backends, so the pooler check query path must clean the
  backend itself even when the response write to the client fails.

  Background:
    Given PostgreSQL started with options "-c log_statement=all -c logging_collector=off" and pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        pooler_check_query: "SELECT 7, pg_sleep(1)"
        cleanup_server_connections: always
        cleanup_server_query: "SELECT set_config('doorman.cleanup_ran', 'yes', false)"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """

  Scenario: A failed client write after a pooler check query still cleans the backend
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store response
    And we store backend_pid from last response of session "one"
    # The first response already ran one cleanup in always mode; truncate the log
    # so the marker count below covers only the check query response.
    And we truncate PostgreSQL log
    # Send the check query without reading the response and RST the socket while the
    # backend is inside pg_sleep(1): the response write to the client must fail.
    And we send SimpleQuery "SELECT 7, pg_sleep(1)" to session "one" without waiting
    And we abort TCP connection with RST for session "one"
    And we sleep 1500ms
    Then PostgreSQL log should contain exactly 1 occurrences of "set_config('doorman.cleanup_ran'"
    # The backend was cleaned, not retired, so the next checkout reuses the same pid
    # and sees the cleanup marker.
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid(), current_setting('doorman.cleanup_ran')" to session "next" and store response
    Then session "next" should receive exactly one DataRow "${one_pid}|yes"
    And session "next" should receive ReadyForQuery "I"
