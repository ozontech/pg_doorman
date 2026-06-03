@issue-267 @large-object-fastpath
Feature: PostgreSQL large object fastpath API
  Clients using PostgreSQL LargeObject API must complete fastpath calls
  while connected through transaction pooling.

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      connect_timeout = 5000
      admin_username = "admin"
      admin_password = "admin"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = "md58a67a0c805a5ee0384ea28e0dea557b6"
      pool_size = 1
      """

  @rust-1 @issue-267-fastpath-wire
  Scenario: Fastpath FunctionCall holds the backend until transaction end
    When we create session "client" to pg_doorman as "example_user_1" with password "test" and database "example_db"
    And we send SimpleQuery "BEGIN" to session "client" and store response
    Then session "client" should receive ReadyForQuery "T"
    # 957 is pg_catalog.lo_creat(integer) in the standard PostgreSQL catalog.
    # pgjdbc discovers fastpath function OIDs from pg_proc; this raw step pins the wire path.
    When we send FunctionCall 957 with int args "393216" to session "client"
    Then we read PostgreSQL response from session "client" within 1000ms
    And session "client" should receive FunctionCallResponse with 4 byte result
    And session "client" should receive ReadyForQuery "T"
    When we create admin session "admin_tx" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin_tx" and store response
    Then admin session "admin_tx" column "cl_active" for row with "user" = "example_user_1" should be between 1 and 1
    And admin session "admin_tx" column "sv_active" for row with "user" = "example_user_1" should be between 1 and 1
    When we send SimpleQuery "COMMIT" to session "client" and store response
    Then session "client" should receive ReadyForQuery "I"
    When we execute "SHOW POOLS" on admin session "admin_tx" and store response
    Then admin session "admin_tx" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin_tx" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0

  @rust-1 @issue-267-fastpath-autocommit
  Scenario: Fastpath FunctionCall releases the backend when ReadyForQuery is idle
    When we create session "autocommit" to pg_doorman as "example_user_1" with password "test" and database "example_db"
    # 957 is pg_catalog.lo_creat(integer) in the standard PostgreSQL catalog.
    # pgjdbc discovers fastpath function OIDs from pg_proc; this raw step pins the wire path.
    And we send FunctionCall 957 with int args "393216" to session "autocommit"
    Then we read PostgreSQL response from session "autocommit" within 1000ms
    And session "autocommit" should receive FunctionCallResponse with 4 byte result
    And session "autocommit" should receive ReadyForQuery "I"
    When we create admin session "admin_idle" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW POOLS" on admin session "admin_idle" and store response
    Then admin session "admin_idle" column "cl_active" for row with "user" = "example_user_1" should be between 0 and 0
    And admin session "admin_idle" column "sv_active" for row with "user" = "example_user_1" should be between 0 and 0
    When we send SimpleQuery "SELECT 1" to session "autocommit" and store response
    Then session "autocommit" should receive DataRow with "1"
    And session "autocommit" should receive ReadyForQuery "I"

  @java @issue-267-java
  Scenario: pgjdbc LargeObject API completes through pg_doorman
    When I run shell command:
      """
      export DATABASE_URL="jdbc:postgresql://127.0.0.1:${DOORMAN_PORT}/example_db?user=example_user_1&password=test"
      timeout 300s tests/java/run_test.sh pgjdbc_largeobject_fastpath_issue_267 pgjdbc_largeobject_fastpath_issue_267.java
      """
    Then the command should succeed
    And the command output should contain "issue_267_pgjdbc_lob complete"
