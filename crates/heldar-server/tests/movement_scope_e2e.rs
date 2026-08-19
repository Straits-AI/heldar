//! heldar-movement, END TO END: every handler, real credentials, both directions.
//!
//! Every prior scope round spot-checked this crate or read its handlers, and each one recorded that it
//! had never been driven end to end. This file closes that. It builds the composed router, applies the
//! app schemas the way `heldar_server::run` does, mints credentials through the REAL
//! `POST /api/v1/api-keys`, drives the ENGINES to produce the links/candidates/breaches under test,
//! and then calls every movement route with four credentials:
//!
//!   `fleet`     — unscoped. The control: nothing below may narrow what it sees or may do.
//!   `both_ends` — scoped to BOTH cameras of one link. The false-deny control.
//!   `one_end`   — scoped to ONE end of a link whose other end it does not hold. The hard case.
//!   `neither`   — scoped to a camera on neither end. Must learn nothing at all.
//!
//! # The containment rule, and why it is this one
//!
//! Movement is inherently cross-camera: its subject matter IS the relationship between two cameras. So
//! "what should a one-camera credential see of a link between `cam_own` and `cam_SENTINEL_B`?" has to
//! be decided rather than inherited, and the answer the crate implements — asserted throughout below —
//! is NOTHING, in either direction:
//!
//! * A resource naming two cameras (a `camera_links` row, a `movement_candidates` row) is visible and
//!   actionable ONLY to a credential holding BOTH ends. Anything less hands a `cam_own` credential the
//!   fact that `cam_SENTINEL_B` exists, is physically adjacent, and how long the walk between them
//!   takes — the camera roster plus the site's floor plan.
//! * A refusal must carry no bits either. Half-held, not held at all, and does-not-exist answer with
//!   the SAME error value, so the id space cannot be enumerated by probing.
//! * The same rule runs backwards: a credential holding both ends must be able to work its own links,
//!   candidates and breaches. A scope layer that 403s the resources it just authorised is a broken
//!   feature, and it looks exactly like enforcement working.
//!
//! # What is reachable, and why the read routes look "unproven"
//!
//! Every movement READ (`GET links|candidates|breaches`, both searches, the module UI) is gated on
//! `Cap::EventsRead`, which is in `auth::UNSCOPABLE_CAPS`: `validate_grant` refuses to mint it
//! alongside a camera scope and `Principal::from_api_key` denies a stored key that carries both. No
//! camera-scoped credential can therefore reach those handlers at all — their confinement is defence
//! in depth behind a capability wall, not the live boundary. That is a fact about the product, not an
//! assumption of this test: `read_routes_are_confined_or_capability_unreachable` PROVES it per route,
//! and accepts EITHER answer — a capability refusal, or a filtered 200 that names no out-of-scope
//! camera. If `events:read` is ever made scopable, those handlers start being exercised here on the
//! next run instead of silently going unchecked.
//!
//! The reachable surface — link create/delete, candidate review, breach work, engine run — is driven
//! for real.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{SecondsFormat, TimeDelta, Utc};
use heldar_kernel::state::AppState;
use serde_json::Value;
use tower::Service;

// Cameras. `cam_SENTINEL_*` never appears in a body a scoped credential is allowed to see, so a
// substring search over the SERIALIZED response is a sound roster-containment check. They are not
// substrings of the held ids, and vice versa.
const OWN: &str = "cam_own";
const OWN2: &str = "cam_own2";
const OTHER: &str = "cam_SENTINEL_BRAVO";
const FAR: &str = "cam_SENTINEL_CHARLIE";

/// Everything the product will pair with a camera scope: every capability except `admin` and the two
/// `UNSCOPABLE_CAPS`. A maximal grant isolates the scope boundary — if a route lets this through,
/// only a missing scope check can be responsible.
const SCOPED_CAPS: &[&str] = &[
    "registry:manage",
    "gate:operate",
    "camera:read",
    "video:live",
    "video:playback",
    "video:export",
    "ai:tasks",
    "ai:frames",
    "ai:ingest",
    "ai:embedwork",
    "system:read",
    "net:scan",
    "module:proxy",
];

/// The unscoped control's grant: the scopable set plus the two capabilities a camera scope may not be
/// combined with. Every movement read needs `events:read`, so without these the control would be
/// refused for capability reasons and would prove nothing about scope.
fn fleet_caps() -> Vec<&'static str> {
    let mut v = SCOPED_CAPS.to_vec();
    v.push("events:read");
    v.push("identity:read");
    v
}

// ---- harness ---------------------------------------------------------------

/// A state wired like `heldar_server::run`: kernel migrations, then each app's own schema. The route
/// census records seven movement routes as unprovable because the shared harness skips this step and
/// they 500 before reaching their guard; running it is what makes this file end to end.
async fn test_state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    // heldar-entry owns `entry_events` + the `entry_events_read` contract the ReID proposer joins on.
    heldar_entry::schema::init(&pool).await.unwrap();
    heldar_movement::schema::init(&pool).await.unwrap();
    heldar_search::schema::init(&pool).await.unwrap();
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = true;
    let cfg = std::sync::Arc::new(cfg);
    AppState {
        recorder: heldar_kernel::services::recorder::RecorderManager::new(
            pool.clone(),
            cfg.clone(),
        ),
        sampler: heldar_kernel::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
        live: heldar_kernel::services::live_publisher::LivePublisherManager::new(
            pool.clone(),
            cfg.clone(),
            heldar_kernel::reqwest::Client::new(),
        ),
        mirror: None,
        consumers: std::sync::Arc::new(Vec::new()),
        modules: std::sync::Arc::new(Vec::new()),
        catalog: std::sync::Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
        http: heldar_kernel::reqwest::Client::new(),
        media_jobs: heldar_kernel::services::media_jobs::MediaJobGovernor::new(2),
        started_at: Utc::now(),
        pool,
        cfg,
    }
}

/// Movement's config, built literally rather than from the environment: the engine assertions below
/// depend on the scan window and the min score, and a developer with `HELDAR_MOVEMENT_*` exported
/// must not silently turn them into no-ops.
fn movement_cfg() -> std::sync::Arc<heldar_movement::config::MovementConfig> {
    std::sync::Arc::new(heldar_movement::config::MovementConfig {
        engine_interval_s: 60,
        scan_window_s: 3600,
        min_candidate_score: 0.5,
        red_zone_kinds: vec!["restricted".to_string()],
        retention_days: 365,
        appearance_scoring: false,
        appearance_window_s: 5,
    })
}

/// Kernel + entry + movement + search, as `heldar_server::run` composes them.
fn composed_router(st: &AppState) -> axum::Router {
    let search_cfg = std::sync::Arc::new(heldar_search::config::SearchConfig::from_env());
    axum::Router::new()
        .merge(heldar_kernel::routes::api_router())
        .merge(heldar_entry::routes::router())
        .merge(heldar_movement::routes::router(movement_cfg()))
        .merge(heldar_search::routes::router(search_cfg))
        .with_state(st.clone())
}

async fn call(
    st: &AppState,
    token: &str,
    method: &str,
    path: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut app = composed_router(st);
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("X-API-Key", token)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.call(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 22)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn get(st: &AppState, token: &str, path: &str) -> (StatusCode, String) {
    call(st, token, "GET", path, "").await
}

/// `now` as an RFC3339 stamp safe to drop into a QUERY STRING unescaped. `to_rfc3339()` emits a
/// `+00:00` offset, and `+` decodes to a space in a query, so the handler rejects it as unparseable —
/// which would silently turn every person-walk assertion below into a 400 rather than a scope test.
fn query_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The bootstrap admin, written straight to `api_keys`. It is NOT a subject of any assertion — it
/// exists only because minting through the real endpoint needs an existing unscoped admin, exactly as
/// a first-boot operator does. Every credential under test is minted through the API below.
async fn bootstrap_admin(st: &AppState) -> String {
    let token = format!("vok_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at,
                               capabilities, scope_kind, scope_cameras, expires_at)
         VALUES (?,?,?,?,'admin',1,?,NULL,'all',NULL,NULL)",
    )
    .bind(format!("key_{}", uuid::Uuid::new_v4().simple()))
    .bind("bootstrap")
    .bind(heldar_kernel::auth::token_hash(&token))
    .bind(&token[..8])
    .bind(Utc::now())
    .execute(&st.pool)
    .await
    .unwrap();
    token
}

/// Mint a credential the way an operator does. A fixture written straight into `api_keys` can be a
/// shape the product refuses to issue, and every false-deny assertion against such a principal is
/// vacuous — which is how an inert fix shipped green here before.
async fn mint(st: &AppState, admin: &str, caps: &[&str], cameras: Option<&[&str]>) -> String {
    let (kind, list) = match cameras {
        Some(c) => ("cameras", serde_json::json!(c)),
        None => ("all", serde_json::json!([])),
    };
    let body = serde_json::json!({
        "name": "movement-e2e",
        "role": "integration",
        "capabilities": caps,
        "scope_kind": kind,
        "scope_cameras": list,
        "confirm_privileged": true,
    })
    .to_string();
    let (status, resp) = call(st, admin, "POST", "/api/v1/api-keys", &body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the API refused to mint {caps:?} scoped to {cameras:?}; a test cannot assert anything about \
         a credential the product will not issue: {resp}"
    );
    serde_json::from_str::<Value>(&resp)
        .ok()
        .and_then(|v| v["key"].as_str().map(String::from))
        .unwrap_or_else(|| panic!("mint returned no plaintext key: {resp}"))
}

async fn seed_camera(st: &AppState, id: &str) {
    let now = Utc::now();
    sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
        .bind(id)
        .bind(format!("Camera {id}"))
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
}

/// An ANPR sighting in the access-control app's table. The ReID proposer reads it through the
/// `entry_events_read` contract view.
async fn seed_sighting(
    st: &AppState,
    id: &str,
    camera: &str,
    plate: &str,
    at: chrono::DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO entry_events (id, camera_id, event_type, timestamp, direction, plate, subject,
                                   authorization, auth_status, evidence, workflow_status, workflow,
                                   audit, track_id, created_at)
         VALUES (?, ?, 'anpr', ?, 'inbound', ?, '{}', '{}', 'matched', '{}', 'pending', '{}', '{}',
                 'trk_1', ?)",
    )
    .bind(id)
    .bind(camera)
    .bind(at)
    .bind(plate)
    .bind(at)
    .execute(&st.pool)
    .await
    .unwrap();
}

/// A restricted zone plus an `enter` event on it — the breach engine's only input.
async fn seed_red_zone_entry(st: &AppState, zone: &str, camera: &str, event: &str) {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO zones (id, camera_id, name, kind, polygon, enabled, created_at, updated_at)
         VALUES (?, ?, ?, 'restricted', '[[0,0],[1,0],[1,1]]', 1, ?, ?)",
    )
    .bind(zone)
    .bind(camera)
    .bind(format!("red-{camera}"))
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO zone_events (id, camera_id, zone_id, zone_name, event_type, timestamp, created_at)
         VALUES (?, ?, ?, ?, 'enter', ?, ?)",
    )
    .bind(event)
    .bind(camera)
    .bind(zone)
    .bind(format!("red-{camera}"))
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
}

/// Create a link through the REAL endpoint and return its id.
async fn create_link(st: &AppState, token: &str, from: &str, to: &str) -> String {
    let body = serde_json::json!({ "from_camera": from, "to_camera": to, "transit_seconds": 300 })
        .to_string();
    let (status, resp) = call(st, token, "POST", "/api/v1/movement/links", &body).await;
    assert_eq!(status, StatusCode::CREATED, "link {from}->{to}: {resp}");
    serde_json::from_str::<Value>(&resp).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Cameras, links, sightings and a red-zone entry, then ONE engine run — so the candidates and
/// breaches every assertion below works on are the ones the product actually produces.
struct World {
    st: AppState,
    fleet: String,
    both_ends: String,
    one_end: String,
    neither: String,
    /// `OWN -> OWN2`: both ends inside `both_ends`' scope.
    link_own: String,
    /// `OWN -> OTHER`: `one_end` holds exactly one end.
    link_half: String,
    /// `OTHER -> FAR`: `one_end` and `both_ends` hold neither end.
    link_far: String,
}

impl World {
    async fn build() -> World {
        let st = test_state().await;
        for cam in [OWN, OWN2, OTHER, FAR] {
            seed_camera(&st, cam).await;
        }
        let admin = bootstrap_admin(&st).await;
        let fleet = mint(&st, &admin, &fleet_caps(), None).await;
        let both_ends = mint(&st, &admin, SCOPED_CAPS, Some(&[OWN, OWN2])).await;
        let one_end = mint(&st, &admin, SCOPED_CAPS, Some(&[OWN])).await;
        let neither = mint(&st, &admin, SCOPED_CAPS, Some(&[FAR])).await;

        let link_own = create_link(&st, &fleet, OWN, OWN2).await;
        let link_half = create_link(&st, &fleet, OWN, OTHER).await;
        let link_far = create_link(&st, &fleet, OTHER, FAR).await;

        // One plate walking OWN -> OWN2 -> OTHER inside the transit window, so the proposer emits a
        // both-ends-held candidate AND a half-held one.
        let t0 = Utc::now() - TimeDelta::try_seconds(600).unwrap();
        seed_sighting(&st, "evt_own", OWN, "ABC123", t0).await;
        seed_sighting(
            &st,
            "evt_own2",
            OWN2,
            "ABC123",
            t0 + TimeDelta::try_seconds(120).unwrap(),
        )
        .await;
        seed_sighting(
            &st,
            "evt_other",
            OTHER,
            "ABC123",
            t0 + TimeDelta::try_seconds(240).unwrap(),
        )
        .await;
        seed_red_zone_entry(&st, "zn_own", OWN, "ze_own").await;
        seed_red_zone_entry(&st, "zn_other", OTHER, "ze_other").await;

        let (status, resp) = call(&st, &fleet, "POST", "/api/v1/movement/run", "{}").await;
        assert_eq!(status, StatusCode::OK, "engine run: {resp}");
        World {
            st,
            fleet,
            both_ends,
            one_end,
            neither,
            link_own,
            link_half,
            link_far,
        }
    }

    /// Candidate ids by (from, to), read straight from the table the engine wrote.
    async fn candidate(&self, from: &str, to: &str) -> String {
        sqlx::query_scalar::<_, String>(
            "SELECT id FROM movement_candidates WHERE from_camera = ? AND to_camera = ?",
        )
        .bind(from)
        .bind(to)
        .fetch_one(&self.st.pool)
        .await
        .unwrap_or_else(|e| panic!("no engine-proposed candidate {from}->{to}: {e}"))
    }

    async fn breach_on(&self, camera: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT id FROM breach_alerts WHERE camera_id = ?")
            .bind(camera)
            .fetch_one(&self.st.pool)
            .await
            .unwrap_or_else(|e| panic!("no engine-produced breach on {camera}: {e}"))
    }

    async fn link_exists(&self, id: &str) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM camera_links WHERE id = ?")
            .bind(id)
            .fetch_one(&self.st.pool)
            .await
            .unwrap()
            > 0
    }

    async fn candidate_status(&self, id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT status FROM movement_candidates WHERE id = ?")
            .bind(id)
            .fetch_one(&self.st.pool)
            .await
            .unwrap()
    }

    async fn breach_status(&self, id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT status FROM breach_alerts WHERE id = ?")
            .bind(id)
            .fetch_one(&self.st.pool)
            .await
            .unwrap()
    }
}

/// A body a scoped credential received must never carry a camera outside its scope, whatever the
/// status code. Checked over the SERIALIZED response, so a leak through any field — an error string,
/// a nested `signals` blob, an evidence path — is caught, not only through the one field a hand-written
/// assertion happened to look at.
fn assert_no_roster_leak(what: &str, status: StatusCode, body: &str, forbidden: &[&str]) {
    for cam in forbidden {
        assert!(
            !body.contains(cam),
            "{what} -> {status} leaked {cam} to a credential that does not hold it: {body}"
        );
    }
}

// ---- link CRUD -------------------------------------------------------------

/// LINK CRUD, the sharpest surface: a link is a two-camera object, and `registry:manage` (unlike
/// `events:read`) is scopable, so a camera-scoped credential really does reach these handlers.
#[tokio::test]
async fn link_crud_requires_both_ends_and_leaks_no_camera_either_way() {
    let w = World::build().await;

    // --- CREATE: naming a camera the credential does not hold is refused ---
    for (cred, name, from, to) in [
        (&w.one_end, "one_end", OWN, OTHER),
        (&w.one_end, "one_end", OTHER, OWN),
        (&w.one_end, "one_end", OTHER, FAR),
        (&w.neither, "neither", OWN, OWN2),
    ] {
        let body =
            serde_json::json!({ "from_camera": from, "to_camera": to, "transit_seconds": 60 })
                .to_string();
        let (status, resp) = call(&w.st, cred, "POST", "/api/v1/movement/links", &body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{name} created a link {from}->{to} it does not hold both ends of: {resp}"
        );
    }
    // The rows really are absent — a 403 that still wrote would be the worst of both.
    let extra: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM camera_links")
        .fetch_one(&w.st.pool)
        .await
        .unwrap();
    assert_eq!(extra, 3, "a refused create still wrote a link");

    // --- CREATE: the credential's OWN pair is allowed (no false deny) ---
    let (status, resp) = call(
        &w.st,
        &w.both_ends,
        "POST",
        "/api/v1/movement/links",
        &serde_json::json!({ "from_camera": OWN2, "to_camera": OWN }).to_string(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "both_ends was refused a link between its own two cameras: {resp}"
    );
    let mine = serde_json::from_str::<Value>(&resp).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // --- CREATE: a whitespace-padded self-link is still a self-link ---
    let (status, resp) = call(
        &w.st,
        &w.both_ends,
        "POST",
        "/api/v1/movement/links",
        &serde_json::json!({ "from_camera": OWN, "to_camera": format!(" {OWN}") }).to_string(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a padded id slipped past the self-link guard, which compares the RAW fields while the \
         insert binds the trimmed ones: {resp}"
    );

    // --- DELETE: identical refusal for half-held, not-held and nonexistent ---
    let (half_status, half) = call(
        &w.st,
        &w.one_end,
        "DELETE",
        &format!("/api/v1/movement/links/{}", w.link_half),
        "",
    )
    .await;
    let (far_status, far) = call(
        &w.st,
        &w.one_end,
        "DELETE",
        &format!("/api/v1/movement/links/{}", w.link_far),
        "",
    )
    .await;
    let (ghost_status, ghost) = call(
        &w.st,
        &w.one_end,
        "DELETE",
        "/api/v1/movement/links/lnk_does_not_exist",
        "",
    )
    .await;
    // ...and a credential holding NEITHER end of a link between two other cameras gets the same.
    let (none_status, none) = call(
        &w.st,
        &w.neither,
        "DELETE",
        &format!("/api/v1/movement/links/{}", w.link_own),
        "",
    )
    .await;
    assert_eq!(
        (none_status, none.as_str()),
        (ghost_status, ghost.as_str()),
        "`neither` can tell an existing link from a nonexistent one"
    );
    assert!(w.link_exists(&w.link_own).await);
    assert_eq!(half_status, StatusCode::FORBIDDEN, "{half}");
    assert_eq!(
        (half_status, half.as_str()),
        (far_status, far.as_str()),
        "half-held and not-held links answer differently — the boundary reports whether the \
         credential's own camera is an endpoint"
    );
    assert_eq!(
        (half_status, half.as_str()),
        (ghost_status, ghost.as_str()),
        "an existing link and a nonexistent id answer differently — the link id space is \
         enumerable, and each hit names a camera pair"
    );
    assert_no_roster_leak("DELETE half link", half_status, &half, &[OTHER, FAR]);
    assert!(
        !half.contains(&w.link_half),
        "the refusal echoes the probed id back: {half}"
    );
    assert!(
        w.link_exists(&w.link_half).await && w.link_exists(&w.link_far).await,
        "a refused delete still severed the topology"
    );

    // --- DELETE: the credential's own link goes (no false deny) ---
    let (status, resp) = call(
        &w.st,
        &w.both_ends,
        "DELETE",
        &format!("/api/v1/movement/links/{mine}"),
        "",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "both_ends could not delete the link it had just created: {resp}"
    );
    assert!(!w.link_exists(&mine).await);

    // --- RETARGET: there is no update route, and adding one must not skip the both-ends check ---
    for method in ["PATCH", "PUT"] {
        let (status, _) = call(
            &w.st,
            &w.one_end,
            method,
            &format!("/api/v1/movement/links/{}", w.link_half),
            &serde_json::json!({ "to_camera": OWN }).to_string(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} /movement/links/{{id}} now exists; retargeting a link changes WHICH cameras it \
             names, so it must re-run the both-ends check against the OLD and the NEW endpoints — \
             and this test must be extended to prove it does"
        );
    }

    // --- CONTROL: none of the above narrowed the unscoped credential ---
    let (status, body) = get(&w.st, &w.fleet, "/api/v1/movement/links").await;
    assert!(status.is_success(), "fleet GET links -> {status}: {body}");
    let links: Vec<Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(links.len(), 3, "fleet must still see the whole topology");
    assert!(body.contains(OTHER) && body.contains(FAR));
}

// ---- candidate review ------------------------------------------------------

/// Candidate review: a durable, attributed judgement on a claim about TWO cameras.
#[tokio::test]
async fn candidate_review_requires_both_ends_and_leaks_no_camera_either_way() {
    let w = World::build().await;
    let held = w.candidate(OWN, OWN2).await;
    let half = w.candidate(OWN, OTHER).await;

    // The credential holding both ends reviews its own candidate.
    let (status, resp) = call(
        &w.st,
        &w.both_ends,
        "POST",
        &format!("/api/v1/movement/candidates/{held}/confirm"),
        "{}",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED.min(StatusCode::OK),
        "both_ends was refused review of a candidate naming only its own cameras: {resp}"
    );
    assert_eq!(w.candidate_status(&held).await, "confirmed");
    assert_no_roster_leak("confirm own candidate", status, &resp, &[OTHER, FAR]);

    // One end held, no end held, and a nonexistent id are indistinguishable — and none of them
    // changed a row.
    let mut answers = Vec::new();
    for (cred, name, id) in [
        (&w.one_end, "one_end", half.as_str()),
        (&w.one_end, "one_end", "cand_does_not_exist"),
        (&w.neither, "neither", half.as_str()),
        (&w.neither, "neither", held.as_str()),
    ] {
        for action in ["confirm", "reject"] {
            let (status, resp) = call(
                &w.st,
                cred,
                "POST",
                &format!("/api/v1/movement/candidates/{id}/{action}"),
                "{}",
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{name} could {action} candidate {id}: {resp}"
            );
            assert_no_roster_leak("candidate review", status, &resp, &[OTHER, FAR]);
            answers.push(resp);
        }
    }
    assert!(
        answers.windows(2).all(|w| w[0] == w[1]),
        "candidate refusals differ between held/half-held/nonexistent, so the candidate id space \
         and its camera pairs are probeable: {answers:?}"
    );
    assert_eq!(
        w.candidate_status(&half).await,
        "pending",
        "a refused review still wrote a verdict onto another camera's candidate"
    );
    assert_eq!(
        w.candidate_status(&held).await,
        "confirmed",
        "the `neither` credential overwrote a verdict on a candidate it does not hold"
    );

    // CONTROL: the unscoped credential still reviews anything, including the half-held candidate.
    let (status, resp) = call(
        &w.st,
        &w.fleet,
        "POST",
        &format!("/api/v1/movement/candidates/{half}/reject"),
        "{}",
    )
    .await;
    assert!(status.is_success(), "fleet reject -> {status}: {resp}");
    assert_eq!(w.candidate_status(&half).await, "rejected");
}

// ---- breach workflow -------------------------------------------------------

/// Breach work: unlike a link or a candidate, a breach names exactly ONE camera, so the containment
/// rule is the ordinary one and the false-deny risk is the sharper half.
#[tokio::test]
async fn breach_work_is_confined_to_the_breachs_own_camera() {
    let w = World::build().await;
    let own = w.breach_on(OWN).await;
    let other = w.breach_on(OTHER).await;

    // Own camera: allowed, both transitions.
    for (action, expect) in [("ack", "acknowledged"), ("resolve", "resolved")] {
        let (status, resp) = call(
            &w.st,
            &w.one_end,
            "POST",
            &format!("/api/v1/movement/breaches/{own}/{action}"),
            "{}",
        )
        .await;
        assert!(
            status.is_success(),
            "one_end was refused {action} on a breach on its OWN camera -> {status}: {resp}"
        );
        assert_eq!(w.breach_status(&own).await, expect);
        assert_no_roster_leak("breach work", status, &resp, &[OTHER, FAR]);
    }

    // Another camera's breach, and a nonexistent id: same refusal, and the incident stays open.
    let (a_status, a) = call(
        &w.st,
        &w.one_end,
        "POST",
        &format!("/api/v1/movement/breaches/{other}/ack"),
        "{}",
    )
    .await;
    let (g_status, g) = call(
        &w.st,
        &w.one_end,
        "POST",
        "/api/v1/movement/breaches/brc_does_not_exist/ack",
        "{}",
    )
    .await;
    assert_eq!(a_status, StatusCode::FORBIDDEN, "{a}");
    assert_eq!(
        (a_status, a.as_str()),
        (g_status, g.as_str()),
        "an existing out-of-scope breach and a nonexistent one answer differently — the alert id \
         space is enumerable"
    );
    assert_no_roster_leak("breach ack", a_status, &a, &[OTHER, FAR]);
    assert_eq!(
        w.breach_status(&other).await,
        "open",
        "a refused ack silently retired another operator's open incident"
    );

    // A camera-less breach has no camera by which a scoped credential could hold it — refused for a
    // scoped caller, and unchanged for the fleet.
    sqlx::query(
        "INSERT INTO breach_alerts (id, camera_id, rule, severity, status, detail, created_at)
         VALUES ('brc_null', NULL, 'red_zone_entry', 'warning', 'open', '{}', ?)",
    )
    .bind(Utc::now())
    .execute(&w.st.pool)
    .await
    .unwrap();
    let (status, resp) = call(
        &w.st,
        &w.one_end,
        "POST",
        "/api/v1/movement/breaches/brc_null/ack",
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{resp}");
    let (status, resp) = call(
        &w.st,
        &w.fleet,
        "POST",
        "/api/v1/movement/breaches/brc_null/ack",
        "{}",
    )
    .await;
    assert!(
        status.is_success(),
        "the unscoped credential lost access to a camera-less breach -> {status}: {resp}"
    );
    // CONTROL: and to the out-of-scope one.
    let (status, resp) = call(
        &w.st,
        &w.fleet,
        "POST",
        &format!("/api/v1/movement/breaches/{other}/resolve"),
        "{}",
    )
    .await;
    assert!(status.is_success(), "fleet resolve -> {status}: {resp}");
}

// ---- engine run ------------------------------------------------------------

/// `POST /movement/run` sweeps EVERY link and EVERY zone event on the box and takes no camera id, so
/// containment is a refusal. The control is that it still works, and still produces cross-camera work,
/// for the fleet credential — `World::build` depends on it, so a run that silently did nothing would
/// make every other test in this file vacuous.
#[tokio::test]
async fn the_engine_run_is_refused_to_a_scoped_credential_and_still_works_for_the_fleet() {
    let w = World::build().await;
    for (cred, name) in [
        (&w.both_ends, "both_ends"),
        (&w.one_end, "one_end"),
        (&w.neither, "neither"),
    ] {
        let (status, resp) = call(&w.st, cred, "POST", "/api/v1/movement/run", "{}").await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{name} drove the fleet-wide movement engines: {resp}"
        );
    }
    // The run in `build()` really did the cross-camera work these tests stand on.
    let candidates: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM movement_candidates")
        .fetch_one(&w.st.pool)
        .await
        .unwrap();
    let breaches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM breach_alerts")
        .fetch_one(&w.st.pool)
        .await
        .unwrap();
    assert!(
        candidates >= 2 && breaches >= 2,
        "the engines produced {candidates} candidates and {breaches} breaches; every assertion in \
         this file about engine-produced rows would be vacuous"
    );
}

// ---- reads -----------------------------------------------------------------

/// Every movement READ, driven with all four credentials.
///
/// Each read must satisfy ONE of two properties, and the test says which it observed:
///
///   * refused for a CAPABILITY the credential cannot hold (today: `events:read` is unscopable, so
///     this is what every one of them does), or
///   * answered, and named no camera outside the caller's scope.
///
/// Written this way rather than as "assert 403" so that making `events:read` scopable turns these into
/// live confinement assertions automatically instead of leaving the handlers unchecked. Either way the
/// roster-containment check runs over the body, so a refusal that names another camera fails too.
#[tokio::test]
async fn read_routes_are_confined_or_capability_unreachable() {
    let w = World::build().await;
    let at = query_ts();
    let reads: Vec<String> = vec![
        "/api/v1/movement/links".to_string(),
        "/api/v1/movement/candidates".to_string(),
        "/api/v1/movement/candidates?anchor=ABC123".to_string(),
        "/api/v1/movement/candidates?status=pending&limit=5000".to_string(),
        "/api/v1/movement/breaches".to_string(),
        "/api/v1/movement/breaches?status=open&limit=5000".to_string(),
        "/api/v1/movement/search/plate/ABC-123".to_string(),
        format!("/api/v1/movement/search/person?camera={OWN}&track=trk_1&at={at}"),
        format!("/api/v1/movement/search/person?camera={OTHER}&track=trk_1&at={at}"),
        "/api/v1/modules/movement/ui/index.js".to_string(),
    ];

    // Counted apart rather than summed: rounding "the capability wall stopped it" up into the same
    // figure as "the scope filter answered and confined it" is how a coverage number starts
    // reassuring instead of informing. A movement of the FIRST number is the signal that
    // `events:read` became scopable and the confinement is now the live boundary.
    let mut by_capability = 0usize;
    let mut by_scope = 0usize;
    let mut answered = 0usize;
    for path in &reads {
        for (cred, name, forbidden) in [
            (&w.both_ends, "both_ends", vec![OTHER, FAR]),
            (&w.one_end, "one_end", vec![OWN2, OTHER, FAR]),
            (&w.neither, "neither", vec![OWN, OWN2, OTHER]),
        ] {
            let (status, body) = get(&w.st, cred, path).await;
            assert_no_roster_leak(&format!("{name} GET {path}"), status, &body, &forbidden);
            if status == StatusCode::FORBIDDEN {
                if body.contains("missing capability") {
                    by_capability += 1;
                } else if body.contains("not scoped") {
                    by_scope += 1;
                } else {
                    panic!("{name} GET {path} -> 403 for neither a capability nor a scope reason: {body}");
                }
            } else {
                assert!(status.is_success(), "{name} GET {path} -> {status}: {body}");
                answered += 1;
            }
        }
    }
    eprintln!(
        "movement reads over {} route/credential pairs: {by_capability} refused on an UNHOLDABLE \
         capability (unreachable by design, NOT proof of confinement), {by_scope} refused on scope, \
         {answered} answered and confined",
        reads.len() * 3
    );
    assert_eq!(
        by_capability + by_scope + answered,
        reads.len() * 3,
        "a route/credential pair was neither refused nor answered"
    );

    // CONTROL: the unscoped credential reads all of it, cross-camera content included. Without this,
    // gating every read on an unholdable capability would satisfy the loop above.
    for path in &reads {
        let (status, body) = get(&w.st, &w.fleet, path).await;
        assert!(status.is_success(), "fleet GET {path} -> {status}: {body}");
    }
    let (_, links) = get(&w.st, &w.fleet, "/api/v1/movement/links").await;
    assert!(links.contains(OTHER) && links.contains(FAR));
    let (_, cands) = get(&w.st, &w.fleet, "/api/v1/movement/candidates").await;
    assert!(
        cands.contains(OTHER),
        "the fleet credential lost the cross-camera candidate: {cands}"
    );
    let (_, trail) = get(&w.st, &w.fleet, "/api/v1/movement/search/plate/ABC-123").await;
    let trail: Value = serde_json::from_str(&trail).unwrap();
    assert_eq!(
        trail["appearances"].as_array().unwrap().len(),
        3,
        "the fleet plate trail lost sightings: {trail}"
    );
    let (_, walk) = get(
        &w.st,
        &w.fleet,
        &format!("/api/v1/movement/search/person?camera={OWN}&track=trk_1&at={at}"),
    )
    .await;
    assert!(
        walk.contains("candidates"),
        "the fleet person walk returned no candidate envelope: {walk}"
    );
}

// ---- accountability --------------------------------------------------------

/// A credential holding BOTH ends must see its own two-camera acts.
///
/// `subject_camera_id` is deliberately NULL for an act spanning two cameras — one column cannot say
/// "both ends", and naming either one would disclose adjacency to that end's holder. But that
/// argument is about a HALF-holder. It says nothing about a caller that already holds every camera
/// involved and performed the act itself, and applying it to them meant a credential could create and
/// delete a link (both allowed — it owns both cameras) and then find its own acts absent from its own
/// audit trail. `list_audit` now also admits a subject-less row whose `detail.camera_ids` is entirely
/// within the caller's scope.
///
/// This is the one direction the four-credential matrix never drove: `/api/v1/audit` was read with
/// `fleet` and `one_end` only, and `both_ends` — the single credential that exposes this — was never
/// pointed at it.
#[tokio::test]
async fn a_both_ends_credential_sees_its_own_two_camera_acts() {
    let w = World::build().await;

    // `both_ends` deletes the OWN -> OWN2 link the fixture seeded, then re-creates it. Both acts
    // name two cameras and both are permitted, because it holds each end.
    let (status, resp) = call(
        &w.st,
        &w.both_ends,
        "DELETE",
        &format!("/api/v1/movement/links/{}", w.link_own),
        "",
    )
    .await;
    assert!(status.is_success(), "both_ends delete -> {status}: {resp}");
    let (status, resp) = call(
        &w.st,
        &w.both_ends,
        "POST",
        "/api/v1/movement/links",
        &format!(r#"{{"from_camera":"{OWN}","to_camera":"{OWN2}"}}"#),
    )
    .await;
    assert!(
        status.is_success(),
        "both_ends may link its own two cameras -> {status}: {resp}"
    );

    let (status, body) = get(&w.st, &w.both_ends, "/api/v1/audit?limit=5000").await;
    assert!(
        status.is_success(),
        "both_ends GET /audit -> {status}: {body}"
    );
    assert!(
        body.contains("movement_link_create"),
        "a credential holding both ends cannot see the link it created: {body}"
    );
    assert!(
        body.contains("movement_link_delete"),
        "a credential holding both ends cannot see the link it deleted: {body}"
    );

    // ...and a HALF-holder still cannot: that is the inference the NULL subject protects.
    let (status, body) = get(&w.st, &w.one_end, "/api/v1/audit?limit=5000").await;
    assert!(
        status.is_success(),
        "one_end GET /audit -> {status}: {body}"
    );
    assert!(
        !body.contains("movement_link_create") && !body.contains("movement_link_delete"),
        "a credential holding ONE end learned of a link to a camera it does not hold: {body}"
    );
}

/// The audit trail of movement's own actions, read back through `GET /api/v1/audit`.
///
/// That route filters on `audit_log.subject_camera_id` and is FAIL-CLOSED: a NULL subject is hidden
/// from a camera-scoped reader. `registry:manage` is scopable, so a scoped operator really does read
/// it — and movement audited every action with an empty `detail`, so `subject_camera_id` was NULL on
/// every row. A scoped operator's own single-camera acts were therefore invisible in its own audit
/// trail while the fleet auditor saw them: the same hole the kernel's archive export had, reached
/// from this crate.
///
/// The rule asserted here (see `routes::audit_subject`): an act about exactly ONE camera names it;
/// an act spanning TWO stays NULL, because `subject_camera_id` cannot express "both ends" and naming
/// either one would show the row to a credential holding only that end — the very inference the
/// both-ends rule denies.
#[tokio::test]
async fn a_single_camera_act_is_visible_in_its_own_cameras_audit_trail() {
    let w = World::build().await;
    let own = w.breach_on(OWN).await;
    let other = w.breach_on(OTHER).await;

    // The FLEET credential works both breaches, so what follows tests the audit READER's scope and
    // not the actor's — the reader must see acts on its camera whoever performed them.
    for id in [&own, &other] {
        let (status, resp) = call(
            &w.st,
            &w.fleet,
            "POST",
            &format!("/api/v1/movement/breaches/{id}/ack"),
            "{}",
        )
        .await;
        assert!(status.is_success(), "fleet ack {id} -> {status}: {resp}");
    }
    let (status, resp) = get(
        &w.st,
        &w.fleet,
        &format!(
            "/api/v1/movement/search/person?camera={OWN}&track=trk_1&at={}",
            query_ts()
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "fleet person search -> {status}: {resp}"
    );

    // The holder of OWN sees the acts on OWN...
    let (status, body) = get(&w.st, &w.one_end, "/api/v1/audit?limit=5000").await;
    assert!(
        status.is_success(),
        "one_end GET /audit -> {status}: {body}"
    );
    assert!(
        body.contains(&own),
        "the breach acknowledged on this credential's OWN camera is missing from its audit trail — \
         movement's audit rows carry no subject camera, so `subject_camera_id` is NULL and the \
         fail-closed filter hides them: {body}"
    );
    assert!(
        body.contains("movement_search_person"),
        "an identity-like person search over this credential's own camera is missing from its audit \
         trail: {body}"
    );
    // ...and nothing about the camera it does not hold.
    assert_no_roster_leak("one_end GET /audit", status, &body, &[OTHER, FAR, OWN2]);
    assert!(
        !body.contains(&other),
        "the audit trail handed a scoped reader an act on a camera it does not hold: {body}"
    );

    // A two-camera act stays fleet-only, deliberately: `subject_camera_id` cannot say "both ends",
    // and naming one end would tell its holder that the camera is linked to something.
    assert!(
        !body.contains("movement_link_create"),
        "a cross-camera act became visible to a credential holding one end — it now learns its \
         camera is adjacent to something: {body}"
    );

    // CONTROL: the fleet auditor still sees everything, cross-camera acts included.
    let (status, all) = get(&w.st, &w.fleet, "/api/v1/audit?limit=5000").await;
    assert!(status.is_success(), "fleet GET /audit -> {status}: {all}");
    for must in [
        own.as_str(),
        other.as_str(),
        "movement_link_create",
        "movement_search_person",
    ] {
        assert!(all.contains(must), "fleet audit trail lost {must}: {all}");
    }
}

// ---- the search executor, over movement's own data -------------------------

/// `heldar-search` reads movement's breach alerts through the `breach_alerts_read` contract, and its
/// executor fetches each source with `ORDER BY … DESC LIMIT fetch_cap` before filtering by camera.
/// A page filled by cameras the caller did not name therefore evicted the caller's own matches, and
/// `truncated` — raised from that unfiltered count — reported the FLEET's volume.
///
/// `heldar-search/tests/camera_confinement.rs` pins the executor directly with a deterministic cap;
/// this is the composed-route half, over rows movement produced, so the fix is proven where it is
/// actually consumed rather than only where it is defined.
#[tokio::test]
async fn a_camera_confined_search_over_movement_breaches_survives_a_full_page() {
    let w = World::build().await;
    // `SearchConfig::max_results` defaults to 200 ⇒ fetch_cap 1000. The caller's own breach is the
    // OLDEST row, so every filler below outranks it in the page.
    sqlx::query("UPDATE breach_alerts SET created_at = ? WHERE camera_id = ?")
        .bind(Utc::now() - TimeDelta::try_hours(20).unwrap())
        .bind(OWN)
        .execute(&w.st.pool)
        .await
        .unwrap();
    let now = Utc::now();
    let mut tx = w.st.pool.begin().await.unwrap();
    for i in 0..1_100i64 {
        sqlx::query(
            "INSERT INTO breach_alerts (id, camera_id, rule, severity, status, detail, created_at)
             VALUES (?, ?, 'red_zone_entry', 'warning', 'open', '{}', ?)",
        )
        .bind(format!("brc_filler_{i}"))
        .bind(OTHER)
        .bind(now - TimeDelta::try_seconds(i).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let body = serde_json::json!({ "cameras": [OWN], "sources": ["breach"] }).to_string();
    let (status, resp) = call(&w.st, &w.fleet, "POST", "/api/v1/search/events", &body).await;
    assert!(status.is_success(), "search -> {status}: {resp}");
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(
        (v["count"].as_i64().unwrap(), v["truncated"].as_bool().unwrap()),
        (1, false),
        "(count, truncated) — the caller's own breach was evicted from the fetch page by 1100 newer \
         rows on a camera it did not name, and/or `truncated` reported the fleet's in-window volume \
         rather than the caller's: {resp}"
    );
    assert_no_roster_leak("confined search", status, &resp, &[OTHER, FAR]);
}
