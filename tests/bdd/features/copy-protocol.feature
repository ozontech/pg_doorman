@rust @copy-protocol
Feature: COPY Protocol
  Testing COPY protocol support to ensure pg_doorman handles COPY operations identically to PostgreSQL
  Including error cases like bad SQL syntax

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

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [pools.example_db.users.0]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  @copy-protocol-first
  Scenario: COPY TO STDOUT with generate_series gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT * FROM generate_series(1, 10)) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-second
  Scenario: COPY TO STDOUT with CSV format gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT i, i*2, 'row_' || i FROM generate_series(1, 5) AS i) TO STDOUT WITH (FORMAT CSV)" to both
    Then we should receive identical messages from both

  @copy-protocol-third
  Scenario: COPY TO STDOUT with CSV header gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT i AS id, i*2 AS doubled, 'row_' || i AS name FROM generate_series(1, 5) AS i) TO STDOUT WITH (FORMAT CSV, HEADER)" to both
    Then we should receive identical messages from both

  @copy-protocol-fourth
  Scenario: COPY TO STDOUT with binary format gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT * FROM generate_series(1, 5)) TO STDOUT WITH (FORMAT BINARY)" to both
    Then we should receive identical messages from both

  @copy-protocol-fifth
  Scenario: COPY TO STDOUT with delimiter gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT i, i*2 FROM generate_series(1, 5) AS i) TO STDOUT WITH (DELIMITER '|')" to both
    Then we should receive identical messages from both

  @copy-protocol-sixth
  Scenario: COPY TO STDOUT empty result gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT * FROM generate_series(1, 0)) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-seventh
  Scenario: COPY TO STDOUT large dataset gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT * FROM generate_series(1, 1000)) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-error-first
  Scenario: COPY with bad SQL syntax gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELEC * FROM generate_series(1, 10)) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-error-second
  Scenario: COPY from non-existent table gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY non_existent_table_xyz TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-error-third
  Scenario: COPY with invalid format gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT 1) TO STDOUT WITH (FORMAT INVALID_FORMAT)" to both
    Then we should receive identical messages from both

  @copy-protocol-error-fourth
  Scenario: COPY with syntax error in subquery gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT * FROM) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-error-fifth
  Scenario: COPY TO with missing destination gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT 1) TO" to both
    Then we should receive identical messages from both

  @copy-protocol-error-sixth
  Scenario: COPY with unclosed parenthesis gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT * FROM generate_series(1, 10) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-from-first
  Scenario: COPY FROM STDIN basic operation gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TEMP TABLE copy_test_pg (id int, name text)" to both
    And we send CopyFromStdin "COPY copy_test_pg FROM STDIN" with data "1\ttest1\n2\ttest2\n3\ttest3\n" to both
    And we send SimpleQuery "SELECT * FROM copy_test_pg ORDER BY id" to both
    Then we should receive identical messages from both

  @copy-protocol-from-second
  Scenario: COPY FROM STDIN with CSV format gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TEMP TABLE copy_test_csv (id int, name text)" to both
    And we send CopyFromStdin "COPY copy_test_csv FROM STDIN WITH (FORMAT CSV)" with data "1,test1\n2,test2\n" to both
    And we send SimpleQuery "SELECT * FROM copy_test_csv ORDER BY id" to both
    Then we should receive identical messages from both

  @copy-protocol-from-third
  Scenario: COPY FROM STDIN with empty data gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TEMP TABLE copy_test_empty (id int, name text)" to both
    And we send CopyFromStdin "COPY copy_test_empty FROM STDIN" with data "" to both
    And we send SimpleQuery "SELECT COUNT(*) FROM copy_test_empty" to both
    Then we should receive identical messages from both

  @copy-protocol-from-error-first
  Scenario: COPY FROM STDIN to non-existent table gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send CopyFromStdin "COPY non_existent_table FROM STDIN" with data "1\ttest\n" to both
    Then we should receive identical messages from both

  @copy-protocol-from-error-second
  Scenario: COPY FROM STDIN with bad SQL gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send CopyFromStdin "COPY FROM STDIN" with data "1\ttest\n" to both
    Then we should receive identical messages from both

  @copy-protocol-from-error-third
  Scenario: COPY FROM STDIN with wrong column count gives identical error
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "CREATE TEMP TABLE copy_test_cols (id int, name text, value int)" to both
    And we send CopyFromStdin "COPY copy_test_cols FROM STDIN" with data "1\ttest\n" to both
    Then we should receive identical messages from both

  @copy-protocol-mixed-first
  Scenario: Multiple COPY operations in sequence gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "COPY (SELECT 1) TO STDOUT" to both
    And we send SimpleQuery "COPY (SELECT 2) TO STDOUT" to both
    And we send SimpleQuery "COPY (SELECT 3) TO STDOUT" to both
    Then we should receive identical messages from both

  @copy-protocol-mixed-second
  Scenario: COPY interleaved with regular queries gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 'before_copy'" to both
    And we send SimpleQuery "COPY (SELECT * FROM generate_series(1, 3)) TO STDOUT" to both
    And we send SimpleQuery "SELECT 'after_copy'" to both
    Then we should receive identical messages from both

  @copy-protocol-mixed-third
  Scenario: COPY in transaction gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to both
    And we send SimpleQuery "CREATE TEMP TABLE copy_tx_test (id int)" to both
    And we send CopyFromStdin "COPY copy_tx_test FROM STDIN" with data "1\n2\n3\n" to both
    And we send SimpleQuery "COPY (SELECT * FROM copy_tx_test ORDER BY id) TO STDOUT" to both
    And we send SimpleQuery "COMMIT" to both
    Then we should receive identical messages from both

  @copy-protocol-mixed-fourth
  Scenario: COPY in rolled back transaction gives identical results
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "BEGIN" to both
    And we send SimpleQuery "CREATE TEMP TABLE copy_rollback_test (id int)" to both
    And we send CopyFromStdin "COPY copy_rollback_test FROM STDIN" with data "1\n2\n3\n" to both
    And we send SimpleQuery "ROLLBACK" to both
    And we send SimpleQuery "SELECT EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'copy_rollback_test')" to both
    Then we should receive identical messages from both
