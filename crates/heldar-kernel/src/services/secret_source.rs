//! Where a secret comes from (#126).
//!
//! Every deployment secret — the encryption-at-rest master key, the bootstrap admin password, the
//! control-plane client key, the SMTP password — was read from one place: an environment variable.
//! That is fine for a sealed single-operator appliance and weak everywhere else. An environment
//! variable is visible in `/proc/<pid>/environ` to anyone who can read it, is inherited by every
//! child process the box spawns (including ffmpeg), and lands in `docker inspect` output, shell
//! history and crash dumps.
//!
//! # The shape
//!
//! A secret is named once and resolved from the first source that has it:
//!
//! 1. `NAME` — the environment variable, unchanged, so every existing deployment keeps working
//! 2. `NAME_FILE` — a path whose contents are the secret (Docker/Compose/Kubernetes secrets, and
//!    the shape most orchestrators already produce)
//! 3. `$CREDENTIALS_DIRECTORY/<name>` — a systemd credential, where the unit file names it with
//!    `LoadCredential=` and systemd hands it over at 0400 in a tmpfs the service alone can read
//!
//! There is deliberately no HTTP provider here. The issue asks for one and this module is the seam
//! it plugs into, but a recorder must not become synchronously dependent on a remote service after
//! boot: a network blip during a reconnect would stop recording. Resolution happens once, at
//! startup, into process memory — a sidecar that writes to a path is already supported by (2), and
//! that is the integration shape to document rather than a client this crate maintains.
//!
//! # What this does not do
//!
//! It does not make a secret unreadable by root, and nothing at this layer can. It narrows who can
//! read one from "anyone who can list a process's environment" to "whoever can read a file the
//! service user owns", which is the difference between a shared Docker host and a sealed one.
//!
//! Values are never logged, and [`Resolved::source`] exists so an operator can be told WHERE a
//! secret came from without being told what it is.

use std::path::PathBuf;

use serde::Serialize;

/// Where a resolved secret was found. Reportable; never carries the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretSource {
    /// The environment variable itself. Readable from `/proc/<pid>/environ` and inherited by
    /// children — the weakest of the three, and the default only for compatibility.
    Env,
    /// A file named by `<NAME>_FILE`.
    File,
    /// A systemd credential under `$CREDENTIALS_DIRECTORY`.
    SystemdCredential,
    /// Nothing supplied it.
    Unset,
}

impl SecretSource {
    /// Whether this source keeps the value out of the process environment.
    ///
    /// Used by the production preflight to say plainly which secrets are still exposed, rather than
    /// reporting a single "secrets configured" boolean that hides the difference.
    pub fn is_hardened(self) -> bool {
        matches!(self, Self::File | Self::SystemdCredential)
    }
}

/// A resolved secret and where it came from.
///
/// `Debug` is implemented by hand: deriving it would put the value in every `{:?}` — a panic
/// message, a `tracing` field, an `anyhow` chain — which is exactly how a master key ends up in a
/// log file nobody meant to write.
pub struct Resolved {
    value: String,
    pub source: SecretSource,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("value", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

impl Resolved {
    /// The secret itself. Named so a reader has to mean it.
    pub fn expose(&self) -> &str {
        &self.value
    }
}

/// Resolve `name` from the environment, a `<name>_FILE` path, or a systemd credential.
///
/// Returns `None` when no source supplies it — an unset secret is a normal state (encryption at
/// rest is optional), not an error.
///
/// A file that is named but unreadable IS an error: an operator who set `HELDAR_SECRET_KEY_FILE`
/// asked for encryption, and silently falling through to "no key" would store credentials in
/// plaintext while the deployment believed otherwise. Failing closed on a stated intent is the whole
/// point of stating it.
pub fn resolve(name: &str) -> anyhow::Result<Option<Resolved>> {
    if let Some(v) = std::env::var(name).ok().filter(|v| !v.trim().is_empty()) {
        // VERBATIM. `env::var()` has always returned the value unchanged — it only *filters* on
        // trim — so trimming here would silently change the secret an existing box has been using.
        // A password with a trailing space stops authenticating; a bootstrap password fails a
        // length gate it used to pass. The env branch must be byte-identical to the old behaviour.
        return Ok(Some(Resolved {
            value: v,
            source: SecretSource::Env,
        }));
    }

    let file_var = format!("{name}_FILE");
    if let Some(path) = std::env::var(&file_var)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        let value = read_secret_file(PathBuf::from(path.trim()))
            .map_err(|e| anyhow::anyhow!("{file_var} is set but unusable: {e}"))?;
        return Ok(Some(Resolved {
            value,
            source: SecretSource::File,
        }));
    }

    if let Some(dir) = std::env::var("CREDENTIALS_DIRECTORY")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        let path = PathBuf::from(dir.trim()).join(name);
        // `exists()` is FALSE for a permission error, so gating on it made this branch fail OPEN:
        // an unreadable credentials directory booted the box with no key and stored camera
        // credentials in plaintext — the outcome the `_FILE` branch panics to prevent, reached
        // silently one source over. Distinguish "no such credential" (fine: a unit may load two of
        // five) from "there but unreadable" (fatal).
        match std::fs::metadata(&path) {
            Ok(_) => {
                let value = read_secret_file(path.clone()).map_err(|e| {
                    anyhow::anyhow!("systemd credential {} is unusable: {e}", path.display())
                })?;
                return Ok(Some(Resolved {
                    value,
                    source: SecretSource::SystemdCredential,
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                anyhow::bail!("cannot stat systemd credential {}: {e}", path.display());
            }
        }
    }

    Ok(None)
}

/// Read a secret from a file, trimming the trailing newline every editor and `echo >` adds.
///
/// An empty file is an error rather than an empty secret. `echo -n "" > key` and a failed `docker
/// secret` mount look identical on disk, and treating either as "the operator chose no key" is how a
/// box silently downgrades to plaintext.
fn read_secret_file(path: PathBuf) -> anyhow::Result<String> {
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    // A Notepad-authored file starts with a UTF-8 BOM. It is invisible, it is not part of the
    // secret, and for a base64 key it produces a decode error three layers away from the cause.
    let trimmed = raw.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    Ok(trimmed.to_string())
}

/// Resolve, reporting only the SOURCE at info level. Never the value, and never its length —
/// a length is a meaningful hint about a key.
pub fn resolve_and_report(name: &str) -> anyhow::Result<Option<Resolved>> {
    let got = resolve(name)?;
    match &got {
        Some(r) => tracing::info!(
            target: "heldar::security",
            secret = name,
            source = ?r.source,
            hardened = r.source.is_hardened(),
            "secret resolved"
        ),
        None => tracing::debug!(target: "heldar::security", secret = name, "secret not configured"),
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The environment is process-global, so these run under one lock rather than in parallel.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Env(Vec<String>);
    impl Env {
        fn new() -> Self {
            Env(Vec::new())
        }
        fn set(&mut self, k: &str, v: &str) -> &mut Self {
            std::env::set_var(k, v);
            self.0.push(k.to_string());
            self
        }
    }
    impl Drop for Env {
        fn drop(&mut self) {
            for k in &self.0 {
                std::env::remove_var(k);
            }
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("heldar-secret-src-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_unset_secret_is_none_not_an_error() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Env::new();
        e.set("HELDAR_TEST_A", "");
        assert!(resolve("HELDAR_TEST_A").unwrap().is_none());
        assert!(resolve("HELDAR_TEST_NEVER_SET").unwrap().is_none());
    }

    #[test]
    fn the_environment_still_works_unchanged() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Env::new();
        e.set("HELDAR_TEST_B", "  s3cret  ");
        let r = resolve("HELDAR_TEST_B").unwrap().expect("resolved");
        assert_eq!(
            r.expose(),
            "  s3cret  ",
            "an environment value must arrive VERBATIM — `env::var()` only ever FILTERED on trim and \
             returned the value unchanged, so reshaping it here silently changes the secret an \
             existing box has been using"
        );
        assert_eq!(r.source, SecretSource::Env);
        assert!(!r.source.is_hardened(), "an env var is the exposed source");
    }

    #[test]
    fn a_file_supplies_the_secret_and_the_trailing_newline_is_not_part_of_it() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp("file");
        let p = dir.join("key");
        // Every editor and `echo >` adds this; a key that silently includes it decrypts nothing.
        std::fs::write(&p, "s3cret\n").unwrap();
        let mut e = Env::new();
        e.set("HELDAR_TEST_C_FILE", p.to_str().unwrap());
        let r = resolve("HELDAR_TEST_C").unwrap().expect("resolved");
        assert_eq!(r.expose(), "s3cret");
        assert_eq!(r.source, SecretSource::File);
        assert!(r.source.is_hardened());
    }

    #[test]
    fn the_environment_wins_over_a_file_so_an_upgrade_cannot_change_behaviour() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp("both");
        let p = dir.join("key");
        std::fs::write(&p, "from-file").unwrap();
        let mut e = Env::new();
        e.set("HELDAR_TEST_D", "from-env");
        e.set("HELDAR_TEST_D_FILE", p.to_str().unwrap());
        let r = resolve("HELDAR_TEST_D").unwrap().expect("resolved");
        assert_eq!(
            r.expose(),
            "from-env",
            "a box that already sets the variable must keep the value it has been using; a new \
             source silently taking precedence would rotate the key on upgrade day"
        );
    }

    /// A stated intent that cannot be honoured is an error, never a silent fall-through.
    #[test]
    fn a_named_but_unusable_file_fails_closed() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp("bad");
        let mut e = Env::new();

        e.set("HELDAR_TEST_E_FILE", dir.join("absent").to_str().unwrap());
        let err =
            resolve("HELDAR_TEST_E").expect_err("a missing file must not read as 'no secret'");
        assert!(format!("{err:#}").contains("unusable"), "{err:#}");

        let empty = dir.join("empty");
        std::fs::write(&empty, "   \n").unwrap();
        e.set("HELDAR_TEST_F_FILE", empty.to_str().unwrap());
        let err = resolve("HELDAR_TEST_F").expect_err("an empty file is not a choice to use none");
        assert!(format!("{err:#}").contains("empty"), "{err:#}");
    }

    #[test]
    fn a_systemd_credential_is_found_by_name() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp("systemd");
        std::fs::write(dir.join("HELDAR_TEST_G"), "from-systemd").unwrap();
        let mut e = Env::new();
        e.set("CREDENTIALS_DIRECTORY", dir.to_str().unwrap());
        let r = resolve("HELDAR_TEST_G").unwrap().expect("resolved");
        assert_eq!(r.expose(), "from-systemd");
        assert_eq!(r.source, SecretSource::SystemdCredential);
        assert!(r.source.is_hardened());

        // A credentials directory that does not hold this name is not an error — the box simply has
        // no such secret, which is how a unit that loads two of five credentials behaves.
        assert!(resolve("HELDAR_TEST_H").unwrap().is_none());
    }

    /// The whole point of the wrapper. A secret that reaches a log through `{:?}` is a secret in a
    /// log, and every panic message and `tracing` field goes through `Debug`.
    #[test]
    fn debug_never_prints_the_value() {
        let r = Resolved {
            value: "TOPSECRET".into(),
            source: SecretSource::File,
        };
        let printed = format!("{r:?}");
        assert!(!printed.contains("TOPSECRET"), "{printed}");
        assert!(printed.contains("redacted"), "{printed}");
        assert!(
            printed.contains("File"),
            "the SOURCE is safe and useful: {printed}"
        );
    }
}
