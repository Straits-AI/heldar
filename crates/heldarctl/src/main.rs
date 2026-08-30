//! `heldarctl` — the supported operator and automation interface for a Heldar box (#122).
//!
//! Read-only and diagnostic commands first, which is what the issue asks: a mutation needs its
//! idempotency and dry-run behaviour defined before it ships, and shipping one without that is how
//! an automation replays a destructive call.
//!
//! # Exit codes, because a script has to branch on something
//!
//! ```text
//! 0  success
//! 1  invalid input or usage
//! 2  authentication failed
//! 3  the server could not be reached
//! 4  contract incompatibility — this CLI's answers would be unreliable
//! 5  findings present at a blocking severity
//! 6  the server returned an error
//! ```
//!
//! `5` is separate from `6` on purpose: `doctor` finding a broken camera is not the same event as
//! `doctor` failing to run, and a CI job wants to treat them differently.
//!
//! # It never prints a secret
//!
//! Not a token, not a camera password, not a signed media URL, not an RTSP URL with userinfo in it.
//! CLI output gets pasted into tickets and chat far more readily than a server log does.

mod context;
mod doctor;
mod output;

use anyhow::{bail, Context as _, Result};

/// The contract version this CLI was generated against.
const CLI_CONTRACT: &str = "0.1.0";

/// Exit codes. Documented above and in `--help`.
mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 1;
    pub const AUTH: i32 = 2;
    pub const UNREACHABLE: i32 = 3;
    pub const INCOMPATIBLE: i32 = 4;
    pub const FINDINGS: i32 = 5;
    pub const SERVER: i32 = 6;
}

fn main() {
    let code = match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("heldarctl: {e:#}");
            exit::USAGE
        }
    };
    std::process::exit(code);
}

#[tokio::main(flavor = "current_thread")]
async fn run() -> Result<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--output=json" || a == "--json");
    let ctx_name = flag(&args, "--context");

    match args.first().map(String::as_str) {
        None | Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(exit::OK)
        }
        Some("version") => {
            let v = serde_json::json!({
                "heldarctl": env!("CARGO_PKG_VERSION"),
                "contract": CLI_CONTRACT,
            });
            output::emit(&v, json, |v| {
                format!(
                    "heldarctl {}  (API contract {})",
                    v["heldarctl"].as_str().unwrap_or("?"),
                    v["contract"].as_str().unwrap_or("?")
                )
            });
            Ok(exit::OK)
        }
        Some("context") => context_cmd(&args[1..], json),
        Some("status") => status_cmd(ctx_name.as_deref(), json).await,
        Some("doctor") => doctor_cmd(ctx_name.as_deref(), json).await,
        Some(other) => {
            eprintln!("heldarctl: unknown command {other:?} — try `heldarctl help`");
            Ok(exit::USAGE)
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    if let Some(i) = args.iter().position(|a| a == name) {
        return args.get(i + 1).cloned();
    }
    args.iter()
        .find_map(|a| a.strip_prefix(&format!("{name}=")).map(str::to_string))
}

fn context_cmd(args: &[String], json: bool) -> Result<i32> {
    let mut cfg = context::load()?;
    match args.first().map(String::as_str) {
        Some("list") | None => {
            let rows: Vec<serde_json::Value> = cfg
                .contexts
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "base_url": c.base_url,
                        "token": c.token.describe(),
                        "current": cfg.current.as_deref() == Some(c.name.as_str()),
                    })
                })
                .collect();
            let v = serde_json::json!({ "contexts": rows });
            output::emit(&v, json, |v| {
                let mut s = String::new();
                for c in v["contexts"].as_array().unwrap_or(&vec![]) {
                    s.push_str(&format!(
                        "{} {}  {}  token={}\n",
                        if c["current"].as_bool() == Some(true) {
                            "*"
                        } else {
                            " "
                        },
                        c["name"].as_str().unwrap_or(""),
                        c["base_url"].as_str().unwrap_or(""),
                        c["token"].as_str().unwrap_or(""),
                    ));
                }
                if s.is_empty() {
                    s.push_str("no contexts — add one with `heldarctl context add`\n");
                }
                s.trim_end().to_string()
            });
            Ok(exit::OK)
        }
        Some("add") => {
            let name = flag(args, "--name").context("--name is required")?;
            let url = flag(args, "--url").context("--url is required")?;
            // The token SOURCE, never the token — so it cannot land in shell history.
            let token = match (flag(args, "--token-env"), flag(args, "--token-file")) {
                (Some(n), None) => context::TokenSource::Env { name: n },
                (None, Some(p)) => context::TokenSource::File { path: p.into() },
                (None, None) => context::TokenSource::Stdin,
                (Some(_), Some(_)) => {
                    bail!("give --token-env or --token-file, not both")
                }
            };
            cfg.contexts.retain(|c| c.name != name);
            cfg.contexts.push(context::Context {
                name: name.clone(),
                base_url: url.trim_end_matches('/').to_string(),
                token,
                ca_path: flag(args, "--ca").map(Into::into),
            });
            if cfg.current.is_none() {
                cfg.current = Some(name.clone());
            }
            context::save(&cfg)?;
            println!("context {name} added");
            Ok(exit::OK)
        }
        Some("use") => {
            let name = args.get(1).context("usage: heldarctl context use <name>")?;
            if cfg.get(name).is_none() {
                bail!("no context named {name:?}");
            }
            cfg.current = Some(name.clone());
            context::save(&cfg)?;
            println!("using context {name}");
            Ok(exit::OK)
        }
        Some("remove") => {
            let name = args
                .get(1)
                .context("usage: heldarctl context remove <name>")?;
            let before = cfg.contexts.len();
            cfg.contexts.retain(|c| &c.name != name);
            if cfg.contexts.len() == before {
                bail!("no context named {name:?}");
            }
            if cfg.current.as_deref() == Some(name.as_str()) {
                cfg.current = None;
            }
            context::save(&cfg)?;
            println!("context {name} removed");
            Ok(exit::OK)
        }
        Some(other) => {
            eprintln!("heldarctl context: unknown subcommand {other:?}");
            Ok(exit::USAGE)
        }
    }
}

/// A configured HTTP client for a context.
async fn client(ctx: &context::Context) -> Result<(reqwest::Client, String)> {
    let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20));
    if let Some(ca) = &ctx.ca_path {
        let pem = std::fs::read(ca).with_context(|| format!("reading {}", ca.display()))?;
        b = b.add_root_certificate(
            reqwest::Certificate::from_pem(&pem)
                .context("the --ca file is not a PEM certificate")?,
        );
    }
    Ok((b.build()?, ctx.token.resolve()?))
}

/// GET a path, mapping transport and status failures onto the documented exit codes.
async fn get(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    path: &str,
) -> Result<std::result::Result<serde_json::Value, i32>> {
    let resp = match http
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("heldarctl: cannot reach {base}: {e}");
            return Ok(Err(exit::UNREACHABLE));
        }
    };
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // The correlation id is what a support engineer needs to find this in the box's logs.
        let rid = resp
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        eprintln!("heldarctl: {status} on {path} (request id {rid})");
        return Ok(Err(exit::AUTH));
    }
    if !status.is_success() {
        let rid = resp
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        let body = resp.text().await.unwrap_or_default();
        eprintln!(
            "heldarctl: {status} on {path} (request id {rid}): {}",
            body.trim()
        );
        return Ok(Err(exit::SERVER));
    }
    Ok(Ok(resp
        .json()
        .await
        .context("the server's reply is not JSON")?))
}

async fn status_cmd(ctx_name: Option<&str>, json: bool) -> Result<i32> {
    let cfg = context::load()?;
    let ctx = cfg.select(ctx_name)?;
    let (http, token) = client(ctx).await?;
    let system = match get(&http, &ctx.base_url, &token, "/api/v1/system").await? {
        Ok(v) => v,
        Err(code) => return Ok(code),
    };
    output::emit(&system, json, |v| {
        format!(
            "{} {}  (API contract {})\n  uptime {}s   cameras {} total, {} recording",
            v["name"].as_str().unwrap_or("Heldar"),
            v["version"].as_str().unwrap_or("?"),
            v["api_version"].as_str().unwrap_or("?"),
            v["uptime_seconds"].as_i64().unwrap_or(0),
            v["cameras_total"].as_i64().unwrap_or(0),
            v["cameras_recording"].as_i64().unwrap_or(0),
        )
    });
    Ok(exit::OK)
}

async fn doctor_cmd(ctx_name: Option<&str>, json: bool) -> Result<i32> {
    let cfg = context::load()?;
    let ctx = cfg.select(ctx_name)?;
    let (http, token) = client(ctx).await?;
    let base = &ctx.base_url;

    let system = match get(&http, base, &token, "/api/v1/system").await? {
        Ok(v) => v,
        Err(code) => return Ok(code),
    };
    let mut findings = vec![doctor::compatibility(
        CLI_CONTRACT,
        system["api_version"].as_str(),
    )];
    // A major mismatch stops here rather than reporting more: every finding below is derived from
    // shapes this CLI may be reading wrong, and a confident wrong diagnosis is worse than none.
    if findings.iter().any(|f| f.code == "compat.major_mismatch") {
        emit_findings(&findings, json);
        return Ok(exit::INCOMPATIBLE);
    }

    // The box's own posture and health, not a second implementation of either.
    if let Ok(p) = get(&http, base, &token, "/api/v1/system/posture").await? {
        findings.extend(doctor::from_posture(&p));
    }
    let cameras = get(&http, base, &token, "/api/v1/cameras").await?.ok();
    let health = get(&http, base, &token, "/api/v1/health/cameras")
        .await?
        .ok();
    if let (Some(c), Some(h)) = (cameras, health) {
        findings.extend(doctor::camera_health(&c, &h));
    }

    let blocking = doctor::blocks(&findings);
    emit_findings(&findings, json);
    Ok(if blocking { exit::FINDINGS } else { exit::OK })
}

fn emit_findings(findings: &[doctor::Finding], json: bool) {
    let v = serde_json::json!({
        "findings": findings,
        "blocking": findings.iter().filter(|f| f.severity == doctor::Severity::Blocking).count(),
    });
    output::emit(&v, json, |v| {
        let mut s = String::new();
        for f in v["findings"].as_array().unwrap_or(&vec![]) {
            let sev = f["severity"].as_str().unwrap_or("info");
            if sev == "info" {
                continue;
            }
            s.push_str(&format!(
                "{:<8} {:<28} {}\n         -> {}\n",
                sev.to_uppercase(),
                f["code"].as_str().unwrap_or(""),
                f["detail"].as_str().unwrap_or(""),
                f["remediation"].as_str().unwrap_or(""),
            ));
        }
        if s.is_empty() {
            s.push_str("no warnings or blocking findings\n");
        }
        s.trim_end().to_string()
    });
}

fn print_help() {
    println!(
        "heldarctl — the supported operator interface for a Heldar box\n\
         \n\
         Usage:\n  \
           heldarctl version\n  \
           heldarctl status                       what this box is and whether it is recording\n  \
           heldarctl doctor                       what is wrong with it\n  \
           heldarctl context add --name N --url U [--token-env VAR | --token-file PATH] [--ca PEM]\n  \
           heldarctl context list|use <name>|remove <name>\n\
         \n\
         Options:\n  \
           --context <name>   use a named context instead of the current one\n  \
           --output=json      stable machine-readable output (alias: --json)\n\
         \n\
         Exit codes:\n  \
           0 success   1 usage   2 auth   3 unreachable\n  \
           4 contract incompatible   5 blocking findings   6 server error\n\
         \n\
         Read-only and diagnostic commands only for now: a mutation needs its idempotency and\n\
         dry-run behaviour defined before it ships."
    );
}
