# heldar-search

*Generic searchable visual-event memory: deterministic structured search over kernel event facts, an LLM-as-query-planner natural-language layer, and a proof/confidence layer.*

[![crates.io](https://img.shields.io/crates/v/heldar-search.svg)](https://crates.io/crates/heldar-search)
[![docs.rs](https://docs.rs/heldar-search/badge.svg)](https://docs.rs/heldar-search)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/Straits-AI/heldar/blob/main/LICENSE)

`heldar-search` turns the platform's accumulated event facts into a queryable memory of who, what, where, when, with confidence and evidence. It is part of the **Heldar** open-core platform and is built on the [`heldar-kernel`](https://crates.io/crates/heldar-kernel) crate. The governing principle: the LLM is a query *planner*, never the source of truth. A question is translated into a structured query plan, the plan runs deterministically against the kernel's stored facts, and the answer is those rows, not anything a model said.

## What it does

- Executes a flat, all-optional `QueryPlan` over the kernel fact tables (`entry_events`, `zone_events`, `breach_alerts`) as one time-bounded SQL fetch per source plus in-Rust field filters, merged newest-first and capped.
- Applies a default 7-day window when the plan leaves `from`/`to` unset, so an unbounded query never scans the whole history.
- Filters on cameras, time-of-day (`hour_min`/`hour_max`, including overnight wraparound), plate, colour, vehicle type, subject type, auth status, event type, zone kind, and a free-text substring across plate / zone / kind / subject.
- Parses natural language into a plan with `parse_rules`, a transparent, dependency-free keyword planner (colour, vehicle type, subject, authorization wording, event/source intent, camera name resolution, relative dates, time-of-day, plate-like tokens). This is the always-available offline default.
- Optionally calls `plan_llm`, an OpenAI-compatible chat-completions endpoint asked only to emit a strict plan JSON (`temperature: 0`, `response_format: json_object`). It never sees or returns data, its output is run through `sanitize`, and any failure falls back to the rule parser.
- Offers a plan dry-run that shows how a question is interpreted without executing it or returning data.
- Wraps every answer in a proof claim ladder (observation -> track -> event -> aggregate -> inference) via `proof::build`, attaching evidence (source row ids, evidence frame paths, by-source/by-day breakdown) and confidence, and marking the natural-language-to-plan reading as the single fallible inference.
- Logs every search to its own `search_log` table and audits identity-bearing (plate-targeted) queries to the kernel `audit_log`.
- Read-only: it is not a `DetectionConsumer` and runs no background loop. All routes require the kernel's `view` capability.

## Where it fits

`heldar-search` is a *library* crate. It does not ship a binary; the runnable `heldar-core` server lives in the un-published `heldar-server` crate, which composes this crate alongside the other Heldar apps over the kernel's seams. Key public surface (see `src/lib.rs`):

- `routes::router(cfg) -> Router<AppState>` mounts `POST /api/v1/search/events` (structured), `POST /api/v1/search/nl` (natural language), and `POST /api/v1/search/plan` (dry-run).
- `config::SearchConfig` with `SearchConfig::from_env()` reads the optional LLM seam and result cap from the environment.
- `query::{QueryPlan, SearchHit, execute, window, breakdown}` is the plan type and its deterministic executor.
- `planner::{parse_rules, plan_llm, sanitize}` is the natural-language-to-plan layer.
- `proof::build` constructs the claim ladder; `schema::init` applies the crate's `search_log` table idempotently.

## Usage

```sh
cargo add heldar-search
```

The crate is mounted by the composing server (see `crates/heldar-server/src/main.rs`):

```rust
use std::sync::Arc;
use heldar_search::{config::SearchConfig, routes, schema};

// 1. apply the search query-log schema against the shared kernel pool
schema::init(&pool).await?;

// 2. load config from the environment (LLM seam optional; unset => offline rule parser)
let search_cfg = Arc::new(SearchConfig::from_env());

// 3. merge the search routes into the kernel's Router<AppState>
let app = app.merge(routes::router(search_cfg));
```

Configuration is via environment variables: `HELDAR_SEARCH_LLM_URL` (unset means fully offline on the rule parser), `HELDAR_SEARCH_LLM_API_KEY`, `HELDAR_SEARCH_LLM_MODEL` (default `gpt-4o-mini`), and `HELDAR_SEARCH_MAX_RESULTS` (default 200, clamped 1..5000). The full integrator guide is [docs/SEARCH.md](https://github.com/Straits-AI/heldar/blob/main/docs/SEARCH.md).

## Documentation

- Repository: [github.com/Straits-AI/heldar](https://github.com/Straits-AI/heldar)
- Guide: [docs/SEARCH.md](https://github.com/Straits-AI/heldar/blob/main/docs/SEARCH.md)
- API docs: [docs.rs/heldar-search](https://docs.rs/heldar-search)

## License

Apache-2.0
