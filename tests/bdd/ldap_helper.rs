//! Real, scenario-owned OpenLDAP fixtures. No pg_doorman LDAP client is involved.

use crate::utils::set_file_permissions;
use crate::world::DoormanWorld;
use base64::Engine;
use cucumber::{gherkin::Step, given, then, when};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const DEFAULT_BASE_DN: &str = "dc=example,dc=test";
const ADMIN_PASSWORD: &str = "fixture-admin-password";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct FixtureSpec {
    base_dn: String,
    ldif: String,
}

impl FixtureSpec {
    fn from_step(world: &DoormanWorld, base_dn: &str, step: &Step) -> Self {
        Self {
            base_dn: world.replace_placeholders(base_dn),
            ldif: world.replace_placeholders(
                step.docstring
                    .as_ref()
                    .expect("LDAP startup requires an LDIF docstring"),
            ),
        }
    }
}

fn validate_dn(dn: &str) -> TestResult<()> {
    if dn.trim().is_empty() || dn.contains(['\r', '\n', '\0']) {
        return Err("LDAP fixture DN must be nonempty and contain no CR, LF or NUL".into());
    }
    Ok(())
}

/// Quote a complete DN for slapd.conf without changing its LDAP escaping.
fn config_dn(dn: &str) -> TestResult<String> {
    validate_dn(dn)?;
    Ok(format!(
        "\"{}\"",
        dn.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Owns every subprocess, including setup commands and partially started slapd.
/// Output goes to files, so a verbose child cannot block on a full pipe.
struct Process(Child);

impl Process {
    fn status(&mut self) -> std::io::Result<Option<ExitStatus>> {
        // Child caches a reaped status, so Drop can safely call stop again.
        self.0.try_wait()
    }

    fn stop(&mut self) -> std::io::Result<()> {
        let child = &mut self.0;
        if child.try_wait()?.is_none() {
            // The unreaped Child still owns this PID.
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(2);
            while child.try_wait()?.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
        }
        child.wait()?;
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("LDAP fixture child cleanup failed: {error}");
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct CommandOutcome {
    program: String,
    status: ExitStatus,
    // Assertions need the unmodified protocol output, even if a password
    // happens to match part of a DN or a result description.
    stdout: String,
    stderr: String,
    diagnostics: String,
}

impl std::fmt::Debug for CommandOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandOutcome")
            .field("program", &self.program)
            .field("status", &self.status)
            .field("output", &self.diagnostics)
            .finish()
    }
}

impl CommandOutcome {
    fn expect_code(&self, code: i32) -> TestResult<()> {
        if self.status.code() != Some(code) {
            return Err(format!("LDAP fixture command: expected exit {code}, got {self:?}").into());
        }
        Ok(())
    }

    fn expect_identity(&self, dn: &str) -> TestResult<()> {
        self.expect_code(0)?;
        if self.stdout.trim() != format!("dn:{dn}") {
            return Err(
                format!("LDAP bind did not establish the requested identity: {self:?}").into(),
            );
        }
        Ok(())
    }
}

fn redact_text(mut text: String, secrets: &[&str]) -> String {
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[redacted]");
    }
    text
}

async fn run_command(
    command: &mut Command,
    deadline: Instant,
    redact: &[&str],
) -> TestResult<CommandOutcome> {
    let program = command.get_program().to_string_lossy().into_owned();
    let stdout = NamedTempFile::new()?;
    let stderr = NamedTempFile::new()?;
    let mut process = Process(
        command
            .stdin(Stdio::null())
            .stdout(stdout.reopen()?)
            .stderr(stderr.reopen()?)
            .spawn()
            .map_err(|error| format!("LDAP fixture {program}: could not spawn process: {error}"))?,
    );
    let status = loop {
        if let Some(status) = process.status()? {
            break status;
        }
        if Instant::now() >= deadline {
            process.stop()?;
            return Err(
                format!("LDAP fixture {program} exceeded its deadline (child reaped)").into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let stdout = fs::read_to_string(stdout.path())?;
    let stderr = fs::read_to_string(stderr.path())?;
    let diagnostics = redact_text(format!("stdout:\n{stdout}\nstderr:\n{stderr}"), redact);
    Ok(CommandOutcome {
        program,
        status,
        stdout,
        stderr,
        diagnostics,
    })
}

async fn openssl(directory: &Path, args: &[&str]) -> TestResult<()> {
    run_command(
        Command::new("openssl").current_dir(directory).args(args),
        Instant::now() + Duration::from_secs(15),
        &[],
    )
    .await?
    .expect_code(0)
}

async fn generate_certificates(directory: &Path) -> TestResult<()> {
    for (key, cert, subject) in [
        ("ca.key", "ca.pem", "/CN=BDD LDAP CA"),
        ("wrong-ca.key", "wrong-ca.pem", "/CN=Unrelated BDD CA"),
    ] {
        openssl(
            directory,
            &[
                "req",
                "-new",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                key,
                "-out",
                cert,
                "-days",
                "1",
                "-subj",
                subject,
                "-addext",
                "basicConstraints=critical,CA:TRUE",
                "-addext",
                "keyUsage=critical,keyCertSign,cRLSign",
            ],
        )
        .await?;
        set_file_permissions(&directory.join(key), 0o600);
    }
    openssl(
        directory,
        &[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
            "-subj",
            "/CN=localhost",
        ],
    )
    .await?;
    set_file_permissions(&directory.join("server.key"), 0o600);
    fs::write(
        directory.join("server.ext"),
        concat!(
            "basicConstraints=critical,CA:FALSE\n",
            "keyUsage=critical,digitalSignature,keyEncipherment\n",
            "extendedKeyUsage=serverAuth\n",
            "subjectAltName=DNS:localhost,IP:127.0.0.1\n",
        ),
    )?;
    openssl(
        directory,
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "server.pem",
            "-days",
            "1",
            "-extfile",
            "server.ext",
        ],
    )
    .await
}

#[derive(Clone, Copy)]
enum Transport {
    Ldap,
    Ldaps,
    StartTls,
}

impl Transport {
    fn parse(value: &str) -> TestResult<Self> {
        match value {
            "LDAP" => Ok(Self::Ldap),
            "LDAPS" => Ok(Self::Ldaps),
            "StartTLS" => Ok(Self::StartTls),
            _ => Err(format!("unknown LDAP fixture transport {value}").into()),
        }
    }
}

enum SearchScope {
    Base,
    Subtree,
}

/// Drop order keeps config, MDB and certificates alive until slapd is reaped.
pub struct LdapServer {
    process: Option<Process>,
    directory: TempDir,
    fixture: FixtureSpec,
    admin_dn: String,
    ldap_port: u16,
    ldaps_port: u16,
}

impl LdapServer {
    /// Build the initial directory before reserving ports or starting slapd.
    async fn prepare(fixture: FixtureSpec) -> TestResult<Self> {
        let admin_dn = format!("cn=admin,{}", fixture.base_dn);
        let config_base_dn = config_dn(&fixture.base_dn)?;
        let config_admin_dn = config_dn(&admin_dn)?;
        if fixture.ldif.trim().is_empty() {
            return Err("LDAP startup LDIF docstring is empty; include the base DN entry".into());
        }
        let schema = std::env::var_os("LDAP_SCHEMA_DIR")
            .ok_or("LDAP_SCHEMA_DIR missing; use the Nix LDAP test environment")?;
        let directory = tempfile::Builder::new().prefix("bdd-ldap-").tempdir()?;
        let mut server = Self {
            process: None,
            directory,
            fixture,
            admin_dn,
            ldap_port: 0,
            ldaps_port: 0,
        };
        let dir = server.directory.path().to_path_buf();
        fs::create_dir(dir.join("data"))?;
        generate_certificates(&dir).await?;
        let config = include_str!("fixtures/ldap/slapd.conf")
            .replace(
                "@DIRECTORY@",
                dir.to_str().ok_or("non-UTF8 fixture directory")?,
            )
            .replace(
                "@SCHEMA@",
                Path::new(&schema).to_str().ok_or("non-UTF8 schema path")?,
            )
            .replace("@BASE_DN@", &config_base_dn)
            .replace("@ADMIN_DN@", &config_admin_dn);
        fs::write(dir.join("slapd.conf"), config)?;
        fs::write(dir.join("initial.ldif"), &server.fixture.ldif)?;
        set_file_permissions(&dir.join("initial.ldif"), 0o600);
        run_command(
            Command::new("slaptest")
                .args(["-u", "-f"])
                .arg(dir.join("slapd.conf")),
            Instant::now() + COMMAND_TIMEOUT,
            &[],
        )
        .await?
        .expect_code(0)?;
        let import = run_command(
            Command::new("slapadd")
                .arg("-f")
                .arg(dir.join("slapd.conf"))
                .arg("-l")
                .arg(dir.join("initial.ldif")),
            Instant::now() + COMMAND_TIMEOUT,
            &[],
        )
        .await?;
        if !import.status.success() {
            // slapadd diagnostics can echo arbitrary LDIF lines, including
            // plaintext userPassword values. Do not reproduce that input.
            return Err(format!(
                "slapadd rejected the initial LDAP LDIF ({}); check its syntax, schema and base DN entry (LDIF diagnostics withheld)",
                import.status
            )
            .into());
        }
        server.admin_dn = server.pretty_dn(&server.admin_dn).await?;
        Ok(server)
    }

    async fn start(fixture: FixtureSpec, first_ldaps_port: Option<u16>) -> TestResult<Self> {
        let slapd = std::env::var_os("LDAP_SLAPD_BIN")
            .ok_or("LDAP_SLAPD_BIN missing; use the Nix LDAP test environment")?;
        let mut server = Self::prepare(fixture).await?;
        let dir = server.directory.path().to_path_buf();

        // Reserve both ports together; retry only a diagnosed bind collision
        // in the small interval between releasing reservations and slapd bind.
        let deadline = Instant::now() + START_TIMEOUT;
        for attempt in 0..5 {
            let ldap = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let ldaps = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            server.ldap_port = ldap.local_addr()?.port();
            server.ldaps_port = ldaps.local_addr()?.port();
            if attempt == 0 {
                server.ldaps_port = first_ldaps_port.unwrap_or(server.ldaps_port);
            }
            if server.ldap_port < 1024 || server.ldaps_port < 1024 {
                continue;
            }
            let log = File::create(dir.join("slapd.log"))?;
            let urls = format!(
                "{} {}",
                server.url(Transport::Ldap),
                server.url(Transport::Ldaps)
            );
            let mut command = Command::new(&slapd);
            command
                .current_dir(&dir)
                .arg("-f")
                .arg(dir.join("slapd.conf"))
                // stats keeps foreground mode and startup errors visible,
                // without packet/BER logging that could expose passwords.
                .args(["-h", &urls, "-d", "256"])
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            // slapd sizes connection bookkeeping from RLIMIT_NOFILE. Some
            // Docker daemons inherit limits above a billion, exhausting memory
            // before the first bind. Bound only this child, preserving a lower
            // inherited limit; never change the host or the BDD runner's limit.
            unsafe {
                command.pre_exec(|| {
                    let mut limit = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    limit.rlim_cur = limit.rlim_cur.min(4096);
                    if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let core = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::setrlimit(libc::RLIMIT_CORE, &core) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            drop((ldap, ldaps));
            server.process = Some(Process(command.spawn()?));
            match server.wait_ready(deadline).await {
                Ok(()) => return Ok(server),
                Err(error) => {
                    server.process.take();
                    let log = fs::read_to_string(dir.join("slapd.log"))?;
                    if attempt < 4
                        && Instant::now() < deadline
                        && log.contains("Address already in use")
                    {
                        continue;
                    }
                    return Err(format!("LDAP fixture startup failed: {error}\n{log}").into());
                }
            }
        }
        Err("LDAP fixture could not acquire nonprivileged ports".into())
    }

    fn url(&self, transport: Transport) -> String {
        match transport {
            Transport::Ldaps => format!("ldaps://127.0.0.1:{}", self.ldaps_port),
            _ => format!("ldap://127.0.0.1:{}", self.ldap_port),
        }
    }

    /// Match the DN representation returned by the OpenLDAP clients.
    async fn pretty_dn(&self, dn: &str) -> TestResult<String> {
        validate_dn(dn)?;
        let result = run_command(
            Command::new("slapdn")
                .arg("-f")
                .arg(self.directory.path().join("slapd.conf"))
                .args(["-P", dn]),
            Instant::now() + COMMAND_TIMEOUT,
            &[],
        )
        .await?;
        result.expect_code(0)?;
        Ok(result.stdout.trim().to_string())
    }

    async fn command(
        &self,
        tool: &str,
        transport: Transport,
        credentials: (&str, &str),
        wrong_ca: bool,
        args: &[&str],
        deadline: Instant,
    ) -> TestResult<CommandOutcome> {
        let (dn, password) = credentials;
        validate_dn(dn)?;
        let mut secret = NamedTempFile::new_in(self.directory.path())?;
        secret.write_all(password.as_bytes())?;
        secret.flush()?;
        let ca = self
            .directory
            .path()
            .join(if wrong_ca { "wrong-ca.pem" } else { "ca.pem" });
        let mut command = Command::new(tool);
        command
            .current_dir(self.directory.path())
            // Ignore host ldap.conf, ldaprc and LDAP environment defaults.
            // Explicit -o options are applied after this initialization guard.
            .env("LDAPNOINIT", "1")
            .env("LC_ALL", "C")
            .args(["-x", "-H", &self.url(transport), "-D", dn, "-y"])
            .arg(secret.path())
            .args([
                "-o",
                "nettimeout=2",
                "-o",
                "tls_reqcert=demand",
                "-o",
                "tls_reqsan=demand",
                "-o",
            ])
            .arg(format!("tls_cacert={}", ca.display()));
        if matches!(transport, Transport::StartTls) {
            command.arg("-ZZ");
        }
        // TLS-only trace on a rejected handshake; captured and redacted,
        // never streamed. It distinguishes CA rejection from TCP failure.
        if wrong_ca {
            command.args(["-d", "1"]);
        }
        command.args(args);
        run_command(&mut command, deadline, &[password, ADMIN_PASSWORD]).await
    }

    async fn bind(
        &self,
        transport: Transport,
        dn: &str,
        password: &str,
        wrong_ca: bool,
        deadline: Instant,
    ) -> TestResult<CommandOutcome> {
        self.command(
            "ldapwhoami",
            transport,
            (dn, password),
            wrong_ca,
            &[],
            deadline,
        )
        .await
    }

    /// Search for entry DNs as this directory's technical administrator.
    async fn search(
        &self,
        transport: Transport,
        base: &str,
        scope: SearchScope,
        filter: &str,
        deadline: Instant,
    ) -> TestResult<CommandOutcome> {
        validate_dn(base)?;
        self.command(
            "ldapsearch",
            transport,
            (&self.admin_dn, ADMIN_PASSWORD),
            false,
            &[
                "-LLL",
                "-o",
                "ldif_wrap=no",
                "-b",
                base,
                "-s",
                match scope {
                    SearchScope::Base => "base",
                    SearchScope::Subtree => "sub",
                },
                filter,
                "dn",
            ],
            deadline,
        )
        .await
    }

    async fn wait_ready(&mut self, deadline: Instant) -> TestResult<()> {
        loop {
            if let Some(status) = self.process.as_mut().unwrap().status()? {
                return Err(format!("slapd exited with {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("LDAP bind/search readiness deadline exceeded".into());
            }
            let probe_deadline = deadline.min(Instant::now() + COMMAND_TIMEOUT);
            let result = self
                .search(
                    Transport::Ldap,
                    &self.fixture.base_dn,
                    SearchScope::Base,
                    "(objectClass=*)",
                    probe_deadline,
                )
                .await?;
            if result.status.success()
                && result
                    .stdout
                    .lines()
                    .filter(|s| s.starts_with("dn:"))
                    .count()
                    == 1
            {
                let tls = self
                    .bind(
                        Transport::Ldaps,
                        &self.admin_dn,
                        ADMIN_PASSWORD,
                        false,
                        deadline,
                    )
                    .await?;
                tls.expect_identity(&self.admin_dn)?;
                return Ok(());
            }
            // -1 maps to 255 in the CLI. Other results (including 49) are
            // configuration/fixture failures, not something readiness retries.
            result.expect_code(255)?;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn stop(mut self) -> TestResult<()> {
        if let Some(mut child) = self.process.take() {
            child.stop()?;
        }
        let path = self.directory.path().to_path_buf();
        self.directory.close()?;
        if path.exists() {
            return Err("LDAP fixture directory survived cleanup".into());
        }
        Ok(())
    }

    fn variables(&self, name: &str) -> HashMap<String, String> {
        let prefix = format!("LDAP_{}", name.to_uppercase());
        HashMap::from([
            (format!("{prefix}_URL"), self.url(Transport::Ldap)),
            (format!("{prefix}_LDAPS_URL"), self.url(Transport::Ldaps)),
            (format!("{prefix}_PORT"), self.ldap_port.to_string()),
            (format!("{prefix}_LDAPS_PORT"), self.ldaps_port.to_string()),
            (format!("{prefix}_BASE_DN"), self.fixture.base_dn.clone()),
            (format!("{prefix}_ADMIN_DN"), self.admin_dn.clone()),
            (
                format!("{prefix}_CA_CERT"),
                self.directory.path().join("ca.pem").display().to_string(),
            ),
        ])
    }
}

fn server<'a>(world: &'a DoormanWorld, name: &str) -> &'a LdapServer {
    world
        .ldap_servers
        .get(name)
        .expect("named LDAP fixture not started")
}

/// Run on successful and failed scenarios; Drop also covers cancelled setup.
pub fn stop_ldap_servers(world: &mut DoormanWorld) {
    let mut errors = Vec::new();
    for (name, server) in world.ldap_servers.drain() {
        for key in server.variables(&name).keys() {
            world.vars.remove(key);
        }
        if let Err(error) = server.stop() {
            errors.push(format!("{name}: {error}"));
        }
    }
    assert!(
        errors.is_empty(),
        "LDAP fixture cleanup: {}",
        errors.join("; ")
    );
}

#[given(expr = "LDAP server {string} is started with LDIF:")]
async fn start_with_ldif(world: &mut DoormanWorld, name: String, step: &Step) {
    start_with_base_and_ldif(world, name, DEFAULT_BASE_DN.to_string(), step).await;
}

#[given(expr = "LDAP server {string} is started with base DN {string} and LDIF:")]
async fn start_with_base_and_ldif(
    world: &mut DoormanWorld,
    name: String,
    base_dn: String,
    step: &Step,
) {
    let fixture = FixtureSpec::from_step(world, &base_dn, step);
    insert_server(world, name, fixture, None).await;
}

#[given(expr = "LDAP server {string} is started after a port collision with LDIF:")]
async fn start_after_collision(world: &mut DoormanWorld, name: String, step: &Step) {
    let fixture = FixtureSpec::from_step(world, DEFAULT_BASE_DN, step);
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    insert_server(
        world,
        name.clone(),
        fixture,
        Some(occupied.local_addr().unwrap().port()),
    )
    .await;
    assert_ne!(
        server(world, &name).ldaps_port,
        occupied.local_addr().unwrap().port()
    );
}

async fn insert_server(
    world: &mut DoormanWorld,
    name: String,
    fixture: FixtureSpec,
    first_ldaps_port: Option<u16>,
) {
    assert!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "LDAP fixture names must be lowercase ASCII identifiers"
    );
    assert!(
        !world.ldap_servers.contains_key(&name),
        "LDAP fixture already exists"
    );
    let server = LdapServer::start(fixture, first_ldaps_port)
        .await
        .expect("start LDAP fixture");
    world.vars.extend(server.variables(&name));
    world.ldap_servers.insert(name, server);
}

#[when(expr = "LDAP server {string} is stopped")]
async fn stop_server(world: &mut DoormanWorld, name: String) {
    let server = world
        .ldap_servers
        .remove(&name)
        .expect("LDAP fixture not started");
    for key in server.variables(&name).keys() {
        world.vars.remove(key);
    }
    server
        .stop()
        .expect("stop and reap LDAP fixture, remove its files");
}

#[when(expr = "LDAP server {string} is restarted with fresh data")]
async fn restart_server(world: &mut DoormanWorld, name: String) {
    let fixture = server(world, &name).fixture.clone();
    stop_server(world, name.clone()).await;
    insert_server(world, name, fixture, None).await;
}

#[then(expr = "LDAP server {string} accepts bind over {string} as {string} with password {string}")]
async fn accepts_bind(
    world: &mut DoormanWorld,
    name: String,
    transport: String,
    dn: String,
    password: String,
) {
    check_bind(world, name, transport, dn, &password, 0).await;
}

#[then(
    expr = "LDAP server {string} rejects bind over {string} as {string} with password {string} with result 49"
)]
async fn rejects_bind(
    world: &mut DoormanWorld,
    name: String,
    transport: String,
    dn: String,
    password: String,
) {
    check_bind(world, name, transport, dn, &password, 49).await;
}

async fn check_bind(
    world: &mut DoormanWorld,
    name: String,
    transport: String,
    dn: String,
    password: &str,
    expected_code: i32,
) {
    let dn = world.replace_placeholders(&dn);
    let password = world.replace_placeholders(password);
    let result = server(world, &name)
        .bind(
            Transport::parse(&world.replace_placeholders(&transport)).unwrap(),
            &dn,
            &password,
            false,
            Instant::now() + COMMAND_TIMEOUT,
        )
        .await
        .expect("LDAP bind command");
    if expected_code == 0 {
        let expected_dn = server(world, &name).pretty_dn(&dn).await.unwrap();
        result.expect_identity(&expected_dn).unwrap();
    } else {
        result.expect_code(expected_code).unwrap();
        assert!(
            result.stderr.contains("Invalid credentials (49)"),
            "expected LDAP invalidCredentials: {result:?}"
        );
    }
}

#[then(expr = "LDAP server {string} rejects an unrelated CA over {string}")]
async fn rejects_ca(world: &mut DoormanWorld, name: String, transport: String) {
    let server = server(world, &name);
    let transport = Transport::parse(&world.replace_placeholders(&transport)).unwrap();
    assert!(
        !matches!(transport, Transport::Ldap),
        "CA verification requires TLS"
    );
    for wrong_ca in [false, true, false] {
        let result = server
            .bind(
                transport,
                &server.admin_dn,
                ADMIN_PASSWORD,
                wrong_ca,
                Instant::now() + COMMAND_TIMEOUT,
            )
            .await
            .expect("CA verification command");
        if wrong_ca {
            // ldapwhoami reports an LDAPS connection error as -1 (exit 255),
            // while mandatory StartTLS exits 1 before attempting the bind.
            result
                .expect_code(if matches!(transport, Transport::StartTls) {
                    1
                } else {
                    255
                })
                .unwrap();
            assert!(
                // OpenSSL X509_V_ERR_SELF_SIGNED_CERT_IN_CHAIN / UNABLE_TO_GET_ISSUER_CERT_LOCALLY.
                (result.stderr.contains("err: 19,") || result.stderr.contains("err: 20,"))
                    && result
                        .stderr
                        .contains("TLS certificate verification: Error"),
                "expected certificate trust failure, not a TCP/auth failure: {result:?}"
            );
        } else {
            result.expect_identity(&server.admin_dn).unwrap();
        }
    }
}

#[then(
    expr = "LDAP server {string} search over {string} at {string} for {string} returns DN {string}"
)]
async fn search_dn(
    world: &mut DoormanWorld,
    name: String,
    transport: String,
    base: String,
    filter: String,
    dn: String,
) {
    let base = world.replace_placeholders(&base);
    let filter = world.replace_placeholders(&filter);
    let dn = world.replace_placeholders(&dn);
    let server = server(world, &name);
    let dn = server.pretty_dn(&dn).await.unwrap();
    let result = server
        .search(
            Transport::parse(&world.replace_placeholders(&transport)).unwrap(),
            &base,
            SearchScope::Subtree,
            &filter,
            Instant::now() + COMMAND_TIMEOUT,
        )
        .await
        .expect("LDAP search command");
    result.expect_code(0).unwrap();
    let dns: Vec<_> = result
        .stdout
        .lines()
        .filter_map(|s| s.strip_prefix("dn: "))
        .collect();
    assert!(
        dns == vec![dn.as_str()],
        "search must return exactly the expected entry: {result:?}"
    );
}

#[when(expr = "LDAP server {string} password for {string} becomes {string}")]
async fn change_password(world: &mut DoormanWorld, name: String, dn: String, password: String) {
    let dn = world.replace_placeholders(&dn);
    let password = world.replace_placeholders(&password);
    validate_dn(&dn).unwrap();
    let server = server(world, &name);
    // Store only a hash in the change LDIF. The CLI receives credentials in
    // a mode-0600 password file, never in command-line arguments.
    let digest = openssl::sha::sha1(password.as_bytes());
    let hash = base64::engine::general_purpose::STANDARD.encode(digest);
    let encoded_dn = base64::engine::general_purpose::STANDARD.encode(dn.as_bytes());
    let mut ldif = NamedTempFile::new_in(server.directory.path()).unwrap();
    write!(
        ldif,
        "dn:: {encoded_dn}\nchangetype: modify\nreplace: userPassword\nuserPassword: {{SHA}}{hash}\n"
    )
    .unwrap();
    ldif.flush().unwrap();
    let result = server
        .command(
            "ldapmodify",
            Transport::Ldap,
            (&server.admin_dn, ADMIN_PASSWORD),
            false,
            &["-f", ldif.path().to_str().unwrap()],
            Instant::now() + COMMAND_TIMEOUT,
        )
        .await
        .expect("LDAP modify command");
    result.expect_code(0).unwrap();
}
