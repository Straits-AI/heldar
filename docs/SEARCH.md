# Heldar Core — Semantic Search (Stage 7) Operator & Integrator Guide

This is the definitive guide to **Semantic Search** **as actually built** in
`crates/heldar-search`: turn the platform's accumulated event facts into a queryable
**visual-event memory** — *who / what / where / when / confidence / evidence* — answered
by **structured search**, by **natural-language search** (a question is *planned* into a
structured query, the plan is executed, the rows are the answer), and a **plan dry-run**,
with a **proof layer** that decomposes every answer into claim levels with evidence and
confidence.

Implementation: `query.rs` (the [`QueryPlan`](#3-the-queryplan-schema-queryrs) + the
[deterministic executor](#4-the-deterministic-executor-queryrs)), `planner.rs` (the
[rule parser](#5-the-rule-based-planner-the-offline-default-plannerrs) + the
[optional LLM seam](#6-the-optional-llm-planner-the-seam-plannerrs)), `proof.rs` (the
[claim ladder](#7-the-proof-layer-proofrs)), `routes.rs` (the
[HTTP surface](#9-http-api-surface-routesrs) + audit + log), `config.rs`
([env](#10-configuration-configrs)), `schema.sql` (the one query-log table), `lib.rs`
(the governing principle). The kernel architecture is in
[`ARCHITECTURE.md`](../ARCHITECTURE.md) §20.

Stage 7 builds **entirely on stored kernel + app data** (`entry_events`, `zone_events`,
`breach_alerts`) and adds **no ingest path, no decode, no background loop, and no new fact
table**. It is **not** a `DetectionConsumer` and **not** a `spawn_supervised` service — it
is a **read-only query layer over the kernel's facts**: three HTTP routes plus one small
query log (history + accountability). The kernel is unaware it exists.

---

## 1. The governing principle (`lib.rs`)

> **The LLM is a query PLANNER, never the source of truth.**

Everything in this stage follows from that one rule:

1. **The answer is always the executed query's rows — never anything a model "said".** A
   natural-language question is translated into a structured **query plan** (a
   deterministic filter), the plan is executed against the kernel's stored facts
   (`entry_events`, `zone_events`, `breach_alerts`), and the result is *those rows*. No
   model ever sees the data, summarizes it, or generates an answer about it.

2. **The rule-based planner works fully offline.** When no LLM endpoint is configured (the
   default), a transparent keyword parser (`parse_rules`) produces the same `QueryPlan`
   type. The feature is complete with **zero external dependencies** — no API key, no
   network, no model.

3. **The LLM is optional and only plans.** If an OpenAI-compatible endpoint *is*
   configured, it is asked to translate the question into a plan JSON — and **only** that.
   It never sees or returns data. On *any* failure (no endpoint, non-2xx, unparseable
   JSON) the caller **falls back to the rule parser**.

4. **The plan is always shown back to the caller.** Every response echoes the `planner`
   (`rules` | `llm` | `structured`) and the exact `plan` that ran, and the
   [proof layer](#7-the-proof-layer-proofrs) flags the NL→plan reading as the *single*
   fallible inference in the answer. There is nothing hidden between the question and the
   rows.

This is what makes the feature trustworthy and commercially safe: the inference surface is
reduced to one explicit, inspectable, fallible step (how the question was read), and that
step is **decoupled** from the data it selects.

---

## 2. Overview

```
   kernel + app fact tables (already written by Stages 3/4/6)
     entry_events    — one canonical ANPR event per vehicle (plate, subject attrs, auth_status, evidence)
     zone_events     — enter/exit/dwell on polygon zones (joined to zones for `kind`)
     breach_alerts   — worked red-zone incidents (subject correlated to a plate when known)
        │
        │  ── search READS these tables; it never sees RTSP, frames, or the ingest batch ──
        ▼
   heldar-search (three HTTP routes, no loop, no consumer)

   POST /api/v1/search/events   structured ─┐
                                            ├─► QueryPlan ─► execute() ─► rows ─► proof ─► response
   POST /api/v1/search/nl        question ──┘     ▲
                                            plan_llm()  (if HELDAR_SEARCH_LLM_URL set)
                                            else parse_rules()  (transparent, offline, default)

   POST /api/v1/search/plan      question ─► plan_llm()/parse_rules() ─► {plan}   (dry-run: NO execution, NO data)

        │
        ▼
   every search → search_log row;  plate-targeted query → kernel audit_log
```

The flow for a question (`/search/nl`) is exactly: **plan → execute → prove**. `plan_llm`
is tried first **only when an LLM URL is configured**; otherwise (and on any LLM failure)
`parse_rules` runs. Either way `query::execute` runs the plan deterministically, and
`proof::build` wraps the rows in the claim ladder.

---

## 3. The `QueryPlan` schema (`query.rs`)

The `QueryPlan` is the **only** thing the NL layer produces. It is a flat struct of
**all-optional** fields (an empty plan ⇒ "everything in the default window"). It is what
`/search/events` accepts directly, what the planner emits, what is echoed in every
response, and what is stored in `search_log`.

| Field | Type | Meaning |
|---|---|---|
| `from` | `string?` (RFC3339) | Window start. Default: **now − 7 days**. |
| `to` | `string?` (RFC3339) | Window end. Default: **now + 1 minute**. |
| `hour_min` | `int?` (0–23) | Time-of-day floor — keep events whose **UTC hour ≥** this (`"after 6pm"` ⇒ 18). |
| `hour_max` | `int?` (0–23) | Time-of-day ceiling — keep events whose **UTC hour ≤** this (`"before 9am"` ⇒ 9). |
| `cameras` | `string[]` | Camera **ids**; empty ⇒ all cameras. |
| `sources` | `string[]` | Which fact tables to search: subset of `entry` \| `zone` \| `breach`; empty ⇒ **all three**. |
| `plate` | `string?` | Exact normalized plate (UPPERCASE alphanumeric). **Identity-bearing** — triggers audit (§8). |
| `color` | `string?` | Vehicle colour; matched case-insensitively against `subject.color`. |
| `vehicle_type` | `string?` | Vehicle type (`car`/`truck`/…); matched against `subject.vehicle_type`. |
| `subject_type` | `string?` | `"vehicle"` or `"person"` (see the executor's subject logic below). |
| `auth_status` | `string[]` | Subset of `matched` \| `exception` \| `unmatched` \| `blocked`; matched against an entry event's `auth_status`. |
| `event_type` | `string?` | e.g. `vehicle_entry` / `vehicle_exit`; matched case-insensitively against a hit's `kind`. |
| `zone_kind` | `string?` | Zone kind (`restricted`/`shelf`/…); matched against the zone's `kind`. |
| `text` | `string?` | Free-text substring matched (lowercased) across **plate + zone + kind + subject** of each hit. |
| `limit` | `int?` | Max rows returned; clamped to `[1, max_results]` (default cap 200, §10). |

Every result is normalized to a **`SearchHit`** regardless of which table it came from:
`source` (`entry`/`zone`/`breach`), `id`, `timestamp`, `camera_id`, `kind`, `plate`,
`subject` (JSON), `auth_status`, `zone`, `zone_kind`, `evidence_path`, and
`claim_level` (always `"event"` — see the proof ladder).

---

## 4. The deterministic executor (`query.rs`)

`execute(pool, plan, max)` runs the plan against the kernel's facts. It is **pure SQL +
Rust** — no model, no randomness, fully reproducible.

**1. Time window.** `from`/`to` are parsed (`heldar_kernel::util::parse_rfc3339`);
unset `from` defaults to **now − 7 days** and unset `to` to **now + 1 min**, so an
unbounded query never scans the whole history. This default 7-day window is the single
most important guardrail on cost.

**2. Time-bounded fetch per source.** For each requested source (`want()` = `sources`
empty *or* contains that source) it issues **one time-bounded, newest-first SQL query**,
capped at `fetch_cap = (max × 5).clamp(100, 20_000)` rows:

| `source` | Table | Notes |
|---|---|---|
| `entry` | `entry_events` (`timestamp` between `from`/`to`) | `evidence_path` from `evidence.snapshot_path`; carries `plate`, `subject`, `auth_status`. |
| `zone` | `zone_events ze LEFT JOIN zones z ON z.id = ze.zone_id` | `zone_kind` from the joined `z.kind`; `subject = {label}`. |
| `breach` | `breach_alerts` (`created_at` between `from`/`to`) | the correlated `subject` becomes the hit's `plate`; `subject = {subject_type, severity}`. |

Only the **time window** and the **fetch cap** are pushed into SQL (so the query is always
indexed and bounded); everything else is applied in Rust.

**3. Rust field filters.** The fetched hits are filtered in-process (`hits.retain`) against
the remaining plan fields, in this order: `cameras` (membership) → `hour_min`/`hour_max`
(UTC hour of the timestamp) → `plate` (exact) → `color` / `vehicle_type` (case-insensitive
on `subject`) → `subject_type` → `auth_status` (membership) → `event_type` (case-insensitive
on `kind`) → `zone_kind` (case-insensitive) → `text` (lowercased substring).

The `subject_type` filter is deliberately lenient because the three tables carry subjects
differently: `"vehicle"` keeps a hit if `subject.type == "vehicle"` **or** it has a
`plate`; `"person"` keeps it if `subject.type == "person"` **or** `subject.label ==
"person"`.

**4. Sort + limit.** Surviving hits from all sources are merged, **sorted newest-first**
(`timestamp` descending), and **truncated** to `plan.limit.unwrap_or(max).clamp(1, max)`.
(The planner also de-duplicates the `cameras` list it produces.)

**5. Breakdown.** `breakdown(hits)` builds the aggregate the proof layer reports: counts
**by source** and **by day** over the returned hits.

> **Window semantics to know.** Time filtering is on the event timestamp and `hour_min`/
> `hour_max` compare the event's **UTC** hour (not a site-local hour) — operators in
> non-UTC sites should account for the offset. The `auth_status` filter only ever matches
> `entry_events` (zone/breach hits carry no `auth_status` and are dropped when it is set).

---

## 5. The rule-based planner (the offline default, `planner.rs`)

`parse_rules(query, cameras)` is the **always-available** planner: a transparent,
dependency-free keyword parser over the lowercased question. `cameras` is a list of
`(id, name)` pairs so phrases resolve to camera ids. It is **best-effort** by design — it
recognizes the patterns below and leaves everything else to the default window.

| Pattern | Recognized tokens | Sets |
|---|---|---|
| **Colour** | `white black gray/grey silver red blue green yellow orange brown purple` | `color` (`grey`→`gray`). |
| **Vehicle type** | `car truck bus motorcycle van suv bicycle motorbike` | `vehicle_type` (`motorbike`→`motorcycle`) + `subject_type=vehicle`. |
| **Subject (vehicle)** | `car` / `vehicle` / `truck` / `van` | `subject_type=vehicle`. |
| **Subject (person)** | `person` / `people` / `pedestrian` / `customer` / `visitor` | `subject_type=person`. |
| **Authorization** | `unauthor` / `without authoriz` / `unmatched` / `unknown` → `unmatched`; `exception` / `mismatch` → `exception`; `blocked` / `blacklist` / `stolen` → `blocked` | appends to `auth_status`. |
| **Event / source** | `red zone` / `restricted` / `breach` / `intrusion` → `sources+=breach`; **else** `enter` / `entry` / `arriv` → `event_type=vehicle_entry`; **else** `exit` / `leav` / `left` → `event_type=vehicle_exit` | (breach intent wins; otherwise entry/exit). |
| **Camera name** | a camera's `name` or `id` appears in the question (matched **longest-name-first**, so `"gate b annex"` beats `"gate b"`) | appends the camera `id` to `cameras` (deduped). |
| **Relative date** | `yesterday` (full prior day) · `today` (since midnight) · `last/past/this week` (now − 7 d) · `last/past N days` (now − N d, N clamped 1–365) | `from` / `to`. |
| **Time of day** | `after <time>` → `hour_min`; `before <time>` → `hour_max`; accepts `6pm`, `6 pm`, `18:00` (am/pm normalized to a 0–23 UTC hour) | `hour_min` / `hour_max`. |
| **Plate** | the first plate-like token: 4–10 alphanumerics containing **both** a letter and a digit (normalized UPPERCASE) | `plate`. |

### Worked examples

These are the target queries, with the plan `parse_rules` produces
(assuming a camera named `Gate B` with id `gate_b`):

**`"unknown white cars entering Gate B after 6pm last week"`**
```json
{ "color": "white", "vehicle_type": "car", "subject_type": "vehicle",
  "auth_status": ["unmatched"], "event_type": "vehicle_entry",
  "cameras": ["gate_b"], "from": "<now-7d>", "hour_min": 18 }
```
→ entry events on `gate_b` in the last week, white cars, after 18:00 UTC, that resolved as
`unmatched` (unknown).

**`"people who entered red zones yesterday without authorization"`**
```json
{ "subject_type": "person", "auth_status": ["unmatched"],
  "sources": ["breach"], "from": "<yesterday 00:00>", "to": "<today 00:00>" }
```
→ red-zone breach incidents from yesterday for person subjects. (Note the best-effort edge:
`breach_alerts` carry no `auth_status`, so `"without authorization"` does not narrow the
breach source further — the `breach` source *is* the restricted-zone signal here.)

**`"customers who waited >5 min and left without checkout"`**
```json
{ "subject_type": "person", "event_type": "vehicle_exit" }
```
→ best-effort only: the rule parser maps `customer`→person and `left`→`vehicle_exit`, but
it **cannot** express a dwell threshold or a "no checkout" join. This is a behaviour query
better served by a proprietary retail-analytics vertical (`heldar-bakery`) that lives in a
separate private repo, a deliberate boundary surfaced honestly rather than faked.

> Use the [`/search/plan` dry-run](#9-http-api-surface-routesrs) to see exactly how any
> question is parsed before running it.

---

## 6. The optional LLM planner (the seam, `planner.rs`)

`plan_llm(http, cfg, query, cameras)` is engaged **only if `HELDAR_SEARCH_LLM_URL` is
set**. It asks an OpenAI-compatible chat-completions endpoint to translate the question
into a strict plan JSON:

- **`temperature: 0`**, **`response_format: { type: "json_object" }`**, a system prompt
  that spells out the exact `QueryPlan` schema and the **known camera ids/names**, and the
  hard instruction *"You ONLY produce the query plan; you never answer the question or
  invent data."*
- `model` = `HELDAR_SEARCH_LLM_MODEL` (default `gpt-4o-mini`); `Authorization: Bearer`
  added if `HELDAR_SEARCH_LLM_API_KEY` is set.
- The response's `choices[0].message.content` is parsed as a `QueryPlan`.

**It returns `None` (and the caller falls back to `parse_rules`) on any failure** —
endpoint unreachable, non-2xx status, or content that does not parse as a plan (both logged
at `warn`). A returned plan is passed through `sanitize()`, which clamps out-of-range
`hour_min`/`hour_max` (a defensive guard against an LLM emitting nonsense). The model
**never** sees, summarizes, or returns surveillance data — only a plan flows out of it, and
that plan is executed deterministically and shown back to the caller exactly like a
rule-parsed one.

---

## 7. The proof layer (`proof.rs`)

`build(query, planner, plan, hits)` decomposes every answer into the
**claim ladder**, lowest (most certain) to highest (most interpretive):

```
observation → track → event → aggregate → inference   (→ hypothesis)
```

The platform stores facts at the **event** level and below (kernel-produced); this layer
adds the **aggregate** (the executed count/breakdown) and the **inference** (how the
question was read). The proof object carries three levels:

| Level | What it asserts | Confidence | Notes |
|---|---|---|---|
| **inference** *(only for NL queries)* | "Interpreted the question … as the structured plan below." | `medium` (llm) / `medium-low` (rules) | **`fallible: true`** — the *only* non-deterministic step. Evidence = `{ planner, plan }`, plus a caveat to verify the plan matches intent. |
| **aggregate** | "N stored event(s) match the executed plan in the queried window." | `high` | Basis: *deterministic SQL over the kernel fact tables; the answer is these rows, not model output.* Evidence = `{ count, breakdown (by source / by day), window }`. |
| **event** | "N event claim(s); each links to its source row + evidence frame." | per-event (`auth_status` / `plate_confidence` / `severity` on each hit) | Provenance: each event was derived by the kernel from observation+track data in `detections`; pull the clip via the kernel clip API (`POST /api/v1/cameras/{id}/clip`) and the evidence frame via its `evidence_path`. Evidence = the first 50 hit ids + evidence paths. |

The object closes with a `note`: facts are at the event level and below (kernel-produced);
search adds the **aggregate** (a deterministic query) and the **inference** (the NL→plan
reading); **no layer asserts identity or causation.** For a structured `/search/events`
call there is no question to interpret, so the inference level is omitted entirely — a
structured query has *no* fallible step.

> This is the principle made auditable: the one place uncertainty can enter (reading the
> question) is the one place the proof marks `fallible: true`. Everything below it is
> deterministic over stored facts.

---

## 8. Audit & the search log

Two records are written for accountability. **`schema.sql`** owns exactly one table:

```sql
CREATE TABLE IF NOT EXISTS search_log (
    id           TEXT PRIMARY KEY,        -- sl_<uuid>
    actor        TEXT,                    -- principal id
    mode         TEXT NOT NULL,           -- 'nl' | 'structured'
    query_text   TEXT,                    -- the NL question (nl mode only)
    plan         TEXT NOT NULL DEFAULT '{}',  -- the executed plan (JSON)
    planner      TEXT,                    -- 'rules' | 'llm' | 'structured'
    result_count INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL
);  -- + idx_search_log_created
```

- **Every search is logged** to `search_log` (`/search/events` and `/search/nl`) — actor,
  mode, the verbatim question (nl), the executed plan, the planner that produced it, and the
  result count. This is the search history + a record of what each operator asked.
- **Identity-bearing queries are audited** to the kernel `audit_log`. A query is
  identity-bearing when it **targets a specific plate** (`plan.plate.is_some()`, the main
  re-identifying handle here). For those, `auth::audit(...)` writes a
  `search_identity_query` action against the `plate` target, with `{ mode, query }` —
  the same immutable audit trail as the Stage 6 plate-trail searches. The `/search/plan`
  dry-run executes nothing, so it neither logs nor audits.

---

## 9. HTTP API surface (`routes.rs`)

All three routes require the Stage 4 RBAC **`view`** capability
(`principal.require(principal.can_view(), …)`). The router takes `SearchConfig` as an
`Extension` and is `merge`d into the server in `main.rs`.

| Method | Path | Cap | Body | Purpose |
|---|---|---|---|---|
| POST | `/api/v1/search/events` | `view` | a `QueryPlan` (JSON) | **Structured search.** Execute a plan directly; logged as `mode=structured`, `planner=structured`. No inference level in the proof. |
| POST | `/api/v1/search/nl` | `view` | `{ "query": "<question>" }` | **Natural-language search.** Plan (LLM if configured, else rules) → execute → prove; logged as `mode=nl`. Empty `query` ⇒ 400. |
| POST | `/api/v1/search/plan` | `view` | `{ "query": "<question>" }` | **Plan dry-run.** Returns `{ query, planner, plan }` only — **no execution, no data, no log, no audit.** Use it to inspect how a question is read (trust/debug). |

The `/events` and `/nl` responses share one shape:

```json
{
  "query":   "unknown white cars entering Gate B after 6pm last week",  // null for structured
  "planner": "rules",                 // "rules" | "llm" | "structured"
  "plan":    { ...the executed QueryPlan... },
  "count":   3,
  "hits":    [ { "source": "...", "id": "...", "timestamp": "...", "evidence_path": "...", ... } ],
  "proof":   { "claim_levels": [ ...inference?, aggregate, event... ], "note": "..." }
}
```

The plan and planner are **always** echoed, so the caller can see exactly what ran. To pull
footage for any hit, take its timestamp window to the kernel clip API
(`POST /api/v1/cameras/{camera_id}/clip`) and read its `evidence_path` snapshot — the proof
layer's `event` level spells this out per hit.

---

## 10. Configuration (`config.rs`)

`SearchConfig::from_env()`. The LLM seam vars are all optional — **leave `…LLM_URL` unset
to run fully offline on the rule parser** (the default).

| Var | Default | Meaning |
|---|---|---|
| `HELDAR_SEARCH_LLM_URL` | *(unset)* | OpenAI-compatible chat-completions endpoint used **only** to plan a question. **Unset ⇒ the rule parser is used** (and the feature works with no external dependency). |
| `HELDAR_SEARCH_LLM_API_KEY` | *(unset)* | Bearer token sent to that endpoint, if it requires one. |
| `HELDAR_SEARCH_LLM_MODEL` | `gpt-4o-mini` | Model name passed to the endpoint. |
| `HELDAR_SEARCH_MAX_RESULTS` | `200` (clamped `1…5000`) | Hard cap on hits returned per search; also drives the executor's internal `fetch_cap`. |

---

## 11. How it composes (composed, not welded)

Search is wired in `crates/heldar-server/src/main.rs` purely as a bundled app: its
schema is applied after the kernel migrations (`heldar_search::schema::init`), its config
is read from the environment (`SearchConfig::from_env`), and its router is `merge`d in. It
is **absent from the `consumers` vec** (not a `DetectionConsumer`) and has **no
`spawn_supervised` loop** — it touches the ingest/recording/live-view path nowhere. A slow
or failing search request can only affect that request. Adding search was a schema-init +
a `merge` with **zero** change to the kernel — the same "kernel-open, apps-bundled" seam as
every vertical, now as a read-only query layer over the facts the others wrote.

---

## 12. Honest scope — what's built, what's a seam

**Built and production-grade:** the `QueryPlan` schema, the deterministic time-bounded
executor over the three kernel fact tables with the default 7-day window + Rust field
filters + sort/limit, the transparent offline rule parser, the optional LLM planner seam
(with sanitize + fallback), the proof/claim-ladder layer, the search log + identity-query
audit, the RBAC-gated HTTP surface, and the structured / NL / dry-run routes.

**Deliberately deferred (a documented seam, not built):**

- **Open-vocabulary VLM enrichment + event/clip EMBEDDINGS + vector retrieval** are a
  **seam, not built.** They need an **embedding/VLM worker** (the same `Analyzer`-style
  contract as the detection worker) to write embeddings the query layer could rank against.
  This stage ships the deterministic structured + NL-plan + proof core only.
- **Search by image / vehicle crop / person crop** depends on those embeddings and is
  therefore **not available** — today's search is by structured *attributes* (plate, colour,
  type, subject, auth, source, event, zone, time, camera, text), not by visual similarity.
- **VLM-based report interpretation** (natural-language synthesis of findings) is **not**
  here by design — the proof layer reports deterministic aggregates, not generated prose.
- **The LLM planner is optional and untested without a live endpoint.** It is exercised
  only when `HELDAR_SEARCH_LLM_URL` is configured; the default path is the rule parser.
- **The rule parser is best-effort.** It recognizes the patterns in §5 and leaves the rest
  to the default window. It cannot express dwell thresholds, multi-condition joins, or
  arbitrary semantics — use `/search/plan` to confirm a question parsed as intended, or
  send a structured `QueryPlan` directly for full control.

This applies the event-memory-to-latent-world-memory progression to search:
a typed, evidence-backed, deterministic query layer where the **only** inference is reading
the question, and that inference is surfaced, fallible, and decoupled from the answer.
</content>
</invoke>
