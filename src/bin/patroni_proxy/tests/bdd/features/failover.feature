@patroni_proxy
Feature: Patroni proxy failover handling

  Scenario: Connect to master when only one of three Patroni nodes is available
    Given mock backend server 'pg_master' for ping-pong protocol
    And mock Patroni server 'node1' with response:
      """
      {
        "scope": "test_cluster",
        "members": [
          {
            "name": "node1",
            "host": "127.0.0.1",
            "port": 5432,
            "role": "leader",
            "state": "running",
            "api_url": "http://127.0.0.1:8008/patroni",
            "timeline": 1
          }
        ]
      }
      """
    And API listen address is allocated
    And proxy port 'master' is allocated
    And patroni_proxy started with config:
      """
      cluster_update_interval: 1
      listen_address: "${API_ADDR}"

      clusters:
        test_cluster:
          hosts:
            - "http://127.0.0.1:19991"
            - "http://127.0.0.1:${PATRONI_NODE1_PORT}"
            - "http://127.0.0.1:19992"
          
          ports:
            master:
              listen: "${PROXY_MASTER_ADDR}"
              roles: ["leader"]
              host_port: ${BACKEND_PG_MASTER_PORT}
      """
    When wait for 2 seconds
    Then TCP connection to proxy port 'master' succeeds
    Given I open session to 'master' named 'master'
    Then I execute ping on session 'master' and receive pong
