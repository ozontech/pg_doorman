@rust @query-wait-timeout
Feature: Query wait timeout when pool is exhausted
  Test that query_wait_timeout works correctly when all pool connections are busy

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
      query_wait_timeout = "100ms"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 3
      """

  @query-wait-timeout-first
  Scenario: Fourth connection gets query_wait_timeout when pool is exhausted
    # Create 3 sessions that will hold all pool connections with long-running queries
    When we create session "one" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "two" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we create session "three" to pg_doorman as "example_user_1" with password "" and database "example_db"
    # Start long-running queries on all 3 connections (2 seconds each)
    And we send SimpleQuery "select pg_sleep(2)" to session "one" without waiting
    And we send SimpleQuery "select pg_sleep(2)" to session "two" without waiting
    And we send SimpleQuery "select pg_sleep(2)" to session "three" without waiting
    # Wait a bit to ensure all connections are busy
    And we sleep 150ms
    # Fourth connection should timeout waiting for a connection from the pool
    When we create session "four" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "select 1" to session "four" expecting error
    Then session "four" should receive error containing "timeout"
