use super::{
    eval_hba_for_pool_password, JWT_PUB_KEY_PASSWORD_PREFIX, MD5_PASSWORD_PREFIX, SCRAM_SHA_256,
};
use crate::auth::hba::{CheckResult, PgHba};
use crate::errors::ClientIdentifier;

fn base_ci() -> ClientIdentifier {
    ClientIdentifier {
        addr: "127.0.0.1".into(),
        application_name: "test".into(),
        username: "user".into(),
        pool_name: "db".into(),
        is_talos: false,
        hba_scram: CheckResult::NotMatched,
        hba_md5: CheckResult::NotMatched,
    }
}

// Build ClientIdentifier with HBA decisions computed from concrete pg_hba rules
fn ci_from_hba(hba_text: &str, ssl: bool) -> ClientIdentifier {
    let hba = PgHba::from_content(hba_text);
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    let username = "user";
    let database = "db";
    let hba_scram = hba.check_hba(ip, ssl, "scram-sha-256", username, database);
    let hba_md5 = hba.check_hba(ip, ssl, "md5", username, database);
    let mut ci = base_ci();
    ci.hba_scram = hba_scram;
    ci.hba_md5 = hba_md5;
    ci
}

#[test]
fn talos_short_circuit_allows() {
    let mut ci = base_ci();
    ci.is_talos = true;
    let pw = format!("{}something", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Allow);
}

#[test]
fn empty_password_trust_when_any_trust() {
    let mut ci = base_ci();
    ci.hba_scram = CheckResult::Trust;
    assert_eq!(eval_hba_for_pool_password("", &ci), CheckResult::Trust);

    let mut ci2 = base_ci();
    ci2.hba_md5 = CheckResult::Trust;
    assert_eq!(eval_hba_for_pool_password("", &ci2), CheckResult::Trust);
}

#[test]
fn scram_password_trust_cases() {
    let mut ci = base_ci();
    ci.hba_scram = CheckResult::Trust;
    let pw = format!("{}abc", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Trust);

    let mut ci2 = base_ci();
    ci2.hba_scram = CheckResult::NotMatched;
    ci2.hba_md5 = CheckResult::Trust;
    assert_eq!(eval_hba_for_pool_password(&pw, &ci2), CheckResult::Trust);
}

#[test]
fn scram_password_deny_cases() {
    let pw = format!("{}abc", SCRAM_SHA_256);

    // NotMatched + NotMatched => Deny
    let mut ci1 = base_ci();
    ci1.hba_scram = CheckResult::NotMatched;
    ci1.hba_md5 = CheckResult::NotMatched;
    assert_eq!(eval_hba_for_pool_password(&pw, &ci1), CheckResult::Deny);

    // NotMatched + Deny => Deny
    let mut ci2 = base_ci();
    ci2.hba_scram = CheckResult::NotMatched;
    ci2.hba_md5 = CheckResult::Deny;
    assert_eq!(eval_hba_for_pool_password(&pw, &ci2), CheckResult::Deny);

    // Deny => Deny
    let mut ci3 = base_ci();
    ci3.hba_scram = CheckResult::Deny;
    assert_eq!(eval_hba_for_pool_password(&pw, &ci3), CheckResult::Deny);
}

#[test]
fn scram_password_allow_fallthrough() {
    let mut ci = base_ci();
    ci.hba_scram = CheckResult::Allow; // explicitly allowed
    let pw = format!("{}zzz", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Allow);
}

#[test]
fn md5_password_trust_deny_allow() {
    let pw = format!("{}hash", MD5_PASSWORD_PREFIX);

    let mut ci_trust = base_ci();
    ci_trust.hba_md5 = CheckResult::Trust;
    assert_eq!(
        eval_hba_for_pool_password(&pw, &ci_trust),
        CheckResult::Trust
    );

    let mut ci_nm = base_ci();
    ci_nm.hba_md5 = CheckResult::NotMatched;
    assert_eq!(eval_hba_for_pool_password(&pw, &ci_nm), CheckResult::Deny);

    let mut ci_deny = base_ci();
    ci_deny.hba_md5 = CheckResult::Deny;
    assert_eq!(eval_hba_for_pool_password(&pw, &ci_deny), CheckResult::Deny);

    let mut ci_allow = base_ci();
    ci_allow.hba_md5 = CheckResult::Allow;
    assert_eq!(
        eval_hba_for_pool_password(&pw, &ci_allow),
        CheckResult::Allow
    );
}

#[test]
fn other_auth_kinds_default_allow() {
    let mut ci = base_ci();
    ci.hba_md5 = CheckResult::Deny; // shouldn't matter for non-md5/non-scram
    let pw = format!("{}...", JWT_PUB_KEY_PASSWORD_PREFIX);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Allow);

    let ci2 = base_ci();
    let pw2 = "some-unknown-prefix";
    assert_eq!(eval_hba_for_pool_password(pw2, &ci2), CheckResult::Allow);
}

// ========== Concrete PgHba-driven tests ==========

#[test]
fn scram_trust_from_hba_trust_rule() {
    let hba = "host all all 127.0.0.1/32 trust";
    let ci = ci_from_hba(hba, false);
    let pw = format!("{}secret", SCRAM_SHA_256);
    // trust rule should result in Trust regardless of md5/scram
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Trust);
}

#[test]
fn scram_allow_from_hba_scram_rule() {
    let hba = "host all all 127.0.0.1/32 scram-sha-256";
    let ci = ci_from_hba(hba, true);
    let pw = format!("{}secret", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Allow);
}

#[test]
fn scram_not_matched_leads_to_deny_when_no_md5_match() {
    // Rule for a different network, so both scram and md5 will be NotMatched for 127.0.0.1
    let hba = "host all all 10.0.0.0/8 md5";
    let ci = ci_from_hba(hba, false);
    let pw = format!("{}secret", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Deny);
}

#[test]
fn md5_trust_from_hba_trust_rule() {
    let hba = "host all all 127.0.0.1/32 trust";
    let ci = ci_from_hba(hba, false);
    let pw = format!("{}hash", MD5_PASSWORD_PREFIX);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Trust);
}

#[test]
fn md5_allow_from_hba_md5_rule() {
    let hba = "host all all 127.0.0.1/32 md5";
    let ci = ci_from_hba(hba, false);
    let pw = format!("{}hash", MD5_PASSWORD_PREFIX);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Allow);
}

#[test]
fn scram_allow_from_md5_scram_rule() {
    let hba = "host all all 127.0.0.1/32 md5";
    let ci = ci_from_hba(hba, false);
    let pw = format!("{}hash", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Allow);
}

#[test]
fn md5_not_matched_leads_to_deny_even_if_scram_allowed() {
    // Only scram rule matches, so md5 is NotMatched and should Deny
    let hba = "host all all 127.0.0.1/32 scram-sha-256";
    let ci = ci_from_hba(hba, false);
    let pw = format!("{}hash", MD5_PASSWORD_PREFIX);
    assert_eq!(eval_hba_for_pool_password(&pw, &ci), CheckResult::Deny);
}

// ---- New tests to ensure `local` rules are ignored for TCP/IP connections ----
#[test]
fn local_trust_is_ignored_for_tcp() {
    // Only a local rule present; for TCP/IP it must be ignored → NotMatched for both methods
    let hba = "local all all trust";

    // For SCRAM password
    let ci_scram = ci_from_hba(hba, false);
    let pw_scram = format!("{}secret", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw_scram, &ci_scram), CheckResult::Deny);

    // For MD5 password
    let ci_md5 = ci_from_hba(hba, false);
    let pw_md5 = format!("{}hash", MD5_PASSWORD_PREFIX);
    assert_eq!(eval_hba_for_pool_password(&pw_md5, &ci_md5), CheckResult::Deny);
}

#[test]
fn local_then_host_rule_behaves_like_host_only() {
    // Local rule should be ignored; the host rule should define the outcome
    let hba = "local all all trust\nhost all all 127.0.0.1/32 md5";

    // MD5 password should be allowed by the host md5 rule
    let ci_md5 = ci_from_hba(hba, false);
    let pw_md5 = format!("{}hash", MD5_PASSWORD_PREFIX);
    assert_eq!(eval_hba_for_pool_password(&pw_md5, &ci_md5), CheckResult::Allow);

    // SCRAM password should be allowed to proceed (scram NotMatched, md5 Allow → overall Allow)
    let ci_scram = ci_from_hba(hba, false);
    let pw_scram = format!("{}secret", SCRAM_SHA_256);
    assert_eq!(eval_hba_for_pool_password(&pw_scram, &ci_scram), CheckResult::Allow);
}
