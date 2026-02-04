@pbdes-rollback @rollback
Feature: PBDES extended protocol rollback test
  Test pg_doorman PBDES protocol for BEGIN -> error -> ROLLBACK sequence
  and verify identical responses between PostgreSQL and pg_doorman

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 trust
      """
    And self-signed SSL certificates are generated
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      admin_username = "admin"
      admin_password = "admin"
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 10
      """

  Scenario: Test PBDES extended protocol BEGIN -> error -> ROLLBACK
    # Test that pg_doorman returns identical messages to PostgreSQL
    # for BEGIN -> SQL error (select * from unknown) -> ROLLBACK sequence
    When we login to postgres and pg_doorman as "example_user_1" with password "" and database "example_db"
    # Step 1: BEGIN via PBDES
    And we send Parse "" with query "BEGIN" to both
    And we send Bind "" to "" with params "" to both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Sync to both
    # Step 2: Error query via PBDES - relation does not exist
    And we send Parse "" with query "SELECT * FROM unknown" to both
    And we send Bind "" to "" with params "" to both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Sync to both
    # Step 3: ROLLBACK via PBDES
    And we send Parse "" with query "ROLLBACK" to both
    And we send Bind "" to "" with params "" to both
    And we send Describe "P" "" to both
    And we send Execute "" to both
    And we send Sync to both
    Then we should receive identical messages from both
