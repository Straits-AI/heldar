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
        Some("retention") => retention_cmd(&args[1..], ctx_name.as_deref(), json).await,
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

/// `PUT` with the same status handling as [`get`], plus the two headers a mutation needs.
///
/// A `409` is called out separately because for a planned mutation it is not a generic failure: it
/// is the box saying the plan is out of date, and the server's own message says what to do about it.
/// Collapsing it into `SERVER` would send an operator to the logs for something the answer already
/// explains.
async fn put(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    path: &str,
    body: &serde_json::Value,
    idempotency_key: Option<&str>,
) -> Result<std::result::Result<serde_json::Value, i32>> {
    let mut req = http
        .put(format!("{base}{path}"))
        .bearer_auth(token)
        .json(body);
    if let Some(k) = idempotency_key {
        req = req.header("idempotency-key", k);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("heldarctl: cannot reach {base}: {e}");
            return Ok(Err(exit::UNREACHABLE));
        }
    };
    let status = resp.status();
    let rid = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        eprintln!("heldarctl: {status} on {path} (request id {rid})");
        return Ok(Err(exit::AUTH));
    }
    if status == reqwest::StatusCode::CONFLICT {
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| body.trim().to_string());
        eprintln!("heldarctl: the box refused this change (request id {rid}):\n  {msg}");
        return Ok(Err(exit::SERVER));
    }
    if !status.is_success() {
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

/// `retention` — show the recording disk limits, or change them.
///
/// # A mutation is a dry run until you say otherwise
///
/// `set` without `--yes` plans and prints; it changes nothing. That is the same way round as the
/// evidence export's `dry_run` default: the destructive direction is the one you have to ask for.
///
/// With `--yes` it still plans first, prints the same effect, and then commits **carrying the plan
/// hash it just received**. That is what makes the printed effect meaningful rather than decorative:
/// if anything the plan depended on moved in between — another operator changed the cap, the
/// recorded footprint grew past it — the box refuses the commit instead of applying a change to a
/// state nobody looked at.
///
/// Shrinking the cap below what is already recorded deletes the oldest footage FLEET-WIDE on the
/// next sweep, so the effect is printed even in the `--yes` path. An operator who typed the wrong
/// number should see it in the terminal, not discover it from a retention sweep.
async fn retention_cmd(args: &[String], ctx_name: Option<&str>, json: bool) -> Result<i32> {
    // Global flags are parsed by `run` and left in place, so a positional subcommand match sees
    // them. `heldarctl retention --output=json` reported "unknown retention command" until this —
    // found by running it, not by reading it.
    let args: Vec<String> = args
        .iter()
        .filter(|a| !matches!(a.as_str(), "--output=json" | "--json"))
        .cloned()
        .collect();
    let args = strip_flag_pair(&args, "--context");
    let cfg = context::load()?;
    let ctx = cfg.select(ctx_name)?;
    let (http, token) = client(ctx).await?;
    let base = &ctx.base_url;

    match args.first().map(String::as_str) {
        None | Some("show") => {
            let v = match get(&http, base, &token, "/api/v1/system/retention").await? {
                Ok(v) => v,
                Err(code) => return Ok(code),
            };
            output::emit(&v, json, |v| {
                format!(
                    "recordings cap {:.1} GB{}\n  free-disk floor {:.1} GB{}",
                    v["max_recordings_gb"].as_f64().unwrap_or(0.0),
                    if v["max_overridden"].as_bool().unwrap_or(false) {
                        "  (set at runtime)"
                    } else {
                        "  (from the environment)"
                    },
                    v["min_free_disk_gb"].as_f64().unwrap_or(0.0),
                    if v["min_free_overridden"].as_bool().unwrap_or(false) {
                        "  (set at runtime)"
                    } else {
                        "  (from the environment)"
                    },
                )
            });
            Ok(exit::OK)
        }
        Some("set") => retention_set(&http, base, &token, &args[1..], json).await,
        // (`args` is the filtered copy above, so `--output=json` never reaches this match.)
        Some(other) => {
            eprintln!("heldarctl: unknown retention command {other:?}; try `show` or `set`");
            Ok(exit::USAGE)
        }
    }
}

/// Bytes as the GB the server means: `routes::system::BYTES_PER_GB` is 1024³, so dividing by 1e9
/// prints "3.2 GB" for a cap the same command reports as 3.0 GB one line later. A unit mismatch in a
/// number an operator is about to act on is worse than no number.
fn gb(v: &serde_json::Value) -> f64 {
    v.as_f64().unwrap_or(0.0) / (1024.0 * 1024.0 * 1024.0)
}

/// Drop `--flag value` from an argument list, so a positional subcommand match does not see it.
fn strip_flag_pair(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a == flag {
            skip = true;
            continue;
        }
        if let Some(rest) = a.strip_prefix(flag) {
            if rest.starts_with('=') {
                continue;
            }
        }
        out.push(a.clone());
    }
    out
}

async fn retention_set(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    args: &[String],
    json: bool,
) -> Result<i32> {
    let mut max_gb: Option<f64> = None;
    let mut min_free_gb: Option<f64> = None;
    let mut commit = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--yes" | "-y" => commit = true,
            "--max-gb" => max_gb = it.next().and_then(|v| v.parse().ok()),
            "--min-free-gb" => min_free_gb = it.next().and_then(|v| v.parse().ok()),
            other => {
                eprintln!(
                    "heldarctl: unknown option {other:?}; usage: retention set [--max-gb N] \
                     [--min-free-gb N] [--yes]"
                );
                return Ok(exit::USAGE);
            }
        }
    }
    if max_gb.is_none() && min_free_gb.is_none() {
        eprintln!("heldarctl: nothing to set; pass --max-gb and/or --min-free-gb");
        return Ok(exit::USAGE);
    }

    // ALWAYS plan first, including on the --yes path. The hash the plan returns is what the commit
    // carries, so the change that lands is the one that was printed.
    let mut body = serde_json::json!({ "dry_run": true });
    if let Some(v) = max_gb {
        body["max_recordings_gb"] = serde_json::json!(v);
    }
    if let Some(v) = min_free_gb {
        body["min_free_disk_gb"] = serde_json::json!(v);
    }
    let plan = match put(http, base, token, "/api/v1/system/retention", &body, None).await? {
        Ok(v) => v,
        Err(code) => return Ok(code),
    };

    let evict = plan["effect"]["would_evict_bytes"].as_i64().unwrap_or(0);
    let describe = |p: &serde_json::Value| {
        format!(
            "plan {}\n  cap becomes {:.1} GB, {:.1} GB recorded now\n  would evict {:.1} GB \
             ({:.1} GB is evidence-locked and cannot be freed)\n  {}",
            p["plan_hash"].as_str().unwrap_or("?"),
            gb(&p["effect"]["new_cap_bytes"]),
            gb(&p["effect"]["recorded_bytes"]),
            gb(&p["effect"]["would_evict_bytes"]),
            gb(&p["effect"]["evidence_locked_bytes"]),
            p["note"].as_str().unwrap_or(""),
        )
    };

    if !commit {
        output::emit(&plan, json, describe);
        if !json {
            eprintln!(
                "\nNothing changed. Re-run with --yes to apply exactly this plan{}.",
                if evict > 0 {
                    " — it DELETES the oldest footage fleet-wide"
                } else {
                    ""
                }
            );
        }
        return Ok(exit::OK);
    }

    // The effect is printed on the way through even when committing: an operator who typed the wrong
    // number should read it in the terminal rather than learn it from a retention sweep.
    if !json {
        eprintln!("{}", describe(&plan));
    }
    let hash = plan["plan_hash"].as_str().unwrap_or_default().to_string();
    body["dry_run"] = serde_json::json!(false);
    body["plan_hash"] = serde_json::json!(hash);
    // The `Idempotency-Key` is DERIVED FROM THE PLAN HASH, not random (#121).
    //
    // A random key per invocation would protect against almost nothing: the case that matters is an
    // operator whose command timed out and who runs it again, and a fresh key makes that a second
    // distinct request. The plan hash identifies exactly this change against exactly this state, so
    // the re-run carries the SAME key while the box is unchanged — and the box replays the original
    // answer instead of applying the change twice.
    //
    // If the state DID move, the hash differs, so the key differs — and the plan check refuses the
    // commit anyway. The two guards agree by construction rather than by coincidence.
    let key = format!("heldarctl-retention-{hash}");
    let done = match put(
        http,
        base,
        token,
        "/api/v1/system/retention",
        &body,
        Some(&key),
    )
    .await?
    {
        Ok(v) => v,
        Err(code) => return Ok(code),
    };
    output::emit(&done, json, |v| {
        format!(
            "applied: cap {:.1} GB, free-disk floor {:.1} GB",
            v["max_recordings_gb"].as_f64().unwrap_or(0.0),
            v["min_free_disk_gb"].as_f64().unwrap_or(0.0),
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
           heldarctl context list|use <name>|remove <name>\n  \
           heldarctl retention                    the recording disk limits\n  \
           heldarctl retention set [--max-gb N] [--min-free-gb N] [--yes]\n\
         \n\
         Options:\n  \
           --context <name>   use a named context instead of the current one\n  \
           --output=json      stable machine-readable output (alias: --json)\n\
         \n\
         Exit codes:\n  \
           0 success   1 usage   2 auth   3 unreachable\n  \
           4 contract incompatible   5 blocking findings   6 server error\n\
         \n\
         A MUTATION IS A DRY RUN UNTIL YOU SAY OTHERWISE. `retention set` prints what would\n\
         happen and changes nothing; adding --yes commits exactly the plan it printed, and the box\n\
         refuses it if anything moved in between."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's `BYTES_PER_GB` is 1024³. Dividing by 1e9 printed "3.2 GB" for a cap the very
    /// next line of the same command reported as 3.0 GB — a unit mismatch in a number an operator is
    /// about to act on, and the reason this is a named function rather than an inline divide.
    #[test]
    fn gb_uses_the_same_unit_the_server_reports() {
        let three_gib = serde_json::json!(3u64 * 1024 * 1024 * 1024);
        assert!(
            (gb(&three_gib) - 3.0).abs() < 1e-9,
            "got {}",
            gb(&three_gib)
        );
        // A missing or non-numeric field reads as 0 rather than panicking mid-render.
        assert_eq!(gb(&serde_json::Value::Null), 0.0);
        assert_eq!(gb(&serde_json::json!("nope")), 0.0);
    }

    /// Global flags are parsed by `run` and left in the argument list, so a positional subcommand
    /// match sees them: `heldarctl retention --output=json` answered "unknown retention command".
    #[test]
    fn global_flags_do_not_reach_a_positional_subcommand_match() {
        let args: Vec<String> = ["--context", "prod", "show"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            strip_flag_pair(&args, "--context"),
            vec!["show".to_string()]
        );

        // The `--flag=value` spelling too.
        let joined: Vec<String> = ["--context=prod", "set", "--max-gb", "4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            strip_flag_pair(&joined, "--context"),
            vec!["set", "--max-gb", "4"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );

        // A flag that only PREFIXES another must survive: `--contexts` is not `--context`.
        let similar: Vec<String> = ["--contexts", "show"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(strip_flag_pair(&similar, "--context"), similar);

        // Nothing to strip leaves the list alone.
        let plain: Vec<String> = ["set", "--yes"].iter().map(|s| s.to_string()).collect();
        assert_eq!(strip_flag_pair(&plain, "--context"), plain);
    }

    /// The help text is a contract a script branches on, and it now advertises a MUTATION.
    ///
    /// The dry-run-by-default promise is the whole safety story for `retention set`; if the wording
    /// goes, an operator reading `--help` learns that running it applies the change.
    #[test]
    fn the_help_states_that_a_mutation_is_a_dry_run_by_default() {
        // Asserted against this file's own source: `print_help` writes to stdout and capturing it
        // would need plumbing that exists for nothing else. What matters is that the sentence is
        // still there, and that is what this reads.
        let src = include_str!("main.rs");
        assert!(
            src.contains("A MUTATION IS A DRY RUN UNTIL YOU SAY OTHERWISE"),
            "the help no longer states the dry-run default"
        );
        assert!(
            src.contains("retention            show or set the recording disk limits"),
            "the help no longer lists `retention`"
        );
    }
}
