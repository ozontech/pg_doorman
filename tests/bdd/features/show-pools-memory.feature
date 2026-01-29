@rust @admin @pools-memory
Feature: Admin console SHOW POOLS_MEMORY and SHOW PREPARED_STATEMENTS commands
  Test that pg_doorman admin console correctly handles memory monitoring commands

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
        prepared_statements: true
        prepared_statements_cache_size: 100
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

  @admin-commands-pools-memory
  Scenario: SHOW POOLS_MEMORY command returns valid data
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_memory" on admin session "admin" and store row count
    Then admin session "admin" row count should be greater than 0

  @admin-commands-pool-memory-alias
  Scenario: SHOW POOL_MEMORY alias command returns valid data
    When we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pool_memory" on admin session "admin" and store row count
    Then admin session "admin" row count should be greater than 0

  @admin-commands-prepared-statements
  Scenario: SHOW PREPARED_STATEMENTS command returns data after statement is prepared
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "stmt1" with query "select $1::int + $2::int" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "stmt1" with params "10, 20" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show prepared_statements" on admin session "admin" and store response
    Then admin session "admin" response should contain "select $1::int + $2::int"

  @admin-commands-pools-memory-client-metrics
  Scenario: SHOW POOLS_MEMORY shows client-level prepared statement cache metrics
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "client_stmt1" with query "select $1::int * 2" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "client_stmt1" with params "5" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    And we create admin session "admin" to pg_doorman as "admin" with password "admin"
    And we execute "show pools_memory" on admin session "admin" and store response
    Then admin session "admin" response should contain "example_db"
    And admin session "admin" response should contain "example_user_1"
