@rust @rust-3 @cleanup-policy-adaptive
Feature: Built-in cleanup without cleanup_server_query
  A configured query belongs to `always`. Without one, `adaptive` sends built-in
  cleanup only when it observed a session change, and `off` sends none. `off` still
  rolls back a transaction the client left open. The mode is inherited by every pool.

  Background:
    Given PostgreSQL started with options "-c log_statement=all -c logging_collector=off" and pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @cleanup-policy-adaptive-modes
  Scenario Outline: Mode <mode> with pool setting <mode_override> sends <resets> built-in cleanups
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      cleanup_server_connections = <mode>

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "session"
      <mode_override>

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "<first_query>" to session "one"
    And we send SimpleQuery "<second_query>" to session "one"
    And we close session "one"
    And we sleep 300ms
    Then PostgreSQL log should contain exactly <resets> occurrences of "RESET ALL"

    Examples:
      | mode     | mode_override                         | first_query                          | second_query | resets |
      | "adaptive" | # inherit                           | SELECT 1                             | SELECT 1     | 0      |
      | "adaptive" | # inherit                           | SET statement_timeout = 10000        | SELECT 1     | 1      |
      | "adaptive" | # inherit                           | SET statement_timeout = 10000        | DISCARD ALL  | 0      |
      | "adaptive" | # inherit                           | BEGIN; SET statement_timeout = 10000 | SELECT 1     | 1      |
      | "off"    | # inherit                           | SET statement_timeout = 10000        | SELECT 1     | 0      |
      | "adaptive" | cleanup_server_connections = "off"  | SET statement_timeout = 10000        | SELECT 1     | 0      |
      | "off"    | cleanup_server_connections = "adaptive" | SET statement_timeout = 10000  | SELECT 1     | 1      |
      | "adaptive" | cleanup_server_connections = true   | SET statement_timeout = 10000        | SELECT 1     | 1      |
      | "adaptive" | cleanup_server_connections = false  | SET statement_timeout = 10000        | SELECT 1     | 0      |

  @cleanup-policy-adaptive-rollback-split
  Scenario Outline: Mode <mode> rolls back an abandoned transaction and resets session state only in adaptive
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      cleanup_server_connections = <mode>

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "session"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one"
    And we sleep 100ms
    When we truncate PostgreSQL log
    And we send SimpleQuery "<client_query>" to session "one"
    And we close session "one"
    And we sleep 300ms
    # The transaction rollback belongs to every mode: a client that leaves a
    # transaction open must not hand it to the next client.
    Then PostgreSQL log should contain exactly <rollbacks> occurrences of "ROLLBACK"
    # The RESET sequence belongs to adaptive only. off never sends it, even when
    # the session state changed.
    And PostgreSQL log should contain exactly <reset_role> occurrences of "RESET ROLE"
    And PostgreSQL log should contain exactly <reset_all> occurrences of "RESET ALL"
    And PostgreSQL log should contain exactly <close_all> occurrences of "CLOSE ALL"
    # DEALLOCATE ALL belongs to the PREPARE flag alone: cursors and SET must
    # not drag the pooler-side prepared statement cache down with them.
    And PostgreSQL log should contain exactly <deallocates> occurrences of "DEALLOCATE ALL"

    Examples:
      | mode       | client_query                                                     | rollbacks | reset_role | reset_all | close_all | deallocates |
      | "off"      | BEGIN; SET statement_timeout = 10000                             | 1         | 0          | 0         | 0         | 0           |
      | "adaptive" | BEGIN; SET statement_timeout = 10000                             | 1         | 1          | 1         | 0         | 0           |
      | "off"      | BEGIN; DECLARE doorman_cur CURSOR FOR SELECT 1                   | 1         | 0          | 0         | 0         | 0           |
      | "adaptive" | BEGIN; DECLARE doorman_cur CURSOR FOR SELECT 1                   | 1         | 1          | 0         | 1         | 0           |
      | "off"      | SET statement_timeout = 10000                                    | 0         | 0          | 0         | 0         | 0           |
      | "adaptive" | SET statement_timeout = 10000                                    | 0         | 1          | 1         | 0         | 0           |
      | "off"      | BEGIN; DECLARE doorman_cur CURSOR WITH HOLD FOR SELECT 1; COMMIT | 0         | 0          | 0         | 0         | 0           |
      | "adaptive" | BEGIN; DECLARE doorman_cur CURSOR WITH HOLD FOR SELECT 1; COMMIT | 0         | 1          | 0         | 1         | 0           |
      | "off"      | SELECT 1                                                         | 0         | 0          | 0         | 0         | 0           |
      | "adaptive" | SELECT 1                                                         | 0         | 0          | 0         | 0         | 0           |
      | "off"      | LISTEN split_ch                                                  | 0         | 0          | 0         | 0         | 0           |
      | "adaptive" | LISTEN split_ch                                                  | 0         | 1          | 0         | 0         | 0           |
      | "off"      | PREPARE split_stmt AS SELECT 1                                   | 0         | 0          | 0         | 0         | 0           |
      | "adaptive" | PREPARE split_stmt AS SELECT 1                                   | 0         | 1          | 0         | 0         | 1           |
      | "off"      | CREATE TABLE split_perm (i int)                                  | 0         | 0          | 0         | 0         | 0           |
      | "adaptive" | CREATE TABLE split_perm (i int)                                  | 0         | 1          | 0         | 0         | 0           |

  @cleanup-policy-adaptive-prepared-desync
  Scenario Outline: Mode <mode> re-synchronizes the prepared statement cache only in adaptive
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      cleanup_server_connections = <mode>

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "session"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "one"
    And we sleep 100ms
    When we truncate PostgreSQL log
    # pg_doorman keeps the Parse in its own buffer until Sync, so PostgreSQL never
    # sees this statement. The client disconnects before Sync.
    And we send Parse "desync" with query "SELECT 42" to session "one"
    And we send Bind "" to "desync" with params "" to session "one"
    And we send Execute "" to session "one"
    And we close session "one"
    And we sleep 300ms
    Then PostgreSQL log should contain exactly <deallocates> occurrences of "DEALLOCATE ALL"
    And PostgreSQL log should contain exactly <reset_role> occurrences of "RESET ROLE"

    Examples:
      | mode       | deallocates | reset_role |
      | "off"      | 0           | 0          |
      | "adaptive" | 1           | 1          |
