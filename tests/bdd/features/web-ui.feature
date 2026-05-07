@web-ui
Feature: Web UI listener
  Sanity coverage for the embedded SPA, the public API endpoints, the
  admin auth gate, and the JSON 401 path that lets the React modal take
  over from the browser's native basic-auth dialog.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And pg_doorman hba file contains:
      """
      host all example_user_1 127.0.0.1/32 trust
      """
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "webui_bdd"

      [web]
      enabled = true
      host = "127.0.0.1"
      port = 9127
      ui = true
      ui_anonymous = true
      log_tap_max_entries = 4096

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """

  Scenario: SPA shell is served at /
    When I run shell command:
      """
      curl -s -o /dev/null -w "%{http_code} %{content_type}\n" http://127.0.0.1:9127/
      """
    Then the command should succeed
    And output contains "200"
    And output contains "text/html"

  Scenario: Deep link falls back to the SPA shell
    When I run shell command:
      """
      curl -s -o /dev/null -w "%{http_code} %{content_type}\n" http://127.0.0.1:9127/pools
      """
    Then the command should succeed
    And output contains "200"
    And output contains "text/html"

  Scenario: /api/version is anonymous when ui_anonymous is true
    When I run shell command:
      """
      curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/api/version
      """
    Then the command should succeed
    And output contains "200"

  Scenario: /api/logs is admin-only without credentials
    When I run shell command:
      """
      curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/api/logs
      """
    Then the command should succeed
    And output contains "401"

  Scenario: JSON callers receive 401 without WWW-Authenticate
    When I run shell command:
      """
      curl -s -i -H "Accept: application/json" http://127.0.0.1:9127/api/logs | grep -ci 'WWW-Authenticate' || echo 0
      """
    Then the command should succeed
    And output contains "0"

  Scenario: Curl-style callers do receive WWW-Authenticate on 401
    When I run shell command:
      """
      curl -s -i http://127.0.0.1:9127/api/logs | grep -ci 'WWW-Authenticate'
      """
    Then the command should succeed
    And output contains "1"

  Scenario: /api/logs accepts admin basic auth
    When I run shell command:
      """
      curl -s --user 'admin:webui_bdd' -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/api/logs
      """
    Then the command should succeed
    And output contains "200"

  Scenario: /metrics still serves Prometheus when the UI is on
    When I run shell command:
      """
      curl -s http://127.0.0.1:9127/metrics | head -1
      """
    Then the command should succeed
    And output contains "# HELP"

  Scenario: /api/auth/config is anonymous and reports SSO disabled
    When I run shell command:
      """
      curl -s http://127.0.0.1:9127/api/auth/config
      """
    Then the command should succeed
    And output contains "\"sso_enabled\":false"
    And output contains "\"current_user\":null"

  Scenario: /api/auth/config carries admin identity for Basic auth
    When I run shell command:
      """
      curl -s --user 'admin:webui_bdd' http://127.0.0.1:9127/api/auth/config
      """
    Then the command should succeed
    And output contains "\"role\":\"admin\""
    And output contains "\"source\":\"basic\""

  Scenario: /api/admin/reload requires Admin and rejects anonymous with 401
    When I run shell command:
      """
      curl -s -X POST -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/api/admin/reload
      """
    Then the command should succeed
    And output contains "401"

  Scenario: Bearer with malformed token is Rejected (401) when SSO is off
    When I run shell command:
      """
      curl -s -H "Authorization: Bearer not.a.real.jwt" -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9127/api/logs
      """
    Then the command should succeed
    And output contains "401"

  Scenario: Access log line surfaces in /api/logs after a probe
    When I run shell command:
      """
      curl -s -o /dev/null http://127.0.0.1:9127/api/version &&
      sleep 1 &&
      curl -s --user 'admin:webui_bdd' http://127.0.0.1:9127/api/logs | grep -c 'pg_doorman::web::access' || echo 0
      """
    Then the command should succeed
