@rust @rust-3 @cleanup-server-query
Feature: Backend cleanup policy

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @cleanup-server-query-session-reuse
  Scenario Outline: Reuse only after a successful full reset
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        cleanup_server_connections: always
        cleanup_server_query: "<reset_query>"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "SET statement_timeout = 10000; CREATE TEMP TABLE reset_secret (value int); PREPARE reset_stmt AS SELECT 42" to session "old"
    And we send SimpleQuery "SELECT current_setting('statement_timeout') = '10s' AND to_regclass('pg_temp.reset_secret') IS NOT NULL AND EXISTS (SELECT FROM pg_prepared_statements WHERE name = 'reset_stmt')" to session "old" and store response
    Then session "old" should receive DataRow with "t"
    When we send SimpleQuery "BEGIN" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should <comparison> backend_pid from session "old"
    When we send SimpleQuery "SELECT current_setting('statement_timeout') = '0' AND to_regclass('pg_temp.reset_secret') IS NULL AND NOT EXISTS (SELECT FROM pg_prepared_statements WHERE name = 'reset_stmt')" to session "next" and store response
    Then session "next" should receive DataRow with "t"
    And session "next" should receive ReadyForQuery "I"

    Examples:
      | reset_query                                                                               | comparison |
      | RESET ALL; DEALLOCATE ALL; CLOSE ALL; UNLISTEN *; SELECT repeat('x', 20000), pg_advisory_unlock_all(); DISCARD PLANS; DISCARD SEQUENCES; DISCARD TEMP; SET SESSION AUTHORIZATION DEFAULT                                                     | equal      |
      | RESET ALL; DEALLOCATE ALL; SELECT 1 / 0; DISCARD TEMP                                       | not equal  |
      | RESET ALL; DEALLOCATE ALL; DISCARD TEMP; DO $$ BEGIN RAISE NOTICE USING ERRCODE = '0AM01', MESSAGE = 'cleanup notice'; END $$ | equal      |
      | RESET ALL; DEALLOCATE ALL; DISCARD TEMP; DO $$ BEGIN RAISE NOTICE USING ERRCODE = '0A000', MESSAGE = 'cleanup notice'; END $$ | equal      |
      | BEGIN                                                                                     | not equal  |
      | COPY reset_secret FROM STDIN                                                              | not equal  |
      | SELECT pg_sleep(5)                                                                        | not equal  |
      | /* empty reset */                                                                         | not equal  |

  @cleanup-server-query-prepared-reuse
  Scenario: Cached Bind reparses and restores startup parameters after reset
    Given fixtures from "tests/fixture-search-path.sql" applied
    And pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        prepared_statements: true
        sync_server_parameters: true
        cleanup_server_connections: always
        cleanup_server_query: "SELECT 1 / 0"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          cleanup_server_query: "DISCARD ALL"
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "search_path=schema_a"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "before"
    And we send Parse "cached" with query "SELECT val + $1::int FROM t" to session "one"
    And we send Bind "" to "cached" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "11"
    And session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "one" and store response
    # `always` already ran the pool-level DISCARD ALL on the previous Sync.
    Then session "one" should receive DataRow with "0"
    When we send SimpleQuery "SET statement_timeout = 10000" to session "one"
    And we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "one" and store response
    Then session "one" should receive DataRow with "0"
    When we send Bind "" to "cached" with params "11" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "12"
    And session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then named backend_pid "after" from session "one" is same as "before"

  @cleanup-server-query-policy
  Scenario Outline: Cleanup scheduling counts each released backend once
    When I run shell command:
      """
      psql -X -h 127.0.0.1 -p ${PG_PORT} -U postgres -d example_db -v ON_ERROR_STOP=1 -c "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)"
      """
    Then the command should succeed
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        server_idle_check_timeout: 1
        pooler_check_query: "SELECT 7"
        cleanup_server_connections: <mode>
        cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + <increment>"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          <mode_override>
          <query_override>
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    When I run shell command:
      """
      psql -X -h 127.0.0.1 -p ${PG_PORT} -U postgres -d example_db -v ON_ERROR_STOP=1 -c "UPDATE public.cleanup_counter SET n = 0"
      """
    Then the command should succeed
    When we send SimpleQuery "<first_query>" to session "old"
    When I run shell command:
      """
      psql -X -h 127.0.0.1 -p ${PG_PORT} -U postgres -d example_db -At -v ON_ERROR_STOP=1 -c "SELECT n FROM public.cleanup_counter"
      """
    Then the command should succeed
    And the command output should contain "<intermediate>"
    When we sleep 10ms
    And we send SimpleQuery "<second_query>" to session "old" and store response
    Then session "old" should receive DataRow with "<intermediate>"
    When we close session "old"
    And we sleep 10ms
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    When we send SimpleQuery "SELECT count(*) = 1 AND min(n) = <resets> AND current_setting('statement_timeout') = '0' FROM public.cleanup_counter" to session "next" and store response
    Then session "next" should receive DataRow with "t"
    And session "next" should receive ReadyForQuery "T"

    Examples:
      | mode     | mode_override                       | increment | query_override                                                                                                                                 | first_query                                                                                         | second_query                                       | intermediate | resets |
      | always   | # inherit                           | 1         | # inherit                                                                                                                                      | SELECT pg_backend_pid()                                                                             | SELECT min(n) FROM public.cleanup_counter           | 1            | 2      |
      | always   | # inherit                           | 1         | # inherit                                                                                                                                      | SELECT 7                                                                             | SELECT min(n) FROM public.cleanup_counter           | 1            | 2      |
      | always   | # inherit                           | 1         | # inherit                                                                                                                                      | BEGIN; SELECT pg_backend_pid()                                                                      | SELECT min(n) FROM public.cleanup_counter; COMMIT   | 0            | 1      |
      | off      | # inherit                           | 1         | # inherit                                                                                                                                      | BEGIN; INSERT INTO public.cleanup_counter VALUES (99); SELECT pg_backend_pid()                       | SELECT min(n) FROM public.cleanup_counter           | 0            | 0      |
      | off      | cleanup_server_connections: always  | 1         | # inherit                                                                                                                                      | SELECT pg_backend_pid()                                                                             | SELECT min(n) FROM public.cleanup_counter           | 1            | 2      |
      | always   | cleanup_server_connections: off     | 1         | # inherit                                                                                                                                      | BEGIN; INSERT INTO public.cleanup_counter VALUES (99); SELECT pg_backend_pid()                       | SELECT min(n) FROM public.cleanup_counter           | 0            | 0      |
      | always   | cleanup_server_connections: false   | 1         | # inherit                                                                                                                                      | BEGIN; INSERT INTO public.cleanup_counter VALUES (99); SELECT pg_backend_pid()                       | SELECT min(n) FROM public.cleanup_counter           | 0            | 0      |
      | always   | # inherit                           | 100       | cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"                                       | SELECT pg_backend_pid()                                                                             | SELECT min(n) FROM public.cleanup_counter           | 1            | 2      |

  @cleanup-async @cleanup-async-cache
  Scenario Outline: Flush and Sync honor pooling boundaries while Bind remains reusable
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: <mode>
        <query_config>
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: <pool_mode>
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send Parse "probe" with query "SELECT pg_backend_pid(), current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one"
    And we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12DC" from session "one"
    And we store backend_pid from last response of session "one"
    Then session "one" should receive exactly one DataRow "${one_pid}|0|1"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<resets>"
    When we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then backend_pid "after" from session "one" should equal initial backend_pid from session "one"
    When we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "one" and store response
    Then session "one" should receive exactly one DataRow "<cached>"
    When we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "2DC" from session "one"
    Then session "one" should receive exactly one DataRow "${one_pid}|0|1"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<resets>"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "T"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<resets>"
    When we send SimpleQuery "ROLLBACK" to session "one" and store response
    Then session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<total>"

    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    When we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "next" and store response
    Then session "next" should receive exactly one DataRow "<after_close_cached>"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<after_close_resets>"

    Examples:
      | pool_mode   | mode     | query_config                                                                                                                | resets | cached | total | after_close_resets | after_close_cached |
      | transaction | adaptive | # inherit                                                                                                                     | 0      | 1      | 0     | 0                  | 1                  |
      | transaction | always   | cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"                  | 1      | 0      | 2     | 2                  | 0                  |
      | session     | always   | cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"                  | 0      | 1      | 0     | 1                  | 0                  |

  @cleanup-async @cleanup-async-set
  Scenario Outline: Extended SET remains visible until Sync releases the backend
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: <mode>
        <query_config>
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send Parse "set_timeout" with query "SET statement_timeout = 10000" to session "one"
    And we send Bind "" to "set_timeout" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12C" from session "one"
    When we send Parse "probe" with query "SELECT pg_backend_pid(), current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one"
    And we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12DC" from session "one"
    And we store backend_pid from last response of session "one"
    Then session "one" should receive exactly one DataRow "${one_pid}|10s|2"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<resets>"
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    When we send SimpleQuery "SELECT current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "next" and store response
    Then session "next" should receive exactly one DataRow "<timeout>|<cached>"
    And session "next" should receive ReadyForQuery "T"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "<resets>"

    Examples:
      | mode     | query_config                                                                                                   | resets | timeout | cached |
      | adaptive | # inherit                                                                                                      | 0      | 0       | 2      |
      | always   | cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"    | 1      | 0       | 0      |
      | off      | # inherit                                                                                                      | 0      | 10s     | 2      |

  @cleanup-async @cleanup-async-transaction
  Scenario Outline: Sync inside a transaction defers always cleanup until transaction end
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: always
        cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "before"
    When we send Parse "set_timeout" with query "SET statement_timeout = 10000" to session "one"
    And we send Bind "" to "set_timeout" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12C" from session "one"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "T"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send Parse "probe" with query "SELECT pg_backend_pid(), current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one"
    And we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12DC" from session "one"
    And we store backend_pid from last response of session "one"
    Then backend_pid "before" from session "one" should equal initial backend_pid from session "one"
    And session "one" should receive exactly one DataRow "${one_pid}|10s|2"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "T"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send SimpleQuery "<end_transaction>" to session "one" and store response
    Then session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "1"
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "one"
    When we send SimpleQuery "SELECT current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "next" and store response
    Then session "next" should receive exactly one DataRow "0|0"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "1"

    Examples:
      | end_transaction |
      | COMMIT          |
      | ROLLBACK        |

  @cleanup-async @cleanup-async-error
  Scenario Outline: Parse error received before Sync preserves frontend recovery
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: <mode>
        <query_config>
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "<begin_query>" to session "one" and store backend_pid as "before"
    When we send Parse "set_timeout" with query "SET statement_timeout = 10000" to session "one"
    And we send Bind "" to "set_timeout" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12C" from session "one"
    When we send Parse "" with query "SELECT FROM WHERE" to session "one"
    And we send Flush to session "one"
    And we read backend messages "E" from session "one"
    Then session "one" should receive error containing "syntax error" with code "42601"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "<status>"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we send SimpleQuery "<finish_query>" to session "one" and store response
    Then session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then named backend_pid "after" from session "one" is different from "before"
    When we send SimpleQuery "SELECT current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one" and store response
    Then session "one" should receive exactly one DataRow "0|0"
    And session "one" should receive ReadyForQuery "T"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"

    Examples:
      | mode     | query_config                                                                                                   | begin_query                    | status | finish_query |
      | adaptive | # inherit                                                                                                      | SELECT pg_backend_pid()        | I      | SELECT 1     |
      | always   | cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"    | BEGIN; SELECT pg_backend_pid() | E      | ROLLBACK     |

  @cleanup-async @cleanup-async-disconnect
  Scenario: Disconnect after a drained Flush cleans the backend before reuse
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: always
        cleanup_server_query: "RESET ALL; DEALLOCATE ALL; CLOSE ALL; UPDATE public.cleanup_counter SET n = n + 1"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    When we send Parse "set_timeout" with query "SET statement_timeout = 10000" to session "one"
    And we send Bind "" to "set_timeout" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12C" from session "one"
    When we send Parse "probe" with query "SELECT pg_backend_pid(), current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one"
    And we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12DC" from session "one"
    And we store backend_pid from last response of session "one"
    Then session "one" should receive exactly one DataRow "${one_pid}|10s|2"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "0"
    When we abort TCP connection with RST for session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "next" without waiting
    Then we read SimpleQuery response from session "next" within 5000ms
    When we store backend_pid from last response of session "next"
    Then backend_pid from session "next" should equal backend_pid from session "one"
    When we send SimpleQuery "SELECT current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "next" and store response
    Then session "next" should receive exactly one DataRow "0|0"
    And session "next" should receive ReadyForQuery "T"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "1"

  @cleanup-async @cleanup-async-reset-error @cleanup-review
  Scenario: Failed cleanup after Flush preserves ReadyForQuery and retires the dirty backend
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        prepared_statements: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
        cleanup_server_connections: always
        cleanup_server_query: "SELECT nextval('public.cleanup_attempts'); RESET ALL; DEALLOCATE ALL; SELECT 1 / 0"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "CREATE SEQUENCE public.cleanup_attempts" to session "observer"
    When we send Parse "set_timeout" with query "SET statement_timeout = 10000" to session "one"
    And we send Bind "" to "set_timeout" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12C" from session "one"
    When we send Parse "probe" with query "SELECT pg_backend_pid(), current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one"
    And we send Bind "" to "probe" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Flush to session "one"
    And we read backend messages "12DC" from session "one"
    And we store backend_pid from last response of session "one" as "before"
    Then session "one" should receive exactly one DataRow "${one_pid}|10s|2"
    When we send SimpleQuery "SELECT is_called FROM public.cleanup_attempts" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "f"
    When we send Sync to session "one"
    Then session "one" should receive ReadyForQuery "I"
    And we send SimpleQuery "SELECT last_value, is_called FROM public.cleanup_attempts" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "1|t"
    When we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then named backend_pid "after" from session "one" is different from "before"
    When we send SimpleQuery "SELECT current_setting('statement_timeout'), (SELECT count(*) FROM pg_prepared_statements)" to session "one" and store response
    Then session "one" should receive exactly one DataRow "0|0"
    And session "one" should receive ReadyForQuery "T"
