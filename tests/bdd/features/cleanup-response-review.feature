@rust @rust-3 @cleanup-review @cleanup-review-response
Feature: Cleanup preserves completed client responses

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.committed_rows (id int PRIMARY KEY)" to session "observer"

  @cleanup-review-commit
  Scenario Outline: Cleanup errors and notices preserve COMMIT responses
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: always
        cleanup_server_query: "<reset_query>"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "before"
    And we send SimpleQuery "INSERT INTO public.committed_rows VALUES (1)" to session "one"
    And we send SimpleQuery "COMMIT" to session "one" and store response
    Then session "one" should receive CommandComplete "COMMIT"
    And session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT count(*) FROM public.committed_rows" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "1"
    When we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then named backend_pid "after" from session "one" is <comparison> "before"
    When we send SimpleQuery "SELECT count(*) FROM public.committed_rows" to session "one" and store response
    Then session "one" should receive exactly one DataRow "1"
    When we send SimpleQuery "ROLLBACK" to session "one" and store response
    Then session "one" should receive CommandComplete "ROLLBACK"
    And session "one" should receive ReadyForQuery "I"

    Examples:
      | reset_query                                                                           | comparison     |
      | SELECT 1 / 0                                                                          | different from |
      | DO $$ BEGIN RAISE NOTICE USING ERRCODE = '0AM01', MESSAGE = 'cleanup notice'; END $$    | same as        |
      | DO $$ BEGIN RAISE NOTICE USING ERRCODE = '0A000', MESSAGE = 'cleanup notice'; END $$    | same as        |
      | BEGIN                                                                                 | different from |
      | COPY public.committed_rows FROM STDIN                                                  | different from |
      | SELECT pg_sleep(5)                                                                    | different from |
      | /* empty reset */                                                                     | different from |

  @cleanup-review-legacy-flush
  Scenario: Built-in cleanup drains its own response after a disconnected Flush
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "set_timeout" with query "SET statement_timeout = 10000" to session "one"
    And we send Bind "" to "set_timeout" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12C" from session "one"
    And we send Parse "probe" with query "SELECT pg_backend_pid(), current_setting('statement_timeout')" to session "one"
    And we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12DC" from session "one"
    And we store backend_pid from last response of session "one"
    Then session "one" should receive exactly one DataRow "${one_pid}|10s"
    When we abort TCP connection with RST for session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid(), current_setting('statement_timeout')" to session "next" without waiting
    Then we read SimpleQuery response from session "next" within 5000ms
    And session "next" should receive exactly one DataRow "${one_pid}|0"
    And session "next" should receive ReadyForQuery "T"

  @cleanup-review-check-query
  Scenario: A failed reset after a pooler check keeps the frontend usable
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        pooler_check_query: "SELECT 7"
        cleanup_server_connections: always
        cleanup_server_query: "SELECT 1 / 0"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 7" to session "one" and store response
    Then session "one" should receive exactly one DataRow "7"
    And session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT 8" to session "one" and store response
    Then session "one" should receive exactly one DataRow "8"
    And session "one" should receive ReadyForQuery "I"
