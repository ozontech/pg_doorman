@rust @rust-1 @talos-personal-pool-routing
Feature: Talos clientId pool routing

  Background:
    Given PostgreSQL started with pg_hba.conf:
      """
      local all all trust
      host all all 127.0.0.1/32 trust
      """
    And fixtures from "tests/fixture.sql" applied
    And fixtures from "tests/talos_fixture.sql" applied
    And keypair 'talos1' generated for talos with kid 'kid-test'

  @personal-pool
  Scenario: clientId pool is preferred to role pool
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all talos 127.0.0.1/32 md5\nhost all all 127.0.0.1/32 trust"

      [talos]
      keys = ["${TALOS1_PUBKEY_PATH}"]
      databases = ["example_db"]

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5

      [[pools.example_db.users]]
      username = "billing-api"
      password = ""
      pool_size = 5
      """
    When we open Talos session 'c1' as client_id 'billing-api' role 'owner' database 'example_db' signed with 'talos1'
    Then pg_doorman log contains "username=billing-api route=personal_pool"

  @service-pool
  Scenario: srv-clientId pool is used when clientId pool is absent
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all talos 127.0.0.1/32 md5\nhost all all 127.0.0.1/32 trust"

      [talos]
      keys = ["${TALOS1_PUBKEY_PATH}"]
      databases = ["example_db"]

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "example_user_1"
      password = ""
      pool_size = 5

      [[pools.example_db.users]]
      username = "srv-billing-api"
      password = ""
      pool_size = 5
      """
    When we open Talos session 'c1' as client_id 'billing-api' role 'owner' database 'example_db' signed with 'talos1'
    Then pg_doorman log contains "username=srv-billing-api route=service_pool"

  @fallback-max-role-owner
  Scenario: owner role is used when no clientId pool exists
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all talos 127.0.0.1/32 md5\nhost all all 127.0.0.1/32 trust"

      [talos]
      keys = ["${TALOS1_PUBKEY_PATH}"]
      databases = ["example_db"]

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "owner"
      password = ""
      pool_size = 5
      """
    When we open Talos session 'c1' as client_id 'billing-api' role 'owner' database 'example_db' signed with 'talos1'
    Then pg_doorman log contains "username=owner route=max_role"

  @fallback-max-role-read-write
  Scenario: read_write role maps to read_write pool user
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all talos 127.0.0.1/32 md5\nhost all all 127.0.0.1/32 trust"

      [talos]
      keys = ["${TALOS1_PUBKEY_PATH}"]
      databases = ["example_db"]

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "read_write"
      password = ""
      pool_size = 5
      """
    When we open Talos session 'c1' as client_id 'analytics' role 'read_write' database 'example_db' signed with 'talos1'
    Then pg_doorman log contains "username=read_write route=max_role"

  @application-name-stays-client-id
  Scenario: SHOW SERVERS keeps the Talos clientId
    Given pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all talos 127.0.0.1/32 md5\nhost all all 127.0.0.1/32 trust"

      [talos]
      keys = ["${TALOS1_PUBKEY_PATH}"]
      databases = ["example_db"]

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "billing-api"
      password = ""
      pool_size = 5
      """
    When we open Talos session 'c1' as client_id 'billing-api' role 'owner' database 'example_db' signed with 'talos1'
    And we create admin session "admin1" to pg_doorman as "admin" with password "admin"
    And we execute "SHOW SERVERS" on admin session "admin1" and store response
    Then admin session "admin1" response should contain "billing-api"

  @mismatch-personal-but-read-only-token
  Scenario: clientId pool is used for a read_only token
    Given pg_doorman log capture enabled
    And pg_doorman started with config:
      """
      [general]
      host = "127.0.0.1"
      port = ${DOORMAN_PORT}
      admin_username = "admin"
      admin_password = "admin"
      pg_hba.content = "host all talos 127.0.0.1/32 md5\nhost all all 127.0.0.1/32 trust"

      [talos]
      keys = ["${TALOS1_PUBKEY_PATH}"]
      databases = ["example_db"]

      [pools.example_db]
      server_host = "127.0.0.1"
      server_port = ${PG_PORT}

      [[pools.example_db.users]]
      username = "billing-api"
      password = ""
      pool_size = 5

      [[pools.example_db.users]]
      username = "read_only"
      password = ""
      pool_size = 5
      """
    When we open Talos session 'c1' as client_id 'billing-api' role 'read_only' database 'example_db' signed with 'talos1'
    Then pg_doorman log contains "username=billing-api route=personal_pool"
