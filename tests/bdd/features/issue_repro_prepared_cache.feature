@prepared-cache @issue-repro
Feature: Prepared statements cache size limit
  Test that prepared statements cache doesn't exceed the configured limit

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
      prepared_statements_cache_size = 10

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 1
      """

  Scenario: Cache size limit is respected after 100 different Parse requests
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send 100 Parse requests with different queries to session "one"
    And we send Sync to session "one"
    And we send Parse "check_prepared" with query "SELECT count(*) FROM pg_prepared_statements" to session "one"
    And we send Sync to session "one"
    And we send Bind "" to "check_prepared" with params "" to session "one"
    And we send Execute "" to session "one"
    And we send Sync to session "one"
    Then session "one" should receive DataRow with "10"
