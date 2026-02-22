@auth-query
Feature: AuthQueryExecutor — credential fetching from PostgreSQL

  Tests for the AuthQueryExecutor component that connects to PostgreSQL
  and executes auth queries to fetch user credentials from a custom table.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/auth_query_fixture.sql" applied

  Scenario: Fetch MD5 password hash for existing user
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "test_user_md5"
    Then auth_query result should contain user "test_user_md5" with password "md53175bce1d3201d16594cebf9d7eb3f9d"

  Scenario: Fetch SCRAM password hash for existing user
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "test_user_scram"
    Then auth_query result should contain user "test_user_scram" with password starting with "SCRAM-SHA-256"

  Scenario: Non-existent user returns not found
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "no_such_user"
    Then auth_query result should be not found

  Scenario: User with NULL password returns not found
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "test_user_null_pw"
    Then auth_query result should be not found

  Scenario: User with empty password returns not found
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "test_user_empty_pw"
    Then auth_query result should be not found

  Scenario: Multiple rows for same user returns config error
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "duplicate_user"
    Then auth_query result should be config error containing "expected 0 or 1"

  Scenario: Executor handles sequential fetches correctly
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 2
    When auth_query fetches password for user "test_user_md5"
    Then auth_query result should contain user "test_user_md5"
    When auth_query fetches password for user "test_user_scram"
    Then auth_query result should contain user "test_user_scram"
    When auth_query fetches password for user "no_such_user"
    Then auth_query result should be not found

  Scenario: Executor creation fails when server is unreachable
    Then auth_query executor creation should fail for host "127.0.0.1" port 1 with connection error

  Scenario: Executor works with explicit database
    Given auth_query executor connected to database "postgres" with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 1
    When auth_query fetches password for user "test_user_md5"
    Then auth_query result should contain user "test_user_md5" with password "md53175bce1d3201d16594cebf9d7eb3f9d"

  Scenario: Executor works with pool_size 1
    Given auth_query executor connected with query "SELECT username, password FROM auth_users WHERE username = $1" and pool_size 1
    When auth_query fetches password for user "test_user_md5"
    Then auth_query result should contain user "test_user_md5"
    When auth_query fetches password for user "test_user_scram"
    Then auth_query result should contain user "test_user_scram"
