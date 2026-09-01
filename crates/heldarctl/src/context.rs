//! Named deployments, and where their credentials come from (#122).
//!
//! An operator with three boxes should not be pasting a base URL and a bearer token into every
//! command. A context names them once.
//!
//! # Secrets are not stored here
//!
//! A context records WHERE the token comes from, never the token. The issue's requirement is
//! "contexts support multiple deployments without putting secrets in shell history", and a config
//! file full of bearer tokens satisfies the letter of that while being worse: shell history is at
//! least ephemeral, and a file gets committed, backed up and copied to a laptop.
//!
//! So the token is resolved at use time from an environment variable, a file, or stdin. The same
//! three shapes the server itself uses for secrets (#126), for the same reasons.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};

/// Where a context's bearer token comes from. Never the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TokenSource {
    /// Read `$name` at use time.
    Env { name: String },
    /// Read this file at use time. Contents are the token.
    File { path: PathBuf },
    /// Read one line from stdin. For piping out of a password manager.
    Stdin,
}

impl TokenSource {
    /// Resolve to an actual token. Errors name the SOURCE, never the value.
    pub fn resolve(&self) -> Result<String> {
        let raw = match self {
            Self::Env { name } => {
                std::env::var(name).with_context(|| format!("${name} is not set"))?
            }
            Self::File { path } => std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?,
            Self::Stdin => {
                let mut s = String::new();
                std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut s)
                    .context("reading the token from stdin")?;
                s
            }
        };
        let token = raw.trim();
        if token.is_empty() {
            bail!("the token source resolved to an empty value");
        }
        Ok(token.to_string())
    }

    /// How this reads in `context list`. Safe to print.
    pub fn describe(&self) -> String {
        match self {
            Self::Env { name } => format!("env:{name}"),
            Self::File { path } => format!("file:{}", path.display()),
            Self::Stdin => "stdin".to_string(),
        }
    }
}

/// One named deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub name: String,
    pub base_url: String,
    pub token: TokenSource,
    /// PEM of a CA to trust, for a box with a private certificate authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_path: Option<PathBuf>,
}

/// Every context, and which one is current.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub contexts: Vec<Context>,
}

impl Config {
    pub fn get(&self, name: &str) -> Option<&Context> {
        self.contexts.iter().find(|c| c.name == name)
    }

    /// The context a command should use: `--context`, else the current one.
    pub fn select(&self, requested: Option<&str>) -> Result<&Context> {
        match requested {
            Some(n) => self
                .get(n)
                .with_context(|| format!("no context named {n:?} — try `heldarctl context list`")),
            None => {
                let cur = self.current.as_deref().context(
                    "no context selected — add one with `heldarctl context add`, or pass --context",
                )?;
                self.get(cur).with_context(|| {
                    format!(
                        "the current context {cur:?} no longer exists — pick another with \
                             `heldarctl context use`"
                    )
                })
            }
        }
    }
}

/// `$HELDARCTL_CONFIG`, else `$XDG_CONFIG_HOME/heldar/contexts.json`, else `~/.config/...`.
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("HELDARCTL_CONFIG") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| Path::new(&h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("heldar").join("contexts.json")
}

pub fn load() -> Result<Config> {
    let p = config_path();
    match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s)
            .with_context(|| format!("{} is not valid heldarctl config", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let body = serde_json::to_string_pretty(cfg)?;
    write_config(&p, &body)
}

/// 0600 even though this holds no secret. It holds the SHAPE of a deployment — base URLs, which
/// boxes exist, where their tokens live — and that is reconnaissance worth not leaving readable.
///
/// Two whole definitions rather than `#[cfg]` blocks inside one: a `#[cfg]` on a BLOCK makes it a
/// statement, which compiles to `()` on the other platform where the author cannot see it.
#[cfg(unix)]
fn write_config(p: &Path, body: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(p)
        .with_context(|| format!("writing {}", p.display()))?;
    f.write_all(body.as_bytes())?;
    Ok(())
}

#[cfg(not(unix))]
fn write_config(p: &Path, body: &str) -> Result<()> {
    std::fs::write(p, body).with_context(|| format!("writing {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_never_serializes_a_token() {
        let c = Context {
            name: "site-a".into(),
            base_url: "https://box.local:8000".into(),
            token: TokenSource::Env {
                name: "HELDAR_TOKEN".into(),
            },
            ca_path: None,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("HELDAR_TOKEN"), "the NAME is fine: {json}");
        assert!(
            !json.contains("vok_"),
            "a context records where a token comes from, never the token: {json}"
        );
        // The whole struct has nowhere to put one.
        assert!(!json.contains("\"token\":\""), "{json}");
    }

    #[test]
    fn a_token_source_resolves_and_trims() {
        std::env::set_var("HELDARCTL_TEST_TOKEN", "  vok_abc  \n");
        let t = TokenSource::Env {
            name: "HELDARCTL_TEST_TOKEN".into(),
        };
        assert_eq!(t.resolve().unwrap(), "vok_abc");
        std::env::remove_var("HELDARCTL_TEST_TOKEN");
        assert!(
            t.resolve().is_err(),
            "an unset source must be an error, not an empty token"
        );
    }

    /// An error about a token must name the source, never the value — CLI errors get pasted into
    /// tickets and chat far more readily than server logs do.
    #[test]
    fn a_resolution_error_does_not_leak_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, "   \n").unwrap();
        let err = TokenSource::File { path: p.clone() }
            .resolve()
            .expect_err("an empty file is not a token");
        let msg = format!("{err:#}");
        assert!(msg.contains("empty"), "{msg}");
    }

    #[test]
    fn selecting_a_context_explains_itself_when_it_cannot() {
        let mut cfg = Config::default();
        let err = format!("{:#}", cfg.select(None).unwrap_err());
        assert!(err.contains("context add"), "must say how to fix it: {err}");

        cfg.current = Some("gone".into());
        let err = format!("{:#}", cfg.select(None).unwrap_err());
        assert!(
            err.contains("no longer exists"),
            "a dangling current context is a different problem from having none: {err}"
        );

        cfg.contexts.push(Context {
            name: "a".into(),
            base_url: "http://x".into(),
            token: TokenSource::Stdin,
            ca_path: None,
        });
        cfg.current = Some("a".into());
        assert_eq!(cfg.select(None).unwrap().name, "a");
        assert_eq!(cfg.select(Some("a")).unwrap().name, "a");
        assert!(cfg.select(Some("nope")).is_err());
    }
}
