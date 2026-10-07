@greengage @greengage-cleanup
Feature: Backend cleanup against a real Greengage coordinator and segments
  Every scenario owns a database on the Docker Greengage cluster.
  A pool of one and the next checkout form the cleanup completion barrier.

  Background:
    Given a temporary Greengage database on the configured cluster

  @greengage-cleanup-built-in
  Scenario: Built-in cleanup resets parameters, prepared statements and held cursors on the same backend
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
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: session
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SET work_mem = '96MB'; PREPARE cleanup_stmt AS SELECT 42; BEGIN; DECLARE cleanup_cursor CURSOR WITH HOLD FOR SELECT 1; COMMIT" to session "first"
    And we send SimpleQuery "SELECT current_setting('work_mem') = '96MB' AND EXISTS (SELECT 1 FROM pg_prepared_statements WHERE name = 'cleanup_stmt') AND EXISTS (SELECT 1 FROM pg_cursors WHERE name = 'cleanup_cursor')" to session "first" and store response
    Then session "first" should receive exactly one DataRow "t"
    When we send SimpleQuery "SELECT count(*), bool_and(setting = reset_val) FROM gp_dist_random('pg_settings') WHERE name = 'work_mem'" to session "first" and store response
    Then session "first" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}|f"
    When we send SimpleQuery "SELECT 1 / 0" to session "first" expecting error
    Then session "first" should receive ErrorResponse with SQLSTATE "22012"
    And session "first" should receive ReadyForQuery "I"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT (SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem') AND NOT EXISTS (SELECT 1 FROM pg_prepared_statements) AND NOT EXISTS (SELECT 1 FROM pg_cursors)" to session "next" and store response
    Then session "next" should receive exactly one DataRow "t"
    And session "next" should receive only backend messages "TDCZ"
    When we send SimpleQuery "SELECT count(*), bool_and(setting = reset_val) FROM gp_dist_random('pg_settings') WHERE name = 'work_mem'" to session "next" and store response
    Then session "next" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}|t"

  @greengage-cleanup-full
  Scenario: Explicit supported cleanup removes temporary tables from coordinator and all segments
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: always
        cleanup_server_query: "SET SESSION AUTHORIZATION DEFAULT; RESET ALL; DEALLOCATE ALL; CLOSE ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); DISCARD PLANS; DISCARD SEQUENCES; DISCARD TEMP"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: session
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SET work_mem = '96MB'; CREATE TEMP TABLE cleanup_temp (id integer) DISTRIBUTED BY (id); INSERT INTO cleanup_temp VALUES (1), (2); PREPARE cleanup_stmt AS SELECT 42; BEGIN; DECLARE cleanup_cursor CURSOR WITH HOLD FOR SELECT 1; COMMIT" to session "first"
    And we send SimpleQuery "SELECT current_setting('work_mem') = '96MB' AND to_regclass('pg_temp.cleanup_temp') IS NOT NULL AND EXISTS (SELECT 1 FROM pg_prepared_statements WHERE name = 'cleanup_stmt') AND EXISTS (SELECT 1 FROM pg_cursors WHERE name = 'cleanup_cursor')" to session "first" and store response
    Then session "first" should receive exactly one DataRow "t"
    When we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'cleanup_temp'" to session "first" and store response
    Then session "first" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT (SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem') AND to_regclass('pg_temp.cleanup_temp') IS NULL AND NOT EXISTS (SELECT 1 FROM pg_prepared_statements) AND NOT EXISTS (SELECT 1 FROM pg_cursors)" to session "next" and store response
    Then session "next" should receive exactly one DataRow "t"
    And session "next" should receive only backend messages "TDCZ"
    When we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'cleanup_temp'" to session "next" and store response
    Then session "next" should receive exactly one DataRow "0"

  @greengage-cleanup-prepared
  Scenario: An extended prepared statement reparses and restores startup parameters after supported cleanup
    When we send SimpleQuery "CREATE SCHEMA startup_schema; CREATE TABLE startup_schema.values_table (value integer) DISTRIBUTED BY (value); INSERT INTO startup_schema.values_table VALUES (1)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        prepared_statements: true
        sync_server_parameters: true
        cleanup_server_connections: always
        cleanup_server_query: "SET SESSION AUTHORIZATION DEFAULT; RESET ALL; DEALLOCATE ALL; CLOSE ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); DISCARD PLANS; DISCARD SEQUENCES; DISCARD TEMP"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: transaction
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db" and startup parameters "search_path=startup_schema"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "before"
    And we send Parse "cached" with query "SELECT value + $1::int FROM values_table" to session "one"
    And we send Bind "" to "cached" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive exactly one DataRow "11"
    And session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT count(*) FROM pg_prepared_statements" to session "one" and store response
    Then session "one" should receive exactly one DataRow "0"
    When we send Bind "" to "cached" with params "11" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive exactly one DataRow "12"
    And session "one" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then named backend_pid "after" from session "one" is same as "before"

  @greengage-discard-all-notice
  Scenario: Real DISCARD ALL reports its partial cleanup and leaves temporary tables on segments
    When we send SimpleQuery "SET work_mem = '96MB'; CREATE TEMP TABLE discard_temp (id integer) DISTRIBUTED BY (id); PREPARE discard_stmt AS SELECT 42" to session "observer"
    And we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'discard_temp'" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"
    When we send SimpleQuery "DISCARD ALL" to session "observer" and store response
    Then session "observer" should receive NoticeResponse with SQLSTATE "0AM01"
    And session "observer" should receive CommandComplete "DISCARD ALL"
    And session "observer" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT (SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem') AND NOT EXISTS (SELECT 1 FROM pg_prepared_statements) AND to_regclass('pg_temp.discard_temp') IS NULL" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "t"
    When we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'discard_temp'" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"

  @greengage-discard-all-reuse
  Scenario: A real DISCARD ALL cleanup notice preserves backend reuse while segment cleanup remains incomplete
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: always
        cleanup_server_query: "DISCARD ALL"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: session
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SET work_mem = '96MB'; CREATE TEMP TABLE discard_temp (id integer) DISTRIBUTED BY (id)" to session "first"
    And we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'discard_temp'" to session "first" and store response
    Then session "first" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT (SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem') AND to_regclass('pg_temp.discard_temp') IS NULL" to session "next" and store response
    Then session "next" should receive exactly one DataRow "t"
    And session "next" should receive only backend messages "TDCZ"
    When we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'discard_temp'" to session "next" and store response
    Then session "next" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"

  @greengage-discard-all-suppressed
  Scenario: client_min_messages suppresses the DISCARD ALL notice while segment cleanup remains incomplete
    # Greengage emits 0AM01 before resetting client_min_messages. At warning,
    # it sends no NoticeResponse; the command still leaves segment temporary tables.
    When we send SimpleQuery "SET client_min_messages = warning" to session "observer"
    And we send SimpleQuery "DISCARD ALL" to session "observer" and store response
    Then session "observer" should receive only backend messages "CZ"
    And session "observer" should receive CommandComplete "DISCARD ALL"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: always
        cleanup_server_query: "DISCARD ALL"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: session
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SET client_min_messages = warning; CREATE TEMP TABLE suppressed_temp (id integer) DISTRIBUTED BY (id)" to session "first"
    And we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'suppressed_temp'" to session "first" and store response
    Then session "first" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT to_regclass('pg_temp.suppressed_temp') IS NULL" to session "next" and store response
    Then session "next" should receive exactly one DataRow "t"
    And session "next" should receive only backend messages "TDCZ"
    When we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'suppressed_temp'" to session "next" and store response
    Then session "next" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"

  @greengage-user-function-notice
  Scenario Outline: User function notices reach clients and preserve backend reuse when called by cleanup
    When we send SimpleQuery "CREATE FUNCTION public.cleanup_notice(notice_code text) RETURNS integer LANGUAGE plpgsql AS $$ BEGIN RAISE NOTICE USING ERRCODE = notice_code, MESSAGE = 'application notice'; RETURN 7; END $$" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: always
        cleanup_server_query: "RESET ALL; SELECT public.cleanup_notice('<code>')"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: session
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    And we send SimpleQuery "SET work_mem = '96MB'" to session "first"
    And we send SimpleQuery "SELECT current_setting('work_mem') = '96MB'" to session "first" and store response
    Then session "first" should receive exactly one DataRow "t"
    When we send SimpleQuery "SELECT public.cleanup_notice('<code>')" to session "first" and store response
    Then session "first" should receive NoticeResponse with SQLSTATE "<code>"
    And session "first" should receive exactly one DataRow "7"
    And session "first" should receive ReadyForQuery "I"
    When we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid as "after_notice"
    Then backend_pid "after_notice" from session "first" should equal initial backend_pid from session "first"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT setting = reset_val FROM pg_settings WHERE name = 'work_mem'" to session "next" and store response
    Then session "next" should receive exactly one DataRow "t"
    And session "next" should receive only backend messages "TDCZ"

    Examples:
      | code  |
      | 0AM01 |
      | 0A000 |

  @greengage-cleanup-commit
  Scenario: A cleanup SQL error preserves the completed COMMIT response and committed rows
    When we send SimpleQuery "CREATE TABLE public.committed_rows (id integer) DISTRIBUTED BY (id)" to session "observer"
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        cleanup_server_connections: always
        cleanup_server_query: "SELECT 1 / 0"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: transaction
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "before"
    And we send SimpleQuery "INSERT INTO public.committed_rows VALUES (1)" to session "one"
    And we send SimpleQuery "COMMIT" to session "one" and store response
    Then session "one" should receive CommandComplete "COMMIT"
    And session "one" should receive ReadyForQuery "I"
    And session "one" should receive only backend messages "CZ"
    When we send SimpleQuery "SELECT count(*) FROM public.committed_rows" to session "observer" and store response
    Then session "observer" should receive exactly one DataRow "1"
    When we send SimpleQuery "BEGIN; SELECT pg_backend_pid()" to session "one" and store backend_pid as "after"
    Then named backend_pid "after" from session "one" is different from "before"
    When we send SimpleQuery "SELECT count(*) FROM public.committed_rows" to session "one" and store response
    Then session "one" should receive exactly one DataRow "1"
    When we send SimpleQuery "ROLLBACK" to session "one" and store response
    Then session "one" should receive CommandComplete "ROLLBACK"
    And session "one" should receive ReadyForQuery "I"

  @greengage-cleanup-built-in-temp-tables
  Scenario: Built-in DISCARD TEMP clears the temporary table on the coordinator and on the segments
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
          server_host: "${GREENGAGE_HOST}"
          server_port: ${GREENGAGE_PORT}
          server_database: "${GREENGAGE_DATABASE}"
          pool_mode: session
          users: [{username: example_user_1, password: "", server_username: "${GREENGAGE_USER}", server_password: "${GREENGAGE_PASSWORD}", pool_size: 1}]
      """
    When we create session "first" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "first" and store backend_pid
    # One batch is one lease: the temp table must exist here. After the
    # checkin the built-in cleanup drops it, which the next session verifies.
    And we send SimpleQuery "CREATE TEMP TABLE built_in_temp (id integer) DISTRIBUTED BY (id); SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'built_in_temp'" to session "first" and store response
    Then session "first" should receive exactly one DataRow "${GREENGAGE_SEGMENTS}"
    When we close session "first"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "first"
    When we send SimpleQuery "SELECT to_regclass('pg_temp.built_in_temp') IS NULL" to session "next" and store response
    Then session "next" should receive exactly one DataRow "t"
    And session "next" should receive only backend messages "TDCZ"
    When we send SimpleQuery "SELECT count(*) FROM gp_dist_random('pg_class') WHERE relname = 'built_in_temp'" to session "next" and store response
    Then session "next" should receive exactly one DataRow "0"
