@rust @rust-2 @admin-commands
Feature: Admin console SHOW commands
  Test that pg_doorman admin console correctly handles all SHOW commands
  and that the driver can parse the responses

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

  @admin-commands-config
  Scenario: SHOW CONFIG command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show config" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-databases
  Scenario: SHOW DATABASES command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show databases" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-lists
  Scenario: SHOW LISTS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show lists" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-pools
  Scenario: SHOW POOLS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show pools" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-pools-extended
  Scenario: SHOW POOLS_EXTENDED command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_extended" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-clients
  Scenario: SHOW CLIENTS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-servers
  Scenario: SHOW SERVERS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show servers" on admin session "stable" and store row count
    # Servers may be 0 if no connections have been made yet
    Then admin session "stable" row count should be greater than or equal to 0

  @admin-commands-connections
  Scenario: SHOW CONNECTIONS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show connections" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-stats
  Scenario: SHOW STATS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show stats" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-version
  Scenario: SHOW VERSION command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show version" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-users
  Scenario: SHOW USERS command is parseable by driver
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show users" on admin session "stable" and store row count
    Then admin session "stable" row count should be greater than 0

  @admin-commands-sockets
  Scenario: SHOW SOCKETS command is parseable by driver (Linux only)
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show sockets" on admin session "stable" expecting possible error
    # This command only works on Linux, so we just check it doesn't crash

  @admin-commands-help
  Scenario: SHOW HELP command returns help text containing SHOW HELP
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show help" on admin session "stable" and store response
    Then admin session "stable" response should contain "SHOW HELP"
