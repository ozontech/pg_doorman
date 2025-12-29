@go @hba
Feature: Go HBA authentication tests
  Test pg_doorman HBA trust authentication and deny rules

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             all             127.0.0.1/32            trust
      host    all             all             ::1/128                 trust
      hostssl all             all             127.0.0.1/32            trust
      """
    And fixtures from "tests/fixture.sql" applied
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      hostnossl all example_user_nopassword 127.0.0.1/32 reject
      hostssl all example_user_nopassword 127.0.0.1/32 trust
      host all example_user_disconnect 127.0.0.1/32 trust
      host all example_user_prometheus 127.0.0.1/32 trust
      host all all 127.0.0.1/32 md5
      host all all 10.0.0.0/8 md5
      host all all 192.168.0.0/16 md5
      host all all 172.0.0.0/8 md5
      """
    And pg_doorman started with config:
      """
      [prometheus]
      enabled = true
      host = "0.0.0.0"
      port = 9127

      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      pg_hba = {path = "${DOORMAN_HBA_FILE}"}
      tls_private_key = "${DOORMAN_SSL_KEY}"
      tls_certificate = "${DOORMAN_SSL_CERT}"
      virtual_pool_count = 1
      worker_threads = 2
      prepared_statements = true
      prepared_statements_cache_size = 10000
      tcp_keepalives_idle = 1
      tcp_keepalives_count = 5
      tcp_keepalives_interval = 5
      default_tcp_so_linger = 0
      max_message_size = 1048576
      admin_username = "admin"
      admin_password = "admin"

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}
      pool_mode = "transaction"
      log_client_parameter_status_changes = true
      idle_timeout = 40000
      application_name = "doorman_example_user_1"

      [pools.example_db.users.0]
      username = "example_user_nopassword"
      password = ""
      pool_size = 40
      min_pool_size = 0
      pool_mode = "session"
      """

  Scenario: Test HBA trust authentication
    When I run shell command:
      """
      export DATABASE_URL_TRUST="postgresql://example_user_nopassword@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=require"
      cd tests/go && go test -v -run Test_HbaTrust
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_HbaTrust"

  Scenario: Test HBA deny rules
    When I run shell command:
      """
      export DATABASE_URL_NOTRUST="postgresql://example_user_nopassword@127.0.0.1:${DOORMAN_PORT}/example_db?sslmode=disable"
      cd tests/go && go test -v -run Test_HbaDeny
      """
    Then the command should succeed
    And the command output should contain "PASS: Test_HbaDeny"
