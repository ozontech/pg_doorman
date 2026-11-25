@rust @rust-3 @cleanup-review @cleanup-listen
Feature: Adaptive cleanup removes LISTEN subscriptions left by a client
  A backend released to the pool keeps its LISTEN subscriptions. While the
  backend is pooled, nobody reads its socket, so NotificationResponse
  messages queue up and get delivered to whichever client checks the
  backend out next. The built-in cleanup must send UNLISTEN * at checkin
  when a LISTEN was tracked.

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
        admin_username: admin
        admin_password: admin
        connect_timeout: 1000
        pg_hba: {content: "host all all 127.0.0.1/32 trust"}
      web:
        enabled: true
        host: "127.0.0.1"
        port: 9129
      pools:
        example_db:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          pool_mode: transaction
          users: [{username: example_user_1, password: "", pool_size: 1}]
        example_db_session:
          server_host: "127.0.0.1"
          server_port: ${PG_PORT}
          server_database: "example_db"
          pool_mode: session
          users: [{username: example_user_1, password: "", pool_size: 1}]
      """

  @cleanup-listen-subscription
  Scenario: A LISTEN subscription does not survive a checkin
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "LISTEN leak_ch" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    And we send SimpleQuery "SELECT 'leak_ch' IN (SELECT pg_listening_channels())" to session "next" and store response
    Then session "next" should receive DataRow with "f"

  @cleanup-listen-notification
  Scenario: A notification for a released subscription does not reach the next client
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "LISTEN leak_ch" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT 1" to session "next"
    And we create session "notifier" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "NOTIFY leak_ch" to session "notifier"
    And we send SimpleQuery "SELECT 2" to session "next" and store response
    Then session "next" should receive DataRow with "2"
    And session "next" should not receive NotificationResponse for channel "leak_ch"

  @cleanup-listen-partial-unlisten
  Scenario: A partial client UNLISTEN does not suppress the checkin UNLISTEN
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "old" and store backend_pid
    And we send SimpleQuery "LISTEN ch_one" to session "old"
    And we send SimpleQuery "LISTEN ch_two" to session "old"
    # The UNLISTEN tag is shared by UNLISTEN ch and UNLISTEN *, so a partial
    # unsubscribe cannot disarm the flag: ch_two is still subscribed.
    And we send SimpleQuery "UNLISTEN ch_one" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db"
    And we send SimpleQuery "SELECT pg_backend_pid()" to session "next" and store backend_pid
    Then backend_pid from session "next" should equal backend_pid from session "old"
    And we send SimpleQuery "SELECT count(*) FROM pg_listening_channels()" to session "next" and store response
    Then session "next" should receive DataRow with "0"

  @cleanup-listen-discard-all
  Scenario: A client DISCARD ALL suppresses the checkin UNLISTEN entirely
    # Session pooling has a single checkin at disconnect, so LISTEN and the
    # client's DISCARD ALL share one lease. (Transaction pooling checks the
    # backend in after every query: the LISTEN cleanup already ran before
    # the client's DISCARD ALL could matter.)
    # The counter label is the pool name, so this reads the session pool
    # the client actually used.
    When we create session "old" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "LISTEN suppress_ch" to session "old"
    And we send SimpleQuery "DISCARD ALL" to session "old"
    And we close session "old"
    And we create session "next" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "SELECT count(*) FROM pg_listening_channels()" to session "next" and store response
    Then session "next" should receive DataRow with "0"
    When I run shell command:
      """
      python3 - <<'PY'
      import time
      import urllib.request
      deadline = time.monotonic() + 5
      while True:
          body = urllib.request.urlopen('http://127.0.0.1:9129/metrics', timeout=2).read().decode()
          lines = body.splitlines()
          ok = 'pg_doorman_server_cleanup_total{database="example_db_session",result="ok",user="example_user_1"} 0' in lines
          err = 'pg_doorman_server_cleanup_total{database="example_db_session",result="error",user="example_user_1"} 0' in lines
          if ok and err:
              break
          assert time.monotonic() < deadline, body
          time.sleep(0.05)
      PY
      """
    Then the command should succeed

  @cleanup-listen-session-mode
  Scenario: A session mode client keeps receiving notifications
    When we create session "sub" to pg_doorman as "example_user_1" with password "" and database "example_db_session"
    And we send SimpleQuery "LISTEN ping_ch" to session "sub"
    And we create session "notifier" to postgres as "postgres" with password "" and database "example_db"
    And we send SimpleQuery "NOTIFY ping_ch, 'hello'" to session "notifier"
    # The session mode client holds the backend; its next query result is
    # preceded by the queued NotificationResponse.
    And we send SimpleQuery "SELECT 1" to session "sub" and store response
    Then session "sub" should receive DataRow with "1"
    And session "sub" should receive NotificationResponse for channel "ping_ch"
