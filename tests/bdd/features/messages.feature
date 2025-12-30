@rust @messages
Feature: Message comparison
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
      prepared_statements_cache_size = 10000

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  Scenario: SimpleQuery select 1; gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "select 1;" to both
    Then we should receive identical messages from both

  Scenario: Extended query protocol (Prepared Statement) gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send Parse "" with query "select $1::int" to both
    And we send Bind "" to "" with params "1" to both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
@debug
  Scenario: Extended query protocol with many messages
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we repeat 200 times: Parse "" with query "select $1::int", Bind "" to "" with params "1", Describe "P" "", Execute "" to postgres
    Then we should receive 1004 messages from postgres
@debug
  Scenario: Extended query protocol with many messages before Sync gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we repeat 200 times: Parse "" with query "select $1::int", Bind "" to "" with params "1", Describe "P" "", Execute "" to both
    Then we should receive identical messages from both
