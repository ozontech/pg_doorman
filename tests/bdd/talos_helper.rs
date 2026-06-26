use crate::pg_connection::PgConnection;
use crate::world::DoormanWorld;
use cucumber::{given, when};
use jwt::{Header, PKeyWithDigest, SignWithKey, Token};
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

/// Generates an RSA keypair and stores its paths in `world.vars`.
/// The public key file stem is the Talos `kid`.
#[given(regex = r"^keypair '(.+)' generated for talos with kid '(.+)'$")]
pub async fn generate_keypair(world: &mut DoormanWorld, name: String, kid: String) {
    let rsa = Rsa::generate(2048).expect("failed to generate RSA keypair");

    let mut priv_file = NamedTempFile::new().expect("priv tempfile");
    priv_file
        .write_all(&rsa.private_key_to_pem().expect("private pem"))
        .expect("write priv pem");
    priv_file.flush().expect("flush priv pem");

    let pub_dir = tempfile::tempdir().expect("pub dir");
    let pub_path = pub_dir.path().join(format!("{kid}.pem"));
    std::fs::write(&pub_path, rsa.public_key_to_pem().expect("public pem")).expect("write pub pem");

    let upper = name.to_uppercase();
    world.vars.insert(
        format!("{upper}_PUBKEY_PATH"),
        pub_path.to_str().unwrap().to_string(),
    );
    world.vars.insert(
        format!("{upper}_PRIVKEY_PATH"),
        priv_file.path().to_str().unwrap().to_string(),
    );
    world.vars.insert(format!("{upper}_KID"), kid);

    world.talos_pub_keys.push(pub_dir);
    world.talos_priv_keys.push(priv_file);
}

/// Opens a `user=talos` session with a freshly signed JWT.
/// Any auth error fails the step.
#[when(
    regex = r#"^we open Talos session '([^']+)' as client_id '([^']+)' role '([^']+)' database '([^']+)' signed with '([^']+)'$"#
)]
pub async fn open_talos_session(
    world: &mut DoormanWorld,
    session_name: String,
    client_id: String,
    role: String,
    database: String,
    keypair_name: String,
) {
    let upper = keypair_name.to_uppercase();
    let priv_path = world
        .vars
        .get(&format!("{upper}_PRIVKEY_PATH"))
        .expect("keypair not generated; missing Given step")
        .clone();
    let kid = world
        .vars
        .get(&format!("{upper}_KID"))
        .expect("kid not stored")
        .clone();

    let token = build_talos_jwt(&client_id, &role, &database, &priv_path, &kid);

    let doorman_port = world.doorman_port.expect("pg_doorman not started");
    let addr = format!("127.0.0.1:{doorman_port}");

    let mut conn = PgConnection::connect(&addr)
        .await
        .expect("failed to connect to pg_doorman");
    conn.send_startup("talos", &database)
        .await
        .expect("failed to send startup");
    conn.authenticate("talos", &token)
        .await
        .expect("Talos authentication failed");

    world.named_sessions.insert(session_name, conn);
}

#[derive(Serialize)]
struct BddTalosRoles<'a> {
    roles: Vec<&'a str>,
}

#[derive(Serialize)]
struct BddTalosClaims<'a> {
    exp: u64,
    nbf: u64,
    #[serde(rename = "clientId")]
    client_id: &'a str,
    resource_access: HashMap<String, BddTalosRoles<'a>>,
}

fn build_talos_jwt(
    client_id: &str,
    role: &str,
    database: &str,
    priv_path: &str,
    kid: &str,
) -> String {
    let priv_pem = std::fs::read_to_string(priv_path).expect("read priv");
    let rsa = Rsa::private_key_from_pem(priv_pem.as_bytes()).expect("rsa from pem");
    let pkey = PKey::from_rsa(rsa).expect("pkey from rsa");
    let signer = PKeyWithDigest {
        digest: MessageDigest::sha256(),
        key: pkey,
    };

    let header = Header {
        algorithm: jwt::AlgorithmType::Rs256,
        key_id: Some(kid.to_string()),
        type_: Some(jwt::header::HeaderType::JsonWebToken),
        content_type: None,
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();

    let mut resource_access = HashMap::new();
    resource_access.insert(
        format!("postgres.local:{database}"),
        BddTalosRoles { roles: vec![role] },
    );

    let claims = BddTalosClaims {
        exp: now + 60,
        nbf: now.saturating_sub(5),
        client_id,
        resource_access,
    };

    Token::new(header, claims)
        .sign_with_key(&signer)
        .expect("sign jwt")
        .as_str()
        .to_string()
}
