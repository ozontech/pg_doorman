@static-passthrough
Feature: Static user passthrough — backend auth without server_password

  Tests for static user passthrough mode: when a static user has a password
  hash (MD5 or SCRAM verifier) but no server_password, pg_doorman uses the
  hash/ClientKey to authenticate to the backend on behalf of the user.

  The key verification: after retain closes the backend connection, a second
  connection must also succeed — proving the stored hash is properly reused.

  Scenario: MD5 static passthrough — first and reconnect after retain
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/static_passthrough_fixture.sql" applied
    And password hash for PG user "pt_static_md5" is stored as "MD5_HASH"
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        server_lifetime: 2000
        retain_connections_time: 1000
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "pt_static_md5"
              password: "${MD5_HASH}"
              pool_size: 5
      """
    # First connection — backend connection is created, MD5 hash used for auth
    Then psql query "SELECT current_user" as user "pt_static_md5" to database "postgres" with password "md5pass" returns "pt_static_md5"
    # Wait for server_lifetime + retain to close backend connection
    When we sleep for 4000 milliseconds
    # Second connection — new backend connection, MD5 hash reused from config
    Then psql query "SELECT current_user" as user "pt_static_md5" to database "postgres" with password "md5pass" returns "pt_static_md5"

  Scenario: SCRAM static passthrough — first and reconnect after retain
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            scram-sha-256
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/static_passthrough_fixture.sql" applied
    And password hash for PG user "pt_static_scram" is stored as "SCRAM_HASH"
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 scram-sha-256
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        server_lifetime: 2000
        retain_connections_time: 1000
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "pt_static_scram"
              password: "${SCRAM_HASH}"
              pool_size: 5
      """
    # First connection — ClientKey extracted from SCRAM proof, used for backend auth
    Then psql query "SELECT current_user" as user "pt_static_scram" to database "postgres" with password "scrampass" returns "pt_static_scram"
    # Wait for server_lifetime + retain to close backend connection
    When we sleep for 4000 milliseconds
    # Second connection — new backend connection, ClientKey reused from cache
    Then psql query "SELECT current_user" as user "pt_static_scram" to database "postgres" with password "scrampass" returns "pt_static_scram"

  Scenario: Static user with explicit server_password still works (regression)
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/static_passthrough_fixture.sql" applied
    And password hash for PG user "pt_static_md5" is stored as "MD5_HASH"
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "pt_static_md5"
              password: "${MD5_HASH}"
              server_username: "pt_static_md5"
              server_password: "md5pass"
              pool_size: 5
      """
    Then psql query "SELECT current_user" as user "pt_static_md5" to database "postgres" with password "md5pass" returns "pt_static_md5"

  Scenario: Wrong password fails for static passthrough user
    Given PostgreSQL started with pg_hba.conf:
      """
      local   all             all                                     trust
      host    all             postgres        127.0.0.1/32            trust
      host    all             all             127.0.0.1/32            md5
      host    all             all             ::1/128                 trust
      """
    And fixtures from "tests/static_passthrough_fixture.sql" applied
    And password hash for PG user "pt_static_md5" is stored as "MD5_HASH"
    And self-signed SSL certificates are generated
    And pg_doorman hba file contains:
      """
      host all all 127.0.0.1/32 md5
      """
    Given pg_doorman started with config:
      """
      general:
        host: "127.0.0.1"
        port: ${DOORMAN_PORT}
        connect_timeout: 5000
        admin_username: "admin"
        admin_password: "admin"
        tls_private_key: "${DOORMAN_SSL_KEY}"
        tls_certificate: "${DOORMAN_SSL_CERT}"
        pg_hba:
          path: "${DOORMAN_HBA_FILE}"
      pools:
        postgres:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: "transaction"
          users:
            - username: "pt_static_md5"
              password: "${MD5_HASH}"
              pool_size: 5
      """
    Then psql connection to pg_doorman as user "pt_static_md5" to database "postgres" with password "wrongpass" fails
