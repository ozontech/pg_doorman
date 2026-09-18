@ldap
Feature: Isolated real LDAP test infrastructure
  The directory fixture is tested directly with OpenLDAP CLI clients.
  These scenarios do not enable LDAP authentication in pg_doorman.

  Scenario Outline: Inline directory supports bind and search on each transport
    Given LDAP server "primary" is started with LDIF:
      """
      dn: dc=example,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: example
      o: Transport directory

      dn: uid=alice,dc=example,dc=test
      objectClass: inetOrgPerson
      uid: alice
      cn: Alice
      sn: Example
      userPassword: alice-password
      """
    Then LDAP server "primary" accepts bind over "<transport>" as "uid=alice,dc=example,dc=test" with password "alice-password"
    And LDAP server "primary" search over "<transport>" at "dc=example,dc=test" for "(uid=alice)" returns DN "uid=alice,dc=example,dc=test"
    And LDAP server "primary" rejects bind over "<transport>" as "uid=alice,dc=example,dc=test" with password "wrong-password" with result 49
    And LDAP server "primary" rejects bind over "<transport>" as "uid=absent,dc=example,dc=test" with password "wrong-password" with result 49
    When LDAP server "primary" is stopped

    Examples:
      | transport |
      | LDAP      |
      | LDAPS     |
      | StartTLS  |

  Scenario Outline: Certificate trust is enforced by the LDAP client
    Given LDAP server "primary" is started with base DN "dc=company,dc=test" and LDIF:
      """
      dn: dc=company,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: company
      o: Company directory
      """
    Then LDAP server "primary" rejects an unrelated CA over "<transport>"

    Examples:
      | transport |
      | LDAPS     |
      | StartTLS  |

  Scenario: Named directories and restarts have independent state
    Given LDAP server "primary" is started with LDIF:
      """
      dn: dc=example,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: example
      o: Primary directory

      dn: uid=alice,dc=example,dc=test
      objectClass: inetOrgPerson
      uid: alice
      cn: Alice
      sn: Example
      userPassword: alice-password
      """
    And LDAP server "secondary" is started with LDIF:
      """
      dn: dc=example,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: example
      o: Secondary directory

      dn: uid=alice,dc=example,dc=test
      objectClass: inetOrgPerson
      uid: alice
      cn: Alice
      sn: Example
      userPassword: alice-password
      """
    When LDAP server "primary" password for "uid=alice,dc=example,dc=test" becomes "changed-password"
    Then LDAP server "primary" accepts bind over "LDAPS" as "uid=alice,dc=example,dc=test" with password "changed-password"
    And LDAP server "primary" rejects bind over "LDAP" as "uid=alice,dc=example,dc=test" with password "alice-password" with result 49
    And LDAP server "secondary" accepts bind over "LDAP" as "uid=alice,dc=example,dc=test" with password "alice-password"
    When LDAP server "primary" is restarted with fresh data
    Then LDAP server "primary" accepts bind over "LDAPS" as "uid=alice,dc=example,dc=test" with password "alice-password"
    And LDAP server "primary" rejects bind over "LDAP" as "uid=alice,dc=example,dc=test" with password "changed-password" with result 49
    When LDAP server "primary" is stopped
    Then LDAP server "secondary" accepts bind over "StartTLS" as "uid=alice,dc=example,dc=test" with password "alice-password"

  Scenario: A partially started server retries a busy listener port
    Given LDAP server "primary" is started after a port collision with LDIF:
      """
      dn: dc=example,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: example
      o: Port collision directory

      dn: uid=alice,dc=example,dc=test
      objectClass: inetOrgPerson
      uid: alice
      cn: Alice
      sn: Example
      userPassword: alice-password
      """
    Then LDAP server "primary" accepts bind over "LDAP" as "uid=alice,dc=example,dc=test" with password "alice-password"
    And LDAP server "primary" accepts bind over "LDAPS" as "uid=alice,dc=example,dc=test" with password "alice-password"
    When LDAP server "primary" is stopped

  Scenario: Inline LDIF defines the complete directory and its attributes
    Given LDAP server "primary" is started with LDIF:
      """
      dn: dc=example,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: example
      o: Inline directory

      dn: uid=casey,dc=example,dc=test
      objectClass: inetOrgPerson
      uid: casey
      cn: Casey
      sn: Inline
      mail: casey@example.test
      userPassword: casey-inline-password
      """
    Then LDAP server "primary" accepts bind over "LDAP" as "uid=casey,${LDAP_PRIMARY_BASE_DN}" with password "casey-inline-password"
    And LDAP server "primary" search over "LDAP" at "${LDAP_PRIMARY_BASE_DN}" for "(objectClass=inetOrgPerson)" returns DN "uid=casey,${LDAP_PRIMARY_BASE_DN}"
    And LDAP server "primary" search over "LDAP" at "${LDAP_PRIMARY_BASE_DN}" for "(&(uid=casey)(mail=casey@example.test))" returns DN "uid=casey,${LDAP_PRIMARY_BASE_DN}"

  Scenario Outline: Custom inline directories retain their initial data across restarts
    Given LDAP server "primary" is started with base DN "dc=team,dc=internal" and LDIF:
      """
      dn: dc=team,dc=internal
      objectClass: dcObject
      objectClass: organization
      dc: team
      o: Team directory

      dn: uid=ren,dc=team,dc=internal
      objectClass: inetOrgPerson
      uid: ren
      cn: Ren
      sn: Inline
      mail: ren@team.internal
      description: custom inline marker
      userPassword: ren-original-password
      """
    Then LDAP server "primary" accepts bind over "<transport>" as "${LDAP_PRIMARY_ADMIN_DN}" with password "fixture-admin-password"
    And LDAP server "primary" accepts bind over "<transport>" as "uid=ren,${LDAP_PRIMARY_BASE_DN}" with password "ren-original-password"
    And LDAP server "primary" search over "<transport>" at "${LDAP_PRIMARY_BASE_DN}" for "(&(uid=ren)(mail=ren@team.internal)(description=custom inline marker))" returns DN "uid=ren,dc=team,dc=internal"
    When LDAP server "primary" password for "uid=ren,${LDAP_PRIMARY_BASE_DN}" becomes "ren-updated-password"
    Then LDAP server "primary" accepts bind over "<transport>" as "uid=ren,${LDAP_PRIMARY_BASE_DN}" with password "ren-updated-password"
    And LDAP server "primary" rejects bind over "<transport>" as "uid=ren,${LDAP_PRIMARY_BASE_DN}" with password "ren-original-password" with result 49
    When LDAP server "primary" is restarted with fresh data
    Then LDAP server "primary" accepts bind over "<transport>" as "uid=ren,dc=team,dc=internal" with password "ren-original-password"
    And LDAP server "primary" rejects bind over "<transport>" as "uid=ren,${LDAP_PRIMARY_BASE_DN}" with password "ren-updated-password" with result 49
    And LDAP server "primary" search over "<transport>" at "${LDAP_PRIMARY_BASE_DN}" for "(&(uid=ren)(mail=ren@team.internal)(description=custom inline marker))" returns DN "uid=ren,dc=team,dc=internal"

    Examples:
      | transport |
      | LDAP      |
      | LDAPS     |
      | StartTLS  |

  Scenario: Restart preserves expanded inline placeholders after their source is stopped
    Given LDAP server "source" is started with base DN "dc=company,dc=test" and LDIF:
      """
      dn: dc=company,dc=test
      objectClass: dcObject
      objectClass: organization
      dc: company
      o: Source directory
      """
    And LDAP server "primary" is started with base DN "${LDAP_SOURCE_BASE_DN}" and LDIF:
      """
      dn: ${LDAP_SOURCE_BASE_DN}
      objectClass: dcObject
      objectClass: organization
      dc: company
      o: Expanded directory

      dn: uid=sam,${LDAP_SOURCE_BASE_DN}
      objectClass: inetOrgPerson
      uid: sam
      cn: Sam
      sn: Inline
      description: from ${LDAP_SOURCE_BASE_DN}
      userPassword: dc=company,dc=test
      """
    Then LDAP server "primary" accepts bind over "LDAP" as "uid=sam,${LDAP_PRIMARY_BASE_DN}" with password "dc=company,dc=test"
    When LDAP server "source" is stopped
    And LDAP server "primary" password for "uid=sam,${LDAP_PRIMARY_BASE_DN}" becomes "temporary-password"
    Then LDAP server "primary" accepts bind over "StartTLS" as "uid=sam,${LDAP_PRIMARY_BASE_DN}" with password "temporary-password"
    When LDAP server "primary" is restarted with fresh data
    Then LDAP server "primary" accepts bind over "LDAPS" as "uid=sam,dc=company,dc=test" with password "dc=company,dc=test"
    And LDAP server "primary" search over "LDAP" at "${LDAP_PRIMARY_BASE_DN}" for "(&(uid=sam)(description=from dc=company,dc=test))" returns DN "uid=sam,dc=company,dc=test"
    And LDAP server "primary" rejects bind over "StartTLS" as "uid=sam,${LDAP_PRIMARY_BASE_DN}" with password "temporary-password" with result 49

  Scenario: Quoted and escaped base DNs survive configuration and restart
    Given LDAP server "primary" is started with base DN 'o=Quote\"Slash\\Lab,dc=test' and LDIF:
      """
      dn: o=Quote\"Slash\\Lab,dc=test
      objectClass: organization
      o: Quote"Slash\Lab

      dn: uid=lee,o=Quote\"Slash\\Lab,dc=test
      objectClass: inetOrgPerson
      uid: lee
      cn: Lee
      sn: Inline
      userPassword: lee-inline-password
      """
    Then LDAP server "primary" accepts bind over "LDAPS" as "uid=lee,${LDAP_PRIMARY_BASE_DN}" with password "lee-inline-password"
    And LDAP server "primary" search over "LDAP" at "${LDAP_PRIMARY_BASE_DN}" for "(uid=lee)" returns DN "uid=lee,${LDAP_PRIMARY_BASE_DN}"
    When LDAP server "primary" password for "uid=lee,${LDAP_PRIMARY_BASE_DN}" becomes "lee-updated-password"
    Then LDAP server "primary" accepts bind over "StartTLS" as "uid=lee,${LDAP_PRIMARY_BASE_DN}" with password "lee-updated-password"
    When LDAP server "primary" is restarted with fresh data
    Then LDAP server "primary" accepts bind over "LDAPS" as "uid=lee,${LDAP_PRIMARY_BASE_DN}" with password "lee-inline-password"
    And LDAP server "primary" rejects an unrelated CA over "StartTLS"
