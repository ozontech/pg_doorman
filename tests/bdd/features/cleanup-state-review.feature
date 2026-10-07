@rust @rust-3 @cleanup-review @cleanup-state-review
Feature: Cleanup preserves isolation and restores client startup parameters

  Background:
    Given PostgreSQL started with options "-c log_statement=all -c logging_collector=off" and pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @cleanup-review-startup-reset
  Scenario: Built-in RESET ALL restores untracked startup parameters on the same backend
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        sync_server_parameters: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "work_mem=96MB"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SHOW work_mem" to session "first" and store response
    Then session "first" should receive DataRow with "96MB"
    When we send SimpleQuery "SET statement_timeout = 10000" to session "first"
    And we send SimpleQuery "SHOW work_mem" to session "first" and store response
    Then session "first" should receive DataRow with "96MB"
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "work_mem=96MB"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SHOW work_mem" to session "next" and store response
    Then session "next" should receive DataRow with "96MB"

  @cleanup-review-cursor-startup
  Scenario: Cursor-only cleanup restores role and remembers unchanged startup parameters
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        sync_server_parameters: true
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "role=example_user_2,work_mem=96MB"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SELECT current_user = 'example_user_2' AND current_setting('work_mem') = '96MB'" to session "first" and store response
    Then session "first" should receive DataRow with "t"
    When we send SimpleQuery "BEGIN; DECLARE review_cursor CURSOR WITH HOLD FOR SELECT 1; COMMIT" to session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "role=example_user_2"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT current_user" to session "next" and store response
    Then session "next" should receive DataRow with "example_user_2"
    When we send SimpleQuery "SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem'" to session "next" and store response
    Then session "next" should receive DataRow with "t"
    When we send SimpleQuery "SELECT count(*) FROM pg_cursors" to session "next" and store response
    Then session "next" should receive DataRow with "0"

  @cleanup-review-single-reset
  Scenario Outline: A partial client RESET cancels only built-in parameter cleanup
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        <mode_config>
        <query_config>
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SET work_mem = '96MB'; SET statement_timeout = 10000; RESET statement_timeout" to session "first"
    And we send SimpleQuery "SELECT current_setting('work_mem') = '96MB' AND current_setting('statement_timeout') = '0'" to session "first" and store response
    Then session "first" should receive DataRow with "t"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem'" to session "next" and store response
    Then session "next" should receive DataRow with "<is_default>"

    Examples:
      | mode_config                        | query_config                    | is_default |
      | # adaptive                         | # built-in cleanup              | f          |
      | cleanup_server_connections: always | cleanup_server_query: "RESET ALL" | t          |

  @cleanup-review-client-reset-commands
  Scenario Outline: Client reset commands clear only their own built-in cleanup obligation
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "<setup>" to session "first"
    And we send SimpleQuery "<client_reset>" to session "first"
    And we close session "first"
    And we sleep 300ms
    Then PostgreSQL log should contain exactly <cleanups> occurrences of "RESET ROLE"
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT (SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem') AND NOT EXISTS (SELECT FROM pg_cursors) AND NOT EXISTS (SELECT FROM pg_prepared_statements)" to session "next" and store response
    Then session "next" should receive DataRow with "t"

    Examples:
      # Built-in cleanup starts with RESET ROLE, so the log counts it and no client
      # command in these rows produces that tag.
      | setup                                                                                               | client_reset   | cleanups |
      | SET work_mem = '96MB'                                                                                | DISCARD ALL    | 0        |
      | PREPARE review_stmt AS SELECT 1                                                                      | DEALLOCATE ALL | 0        |
      | SET work_mem = '96MB'; PREPARE review_stmt AS SELECT 1                                                 | DEALLOCATE ALL | 1        |
      | BEGIN; DECLARE review_cursor CURSOR WITH HOLD FOR SELECT 1; COMMIT                                    | CLOSE ALL      | 0        |
      | SET work_mem = '96MB'; BEGIN; DECLARE review_cursor CURSOR WITH HOLD FOR SELECT 1; COMMIT               | CLOSE ALL      | 1        |

  @cleanup-review-deallocate-after-error
  Scenario: Client DEALLOCATE ALL clears the prepared cleanup obligation after an error
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
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "PREPARE review_stmt AS SELECT 1" to session "first"
    And we send SimpleQuery "SELECT 1 / 0" to session "first" expecting error
    And we send SimpleQuery "DEALLOCATE ALL" to session "first"
    And we close session "first"
    And we sleep 300ms
    Then PostgreSQL log should contain exactly 0 occurrences of "RESET ROLE"
    When we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "next" and store response
    Then session "next" should receive DataRow with "0"

  @cleanup-review-reload
  Scenario Outline: RELOAD changes cleanup policy for new frontends while existing frontends retain their pool
    When we create session "observer" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TABLE public.cleanup_counter (n int); INSERT INTO public.cleanup_counter VALUES (0)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: always
        cleanup_server_query: "RESET ALL; UPDATE public.cleanup_counter SET n = n + 1"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid as "before_reload"
    And we send SimpleQuery "SET work_mem = '96MB'" to session "old"
    And we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive DataRow with "3"
    When we overwrite pg_doorman config file with:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: <mode>
        <query_config>
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "RELOAD" on admin session "admin"
    And we send SimpleQuery "SET work_mem = '96MB'" to session "old"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid as "after_reload"
    Then named backend_pid "after_reload" from session "old" is same as "before_reload"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive DataRow with "5"
    When we create session "new" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SET work_mem = '96MB'; SELECT pg_backend_pid()" to session "new" and store backend_pid
    Then backend_pid from session "new" should not equal backend_pid from session "old"
    When we send SimpleQuery "SELECT n FROM public.cleanup_counter" to session "observer" and store response
    Then session "observer" should receive DataRow with "<total>"
    When we send SimpleQuery "SELECT current_setting('work_mem') = '96MB'" to session "new" and store response
    Then session "new" should receive DataRow with "<work_mem_kept>"

    Examples:
      # The `old` frontend keeps the backend it was created with, so the two queries
      # it sends after RELOAD are cleaned by the policy of the first config. Every
      # row changes the policy, so `new` frontends get a backend of a rebuilt pool.
      | mode     | query_config                                                                   | total | work_mem_kept |
      | always   | cleanup_server_query: "RESET ALL; UPDATE public.cleanup_counter SET n = n + 2" | 7     | f             |
      | adaptive | # built-in cleanup                                                             | 5     | f             |
      | off      | cleanup_server_query: "RESET ALL; UPDATE public.cleanup_counter SET n = n + 1" | 5     | t             |
