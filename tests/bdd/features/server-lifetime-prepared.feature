@rust @server-lifetime-prepared
Feature: Prepared statements work correctly after server_lifetime expires
  Test that prepared statements continue to work after backend connection is recycled due to server_lifetime

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all all 127.0.0.1/32 trust"
      prepared_statements = true
      prepared_statements_cache_size = 100
      server_lifetime = 100
      retain_connections_time = 200

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  Scenario: Prepared statement works after server_lifetime expires and backend changes
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we remember backend_pid from session "one" as "first_pid"
    When we send Parse "stmt1" with query "select $1::int + $2::int" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "stmt1" with params "10, 20" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "30"
    When we sleep for 2000 milliseconds
    And we send Parse "" with query "select pg_backend_pid()" to session "one"
    And we send Bind "" to "" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then we verify backend_pid from session "one" is different from "first_pid"
    When we send Bind "" to "stmt1" with params "5, 15" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "20"
