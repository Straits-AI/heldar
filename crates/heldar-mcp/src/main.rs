//! `heldar-mcp` — a capability-scoped MCP sidecar for a Heldar box (#123).
//!
//! # Why a sidecar
//!
//! Putting an LLM endpoint inside the recording kernel would make the recorder depend on MCP
//! libraries, model hosts and agent traffic. A recorder's job is to keep recording while everything
//! else is on fire; adding a dependency that can be slow, chatty or exploited is the wrong trade.
//!
//! The sidecar talks to the box over the same HTTP API everything else uses, and **the core remains
//! the sole authorization and camera-scope enforcement point**. This process decides what to expose;
//! it never decides who may see it.
//!
//! # Read-only, structurally
//!
//! Every tool carries `method: "GET"`, and this dispatcher sends nothing else. There is no branch a
//! model can talk its way down — see `tools.rs`.
//!
//! # Transport
//!
//! `heldar-mcp stdio` speaks JSON-RPC over stdin/stdout, which is how an MCP client launches a local
//! server. A listening transport comes after local operation is stable and authenticated, which is
//! what the issue asks.

mod tools;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "stdio".into());
    match mode.as_str() {
        "stdio" => serve_stdio().await,
        "--help" | "-h" | "help" => {
            print_help();
            Ok(())
        }
        other => {
            eprintln!("heldar-mcp: unknown mode {other:?}; try `heldar-mcp --help`");
            std::process::exit(1);
        }
    }
}

fn print_help() {
    println!(
        "heldar-mcp — a read-only MCP sidecar for a Heldar box\n\
         \n\
         Usage:\n  \
           heldar-mcp stdio              speak MCP over stdin/stdout\n\
         \n\
         Environment:\n  \
           HELDAR_URL         the box (default http://127.0.0.1:8000)\n  \
           HELDAR_TOKEN       a capability-scoped API key\n  \
           HELDAR_TOKEN_FILE  a file holding one, preferred over the variable\n\
         \n\
         Use a DEDICATED, capability-scoped key — never an administrator's. This process decides\n\
         what to expose; the box decides who may see it, and a key with more capability than the\n\
         agent needs is a key the agent has."
    );
}

/// The box's base URL and a bearer token.
struct Upstream {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Upstream {
    fn from_env() -> Result<Self> {
        let base = std::env::var("HELDAR_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:8000".into());
        // A file is preferred over the variable, for the same reason the server prefers one:
        // an environment variable is readable from /proc and inherited by children.
        let token = match std::env::var("HELDAR_TOKEN_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            Some(p) => std::fs::read_to_string(&p)
                .with_context(|| format!("reading {p}"))?
                .trim()
                .to_string(),
            None => std::env::var("HELDAR_TOKEN")
                .context("set HELDAR_TOKEN or HELDAR_TOKEN_FILE to a capability-scoped API key")?
                .trim()
                .to_string(),
        };
        anyhow::ensure!(!token.is_empty(), "the token is empty");
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
        })
    }

    /// Call one tool. GET only — the method comes from the tool table, not from the caller.
    async fn call(&self, tool: &tools::Tool, arg: Option<&str>) -> Result<serde_json::Value> {
        let path = match (tool.arg, arg) {
            (None, _) => tool.path.to_string(),
            (Some(name), Some(v)) => {
                // A path segment, not a path. Without this a camera_id of "../../api-keys" would
                // walk out of the route the tool names — the one place an argument becomes URL.
                anyhow::ensure!(
                    !v.is_empty()
                        && v.len() <= 128
                        && v.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                    "{name} must be 1-128 characters of [A-Za-z0-9_-]"
                );
                tool.path.replace("{}", v)
            }
            (Some(name), None) => anyhow::bail!("{name} is required"),
        };
        debug_assert_eq!(
            tool.method, "GET",
            "the dispatcher sends GET and nothing else"
        );
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("the box could not be reached")?;
        let status = resp.status();
        let rid = resp
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        if !status.is_success() {
            // The kernel's own message, not a reinterpretation: a sidecar that rewrites a 403 into
            // something friendlier is a sidecar that hides an authorization boundary from the
            // person debugging it. The correlation id is what joins this to the box's audit log.
            anyhow::bail!("the box returned {status} (request id {rid})");
        }
        let body: serde_json::Value = resp.json().await.context("the reply is not JSON")?;
        Ok(tools::redact(body))
    }
}

async fn serve_stdio() -> Result<()> {
    let upstream = Upstream::from_env()?;
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                write(
                    &mut stdout,
                    &error(None, -32700, &format!("parse error: {e}")),
                )
                .await?;
                continue;
            }
        };
        let id = req.get("id").cloned();
        let method = req["method"].as_str().unwrap_or("");
        let response = match method {
            "initialize" => Some(result(
                id,
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "heldar-mcp",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": "Read-only access to one Heldar box. Every tool is a GET; there \
                                     is no mutation path in this process. Camera scope and \
                                     capability are enforced by the box, not here — a camera you \
                                     cannot see is absent rather than refused.",
                }),
            )),
            // A notification has no id and takes no reply.
            "notifications/initialized" => None,
            "tools/list" => Some(result(id, serde_json::json!({ "tools": tools::specs() }))),
            "tools/call" => Some(handle_call(&upstream, id, &req).await),
            other => Some(error(
                id,
                -32601,
                &format!("method {other:?} is not supported by this server"),
            )),
        };
        if let Some(r) = response {
            write(&mut stdout, &r).await?;
        }
    }
    Ok(())
}

async fn handle_call(
    upstream: &Upstream,
    id: Option<serde_json::Value>,
    req: &serde_json::Value,
) -> serde_json::Value {
    let name = req["params"]["name"].as_str().unwrap_or("");
    let Some(tool) = tools::find(name) else {
        return error(id, -32602, &format!("unknown tool {name:?}"));
    };
    let arg = tool
        .arg
        .and_then(|a| req["params"]["arguments"][a].as_str());
    match upstream.call(tool, arg).await {
        Ok(v) => result(
            id,
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&v).unwrap_or_default(),
                }],
                "isError": false,
            }),
        ),
        // An MCP tool failure is a RESULT with isError, not a protocol error — the model should see
        // it and adapt, not have the connection fault.
        Err(e) => result(
            id,
            serde_json::json!({
                "content": [{ "type": "text", "text": format!("{e:#}") }],
                "isError": true,
            }),
        ),
    }
}

fn result(id: Option<serde_json::Value>, value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(serde_json::Value::Null),
        "result": value,
    })
}

fn error(id: Option<serde_json::Value>, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(serde_json::Value::Null),
        "error": { "code": code, "message": message },
    })
}

async fn write(out: &mut tokio::io::Stdout, v: &serde_json::Value) -> Result<()> {
    out.write_all(serde_json::to_string(v)?.as_bytes()).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_gets_no_reply() {
        // `notifications/initialized` has no id. Replying to it is a protocol violation, and a
        // client that is strict about it will drop the connection.
        let req: serde_json::Value =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(req.get("id").is_none());
    }

    #[test]
    fn responses_carry_the_request_id_including_a_null_one() {
        let r = result(Some(serde_json::json!(7)), serde_json::json!({}));
        assert_eq!(r["id"], 7);
        assert_eq!(r["jsonrpc"], "2.0");
        // An id-less request still gets a well-formed envelope rather than a missing field.
        assert_eq!(error(None, -32601, "x")["id"], serde_json::Value::Null);
    }
}
