@rust @rust-2 @admin-commands
Feature: Admin console SHOW commands
  All SHOW commands return valid, driver-parseable responses.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        admin_username: "admin"
        admin_password: "admin"
        pg_hba:
          content: "host all all 127.0.0.1/32 trust"
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          users:
            - username: "example_user_1"
              password: ""
              pool_size: 10
      """

  @admin-commands-show
  Scenario Outline: SHOW <command> returns rows
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show <command>" on admin session "admin" and store row count
    Then admin session "admin" row count should be greater than or equal to <min_rows>

    Examples:
      | command        | min_rows |
      | config         | 1        |
      | databases      | 1        |
      | lists          | 1        |
      | pools          | 1        |
      | pools_extended | 1        |
      | clients        | 1        |
      | servers        | 0        |
      | connections    | 1        |
      | stats          | 1        |
      | version        | 1        |
      | users          | 1        |

  @admin-commands-sockets
  Scenario: SHOW SOCKETS does not crash (Linux only)
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show sockets" on admin session "admin" expecting possible error

  @admin-commands-help
  Scenario: SHOW HELP returns help text
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show help" on admin session "admin" and store response
    Then admin session "admin" response should contain "SHOW HELP"
