@rust @rust-3 @cleanup-review @cleanup-config-review
Feature: Cleanup policy configuration errors
  Invalid cleanup policies identify the setting and its accepted values.

  Scenario Outline: Invalid <scope> cleanup policy <value> in <format>
    When I run shell command:
      """
      python3 - <<'PY'
      import subprocess
      import sys
      import tempfile

      value = '<value>'
      scope = '<scope>'
      if '<format>' == 'toml':
          content = '[general]\nadmin_username = "admin"\nadmin_password = "admin"\n'
          if scope == 'pool':
              content += '[pools.example_db]\n'
          content += f'cleanup_server_connections = {value}\n'
      else:
          content = 'general:\n  admin_username: admin\n  admin_password: admin\n'
          if scope == 'pool':
              content += 'pools:\n  example_db:\n'
          indent = '    ' if scope == 'pool' else '  '
          content += f'{indent}cleanup_server_connections: {value}\n'

      with tempfile.NamedTemporaryFile(mode='w', suffix='.<format>') as config:
          config.write(content)
          config.flush()
          result = subprocess.run(
              ['${DOORMAN_BINARY}', '-t', config.name],
              capture_output=True, text=True, timeout=10,
          )
          print(result.stdout + result.stderr)
          sys.exit(result.returncode)
      PY
      """
    Then the command should fail
    And the command output should contain "cleanup_server_connections"
    And the command output should contain "expected a boolean or one of: off, adaptive, always"

    Examples:
      | format | scope   | value  |
      | toml   | general | 1      |
      | toml   | general | [true] |
      | toml   | general | "auto" |
      | toml   | pool    | 1      |
      | toml   | pool    | [true] |
      | toml   | pool    | "auto" |
      | yaml   | general | 1      |
      | yaml   | general | [true] |
      | yaml   | general | "auto" |
      | yaml   | pool    | 1      |
      | yaml   | pool    | [true] |
      | yaml   | pool    | "auto" |

  @cleanup-review @cleanup-config-review-pairs
  Scenario Outline: Cleanup policy <case> is rejected in <format>
    When I run shell command:
      """
      python3 - <<'PY'
      import subprocess
      import sys
      import tempfile

      fmt = '<format>'

      def block(scope):
          mode = '<general_mode>' if scope == 'general' else '<pool_mode>'
          query = '<general_query>' if scope == 'general' else '<pool_query>'
          indent = '  ' if scope == 'general' else '    '
          settings = ''
          if mode:
              settings += f'cleanup_server_connections = {mode}\n' if fmt == 'toml' \
                  else f'{indent}cleanup_server_connections: {mode}\n'
          if query:
              settings += f'cleanup_server_query = "{query}"\n' if fmt == 'toml' \
                  else f'{indent}cleanup_server_query: "{query}"\n'
          return settings or f'{indent}# inherit defaults\n'

      if fmt == 'toml':
          content = '[general]\nadmin_username = "admin"\nadmin_password = "admin"\n'
          content += block('general')
          content += '[pools.example_db]\n'
          content += block('pool')
      else:
          content = 'general:\n  admin_username: admin\n  admin_password: admin\n'
          content += block('general')
          content += 'pools:\n  example_db:\n    pool_mode: session\n'
          content += block('pool')

      with tempfile.NamedTemporaryFile(mode='w', suffix='.' + fmt) as config:
          config.write(content)
          config.flush()
          result = subprocess.run(
              ['${DOORMAN_BINARY}', '-t', config.name],
              capture_output=True, text=True, timeout=10,
          )
          print(result.stdout + result.stderr)
          sys.exit(result.returncode)
      PY
      """
    Then the command should fail
    And the command output should contain "pools.example_db"
    And the command output should contain "<hint>"

    Examples:
      | format | case                                             | general_mode | general_query | pool_mode  | pool_query  | hint                                      |
      | toml   | adaptive general with a query                    | "adaptive"   | DISCARD ALL   |            |             | requires cleanup_server_connections = always |
      | toml   | legacy true general with a query                 | true         | DISCARD ALL   |            |             | requires cleanup_server_connections = always |
      | toml   | adaptive pool with its own query                 |              |               | "adaptive" | DISCARD ALL | requires cleanup_server_connections = always |
      | toml   | legacy true pool with its own query              |              |               | true       | DISCARD ALL | requires cleanup_server_connections = always |
      | toml   | pool narrowing general always to adaptive        | "always"     | DISCARD ALL   | "adaptive" |             | requires cleanup_server_connections = always |
      | toml   | always general without a query                   | "always"     |               |            |             | requires cleanup_server_query               |
      | toml   | always pool without a query                      |              |               | "always"   |             | requires cleanup_server_query               |
      | yaml   | adaptive general with a query                    | adaptive     | DISCARD ALL   |            |             | requires cleanup_server_connections = always |
      | yaml   | adaptive pool with its own query                 |              |               | adaptive   | DISCARD ALL | requires cleanup_server_connections = always |
      | yaml   | pool narrowing general always to adaptive        | always       | DISCARD ALL   | adaptive   |             | requires cleanup_server_connections = always |
      | yaml   | always general without a query                   | always       |               |            |             | requires cleanup_server_query               |
      | yaml   | always pool without a query                      |              |               | always     |             | requires cleanup_server_query               |

  @cleanup-review @cleanup-config-review-pairs
  Scenario Outline: Cleanup policy <case> in <format>
    When I run shell command:
      """
      python3 - <<'PY'
      import subprocess
      import sys
      import tempfile

      fmt = '<format>'
      mode = '<mode>'
      query = '<query>'
      override = '<pool_override>'
      if fmt == 'toml':
          content = '[general]\nadmin_username = "admin"\nadmin_password = "admin"\n'
          content += f'cleanup_server_connections = {mode}\n'
          if query:
              content += f'cleanup_server_query = "{query}"\n'
          content += '[pools.example_db]\n'
          if override:
              content += f'cleanup_server_connections = {override}\n'
      else:
          content = 'general:\n  admin_username: admin\n  admin_password: admin\n'
          content += f'  cleanup_server_connections: {mode}\n'
          if query:
              content += f'  cleanup_server_query: "{query}"\n'
          content += 'pools:\n  example_db:\n    pool_mode: session\n'
          if override:
              content += f'    cleanup_server_connections: {override}\n'

      with tempfile.NamedTemporaryFile(mode='w', suffix='.' + fmt) as config:
          config.write(content)
          config.flush()
          result = subprocess.run(
              ['${DOORMAN_BINARY}', '-t', config.name],
              capture_output=True, text=True, timeout=10,
          )
          print(result.stdout + result.stderr)
          sys.exit(result.returncode)
      PY
      """
    Then the command should succeed
    And the command output should contain "test is successful"

    Examples:
      | format | case                                          | mode       | query       | pool_override |
      | toml   | adaptive without a query                      | "adaptive" |             |               |
      | toml   | always with a query                           | "always"   | DISCARD ALL |               |
      | toml   | off with a query                              | "off"      | DISCARD ALL |               |
      | toml   | legacy false with a query                     | false      | DISCARD ALL |               |
      | toml   | always pool narrowed to off keeps the query   | "always"   | DISCARD ALL | "off"         |
      | yaml   | adaptive without a query                      | adaptive   |             |               |
      | yaml   | always with a query                           | always     | DISCARD ALL |               |
      | yaml   | off with a query                              | off        | DISCARD ALL |               |
      | yaml   | legacy false with a query                     | false      | DISCARD ALL |               |
      | yaml   | always pool narrowed to off keeps the query   | always     | DISCARD ALL | off           |
