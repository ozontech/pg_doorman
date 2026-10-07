@rust @rust-3 @cleanup-review @cleanup-observability
Feature: Backend cleanup observability

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied

  @cleanup-review-metrics
  Scenario Outline: Count completed cleanup attempts without counting idle checkins or scrapes
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        cleanup_server_connections: <mode>
        <query_config>
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      web:
        enabled: true
        host: "127.0.0.1"
        port: 9129
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "<client_query>" to session "one"
    # Session pooling has not released the backend yet, so there was no cleanup.
    When I run shell command:
      """
      python3 - <<'PY'
      import urllib.request
      body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
      for result in ('ok', 'error'):
          expected = 'pg_doorman_server_cleanup_total{database="example_db",result="' + result + '",user="example_user_1"} 0'
          assert expected in body.splitlines(), body
      PY
      """
    Then the command should succeed
    When we close session "one"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "next"
    # Checkout of the next lease waits for the previous checkin and must not
    # introduce another cleanup attempt. Its session stays open during scrapes.
    # Poll the observable completion of asynchronous session checkin, then scrape
    # again to catch counters that accidentally increase during collection.
    And I run shell command:
      """
      python3 - <<'PY'
      import time
      import urllib.request
      expected = {'ok': <ok>, 'error': <error>}
      deadline = time.monotonic() + 5
      def snapshot():
          body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
          lines = body.splitlines()
          return all('pg_doorman_server_cleanup_total{database="example_db",result="' + result + '",user="example_user_1"} ' + str(count) in lines for result, count in expected.items()), body
      while True:
          matches, body = snapshot()
          if matches:
              break
          assert time.monotonic() < deadline, body
          time.sleep(0.05)
      for _ in range(3):
          matches, body = snapshot()
          assert matches, body
      PY
      """
    Then the command should succeed
    And pg_doorman log <log_expectation> "[example_user_1@example_db] server cleanup failed, retiring backend"

    Examples:
      | mode     | query_config                                      | client_query            | ok | error | log_expectation  |
      | always   | cleanup_server_query: "DISCARD ALL"               | SET work_mem = '96MB'   | 1  | 0     | does not contain |
      | always   | cleanup_server_query: "DISCARD ALL"               | SELECT 1                | 1  | 0     | does not contain |
      | adaptive |                                                   | SELECT 1                | 0  | 0     | does not contain |
      | off      | cleanup_server_query: "DISCARD ALL"               | SET work_mem = '96MB'   | 0  | 0     | does not contain |
      | always   | cleanup_server_query: "SELECT 1 / 0"              | SET work_mem = '96MB'   | 0  | 1     | contains         |
      | always   | cleanup_server_query: "/* empty reset */"         | SET work_mem = '96MB'   | 0  | 1     | contains         |
      | always   | cleanup_server_query: "SELECT pg_sleep(5)"        | SET work_mem = '96MB'   | 0  | 1     | contains         |
      | always   | cleanup_server_query: "DISCARD ALL"               | BEGIN; SELECT 1         | 1  | 0     | does not contain |
      | adaptive |                                                   | SET work_mem = '96MB'   | 1  | 0     | does not contain |
      | adaptive |                                                   | BEGIN; SELECT 1         | 1  | 0     | does not contain |

  @cleanup-review-reset-logs
  Scenario Outline: Routine configured cleanup does not warn when clearing the prepared cache
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: admin
        admin_password: admin
        prepared_statements: true
        cleanup_server_connections: always
        cleanup_server_query: "<reset_query>"
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "one"
    And we send Parse "cached" with query "SELECT $1::int + 1" to session "one"
    And we send Bind "" to "cached" with params "10" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "11"
    And session "one" should receive ReadyForQuery "I"
    When we send Bind "" to "cached" with params "11" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "12"
    And session "one" should receive ReadyForQuery "I"
    And pg_doorman log does not contain "clearing prepared statement cache"

    Examples:
      | reset_query                               |
      | DISCARD ALL                               |
      | RESET ALL; DEALLOCATE ALL; CLOSE ALL       |
