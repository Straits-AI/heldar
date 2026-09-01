//! Deployment secrets resolve from a file or a systemd credential, not only the environment (#126).
//!
//! An environment variable is visible in `/proc/<pid>/environ` to anyone who can read it, is
//! inherited by every child the box spawns — ffmpeg included — and lands in `docker inspect`
//! output. That is acceptable on a sealed appliance and weak on a shared host.
//!
//! These drive the REAL `Config::from_env()`, not the resolver in isolation: the resolver having a
//! file branch proves nothing if the master key never calls it. Every assertion below goes through
//! the config the server actually boots with.
//!
//! `Config::from_env()` reads process-global state, so these run under one lock and restore what
//! they set. They are in one `#[test]` for the same reason — parallel test threads would race on
//! the environment and produce failures that depend on scheduling.

use std::path::PathBuf;

/// `Config::from_env()` reads process-global state and these tests mutate it, so they take one lock
/// rather than racing. Poisoning is ignored deliberately: a panicking test (one of these EXPECTS a
/// panic) must not cascade into unrelated failures.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A deterministic, valid 32-byte key in base64 — the shape `openssl rand -base64 32` produces.
///
/// Generated rather than pasted so no key-shaped literal lives in the repository. `decode_key`
/// rejects anything that is not exactly 32 bytes, so this is checked by use, not by eye — the first
/// hand-written fixture here was 31 bytes and the test caught it.
fn fixture_key(fill: u8) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode([fill; 32])
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("heldar-secret-cfg-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Env(Vec<String>);
impl Env {
    fn new() -> Self {
        Env(Vec::new())
    }
    fn set(&mut self, k: &str, v: &str) {
        std::env::set_var(k, v);
        self.0.push(k.to_string());
    }
    fn clear(&mut self, k: &str) {
        std::env::remove_var(k);
        self.0.push(k.to_string());
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        for k in &self.0 {
            std::env::remove_var(k);
        }
    }
}

#[test]
fn the_master_key_resolves_from_a_file_and_a_systemd_credential() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("master");
    let mut e = Env::new();
    e.clear("HELDAR_SECRET_KEY");
    e.clear("HELDAR_SECRET_KEY_FILE");
    e.clear("CREDENTIALS_DIRECTORY");

    // A real 32-byte key, base64 — the same thing `openssl rand -base64 32` produces.
    // Built here rather than pasted: a base64-of-32-bytes literal in the repo is a key SHAPE, and
    // the secret scanner is right to flag one even in a fixture. This is the same value every run.
    let key = fixture_key(b'k');

    // 1. Unset is a normal state: encryption at rest is optional.
    assert!(
        heldar_kernel::config::Config::from_env()
            .secret_key_b64
            .is_none(),
        "no source configured must read as no key, not an error"
    );

    // 2. A FILE, which is the shape Docker secrets and Kubernetes already produce. The trailing
    //    newline every editor adds must not become part of the key.
    let p = dir.join("master.key");
    std::fs::write(&p, format!("{key}\n")).unwrap();
    e.set("HELDAR_SECRET_KEY_FILE", p.to_str().unwrap());
    assert_eq!(
        heldar_kernel::config::Config::from_env()
            .secret_key_b64
            .as_deref(),
        Some(key.as_str()),
        "the master key must resolve from HELDAR_SECRET_KEY_FILE"
    );

    // 3. And it must actually WORK as a key — resolving a string is not the same as the encryption
    //    layer accepting it, and a test that stopped at the string would not notice a stray byte.
    heldar_kernel::services::secrets::decode_key(&key).expect("resolves to a usable 32-byte key");

    // 4. A SYSTEMD CREDENTIAL, named by the unit's `LoadCredential=` and handed over 0400 in tmpfs.
    e.clear("HELDAR_SECRET_KEY_FILE");
    let cred = tmp("systemd");
    std::fs::write(cred.join("HELDAR_SECRET_KEY"), &key).unwrap();
    e.set("CREDENTIALS_DIRECTORY", cred.to_str().unwrap());
    assert_eq!(
        heldar_kernel::config::Config::from_env()
            .secret_key_b64
            .as_deref(),
        Some(key.as_str()),
        "the master key must resolve from a systemd credential"
    );

    // 5. THE ENVIRONMENT STILL WINS. A box already setting the variable must keep the value it has
    //    been using — a new source taking precedence would rotate the key on upgrade day and fail
    //    to decrypt every stored credential.
    let other = fixture_key(b'o');
    e.set("HELDAR_SECRET_KEY", &other);
    assert_eq!(
        heldar_kernel::config::Config::from_env()
            .secret_key_b64
            .as_deref(),
        Some(other.as_str()),
        "the environment must win, or an upgrade silently re-keys the box"
    );
}

/// Every secret the issue names, not just the master key — a chain wired into one of four is a
/// chain an operator cannot rely on.
#[test]
fn every_deployment_secret_uses_the_chain() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("all");
    let mut e = Env::new();
    for v in [
        "HELDAR_SECRET_KEY",
        "HELDAR_BOOTSTRAP_ADMIN_PASSWORD",
        "HELDAR_SMTP_PASSWORD",
        "CREDENTIALS_DIRECTORY",
    ] {
        e.clear(v);
    }

    for (name, value) in [
        ("HELDAR_BOOTSTRAP_ADMIN_PASSWORD", "bootstrap-pw"),
        ("HELDAR_SMTP_PASSWORD", "smtp-pw"),
    ] {
        let p = dir.join(name);
        std::fs::write(&p, format!("{value}\n")).unwrap();
        e.set(&format!("{name}_FILE"), p.to_str().unwrap());
    }

    let cfg = heldar_kernel::config::Config::from_env();
    assert_eq!(
        cfg.bootstrap_admin_password.as_deref(),
        Some("bootstrap-pw")
    );
    assert_eq!(cfg.smtp_password.as_deref(), Some("smtp-pw"));
}

/// A stated intent that cannot be honoured must be RECORDED, so the server can refuse to boot.
///
/// An operator who set `HELDAR_SECRET_KEY_FILE` asked for encryption at rest. Silently treating an
/// unreadable file as "no key configured" would store every camera credential in plaintext while
/// the deployment believed they were sealed — invisible until someone read the database.
///
/// It is recorded rather than panicked because `Config::from_env()` is this repo's test-config
/// idiom, called from around sixty helpers: a panic there meant one stale variable in a shell
/// detonated 144 unrelated tests. `heldar_server::run` refuses on a non-empty list.
#[test]
fn a_named_but_unusable_secret_file_is_recorded_so_the_boot_can_refuse() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("fatal");
    let mut e = Env::new();
    e.clear("HELDAR_SECRET_KEY");
    heldar_kernel::config::Config::clear_secret_source_errors();

    e.set(
        "HELDAR_SECRET_KEY_FILE",
        dir.join("does-not-exist").to_str().unwrap(),
    );
    let cfg = heldar_kernel::config::Config::from_env();
    assert!(
        cfg.secret_key_b64.is_none(),
        "an unusable source must not produce a key"
    );

    let errors = heldar_kernel::config::Config::secret_source_errors();
    assert_eq!(errors.len(), 1, "the failure must be recorded: {errors:?}");
    assert!(
        errors[0].contains("does-not-exist"),
        "the error must name the file an operator has to go and fix: {errors:?}"
    );
    heldar_kernel::config::Config::clear_secret_source_errors();
}

/// An environment value reaches the secret VERBATIM.
///
/// `env::var()` only ever *filtered* on trim — it returned the value unchanged. Trimming in the new
/// chain silently changed the secret an existing box had been using: an SMTP password with a
/// trailing space stops authenticating, and a bootstrap password fails a length gate it used to
/// pass. "The environment wins" has to mean the same bytes, not merely the same source.
#[test]
fn an_environment_secret_is_not_reshaped_on_the_way_through() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut e = Env::new();
    e.clear("HELDAR_SMTP_PASSWORD_FILE");
    e.set("HELDAR_SMTP_PASSWORD", "  hunter  ");
    assert_eq!(
        heldar_kernel::config::Config::from_env()
            .smtp_password
            .as_deref(),
        Some("  hunter  "),
        "the value must be byte-identical to what env::var() has always returned"
    );
}

/// A credentials directory that cannot be read must FAIL, not quietly yield no secret.
///
/// `Path::exists()` returns false for a permission error, so gating on it made this branch fail
/// OPEN: an unreadable directory booted the box with no key and stored camera credentials in
/// plaintext — the exact outcome the `_FILE` branch refuses, reached silently one source over.
#[cfg(unix)]
#[test]
fn an_unreadable_credentials_directory_fails_closed() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("eacces");
    std::fs::write(dir.join("HELDAR_SECRET_KEY"), "irrelevant").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut e = Env::new();
    e.clear("HELDAR_SECRET_KEY");
    e.clear("HELDAR_SECRET_KEY_FILE");
    e.set("CREDENTIALS_DIRECTORY", dir.to_str().unwrap());

    let got = heldar_kernel::services::secret_source::resolve("HELDAR_SECRET_KEY");
    // Restore before asserting, so a failure does not leave an unreadable directory behind.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        got.is_err(),
        "an unreadable credentials directory must be an error, not silent plaintext: {got:?}"
    );
}

/// A Notepad-authored secret file starts with a UTF-8 BOM. It is invisible and is not the secret.
#[test]
fn a_byte_order_mark_is_not_part_of_the_secret() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("bom");
    let p = dir.join("pw");
    std::fs::write(&p, "\u{feff}pa55w0rd\n").unwrap();
    let mut e = Env::new();
    e.clear("HELDAR_SMTP_PASSWORD");
    e.set("HELDAR_SMTP_PASSWORD_FILE", p.to_str().unwrap());
    assert_eq!(
        heldar_kernel::config::Config::from_env()
            .smtp_password
            .as_deref(),
        Some("pa55w0rd"),
        "a BOM must not become part of the password"
    );
}

/// `Config` lives in an `Arc` for the process lifetime and is handed to every service. `?cfg` is the
/// idiomatic `tracing` shape in this codebase, so a derived `Debug` put the master key in any log
/// line that printed it — and `Resolved`'s careful redaction protected the value for exactly the
/// three statements before it was copied in here.
#[test]
fn the_config_never_prints_a_secret() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut e = Env::new();
    let key = fixture_key(b'd');
    e.set("HELDAR_SECRET_KEY", &key);
    e.set("HELDAR_SMTP_PASSWORD", "smtp-topsecret");
    e.set("HELDAR_BOOTSTRAP_ADMIN_PASSWORD", "bootstrap-topsecret");

    let cfg = heldar_kernel::config::Config::from_env();
    for rendered in [format!("{cfg:?}"), format!("{cfg:#?}")] {
        for secret in [key.as_str(), "smtp-topsecret", "bootstrap-topsecret"] {
            assert!(
                !rendered.contains(secret),
                "Config's Debug leaked {secret:?}:\n{rendered}"
            );
        }
        assert!(
            rendered.contains("<set>"),
            "it should still say WHETHER a secret is set — that half is useful and discloses \
             nothing:\n{rendered}"
        );
    }
}

/// The control-plane mTLS key is a PATH, not a secret, and must NOT go through the chain.
///
/// `_FILE` resolution would substitute the key's CONTENTS where a filename is expected, and
/// `fleet_register` interpolates that filename into an error it logs at ERROR level — so wiring it
/// would have made this branch print a PEM private key on an operator's first attempt to follow its
/// own documentation.
#[test]
fn the_mtls_key_path_is_not_treated_as_a_secret() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("mtls");
    let pem = dir.join("client.key.pem");
    let key_pem = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
    std::fs::write(&pem, key_pem).unwrap();

    let mut e = Env::new();
    // mTLS is all-or-nothing: without the full set `cp_tls` is None and this test would assert
    // against an empty Option, which is how the FIRST version of it passed while the bug was live.
    e.set(
        "HELDAR_CP_TLS_CLIENT_CERT",
        dir.join("cert.pem").to_str().unwrap(),
    );
    e.set("HELDAR_CP_TLS_CA", dir.join("ca.pem").to_str().unwrap());
    e.set("HELDAR_CP_TLS_CLIENT_KEY_FILE", pem.to_str().unwrap());
    e.clear("HELDAR_CP_TLS_CLIENT_KEY");

    let cfg = heldar_kernel::config::Config::from_env();

    // THE INVARIANT: the key's CONTENTS must appear nowhere in the resolved config.
    //
    // With the bug, `_FILE` resolution put the PEM into `client_key` — a `PathBuf` — and
    // `fleet_register` interpolates that into an error it logs at ERROR level, so a private key
    // reached the log of the very branch meant to keep secrets out of logs.
    //
    // With the fix, `_FILE` alone leaves the variable unset, which makes the mTLS set partial and
    // turns mTLS off with a warning. That is the right outcome: a path is not something to
    // reconstruct from a secret source, and the existing design already degrades on a partial set
    // rather than guessing.
    let rendered = format!("{:?}", cfg.cp_tls.as_ref().map(|t| t.client_key.clone()));
    assert!(
        !rendered.contains("BEGIN PRIVATE KEY"),
        "the mTLS key's CONTENTS landed where a PATH belongs: {rendered}"
    );
    assert!(
        cfg.cp_tls.is_none(),
        "with only `_FILE` set the mTLS config is partial and must be disabled, not assembled from \
         a value of the wrong kind: {rendered}"
    );

    // The control: with a real PATH in the variable, mTLS configures normally.
    e.set("HELDAR_CP_TLS_CLIENT_KEY", pem.to_str().unwrap());
    let cfg = heldar_kernel::config::Config::from_env();
    let cp = cfg.cp_tls.as_ref().expect(
        "a full set of PATHS must configure mTLS — without this the assertions above pass \
                 for the trivial reason that nothing is configured at all",
    );
    assert_eq!(cp.client_key, pem, "the path is used verbatim");
    assert!(!cp
        .client_key
        .display()
        .to_string()
        .contains("BEGIN PRIVATE KEY"));
}
