@rust @rust-1 @backend-write-stall @flush-timeout
Feature: Backend writes survive a temporary PostgreSQL stall
  A write that waits for socket capacity must resume when PostgreSQL resumes,
  even when the pause exceeds the former five-second batch send deadline.

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
      prepared_statements = false

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5
      """
    When we create session "client1" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "client1"

  @flush-timeout-basic
  Scenario: An extended batch completes after PostgreSQL resumes
    When we freeze PostgreSQL with SIGSTOP
    And we send large batch of 500 Parse messages with 16KB queries to session "client1"
    And we send Sync to session "client1" without waiting for response
    And we sleep 6000ms
    And we unfreeze PostgreSQL with SIGCONT
    Then session "client1" should finish with 500 ParseComplete and 0 DataRow messages within 15000ms
    And session "client1" should accept another query after the batch

  @flush-timeout-simple-query
  Scenario: A simple query completes after PostgreSQL resumes
    When we freeze PostgreSQL with SIGSTOP
    And we send large SimpleQuery with 8192KB padding to session "client1" without waiting
    And we sleep 6000ms
    And we unfreeze PostgreSQL with SIGCONT
    Then session "client1" should finish with 0 ParseComplete and 1 DataRow messages within 15000ms
    And session "client1" should accept another query after the batch
