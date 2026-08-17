//! The camera-scope route matrix.
//!
//! Camera scope was ADVERTISED on API keys long before it was enforced, and the gap was found by an
//! external audit rather than by this repository. A hand-written list of cases would not have caught
//! it and will not catch the next one: it goes stale the moment someone adds a route. So this test
//! ENUMERATES the camera-keyed routes out of the source and drives every one of them.
//!
//! For each route it asserts three things against a credential scoped to `camera_a`:
//!
//!   camera_a      -> NOT 403   (the scope is not vacuously denying everything)
//!   camera_b      -> 403       (a camera the credential does not hold)
//!   camera_zzzz   -> 403       (a plausible id that does NOT exist — same answer as camera_b, so
//!                               the boundary cannot be used to enumerate the fleet)
//!
//! The third is the one that is easy to get wrong: answering 404 for a nonexistent camera and 403 for
//! an out-of-scope one turns every camera-keyed route into an existence oracle.
//!
//! A newly added `/cameras/{id}/…` route is picked up automatically and fails here until it is
//! scoped, which is the property that makes this worth having.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use heldar_kernel::state::AppState;
use tower::Service;

/// Repo root, from this crate's manifest dir.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/heldar-server -> repo root")
        .to_path_buf()
}

/// Every source file that can register routes.
fn route_sources() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = Vec::new();
    for crate_dir in [
        "crates/heldar-kernel/src/routes",
        "crates/heldar-entry/src",
        "crates/heldar-movement/src",
        "crates/heldar-search/src",
    ] {
        let dir = root.join(crate_dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    assert!(!out.is_empty(), "found no route sources to scan");
    out
}

/// A camera-keyed route discovered in the source: the registered path plus the HTTP methods on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CameraRoute {
    path: String,
    methods: Vec<String>,
}

/// Scrape `.route("<path>", <methods>)` registrations whose path is keyed by a CAMERA id.
///
/// Deliberately a source scan rather than a hand-maintained list: the failure this guards is someone
/// adding a route and not scoping it, and a list they would also have to remember to update cannot
/// catch that.
fn discover_camera_routes() -> Vec<CameraRoute> {
    let mut found: BTreeSet<CameraRoute> = BTreeSet::new();
    for file in route_sources() {
        let src = std::fs::read_to_string(&file).unwrap_or_default();
        for (idx, _) in src.match_indices(".route(") {
            let tail = &src[idx..];
            // The registration's argument list, bounded so a malformed match cannot run away.
            // Bound to THIS `.route(...)` call by balancing parens — a fixed-size window bleeds into
            // the next registration and attributes its methods to this path.
            let mut depth = 0i32;
            let mut end = tail.len();
            for (i, c) in tail.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let chunk = &tail[..end];
            let Some(q1) = chunk.find('"') else { continue };
            let Some(q2) = chunk[q1 + 1..].find('"') else {
                continue;
            };
            let path = &chunk[q1 + 1..q1 + 1 + q2];
            // Camera-keyed means the path is parameterised by a CAMERA id. The kernel keys on
            // `/api/v1/cameras/{id}`; the app crates use their own prefixes with an explicit
            // `{camera_id}` (e.g. `/api/v1/entry/gate/open/{camera_id}`), and missing those was the
            // whole reason the app crates went unscoped.
            let kernel_keyed = path.starts_with("/api/v1/cameras/{id}");
            let app_keyed = path.contains("{camera_id}");
            if !kernel_keyed && !app_keyed {
                continue;
            }
            let mut methods = Vec::new();
            for m in ["get", "post", "patch", "delete", "put"] {
                // `get(handler)` / `axum::routing::patch(handler)` inside this registration.
                if chunk[q1 + q2..].contains(&format!("{m}(")) {
                    methods.push(m.to_uppercase());
                }
            }
            if methods.is_empty() {
                methods.push("GET".into());
            }
            found.insert(CameraRoute {
                path: path.to_string(),
                methods,
            });
        }
    }
    found.into_iter().collect()
}

/// The COMPOSED router — kernel + entry + movement + search — mirroring `heldar_server::run`.
///
/// Building only the kernel router was a real coverage hole: the app crates are where 47 routes had
/// no scope call at all, and a matrix that never routes to them proves nothing about them. If this
/// drifts from the composition in `lib.rs`, the app-crate assertions silently stop running.
fn composed_router(st: &AppState) -> axum::Router {
    let movement_cfg = std::sync::Arc::new(heldar_movement::config::MovementConfig::from_env());
    let search_cfg = std::sync::Arc::new(heldar_search::config::SearchConfig::from_env());
    axum::Router::new()
        .merge(heldar_kernel::routes::api_router())
        // `/metrics` is mounted at the SERVER layer, not inside `api_router()`. Merging it here keeps
        // this matrix's router the same shape as the one `heldar_server::build_app` actually serves —
        // it was invisible to the matrix until it moved, which is how a fleet-wide exposition sat
        // outside the scope sweep.
        .merge(heldar_kernel::routes::metrics::router())
        .merge(heldar_entry::routes::router())
        .merge(heldar_movement::routes::router(movement_cfg))
        .merge(heldar_search::routes::router(search_cfg))
        .with_state(st.clone())
}

/// The `/media/*` plane merged onto the API router, wired exactly as `heldar_server::run` wires it:
/// `nest_service` per subtree behind `media_scope::guard`.
///
/// The archive tests need this because the defect they pin lives in the SEAM between the producer's
/// attribution key and the key the guard derives from the served URL. A test that only calls the API
/// cannot see that seam — only an actual fetch through the guard crosses it.
fn composed_router_with_media(st: &AppState) -> axum::Router {
    let media = axum::Router::new()
        .nest_service(
            "/media/archives",
            tower_http::services::ServeDir::new(&st.cfg.archive_dir),
        )
        .layer(axum::middleware::from_fn_with_state(
            st.clone(),
            heldar_kernel::services::media_scope::guard,
        ));
    composed_router(st).merge(media)
}

async fn test_state() -> AppState {
    test_state_with(|_| {}).await
}

/// [`test_state`] with a hook to adjust config before the state is frozen. The archive tests point
/// `archive_dir` at their own scratch tree so a real export never writes into the developer's box.
async fn test_state_with(tune: impl FnOnce(&mut heldar_kernel::config::Config)) -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = true;
    tune(&mut cfg);
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
        started_at: chrono::Utc::now(),
        pool,
        cfg,
    }
}

async fn seed_camera(st: &AppState, id: &str) {
    let now = chrono::Utc::now();
    sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
        .bind(id)
        .bind(format!("Camera {id}"))
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
    // Seed the rows the read routes need. Without them an unscoped route answers 404 for BOTH
    // cameras and looks indistinguishable from a scoped one — the matrix would pass on absent data
    // rather than on enforcement. `/cameras/{id}/health` was exactly that case.
    sqlx::query("INSERT INTO camera_status (camera_id, state, updated_at) VALUES (?, 'idle', ?)")
        .bind(id)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
}

/// Mint a real API key row and return its plaintext token.
///
/// `cameras: None` mints an UNSCOPED key — the control that proves this matrix is not simply
/// asserting that everything is denied.
async fn seed_key(st: &AppState, cameras: Option<&[&str]>) -> String {
    let token = format!("vok_{}", uuid::Uuid::new_v4().simple());
    let id = format!("key_{}", uuid::Uuid::new_v4().simple());
    let (kind, list) = match cameras {
        Some(c) => (
            "cameras",
            Some(
                serde_json::to_string(&c.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                    .unwrap(),
            ),
        ),
        None => ("all", None),
    };
    // The strongest adversary that can ACTUALLY EXIST. Capabilities are orthogonal to scope
    // (`camera_allowed` does not exempt Cap::Admin), so a maximal cap set isolates the scope boundary:
    // if a route lets this key through, only a missing scope check can be responsible.
    //
    // It is deliberately NOT `admin` + no grant, which is what this fixture used to insert. That
    // combination is now refused at mint AND denied on the read path (a camera scope cannot filter
    // the cross-camera reads Admin implies), so a test built on it would assert against a principal
    // that cannot authenticate — vacuously, and silently. Everything except `admin` and the two
    // unscopable caps, which is exactly the widest grant the product will pair with a camera scope.
    let caps = serde_json::to_string(&[
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
    ])
    .unwrap();
    let caps = if cameras.is_some() { Some(caps) } else { None };
    sqlx::query(
        "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at,
                               capabilities, scope_kind, scope_cameras, expires_at)
         VALUES (?,?,?,?,'admin',1,?,?,?,?,NULL)",
    )
    .bind(&id)
    .bind("matrix")
    .bind(heldar_kernel::auth::token_hash(&token))
    .bind(&token[..8])
    .bind(chrono::Utc::now())
    .bind(caps)
    .bind(kind)
    .bind(list)
    .execute(&st.pool)
    .await
    .unwrap();
    token
}

/// Mint a credential the way a real operator does — `POST /api/v1/api-keys` through the router —
/// so the test exercises a shape the product can actually issue.
///
/// `seed_key` above writes to `api_keys` DIRECTLY, and writes `role='admin'` with no capability grant.
/// That is a deliberate over-privileged adversary for the ESCAPE direction (a superset adversary
/// proves more, not less), but it is worthless in the DENIAL direction: `validate_grant` now refuses
/// `admin` + `scope_kind: cameras` outright, so that fixture is a credential no deployment can hold.
/// A false-deny test built on it asserts against a principal that does not exist in production — which
/// is exactly how the archive false-deny shipped inert with this suite green. Anything asserting that
/// a scoped credential CAN do something must mint here.
async fn mint_key(st: &AppState, caps: &[&str], cameras: Option<&[&str]>) -> String {
    // A real unscoped admin has to exist to mint anything; that is the bootstrap, not the subject.
    let admin = seed_key(st, None).await;
    let (kind, list) = match cameras {
        Some(c) => ("cameras", serde_json::json!(c)),
        None => ("all", serde_json::json!([])),
    };
    let body = serde_json::json!({
        "name": "minted",
        "role": "integration",
        "capabilities": caps,
        "scope_kind": kind,
        "scope_cameras": list,
        "confirm_privileged": true,
    })
    .to_string();
    let (status, resp) = call_body(st, &admin, "POST", "/api/v1/api-keys", &body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the API refused to mint {caps:?} scoped to {cameras:?} — a denial-direction test cannot be \
         written against a credential the product will not issue: {resp}"
    );
    serde_json::from_str::<serde_json::Value>(&resp)
        .ok()
        .and_then(|v| v.get("key").and_then(|k| k.as_str()).map(String::from))
        .unwrap_or_else(|| panic!("mint response carried no plaintext key: {resp}"))
}

async fn call(st: &AppState, token: &str, method: &str, path: &str) -> StatusCode {
    let mut app = composed_router(st);
    let needs_body = matches!(method, "POST" | "PATCH" | "PUT");
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("X-API-Key", token)
        .header("content-type", "application/json")
        .body(if needs_body {
            Body::from("{}")
        } else {
            Body::empty()
        })
        .unwrap();
    app.call(req).await.unwrap().status()
}

/// THE MATRIX.
#[tokio::test]
async fn camera_scope_holds_on_every_camera_keyed_route() {
    let routes = discover_camera_routes();
    assert!(
        routes.len() >= 8,
        "discovery found only {} camera-keyed routes — the scraper is probably broken, and a broken \
         scraper silently asserts nothing",
        routes.len()
    );

    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    let scoped = seed_key(&st, Some(&["camera_a"])).await;
    let unscoped = seed_key(&st, None).await;

    let mut leaks: Vec<String> = Vec::new();
    let mut oracles: Vec<String> = Vec::new();
    let mut vacuous: Vec<String> = Vec::new();
    let mut unproven: Vec<String> = Vec::new();
    // Routes a camera-scoped credential can never reach because their capability is unscopable. Not
    // a failure — a fact about the product worth printing, so the count moving is visible in CI.
    let mut unreachable: Vec<String> = Vec::new();

    for route in &routes {
        for method in &route.methods {
            let for_cam = |cam: &str| route.path.replace("{id}", cam).replace("{camera_id}", cam);

            let b = call(&st, &scoped, method, &for_cam("camera_b")).await;
            // The security property is that the request does not SUCCEED. A 4xx that is not 403 (a
            // body the `Json` extractor rejected before the handler ran) is not a leak — but it also
            // does not prove the scope check, so it is recorded as unproven below rather than passed.
            if b.is_success() {
                leaks.push(format!(
                    "{method} {} -> camera_b SUCCEEDED ({b})",
                    route.path
                ));
            } else if b != StatusCode::FORBIDDEN {
                unproven.push(format!(
                    "{method} {} -> camera_b {b} (rejected before the handler)",
                    route.path
                ));
            }
            // A plausible id that does not exist must answer EXACTLY as an out-of-scope one.
            let z = call(&st, &scoped, method, &for_cam("camera_zzzz")).await;
            if z != b {
                oracles.push(format!(
                    "{method} {} -> nonexistent {z} vs out-of-scope {b} (existence oracle)",
                    route.path
                ));
            }
            // Control: the scope must not be denying the credential its OWN camera.
            //
            // A 403 can mean two different things and only one of them is a bug here. If the body
            // says `missing capability`, the route is gated on something this credential does not
            // hold — and for the UNSCOPABLE caps (events:read, identity:read) no camera-scoped
            // credential can EVER hold it, so the route is out of reach by design rather than
            // wrongly denied. Only a 403 that survives with the capability present indicts the scope.
            let (a, a_body) = call_body(&st, &scoped, method, &for_cam("camera_a"), "{}").await;
            if a == StatusCode::FORBIDDEN {
                if a_body.contains("missing capability") {
                    unreachable.push(format!(
                        "{method} {} -> gated on a capability a scoped credential cannot hold",
                        route.path
                    ));
                } else {
                    vacuous.push(format!("{method} {} -> camera_a wrongly 403", route.path));
                }
            }
        }
    }

    assert!(
        leaks.is_empty(),
        "camera-scoped credential reached cameras it does not hold:\n  {}",
        leaks.join("\n  ")
    );
    assert!(
        oracles.is_empty(),
        "scope boundary leaks camera EXISTENCE:\n  {}",
        oracles.join("\n  ")
    );
    assert!(
        vacuous.is_empty(),
        "scope wrongly denied the credential's OWN camera (this matrix must not pass by denying \
         everything):\n  {}",
        vacuous.join("\n  ")
    );

    // Honesty: routes whose body the extractor rejected never reached their scope check, so this
    // matrix does NOT prove them. Printed rather than silently counted as passes.
    if !unproven.is_empty() {
        eprintln!(
            "route-scope matrix: {} route(s) NOT PROVEN (request rejected before the handler; \
             send a valid body to cover them):\n  {}",
            unproven.len(),
            unproven.join("\n  ")
        );
    }

    if !unreachable.is_empty() {
        eprintln!(
            "route-scope matrix: {} route(s) UNREACHABLE by any camera-scoped credential (gated on \
             an unscopable capability — by design, not a gap):\n  {}",
            unreachable.len(),
            unreachable.join("\n  ")
        );
    }

    // Control: an UNSCOPED credential still reaches BOTH cameras. Without this, deleting every route
    // would make the assertions above pass.
    for route in &routes {
        for method in &route.methods {
            for cam in ["camera_a", "camera_b"] {
                let s = call(&st, &unscoped, method, &route.path.replace("{id}", cam)).await;
                assert_ne!(
                    s,
                    StatusCode::FORBIDDEN,
                    "UNSCOPED credential was refused {method} {} for {cam} — scope must be a no-op \
                     for Scope::All",
                    route.path
                );
            }
        }
    }
}

/// The discovery itself is load-bearing, so pin what it finds.
#[tokio::test]
async fn discovery_finds_the_known_sensitive_routes() {
    let paths: BTreeSet<String> = discover_camera_routes()
        .into_iter()
        .map(|r| r.path)
        .collect();
    // Print the inventory so a coverage regression is visible in CI output, not just in a pass/fail.
    eprintln!("route matrix covers {} camera-keyed routes:", paths.len());
    for p in &paths {
        eprintln!("  {p}");
    }
    for must in [
        "/api/v1/cameras/{id}/liveview",
        "/api/v1/cameras/{id}/clip",
        "/api/v1/cameras/{id}/snapshot",
        // The app crates. Gate open is the sharpest: a camera-scoped guard credential physically
        // opening another camera's barrier is a real-world side effect, not a data leak.
        "/api/v1/entry/gate/open/{camera_id}",
    ] {
        assert!(
            paths.contains(must),
            "route discovery missed {must} — it is one of the routes this whole repair exists for.\n\
             found: {paths:#?}"
        );
    }
}

/// Call helper that returns (status, body) so escalation attempts can be inspected.
async fn call_body(
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
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// PRIVILEGE ESCALATION, not per-route scope.
///
/// An adversarial review demonstrated that the camera boundary could be escaped WITHOUT touching a
/// single camera-keyed route: a scoped credential could PATCH its own key to `scope_kind: "all"`, or
/// POST a brand-new unscoped key and get the plaintext token back. Per-route scoping cannot see that
/// class at all, so every exploit it proved is pinned here.
#[tokio::test]
async fn a_scoped_credential_cannot_escape_through_the_credential_surface() {
    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    let scoped = seed_key(&st, Some(&["camera_a"])).await;

    // Baseline: the boundary holds before the attempt.
    let (before, _) = call_body(&st, &scoped, "GET", "/api/v1/cameras/camera_b/health", "").await;
    assert_eq!(
        before,
        StatusCode::FORBIDDEN,
        "precondition: camera_b must be refused"
    );

    // 1. Mint a NEW unscoped key.
    let (mint, body) = call_body(
        &st,
        &scoped,
        "POST",
        "/api/v1/api-keys",
        r#"{"name":"escape","role":"admin"}"#,
    )
    .await;
    assert_eq!(
        mint,
        StatusCode::FORBIDDEN,
        "a scoped credential minted a key (body: {body}) — it can hand itself an unscoped token"
    );

    // 2. Widen ITSELF. The id is discovered from the listing if that is even permitted; either way
    //    the PATCH surface must be closed to a scoped caller.
    let (widen, body) = call_body(
        &st,
        &scoped,
        "PATCH",
        "/api/v1/api-keys/whatever",
        r#"{"scope_kind":"all"}"#,
    )
    .await;
    assert_eq!(
        widen,
        StatusCode::FORBIDDEN,
        "a scoped credential reached the key-update surface (body: {body})"
    );

    // 3. Register a module — mints a sidecar key and returns its plaintext token.
    let (module, body) = call_body(
        &st,
        &scoped,
        "POST",
        "/api/v1/modules",
        r#"{"id":"escape2","name":"x","base_url":"http://127.0.0.1:9"}"#,
    )
    .await;
    assert_eq!(
        module,
        StatusCode::FORBIDDEN,
        "a scoped credential registered a module (body: {body}) — module registration mints a key"
    );

    // 4. Users carry Scope::All by construction.
    let (user, _) = call_body(
        &st,
        &scoped,
        "POST",
        "/api/v1/users",
        r#"{"username":"esc","password":"correct-horse-battery","role":"admin"}"#,
    )
    .await;
    assert_eq!(
        user,
        StatusCode::FORBIDDEN,
        "a scoped credential created a user"
    );

    // The boundary still holds afterwards — no attempt partially succeeded.
    let (after, _) = call_body(&st, &scoped, "GET", "/api/v1/cameras/camera_b/health", "").await;
    assert_eq!(
        after,
        StatusCode::FORBIDDEN,
        "camera_b became reachable after the attempts"
    );
}

/// EGRESS, not per-route scope. Backup destinations and webhooks move bytes OFF the box, so the
/// media guard never sees them. Creation was already refused; the review proved update/trigger were
/// not, so a scoped credential could repoint an existing destination at attacker storage.
#[tokio::test]
async fn a_scoped_credential_cannot_reach_the_egress_surfaces() {
    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    let scoped = seed_key(&st, Some(&["camera_a"])).await;

    for (method, path, body, what) in [
        (
            "POST",
            "/api/v1/backup/destinations",
            r#"{"name":"x","kind":"sftp","config":{}}"#,
            "create a destination",
        ),
        (
            "PATCH",
            "/api/v1/backup/destinations/bkd_1",
            r#"{"kind":"sftp","config":{"host":"attacker.example"}}"#,
            "repoint a destination",
        ),
        (
            "DELETE",
            "/api/v1/backup/destinations/bkd_1",
            "",
            "delete a destination",
        ),
        (
            "POST",
            "/api/v1/backup/destinations/bkd_1/test",
            "{}",
            "probe a destination",
        ),
        (
            "POST",
            "/api/v1/webhooks",
            r#"{"name":"x","url":"http://attacker.example/c"}"#,
            "create a webhook",
        ),
        ("GET", "/api/v1/outbox", "", "drain the fleet outbox"),
        // The exposition carries `heldar_camera_up{camera=…}` for the WHOLE fleet, so it is refused
        // rather than filtered: a filtered scrape reads to Prometheus as cameras that ceased to
        // exist, writing staleness gaps indistinguishable from real outages into the fleet history.
        ("GET", "/metrics", "", "scrape fleet-wide metrics"),
        // The credential surface — a scoped key that can read it learns every other integrator's
        // camera allowlist, i.e. the fleet roster. The create/update/delete siblings were guarded in
        // an earlier pass; the READS were left out of that batch.
        ("GET", "/api/v1/api-keys", "", "list API keys"),
        ("GET", "/api/v1/users", "", "list users"),
        // Box-level settings with no camera to scope by. `retention` is the sharpest: the value is
        // applied LATER by a sweeper that holds no principal and evicts segments fleet-wide, so a
        // scope-clean request destroys other cameras' footage after it returns.
        (
            "PUT",
            "/api/v1/system/retention",
            r#"{"max_recordings_gb": 0.001}"#,
            "shrink the fleet-wide recording cap",
        ),
        (
            "PUT",
            "/api/v1/system/transcode",
            r#"{"engine":"cpu"}"#,
            "change the transcode engine",
        ),
        (
            "PUT",
            "/api/v1/system/db",
            r#"{"max_db_mb": 1}"#,
            "change the database size cap",
        ),
        (
            "POST",
            "/api/v1/system/db/convert",
            "{}",
            "convert the database",
        ),
    ] {
        let (status, resp) = call_body(&st, &scoped, method, path, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "a camera-scoped credential could {what} ({method} {path}) -> {status}: {resp}"
        );
    }
}

/// Fleet-wide READ surfaces must not hand a scoped credential the roster.
#[tokio::test]
async fn fleet_wide_reads_are_confined_to_the_credentials_cameras() {
    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    let scoped = seed_key(&st, Some(&["camera_a"])).await;

    // `/api/v1/events` is deliberately absent: it needs `events:read`, which is UNSCOPABLE, so no
    // camera-scoped credential can reach it to be filtered. Its refusal is asserted with the other
    // fleet-only surfaces instead.
    for path in ["/api/v1/health/cameras", "/api/v1/cameras"] {
        let (status, body) = call_body(&st, &scoped, "GET", path, "").await;
        assert!(
            status.is_success(),
            "{path} -> {status} (expected a filtered 200): {body}"
        );
        assert!(
            !body.contains("camera_b"),
            "{path} leaked camera_b to a camera_a-scoped credential: {body}"
        );
        assert!(
            body.contains("camera_a") || body == "[]",
            "{path} returned nothing for the credential's OWN camera — filtered too hard: {body}"
        );
    }
}

/// A scoped caller's own rows must survive PAGINATION, not just filtering. Filtering after `LIMIT`
/// bounds rows examined rather than rows returned, so newer fleet rows push a scoped caller's own
/// export off the end — and with no offset or cursor on these endpoints, past the clamp it is
/// unreachable by any query the API accepts.
#[tokio::test]
async fn a_scoped_callers_own_rows_survive_a_page_full_of_fleet_rows() {
    let root = ScratchDir::new("page");
    let st = test_state_with(|cfg| {
        cfg.archive_dir = root.0.join("archives");
        cfg.recordings_dir = root.0.join("recordings");
    })
    .await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_segment(&st, &root.0, "camera_a").await;
    seed_segment(&st, &root.0, "camera_b").await;
    let admin = seed_key(&st, None).await;
    let scoped = mint_key(
        &st,
        &[
            "video:export",
            "system:read",
            "registry:manage",
            "camera:read",
        ],
        Some(&["camera_a"]),
    )
    .await;

    // The scoped caller's own export goes in FIRST, so every fleet row that follows is newer.
    let (status, _) = call_body(
        &st,
        &scoped,
        "POST",
        "/api/v1/archive/export",
        r#"{"camera_ids":["camera_a"]}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "scoped export must be allowed");
    for _ in 0..3 {
        let (status, _) = call_body(
            &st,
            &admin,
            "POST",
            "/api/v1/archive/export",
            r#"{"camera_ids":["camera_b"]}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    // A page smaller than the number of newer fleet rows: the filter has to run in SQL to survive it.
    let (status, body) =
        call_body(&st, &scoped, "GET", "/api/v1/archive/exports?limit=2", "").await;
    assert!(status.is_success(), "{status}: {body}");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).expect("a JSON array");
    assert_eq!(
        rows.len(),
        1,
        "the scoped caller's own export fell off the end of the page: {body}"
    );
    assert!(
        body.contains("camera_a") && !body.contains("camera_b"),
        "wrong rows returned: {body}"
    );
}

/// `/api/v1/system` never names another camera, so no per-route check could catch it — it leaked the
/// fleet's SHAPE as counts. `cameras_total: 2` beside a one-camera `GET /api/v1/cameras` answers
/// "how many cameras exist outside your scope", the exact bit the camera list filters away, and
/// differencing it over time reports fleet changes. Aggregates must be scoped, not merely gated.
#[tokio::test]
async fn system_info_aggregates_do_not_disclose_the_fleet_size() {
    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_camera(&st, "camera_c").await;
    let scoped = seed_key(&st, Some(&["camera_a"])).await;
    let fleet = seed_key(&st, None).await;

    let (status, body) = call_body(&st, &scoped, "GET", "/api/v1/system", "").await;
    assert!(
        status.is_success(),
        "scoped /api/v1/system -> {status}: {body}"
    );
    let v: serde_json::Value = serde_json::from_str(&body).expect("system info is JSON");
    assert_eq!(
        v["cameras_total"], 1,
        "a camera_a-scoped credential must count only its own cameras, not the fleet: {body}"
    );

    // The unscoped credential still sees the whole box — this must scope the answer, not shrink it
    // for everyone. A "fix" that reported 1 to every caller would pass the assertion above.
    let (status, body) = call_body(&st, &fleet, "GET", "/api/v1/system", "").await;
    assert!(
        status.is_success(),
        "fleet /api/v1/system -> {status}: {body}"
    );
    let v: serde_json::Value = serde_json::from_str(&body).expect("system info is JSON");
    assert_eq!(
        v["cameras_total"], 3,
        "an unscoped credential must still see every camera: {body}"
    );
}

// ---- Archive export: the FALSE DENY side of the boundary ----
//
// Every other test here asks whether the boundary lets too much through. These three ask the
// opposite and equally load-bearing question: does it deny what it just authorised? A scope layer that
// 403s the owner of an artifact is not "safe by default", it is a broken feature — and because the
// guard's Unattributed branch answers 403 identically to a genuine scope violation, the breakage
// looks exactly like enforcement working.

/// A scratch tree removed on drop, so an export in this process never touches the real archive dir.
struct ScratchDir(PathBuf);
impl ScratchDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "heldar-archive-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(p.join("archives")).unwrap();
        std::fs::create_dir_all(p.join("recordings")).unwrap();
        ScratchDir(p)
    }
}
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A camera with one real segment file on disk and the row that points at it, so `create_archive`
/// has something to actually zip. An archive built from zero segments is a 404, not an artifact.
async fn seed_segment(st: &AppState, root: &Path, camera_id: &str) {
    let dir = root.join("recordings").join(camera_id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("seg.mp4");
    std::fs::write(&path, format!("footage-of-{camera_id}")).unwrap();
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, container,
                               size_bytes, locked, evidence_locked, created_at)
         VALUES (?, ?, ?, ?, ?, 60.0, 'mp4', ?, 0, 0, ?)",
    )
    .bind(format!("seg_{camera_id}"))
    .bind(camera_id)
    .bind(path.to_string_lossy().to_string())
    .bind(now - chrono::Duration::minutes(5))
    .bind(now)
    .bind(std::fs::metadata(&path).unwrap().len() as i64)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
}

/// GET a `/media/*` URL through the guard, exactly as a browser would.
async fn call_media(st: &AppState, token: &str, path: &str) -> StatusCode {
    let mut app = composed_router_with_media(st);
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("X-API-Key", token)
        .body(Body::empty())
        .unwrap();
    app.call(req).await.unwrap().status()
}

/// Export an archive and return the `output_url` the API handed back.
async fn export_archive(st: &AppState, token: &str, body: &str) -> String {
    let (status, resp) = call_body(st, token, "POST", "/api/v1/archive/export", body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "archive export was refused ({body}) -> {status}: {resp}"
    );
    serde_json::from_str::<serde_json::Value>(&resp)
        .unwrap()
        .get("output_url")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("export returned no output_url: {resp}"))
        .to_string()
}

/// END TO END: an archive a credential is allowed to CREATE must be one it can READ.
///
/// The observed failure was `POST /api/v1/archive/export {"camera_ids":["camera_a"]}` -> 201 followed
/// by `GET /media/archives/<job>.zip` -> 403 for the very credential that made it. `create_archive`
/// wrote `output_url` and returned without registering the .zip in `media_artifacts`, so the guard
/// resolved it `Unattributed` and failed closed: every archive ever exported was unreadable to every
/// camera-scoped credential.
///
/// This drives the real pipeline — segment files on disk, `/usr/bin/zip`, `ServeDir` behind the
/// guard — because the defect lives in the seam between the key the producer writes and the key
/// `artifact_key` derives from the served URL. Nothing short of a real fetch crosses that seam.
#[tokio::test]
async fn an_archive_is_readable_by_the_scoped_credential_that_exported_it() {
    let root = ScratchDir::new("e2e");
    let st = test_state_with(|cfg| {
        cfg.archive_dir = root.0.join("archives");
        cfg.recordings_dir = root.0.join("recordings");
    })
    .await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_segment(&st, &root.0, "camera_a").await;
    seed_segment(&st, &root.0, "camera_b").await;
    let scoped_a = mint_key(
        &st,
        &[
            "video:export",
            "system:read",
            "registry:manage",
            "camera:read",
        ],
        Some(&["camera_a"]),
    )
    .await;
    let scoped_b = mint_key(
        &st,
        &[
            "video:export",
            "system:read",
            "registry:manage",
            "camera:read",
        ],
        Some(&["camera_b"]),
    )
    .await;

    let url = export_archive(&st, &scoped_a, r#"{"camera_ids":["camera_a"]}"#).await;
    assert!(
        url.starts_with("/media/archives/"),
        "unexpected archive url {url}"
    );

    // THE REGRESSION. Not a nicety: the export is useless if its own author cannot fetch it.
    assert_eq!(
        call_media(&st, &scoped_a, &url).await,
        StatusCode::OK,
        "the camera_a-scoped credential that created {url} cannot read it — the scope layer is \
         denying the export it just authorised"
    );

    // ...and the subtree did not simply open up: a credential scoped to a DIFFERENT camera is still
    // refused the same bytes. Without this the fix could be "attribute to every camera" or "stop
    // guarding archives", both of which would satisfy the assertion above.
    assert_eq!(
        call_media(&st, &scoped_b, &url).await,
        StatusCode::FORBIDDEN,
        "a camera_b-scoped credential read camera_a's archive {url}"
    );

    // The attribution names camera_a ONLY — camera_b has a segment on this box and must not have
    // been swept into an export that never contained its footage.
    let mut owners = sqlx::query_scalar::<_, String>(
        "SELECT camera_id FROM media_artifacts WHERE path = ? AND kind = 'archive'",
    )
    .bind(url.trim_start_matches("/media/"))
    .fetch_all(&st.pool)
    .await
    .unwrap();
    owners.sort();
    assert_eq!(owners, vec!["camera_a".to_string()]);
}

/// A FLEET-WIDE export sends `camera_ids: []`, and `[]` means "the whole box" downstream. Attributing
/// the request's list would write zero rows and leave the archive `Unattributed` all over again — the
/// same 403, reached by a different route. The owners must come from the segments actually zipped.
#[tokio::test]
async fn a_fleet_wide_export_is_attributed_to_every_camera_it_contains() {
    let root = ScratchDir::new("fleet");
    let st = test_state_with(|cfg| {
        cfg.archive_dir = root.0.join("archives");
        cfg.recordings_dir = root.0.join("recordings");
    })
    .await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_segment(&st, &root.0, "camera_a").await;
    seed_segment(&st, &root.0, "camera_b").await;
    let unscoped = seed_key(&st, None).await;
    let scoped_a = mint_key(
        &st,
        &[
            "video:export",
            "system:read",
            "registry:manage",
            "camera:read",
        ],
        Some(&["camera_a"]),
    )
    .await;

    let url = export_archive(&st, &unscoped, r#"{"camera_ids":[]}"#).await;

    let mut owners = sqlx::query_scalar::<_, String>(
        "SELECT camera_id FROM media_artifacts WHERE path = ? AND kind = 'archive'",
    )
    .bind(url.trim_start_matches("/media/"))
    .fetch_all(&st.pool)
    .await
    .unwrap();
    owners.sort();
    assert_eq!(
        owners,
        vec!["camera_a".to_string(), "camera_b".to_string()],
        "a fleet-wide export must be attributed to the cameras whose footage it contains, not to \
         the empty `camera_ids` the request carried"
    );

    // The unscoped author reads it; the camera_a credential does not, because the zip also holds
    // camera_b's footage. Being attributed is not the same as being readable.
    assert_eq!(call_media(&st, &unscoped, &url).await, StatusCode::OK);
    assert_eq!(
        call_media(&st, &scoped_a, &url).await,
        StatusCode::FORBIDDEN,
        "a camera_a-scoped credential read a fleet-wide archive containing camera_b"
    );
}

/// Deleting the export must take its attribution with it, so `media_artifacts` cannot outlive the
/// bytes it describes. Retention's mtime prune of `archive_dir` is covered by the existence-based
/// `media_scope::sweep_orphans`; an operator DELETE is immediate and should not wait a sweep cycle.
#[tokio::test]
async fn deleting_an_export_forgets_its_attribution() {
    let root = ScratchDir::new("del");
    let st = test_state_with(|cfg| {
        cfg.archive_dir = root.0.join("archives");
        cfg.recordings_dir = root.0.join("recordings");
    })
    .await;
    seed_camera(&st, "camera_a").await;
    seed_segment(&st, &root.0, "camera_a").await;
    let unscoped = seed_key(&st, None).await;

    let url = export_archive(&st, &unscoped, r#"{"camera_ids":["camera_a"]}"#).await;
    let key = url.trim_start_matches("/media/").to_string();
    let job_id = key
        .trim_start_matches("archives/")
        .trim_end_matches(".zip")
        .to_string();

    // "no row after DELETE" is vacuously true on a box that never wrote one, which is precisely the
    // state this fix removed — so pin the row's EXISTENCE first. Otherwise dropping the attribution
    // and dropping the forget would both leave this test green.
    assert_eq!(
        attribution_rows(&st, &key).await,
        1,
        "the export was never attributed, so this test would prove nothing about forgetting it"
    );

    let (status, body) = call_body(
        &st,
        &unscoped,
        "DELETE",
        &format!("/api/v1/backup/jobs/{job_id}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete failed: {body}");

    assert_eq!(
        attribution_rows(&st, &key).await,
        0,
        "the archive's attribution outlived the archive itself"
    );
}

/// How many `media_artifacts` rows describe `key`.
async fn attribution_rows(st: &AppState, key: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media_artifacts WHERE path = ?")
        .bind(key)
        .fetch_one(&st.pool)
        .await
        .unwrap()
}
