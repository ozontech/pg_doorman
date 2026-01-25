@rust @rust-3 @admin-leak
Feature: Admin console client leak detection
  Test that pg_doorman correctly tracks connected clients in admin console (pgbouncer database)
  and properly cleans up when TCP connections are abruptly closed

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

  @admin-leak-detection
  Scenario: Client leak detection via show clients after TCP abort
    # Connect stable driver connection to admin console
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    # Execute show clients - should see only our connection (1 row)
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1
    # Connect raw session to admin console
    When we create admin session "raw" to pg_doorman as "admin" with password "admin"
    # Execute show clients on raw session
    And we execute "show clients" on admin session "raw" and store row count
    # In stable connection should see 2 clients now
    When we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    # Abort TCP connection for raw session (simulate network failure)
    When we abort TCP connection for session "raw"
    # Wait for pg_doorman to detect the disconnection
    And we sleep 2000ms
    # In stable connection should see only 1 client again
    When we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1

  @admin-leak-aggressive-connect-disconnect
  Scenario: Aggressive connect/disconnect cycle - 5 connections
    # Connect stable driver connection to admin console
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1
    # Aggressively connect and disconnect 5 raw sessions
    When we create admin session "raw1" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw1"
    And we sleep 500ms
    When we create admin session "raw2" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw2"
    And we sleep 500ms
    When we create admin session "raw3" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw3"
    And we sleep 500ms
    When we create admin session "raw4" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw4"
    And we sleep 500ms
    When we create admin session "raw5" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw5"
    And we sleep 2000ms
    # After all aborts, should see only stable connection
    When we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1

  @admin-leak-multiple-simultaneous
  Scenario: Multiple simultaneous connections then all abort
    # Connect stable driver connection to admin console
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1
    # Connect multiple raw sessions simultaneously
    When we create admin session "raw1" to pg_doorman as "admin" with password "admin"
    And we create admin session "raw2" to pg_doorman as "admin" with password "admin"
    And we create admin session "raw3" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 4
    # Abort all raw connections
    When we abort TCP connection for session "raw1"
    And we abort TCP connection for session "raw2"
    And we abort TCP connection for session "raw3"
    And we sleep 2000ms
    # Should see only stable connection
    When we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1

  @admin-leak-rapid-fire
  Scenario: Rapid fire connect/abort without waiting
    # Connect stable driver connection to admin console
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1
    # Rapid fire: connect and immediately abort 10 times
    When we create admin session "rapid1" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid1"
    When we create admin session "rapid2" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid2"
    When we create admin session "rapid3" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid3"
    When we create admin session "rapid4" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid4"
    When we create admin session "rapid5" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid5"
    When we create admin session "rapid6" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid6"
    When we create admin session "rapid7" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid7"
    When we create admin session "rapid8" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid8"
    When we create admin session "rapid9" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid9"
    When we create admin session "rapid10" to pg_doorman as "admin" with password "admin"
    And we abort TCP connection for session "rapid10"
    # Wait for cleanup
    And we sleep 3000ms
    # Should see only stable connection - no leaked clients
    When we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1

  @admin-leak-interleaved-operations
  Scenario: Interleaved connect/query/abort operations
    # Connect stable driver connection to admin console
    When we create admin session "stable" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1
    # Connect raw, execute query, then abort
    When we create admin session "raw1" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "raw1" and store row count
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw1"
    And we sleep 500ms
    # Connect another raw, execute multiple queries, then abort
    When we create admin session "raw2" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "raw2" and store row count
    And we execute "show servers" on admin session "raw2" and store row count
    And we execute "show pools" on admin session "raw2" and store row count
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw2"
    And we sleep 500ms
    # Connect raw, don't execute any query, just abort
    When we create admin session "raw3" to pg_doorman as "admin" with password "admin"
    And we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 2
    When we abort TCP connection for session "raw3"
    And we sleep 2000ms
    # Final check - should see only stable connection
    When we execute "show clients" on admin session "stable" and store row count
    Then admin session "stable" row count should be 1
