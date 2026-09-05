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
        .merge(heldar_kernel::openapi::router())
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
///
/// Runs the APP crates' schemas as well as the kernel's, exactly as `heldar_server::run` does. It
/// did not, and the cost was invisible: seven app routes answered 500 from a missing table, which is
/// neither a pass nor a fail, so they were parked in the census as "app schema not init in harness"
/// and no assertion ever reached their guards. A harness that only migrates half the composition
/// cannot say anything about the other half.
async fn test_state_with(tune: impl FnOnce(&mut heldar_kernel::config::Config)) -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    heldar_entry::schema::init(&pool).await.unwrap();
    heldar_movement::schema::init(&pool).await.unwrap();
    heldar_search::schema::init(&pool).await.unwrap();
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
    mint_key_with_id(st, caps, cameras).await.0
}

/// [`mint_key`] that also returns the key's ID, which is what the credential-lifecycle tests need:
/// re-scoping or revoking a key mid-flight goes through `PATCH /api/v1/api-keys/{id}`.
async fn mint_key_with_id(
    st: &AppState,
    caps: &[&str],
    cameras: Option<&[&str]>,
) -> (String, String) {
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
    let v: serde_json::Value = serde_json::from_str(&resp)
        .unwrap_or_else(|e| panic!("mint response was not JSON ({e}): {resp}"));
    let key = v
        .get("key")
        .and_then(|k| k.as_str())
        .unwrap_or_else(|| panic!("mint response carried no plaintext key: {resp}"))
        .to_string();
    let id = v
        .get("id")
        .and_then(|k| k.as_str())
        .unwrap_or_else(|| panic!("mint response carried no key id: {resp}"))
        .to_string();
    (key, id)
}

/// Re-scope a live key through the REAL `PATCH /api/v1/api-keys/{id}`, as an operator does when a
/// camera changes hands. `caps` must be re-sent: the route re-validates the WHOLE resulting grant.
async fn rescope_key(st: &AppState, key_id: &str, caps: &[&str], cameras: &[&str]) {
    let admin = seed_key(st, None).await;
    let body = serde_json::json!({
        "capabilities": caps,
        "scope_kind": "cameras",
        "scope_cameras": cameras,
        "confirm_privileged": true,
    })
    .to_string();
    let (status, resp) = call_body(
        st,
        &admin,
        "PATCH",
        &format!("/api/v1/api-keys/{key_id}"),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-scope failed: {resp}");
}

/// Soft-revoke a live key through the REAL PATCH.
async fn revoke_key(st: &AppState, key_id: &str) {
    let admin = seed_key(st, None).await;
    let body = serde_json::json!({ "revoked_at": chrono::Utc::now() }).to_string();
    let (status, resp) = call_body(
        st,
        &admin,
        "PATCH",
        &format!("/api/v1/api-keys/{key_id}"),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "revoke failed: {resp}");
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
        // A timezone reinterprets every schedule and every relative search on the box.
        //
        // NOTE WHAT THIS DOES AND DOES NOT PROVE. These entries assert a 403, and they get one —
        // but from `require(can_admin())`, which fires before the scope guard, because the census
        // credential is not an admin. Deleting `require_fleet_scope` from every handler below
        // leaves this whole list green. The scope guards are proven in tests/sites_api.rs, which
        // calls the handlers directly with an admin-BUT-camera-scoped principal; the API refuses to
        // mint one (admin implies the unscopable caps), so it cannot be reached over HTTP at all.
        (
            "GET",
            "/api/v1/system/posture",
            "",
            "read the box's security posture",
        ),
        // Names the credentials still posting ticketless AI ingest, so it is a fleet-wide read of
        // who is talking to this box — same gate as the posture report above.
        (
            "GET",
            "/api/v1/system/provenance-readiness",
            "",
            "read AI ingest provenance readiness",
        ),
        (
            "PUT",
            "/api/v1/system/timezone",
            r#"{"timezone":"America/New_York"}"#,
            "change the box-wide timezone",
        ),
        // A site's zone is what its cameras' schedules are read in, so these move recording
        // windows for every camera on the site — fleet-wide by nature, like the settings above.
        // (Same caveat as the note above: the 403 here is the admin gate.)
        (
            "POST",
            "/api/v1/sites",
            r#"{"id":"evil","name":"E","timezone":"America/New_York"}"#,
            "create a site",
        ),
        (
            "PATCH",
            "/api/v1/sites/site_census",
            r#"{"timezone":"America/New_York"}"#,
            "move a site's clock",
        ),
        ("DELETE", "/api/v1/sites/site_census", "", "delete a site"),
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

// ---- Authorization that outlives the request ----
//
// Everything above asks whether a scope holds at the moment a request arrives. These ask what
// happens to a decision AFTERWARDS — when the credential behind it is re-scoped or revoked while
// something it started is still running or still fetchable. A boundary that is only checked on the
// way in is a boundary with a shelf life, and these tests pin how long each one actually lasts.

/// The `/media/*` plane re-authorizes EVERY request; a URL is not a capability.
///
/// It matters because the artifacts are long-lived and the URLs are handed to a browser: an archive
/// zip, an HLS playlist and its segments, a clip — all fetched many times, over minutes, from a page
/// that was authorized once. `media_scope::guard` runs OUTSIDE the `/api/v1` auth floor, so it has no
/// pre-resolved principal to reuse and resolves the credential from the database on every hit. That
/// is what makes a re-scope take effect on the next byte rather than at the end of the session, and
/// it is why playback sessions and clip exports need no expiry of their own.
///
/// Both directions are asserted: narrowed -> 403 (still a valid credential, no longer this camera's),
/// revoked -> 401 (no credential at all). A single 4xx assertion would pass on either alone.
#[tokio::test]
async fn a_media_artifact_stops_being_readable_the_moment_its_credential_changes() {
    let root = ScratchDir::new("outlives");
    let st = test_state_with(|cfg| {
        cfg.archive_dir = root.0.join("archives");
        cfg.recordings_dir = root.0.join("recordings");
    })
    .await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_segment(&st, &root.0, "camera_a").await;
    const CAPS: &[&str] = &[
        "video:export",
        "video:playback",
        "system:read",
        "registry:manage",
        "camera:read",
    ];
    let (scoped_a, key_id) = mint_key_with_id(&st, CAPS, Some(&["camera_a"])).await;

    let url = export_archive(&st, &scoped_a, r#"{"camera_ids":["camera_a"]}"#).await;
    assert_eq!(
        call_media(&st, &scoped_a, &url).await,
        StatusCode::OK,
        "the exporting credential cannot read its own archive; the rest of this test would be vacuous"
    );

    // The camera changes hands. The archive is unchanged, the URL is unchanged, the capability is
    // unchanged — only the scope moved, and that alone must end the access.
    rescope_key(&st, &key_id, CAPS, &["camera_b"]).await;
    assert_eq!(
        call_media(&st, &scoped_a, &url).await,
        StatusCode::FORBIDDEN,
        "a credential re-scoped off camera_a kept reading camera_a's archive at {url} — the guard is \
         serving a decision made before the re-scope"
    );

    // ...and revocation ends it as a credential, not merely as a scope.
    revoke_key(&st, &key_id).await;
    assert_eq!(
        call_media(&st, &scoped_a, &url).await,
        StatusCode::UNAUTHORIZED,
        "a revoked credential still fetched {url}"
    );
}

/// Seed an enabled AI task on an enabled camera, so lease acquisition has something to hand out.
async fn seed_ai_task(st: &AppState, camera_id: &str, task_id: &str) {
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO ai_tasks (id, camera_id, task_type, enabled, stream_profile, fps, width,
                               config, created_at, updated_at)
         VALUES (?, ?, 'anpr', 1, 'sub', 5, 1280, '{}', ?, ?)",
    )
    .bind(task_id)
    .bind(camera_id)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
}

/// The camera ids in a lease response, sorted.
fn leased_cameras(resp: &str) -> Vec<String> {
    let mut v: Vec<String> = serde_json::from_str::<serde_json::Value>(resp)
        .unwrap_or_else(|e| panic!("lease response was not JSON ({e}): {resp}"))
        .get("tasks")
        .and_then(|t| t.as_array())
        .unwrap_or_else(|| panic!("lease response carried no tasks array: {resp}"))
        .iter()
        .filter_map(|t| {
            t.get("camera_id")
                .and_then(|c| c.as_str())
                .map(String::from)
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// A worker cannot renew a lease onto a camera its credential has lost.
///
/// Acquire and renew are the SAME call, which is what makes this cheap to get right and easy to get
/// wrong: if renewal extended whatever the worker already held, a lease taken while the key was wide
/// would keep being renewed forever after the key was narrowed, and the frame endpoint would keep
/// ticketing it. It does not — `ai_leases::acquire` rebuilds its candidate list from the CURRENT
/// scope on every call, so the lost camera simply stops being offered, and the stale lease row lapses
/// on its own TTL (<= 300 s) without ever being an authorization: both the frame pull and the ingest
/// re-check `require_camera` against the live principal.
#[tokio::test]
async fn a_narrowed_credential_cannot_renew_its_lease_onto_the_camera_it_lost() {
    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_ai_task(&st, "camera_a", "ai_outlives_a").await;
    seed_ai_task(&st, "camera_b", "ai_outlives_b").await;
    const CAPS: &[&str] = &["ai:tasks", "ai:frames", "camera:read"];
    let (worker, key_id) = mint_key_with_id(&st, CAPS, Some(&["camera_a", "camera_b"])).await;

    let body = r#"{"worker_id":"w_outlives"}"#;
    let (status, resp) = call_body(&st, &worker, "POST", "/api/v1/ai/leases", body).await;
    assert_eq!(status, StatusCode::OK, "lease acquire failed: {resp}");
    assert_eq!(
        leased_cameras(&resp),
        vec!["camera_a".to_string(), "camera_b".to_string()],
        "the fixture must start with BOTH cameras leased, or the narrowing below proves nothing"
    );

    rescope_key(&st, &key_id, CAPS, &["camera_a"]).await;

    // The same renew call the worker's poll loop makes every tick.
    let (status, resp) = call_body(&st, &worker, "POST", "/api/v1/ai/leases", body).await;
    assert_eq!(status, StatusCode::OK, "lease renew failed: {resp}");
    assert_eq!(
        leased_cameras(&resp),
        vec!["camera_a".to_string()],
        "the worker renewed a lease on camera_b after its credential was scoped off it"
    );

    // ...and the frame pull that a lease exists to enable is refused for the lost camera, so the
    // stale lease row cannot be turned into a ticket either.
    assert_eq!(
        call(&st, &worker, "GET", "/api/v1/cameras/camera_b/frame").await,
        StatusCode::FORBIDDEN,
        "a lost camera's frames were still served to the holder of its stale lease"
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

/// `DELETE /api/v1/ai/leases/{lease_id}` is the one resource-addressed route in the tree whose guard
/// is NOT camera scope, which is why the census declares it scope-neutral rather than proving it
/// alongside the others — and a declaration with nothing behind it is how this suite has been wrong
/// before. This is what is behind it.
///
/// `release` deletes on `lease_id AND api_key_id`, so a lease id is not a capability on its own. Two
/// things have to hold for that to contain a camera-scoped credential, and both are asserted here:
/// another credential's lease is neither dropped NOR distinguishable from one that never existed, and
/// the credential's OWN lease still releases (a containment that also blocked the legitimate case
/// would be a worker that can never shut down cleanly).
#[tokio::test]
async fn a_scoped_credential_cannot_release_another_credentials_lease() {
    let st = test_state().await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    seed_ai_task(&st, "camera_a", "task_a").await;
    seed_ai_task(&st, "camera_b", "task_b").await;

    // A live lease on camera_b's task, held by a DIFFERENT credential — the shape a guessed lease id
    // would target.
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO ai_task_leases (task_id, lease_id, api_key_id, worker_id, camera_id, task_type,
                                     acquired_at, renewed_at, expires_at)
         VALUES ('task_b','lse_of_camera_b','key_of_camera_b','worker_b','camera_b','detection',?,?,?)",
    )
    .bind(now)
    .bind(now)
    .bind(now + chrono::Duration::minutes(5))
    .execute(&st.pool)
    .await
    .unwrap();

    // Minted through the real endpoint: this assertion runs in the DENIAL direction and in the
    // false-deny direction, and both are vacuous against a credential the API refuses to issue.
    let scoped = mint_key(&st, &["ai:tasks"], Some(&["camera_a"])).await;

    let (foreign_status, foreign_body) = call_body(
        &st,
        &scoped,
        "DELETE",
        "/api/v1/ai/leases/lse_of_camera_b",
        "",
    )
    .await;
    let (absent_status, absent_body) = call_body(
        &st,
        &scoped,
        "DELETE",
        "/api/v1/ai/leases/lse_does_not_exist",
        "",
    )
    .await;
    assert_eq!(
        (foreign_status, &foreign_body),
        (absent_status, &absent_body),
        "another credential's live lease answers differently from a lease id that names nothing, so \
         the route can be walked to learn which lease ids are real"
    );

    // The answer being identical is not enough on its own — it must be identical because NOTHING
    // HAPPENED. A release that dropped the row and still reported `released: 0` would satisfy the
    // assertion above while stopping another camera's worker dead.
    let survivors: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ai_task_leases WHERE lease_id = ?")
            .bind("lse_of_camera_b")
            .fetch_one(&st.pool)
            .await
            .unwrap();
    assert_eq!(
        survivors, 1,
        "a camera_a-scoped credential released camera_b's lease: {foreign_status} {foreign_body}"
    );

    // FALSE DENY control: its own lease, acquired through the real endpoint, still releases.
    let (status, resp) = call_body(
        &st,
        &scoped,
        "POST",
        "/api/v1/ai/leases",
        r#"{"worker_id":"w1"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "acquire was refused: {resp}");
    let acquired: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let own_lease = acquired["lease_id"].as_str().unwrap().to_string();
    assert_eq!(
        acquired["tasks"].as_array().map(Vec::len),
        Some(1),
        "the scoped credential leased {} tasks, so the release below would prove nothing: {resp}",
        acquired["tasks"].as_array().map(Vec::len).unwrap_or(0)
    );
    let (status, resp) = call_body(
        &st,
        &scoped,
        "DELETE",
        &format!("/api/v1/ai/leases/{own_lease}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "own release was refused: {resp}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&resp).unwrap()["released"],
        serde_json::json!(1),
        "the credential could not release the lease it had just acquired on its OWN camera: {resp}"
    );
}

// ---- The census: no route may be silently uncovered ----
//
// The rule and its rationale now live in `heldar-testkit`, so a private workspace composing
// proprietary verticals over the `Verticals` seam runs the SAME rule over the SAME composed router
// instead of reimplementing it — and a reimplementation of this particular rule drifts back to
// "we test the routes we thought of", which is the failure it exists to prevent.
//
// What stays here is what is specific to THIS composition: which routes are declared safe, which the
// harness cannot reach, and the bodies that get a probe past an extractor to the guard.

/// Routes that legitimately answer a camera-scoped credential, each with the reason it is safe.
/// The only escape hatch in the census, so an entry is a security assertion.
const SCOPE_NEUTRAL: &[(&str, &str)] = &[
    // --- unauthenticated by design ---
    ("/healthz", "liveness; no data, no auth"),
    ("/readyz", "readiness; reports only whether SQLite answers"),
    ("/api/v1/auth/login", "the login endpoint itself"),
    // --- scope-FILTERED reads: they answer, but only about the caller's own cameras ---
    ("/api/v1/cameras", "camera_scope_filter on id"),
    ("/api/v1/health/cameras", "camera_scope_filter on camera_id"),
    (
        "/api/v1/system",
        "aggregates scoped; box-level disk facts only",
    ),
    ("/api/v1/backup/policies", "owns_selection_sql subset rule"),
    ("/api/v1/backup/jobs", "owns_selection_sql subset rule"),
    ("/api/v1/archive/exports", "owns_selection_sql subset rule"),
    (
        "/api/v1/evidence/exports",
        "GET is confined to the caller's cameras; POST resolves the camera (from camera_id OR an \
         incident) and camera_scope_checks it before any footage is read",
    ),
    (
        "/api/v1/sites",
        "scope-FILTERED read: a scoped credential sees only the sites its own cameras belong to. \
         A filtered list is something the census cannot check, so the confinement is proven in \
         `a_scoped_credential_sees_only_its_own_sites` below. The WRITES 403 here via the ADMIN \
         gate, which fires first; their fleet-scope guard is proven in tests/sites_api.rs. \
         `/api/v1/sites/{id}` is deliberately NOT declared — it has a fixture, and a declaration \
         would disable it",
    ),
    (
        "/api/v1/system/timezone",
        "GET only: SystemRead, and it discloses no camera. The PUT is NOT neutral and is proven \
         to 403 for a camera-scoped credential in the fleet-scope assertion list below — a \
         scope_neutral declaration makes the census skip the path, so the write needs its own \
         assertion or nothing checks it",
    ),
    (
        "/api/v1/evidence/signing-key",
        "an Ed25519 PUBLIC key. Deliberately readable by any authenticated credential, scoped \
         included: a scoped operator who exported a bundle needs it to verify one, and a public \
         key discloses nothing about which cameras exist",
    ),
    (
        "/api/v1/archive/export",
        "confine_camera_ids on the submitted list",
    ),
    (
        "/api/v1/audit",
        "subject_camera_id filter, fail-closed on NULL",
    ),
    ("/api/v1/ai/tasks", "camera_scope_filter on t.camera_id"),
    ("/api/v1/ai/samplers", "retain on camera_allowed"),
    (
        "/api/v1/ai/leases",
        "acquire passes the camera scope into the candidate query",
    ),
    (
        // Release is keyed on `lease_id AND api_key_id`, so the ONLY leases a credential can drop are
        // its own — and acquire (above) already confines a scoped credential's leases to its own
        // cameras, so its own leases can never name another camera. Another credential's lease id is
        // a no-op answering `released: 0`, byte-identical to a lease that never existed. Pinned in
        // `a_scoped_credential_cannot_release_another_credentials_lease`, including that the row it
        // did not own is still there afterwards.
        "/api/v1/ai/leases/{lease_id}",
        "release is confined to the caller's OWN credential, not to a camera",
    ),
    // --- box-level facts that name no camera ---
    ("/api/v1/site", "site id, product name, version, uptime"),
    (
        "/api/v1/registry",
        "the plugin catalogue; box-level, no camera data",
    ),
    (
        "/api/v1/modules",
        "sidecar MANIFESTS only — base_url/api_key_id are on `detail`, which refuses",
    ),
    (
        "/api/v1/system/retention",
        "reads back box retention config; no per-camera figure",
    ),
    (
        "/api/v1/system/transcode",
        "reads back the box transcode engine",
    ),
    // --- the module proxy: a module id is not a camera resource ---
    // `Cap::ModuleProxy` is scopable, so a camera-scoped credential may legitimately proxy. Scope is
    // not dropped at the boundary — `forward` sends `x-heldar-camera-scope` (absent = fleet-wide)
    // alongside the principal headers, so the sidecar enforces the same confinement. A 404 here is an
    // unknown MODULE, which is not a camera-scope answer either way.
    (
        "/m/{id}",
        "module proxy; scope is forwarded to the sidecar, not resolved here",
    ),
    (
        "/m/{id}/",
        "module proxy; scope is forwarded to the sidecar, not resolved here",
    ),
    // Same route family and the same answer as the two above. The wildcard tail is what the census
    // cannot FILL, but that is a harness limit, not a different classification.
    (
        "/m/{id}/{*rest}",
        "module proxy (wildcard tail); scope is forwarded to the sidecar, not resolved here",
    ),
    // Verified against migration 0010: `embed_queries` has NO camera column — id, kind, payload,
    // vec, model, status. It is a queue of SEARCH TEXT awaiting an embedding, not camera data, so
    // there is nothing to scope it by.
    (
        "/api/v1/ai/embed-queries",
        "a queue of search text; the table carries no camera",
    ),
    // Same table, same answer. Its access control is real but is a DIFFERENT boundary from this
    // census: `embed_query_result` passes `credential_id(&principal)` into the service, so a result
    // can only be submitted by the credential that claimed the query. Camera scope has nothing to
    // say here because there is no camera on the row.
    (
        "/api/v1/ai/embed-queries/{id}/result",
        "same table, no camera; submission is bound to the CLAIMING credential instead",
    ),
    // --- about the caller itself, not about any camera ---
    // The API contract describes the SURFACE, not any camera's data. It sits inside the /api/v1 auth
    // floor so it is not public, but every authenticated caller gets the same document.
    (
        "/api/v1/openapi.json",
        "the API contract; identical for every caller, no camera data",
    ),
    ("/api/v1/auth/me", "describes the caller"),
    ("/api/v1/auth/logout", "ends the caller's own session"),
];

/// Routes the census cannot PROVE, with the reason — named debt, deliberately not counted as
/// coverage. A route may only sit here if the probe genuinely cannot reach its guard; shrink it by
/// correcting a probe body or seeding a real fixture.
///
/// This list was 26 entries. Twenty-two of them were not facts about the product at all: seven app
/// routes 500ed because the harness ran the kernel migrations and not the app crates', nine were
/// addressed by a resource id the census had no way to fill, and six carried a probe body their own
/// extractor rejected — so each was parked with a plausible reason and never probed again. Named
/// debt decays into that if nobody keeps testing whether it is still true.
const CENSUS_UNPROVEN: &[(&str, &str)] = &[
    // EMPTY, and worth keeping that way. Every route is now camera-keyed, provably refuses, proven
    // indistinguishable from a missing resource, or declared with a verified reason. An entry here is
    // a route whose guard this harness cannot reach — legitimate debt, but debt: it is NOT counted as
    // coverage, and the count is printed separately so it cannot quietly grow back.
];

// ---- Seeded fixtures for the resource-addressed routes ----
//
// A route keyed by its own primary key rather than a camera id is the shape that hid four defects in
// an earlier round, and it is the shape a synthetic probe id cannot test at all: the handler 404s on
// the missing row before the guard it is meant to exercise ever runs. Sixteen routes sat in
// CENSUS_UNPROVEN for exactly that reason.
//
// So seed the resource — owned by camera_b, which the probing credential does NOT hold — and hand
// the census both that id and one of the same shape that names nothing. It then requires the two
// answers to be INDISTINGUISHABLE, which is the actual property: "refused" is not enough when 404 is
// itself the leak.
//
// Every id below is written by [`seed_census_fixtures`], and the census re-proves that with the
// unscoped control credential on every run. A fixture that was never really seeded agrees with
// itself perfectly, and reports a clean census having asserted nothing.

/// A resource id that exists, owned by camera_b, and one of the same shape that does not.
const FX_TASK: (&str, &str) = ("task_of_camera_b", "task_does_not_exist");
const FX_ZONE: (&str, &str) = ("zone_of_camera_b", "zone_does_not_exist");
const FX_SCHEDULE: (&str, &str) = ("sched_of_camera_b", "sched_does_not_exist");
const FX_SNAP_SCHEDULE: (&str, &str) = ("snapsched_of_camera_b", "snapsched_does_not_exist");
// The playback session id must satisfy `is_valid_session_id` (pbs_ + alphanumerics), or the handler
// answers 400 for BOTH ids and the agreement proves only that the validator rejects them.
const FX_PLAYBACK: (&str, &str) = ("pbs_ofcamerab", "pbs_doesnotexist");
const FX_INCIDENT: (&str, &str) = ("inc_of_camera_b", "inc_does_not_exist");
const FX_PLATE: (&str, &str) = ("PLATEB", "PLATEZZZZ");
const FX_ENTRY_EVENT: (&str, &str) = ("ev_of_camera_b", "ev_does_not_exist");
const FX_BREACH: (&str, &str) = ("breach_of_camera_b", "breach_does_not_exist");
const FX_CANDIDATE: (&str, &str) = ("cand_of_camera_b", "cand_does_not_exist");
// Backup policies and jobs are camera-owned via their stored `camera_ids` (`owns_selection`), and
// `policy_for`/`job_for` collapse out-of-scope onto the same 404 a missing id gets. That collapse is
// the property worth proving, and until these fixtures existed the census counted the routes as
// covered purely because a synthetic id 404s — which an entirely unguarded route would also do.
// One row per DESTRUCTIVE route. The census's control credential really does perform the mutation it
// probes — that is what proves the fixture was reachable at all — so a single shared id is consumed
// by the first DELETE and every later route then sees a missing row, which silently degrades those
// probes to "agreed about nothing". Separate ids keep each route's evidence its own.
const FX_POLICY: (&str, &str) = ("bkp_of_camera_b", "bkp_does_not_exist");
const FX_POLICY_DEL: (&str, &str) = ("bkp_del_of_camera_b", "bkp_del_does_not_exist");
const FX_JOB: (&str, &str) = ("bkj_of_camera_b", "bkj_does_not_exist");
const FX_JOB_DEL: (&str, &str) = ("bkj_del_of_camera_b", "bkj_del_does_not_exist");
const FX_LINK: (&str, &str) = ("lnk_of_camera_b", "lnk_does_not_exist");
const FX_EVIDENCE: (&str, &str) = ("ev_of_camera_b", "ev_does_not_exist");
const FX_SITE: (&str, &str) = ("site_of_camera_b", "site_does_not_exist");

/// Seed one row per resource-addressed route, all owned by camera_b.
///
/// Written straight to the tables on purpose: these are the SUBJECT of the probe, not the credential
/// making it. The rule that a fixture must be minted through the real API is about principals — an
/// assertion against a credential the product refuses to issue is vacuous. A row is just a row, and
/// the census's control credential proves each one is really there.
async fn seed_census_fixtures(st: &AppState, root: &Path) {
    // A destination + a policy + a completed job, all owned by camera_b alone.
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO backup_destinations (id, name, kind, config, enabled, created_at, updated_at)
         VALUES ('bkd_census','census','local','{\"path\":\"/tmp/census-dest\"}',1,?,?)",
    )
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO backup_policies (id, name, destination_id, camera_ids, enabled,
                                      incident_lock_only, created_at, updated_at)
         VALUES (?, 'census policy', 'bkd_census', '[\"camera_b\"]', 1, 0, ?, ?)",
    )
    .bind(FX_POLICY.0)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO backup_policies (id, name, destination_id, camera_ids, enabled,
                                      incident_lock_only, created_at, updated_at)
         VALUES (?, 'census policy (delete)', 'bkd_census', '[\"camera_b\"]', 1, 0, ?, ?)",
    )
    .bind(FX_POLICY_DEL.0)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO backup_jobs (id, policy_id, destination_id, kind, camera_ids, status,
                                  files_total, files_copied, bytes_copied, incident_lock_only,
                                  created_at)
         VALUES (?, ?, 'bkd_census', 'policy', '[\"camera_b\"]', 'completed', 0, 0, 0, 0, ?)",
    )
    .bind(FX_JOB.0)
    .bind(FX_POLICY.0)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    // An evidence bundle (#118) exported from camera_b. Reading it by id must answer exactly as an
    // id naming nothing does — otherwise a bundle id tells a scoped caller which windows of another
    // camera's footage have been exported, and how large they were.
    sqlx::query(
        "INSERT INTO evidence_bundles (id, camera_id, filename, from_time, to_time, size_bytes,
             sha256, manifest_sha256, key_id, exported_by, created_at)
         VALUES (?, 'camera_b', 'x.heldar-evidence', ?, ?, 0, 'h', 'm', 'sha256:k', 'someone', ?)",
    )
    .bind(FX_EVIDENCE.0)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    // A site holding only camera_b, plus the one the fleet-scope assertions try to mutate. Reading
    // a site the credential holds no camera on must answer exactly as an unknown one, or a site id
    // becomes a way to enumerate the fleet's structure.
    for id in [FX_SITE.0, "site_census"] {
        sqlx::query("INSERT INTO sites (id, name, timezone, created_at) VALUES (?,?,?,?)")
            .bind(id)
            .bind(id)
            .bind("Asia/Kuala_Lumpur")
            .bind(now)
            .execute(&st.pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE cameras SET site_id = ? WHERE id = 'camera_b'")
        .bind(FX_SITE.0)
        .execute(&st.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO backup_jobs (id, policy_id, destination_id, kind, camera_ids, status,
                                  files_total, files_copied, bytes_copied, incident_lock_only,
                                  created_at)
         VALUES (?, ?, 'bkd_census', 'policy', '[\"camera_b\"]', 'completed', 0, 0, 0, 0, ?)",
    )
    .bind(FX_JOB_DEL.0)
    .bind(FX_POLICY.0)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();

    let now = chrono::Utc::now();
    let pool = &st.pool;

    sqlx::query(
        "INSERT INTO ai_tasks (id, camera_id, task_type, created_at, updated_at) VALUES (?,?,?,?,?)",
    )
    .bind(FX_TASK.0)
    .bind("camera_b")
    .bind("detection")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO zones (id, camera_id, name, polygon, created_at, updated_at)
         VALUES (?,?,?,?,?,?)",
    )
    .bind(FX_ZONE.0)
    .bind("camera_b")
    .bind("gate")
    .bind("[[0.1,0.1],[0.9,0.1],[0.9,0.9]]")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO camera_schedules (id, camera_id, time_start, time_end, created_at, updated_at)
         VALUES (?,?,'08:00','18:00',?,?)",
    )
    .bind(FX_SCHEDULE.0)
    .bind("camera_b")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO snapshot_schedules (id, camera_id, created_at, updated_at) VALUES (?,?,?,?)",
    )
    .bind(FX_SNAP_SCHEDULE.0)
    .bind("camera_b")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    // A playback session is a DIRECTORY plus its media attribution; the guard reads the attribution
    // and the delete reads the directory, so a fixture missing either one is only half a session.
    std::fs::create_dir_all(root.join("playback").join(FX_PLAYBACK.0)).unwrap();
    sqlx::query("INSERT INTO media_artifacts (path, camera_id, kind, created_at) VALUES (?,?,?,?)")
        .bind(format!("playback/{}", FX_PLAYBACK.0))
        .bind("camera_b")
        .bind("playback")
        .bind(now)
        .execute(pool)
        .await
        .unwrap();

    // An incident is not a table: it is a tag on camera_b's segments, which is exactly why the route
    // has to filter by camera rather than trust the tag.
    let seg_path = root.join("recordings").join("camera_b_incident.mp4");
    std::fs::write(&seg_path, b"footage").unwrap();
    sqlx::query(
        "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, container,
                               size_bytes, locked, evidence_locked, incident_id, created_at)
         VALUES (?,?,?,?,?,60.0,'mp4',7,0,0,?,?)",
    )
    .bind("seg_incident_b")
    .bind("camera_b")
    .bind(seg_path.to_string_lossy().to_string())
    .bind(now - chrono::Duration::minutes(5))
    .bind(now)
    .bind(FX_INCIDENT.0)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    // camera_b's lane sighting of FX_PLATE: the row behind BOTH the entry-event workflow routes and
    // the plate trail (which reads it through the `entry_events_read` contract view).
    sqlx::query(
        "INSERT INTO entry_events (id, camera_id, event_type, timestamp, direction, plate,
                                   auth_status, created_at)
         VALUES (?,?,'anpr',?,'inbound',?,'matched',?)",
    )
    .bind(FX_ENTRY_EVENT.0)
    .bind("camera_b")
    .bind(now)
    .bind(FX_PLATE.0)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO breach_alerts (id, camera_id, rule, created_at) VALUES (?,?,'red_zone_entry',?)",
    )
    .bind(FX_BREACH.0)
    .bind("camera_b")
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    // Both ends on cameras the probing credential does not hold — a movement row that names two
    // cameras is visible only to a credential holding BOTH.
    sqlx::query(
        "INSERT INTO movement_candidates (id, subject_type, anchor, from_camera, to_camera,
                                          from_ref, to_ref, created_at)
         VALUES (?,'vehicle',?,?,?,?,?,?)",
    )
    .bind(FX_CANDIDATE.0)
    .bind(FX_PLATE.0)
    .bind("camera_b")
    .bind("camera_c")
    .bind("ref_b")
    .bind("ref_c")
    .bind(now)
    .execute(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO camera_links (id, from_camera, to_camera, created_at, updated_at)
         VALUES (?,?,?,?,?)",
    )
    .bind(FX_LINK.0)
    .bind("camera_b")
    .bind("camera_c")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn every_route_is_accounted_for() {
    let root = ScratchDir::new("census");
    let st = test_state_with(|cfg| {
        // The census MUTATES: its control credential deletes a playback session and writes segment
        // read-locks. Point both trees at scratch so nothing here can touch a developer's box.
        cfg.playback_dir = root.0.join("playback");
        cfg.recordings_dir = root.0.join("recordings");
        // A successful control PATCH of a recording schedule calls `recorder.reconcile`, and a
        // control DELETE of an AI task calls `sampler.reconcile`. Both would spawn a real ffmpeg
        // against a camera that does not exist. Nothing in the census reads either flag — only
        // `/api/v1/system` reports them, and it is declared scope-neutral.
        cfg.recorder_enabled = false;
        cfg.ai_enabled = false;
    })
    .await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    // The far end of camera_b's link and candidate. A movement row naming two cameras is held only
    // by a credential holding both, so the second camera has to be a real one.
    seed_camera(&st, "camera_c").await;
    seed_census_fixtures(&st, &root.0).await;
    let scoped = seed_key(&st, Some(&["camera_a"])).await;
    // The control. Not an adversary and not the subject of any assertion: it exists so a fixture that
    // was never really seeded is caught being invisible to a credential that holds every camera.
    let unscoped = seed_key(&st, None).await;

    let census = heldar_testkit::Census::new(vec![
        repo_root().join("crates/heldar-kernel/src"),
        repo_root().join("crates/heldar-entry/src"),
        repo_root().join("crates/heldar-movement/src"),
        repo_root().join("crates/heldar-search/src"),
    ])
    .scope_neutral(SCOPE_NEUTRAL)
    .unproven(CENSUS_UNPROVEN)
    // Where a body names a CAMERA it names camera_b — the one the probing credential does not
    // hold — so the assertion is a scope test and not merely a capability test.
    .probe_body(
        "/api/v1/ai/embeddings",
        r#"{"camera_id":"camera_b","model":"clip","dim":2,"items":[]}"#,
    )
    .probe_body(
        "/api/v1/ai/events",
        r#"{"camera_id":"camera_b","task_type":"detection","detections":[]}"#,
    )
    .probe_body("/api/v1/ai/leases", r#"{"worker_id":"w1"}"#)
    // Names camera_b and dry_run:false, so the probe reaches the scope check rather than stopping at
    // deserialization — a 422 would have counted as "answered" while proving nothing.
    .probe_body(
        "/api/v1/evidence/exports",
        r#"{"camera_id":"camera_b","from":"2026-01-01T00:00:00Z","to":"2026-01-01T00:01:00Z","dry_run":false}"#,
    )
    .probe_body("/api/v1/api-keys", r#"{"name":"probe","role":"viewer"}"#)
    .probe_body(
        "/api/v1/backup/destinations",
        r#"{"name":"probe","kind":"sftp","config":{}}"#,
    )
    .probe_body(
        "/api/v1/cameras/config/bulk",
        r#"{"camera_ids":["camera_b"],"action":{"type":"sync_time"}}"#,
    )
    .probe_body("/api/v1/discover", r#"{"targets":"127.0.0.1"}"#)
    .probe_body("/api/v1/entry/gate/settings", r#"{"kill_switch":true}"#)
    .probe_body(
        "/api/v1/modules",
        r#"{"id":"probe","base_url":"http://127.0.0.1:1"}"#,
    )
    .probe_body(
        "/api/v1/movement/links",
        r#"{"from_camera":"camera_b","to_camera":"camera_c"}"#,
    )
    .probe_body("/api/v1/search/nl", r#"{"query":"red car at the gate"}"#)
    .probe_body("/api/v1/search/plan", r#"{"query":"red car at the gate"}"#)
    .probe_body("/api/v1/passes", r#"{"visitor_name":"probe"}"#)
    .probe_body("/api/v1/vehicles", r#"{"plate":"PROBE1"}"#)
    .probe_body("/api/v1/watchlist", r#"{"plate":"PROBE1","kind":"block"}"#)
    .probe_body(
        "/api/v1/webhooks",
        r#"{"name":"probe","url":"http://127.0.0.1:1/h"}"#,
    )
    .probe_body(
        "/api/v1/users",
        r#"{"username":"probe","password":"pw","role":"viewer"}"#,
    )
    .probe_body("/api/v1/system/transcode", r#"{"engine":"cpu"}"#)
    // Gated on the QUERY, not the body: without these the extractor rejects the probe before the
    // guard runs, and the route can only be recorded as unproven.
    .probe_query("/api/v1/ai/embed-queries", "worker_id=probe_worker")
    // Names camera_b deliberately — the camera the probing credential does not hold.
    .probe_query(
        "/api/v1/movement/search/person",
        "camera=camera_b&track=trk_probe&at=2026-01-01T00:00:00Z",
    )
    // Routes addressed by a RESOURCE id. Each names a row seeded on camera_b (which the probing
    // credential does not hold) and an id of the same shape naming nothing; the census requires the
    // two answers to be indistinguishable, so a 404 cannot be used to walk the id space.
    .fixture("/api/v1/ai-tasks/{task_id}", FX_TASK.0, FX_TASK.1)
    .fixture(
        "/api/v1/backup/policies/{id}",
        FX_POLICY_DEL.0,
        FX_POLICY_DEL.1,
    )
    .fixture(
        "/api/v1/backup/policies/{id}/trigger",
        FX_POLICY.0,
        FX_POLICY.1,
    )
    .fixture("/api/v1/backup/jobs/{id}", FX_JOB.0, FX_JOB.1)
    .fixture(
        "/api/v1/evidence/exports/{id}",
        FX_EVIDENCE.0,
        FX_EVIDENCE.1,
    )
    .fixture("/api/v1/sites/{id}", FX_SITE.0, FX_SITE.1)
    .fixture("/api/v1/zones/{zone_id}", FX_ZONE.0, FX_ZONE.1)
    .fixture(
        "/api/v1/schedules/{schedule_id}",
        FX_SCHEDULE.0,
        FX_SCHEDULE.1,
    )
    .fixture(
        "/api/v1/snapshot-schedules/{schedule_id}",
        FX_SNAP_SCHEDULE.0,
        FX_SNAP_SCHEDULE.1,
    )
    .fixture(
        "/api/v1/playback/sessions/{session_id}",
        FX_PLAYBACK.0,
        FX_PLAYBACK.1,
    )
    .fixture(
        "/api/v1/incidents/{incident_id}/segments",
        FX_INCIDENT.0,
        FX_INCIDENT.1,
    )
    .fixture(
        "/api/v1/movement/search/plate/{plate}",
        FX_PLATE.0,
        FX_PLATE.1,
    )
    .fixture(
        "/api/v1/entry-events/{id}/confirm",
        FX_ENTRY_EVENT.0,
        FX_ENTRY_EVENT.1,
    )
    .fixture(
        "/api/v1/entry-events/{id}/reject",
        FX_ENTRY_EVENT.0,
        FX_ENTRY_EVENT.1,
    )
    .fixture(
        "/api/v1/movement/breaches/{id}/ack",
        FX_BREACH.0,
        FX_BREACH.1,
    )
    .fixture(
        "/api/v1/movement/breaches/{id}/resolve",
        FX_BREACH.0,
        FX_BREACH.1,
    )
    .fixture(
        "/api/v1/movement/candidates/{id}/confirm",
        FX_CANDIDATE.0,
        FX_CANDIDATE.1,
    )
    .fixture(
        "/api/v1/movement/candidates/{id}/reject",
        FX_CANDIDATE.0,
        FX_CANDIDATE.1,
    )
    .fixture("/api/v1/movement/links/{id}", FX_LINK.0, FX_LINK.1)
    // The open build composes 151 routes; a scan finding far fewer is broken, and a census over an
    // empty set would otherwise pass triumphantly.
    .min_routes(100);

    let report = census
        .run_with_control(
            |method, path, body| {
                let st = st.clone();
                let scoped = scoped.clone();
                async move {
                    let (status, resp) = call_body(&st, &scoped, &method, &path, &body).await;
                    (status.as_u16(), resp)
                }
            },
            |method, path, body| {
                let st = st.clone();
                let unscoped = unscoped.clone();
                async move {
                    let (status, resp) = call_body(&st, &unscoped, &method, &path, &body).await;
                    (status.as_u16(), resp)
                }
            },
        )
        .await;
    report.assert_clean();
}

// ---- The live plane: a stream token is withdrawable ----
//
// MediaMTX serves video directly to the browser and calls the kernel back to authorize each read.
// That callback used to consult ONLY the token's signature, so revoking the key that opened a stream
// did not stop it — measured at the time as `200 OK` on a replayed token from a revoked key. These
// drive `POST /internal/mediamtx-auth` exactly as MediaMTX does.

/// Ask the callback the same question MediaMTX asks.
async fn mediamtx_auth(st: &AppState, path: &str, token: &str, action: &str) -> StatusCode {
    let body = serde_json::json!({
        "action": action,
        "path": path,
        "query": format!("token={token}"),
        "ip": "10.0.0.9",
        "user": "",
        "password": "",
        "protocol": "hls",
        "id": "sess_probe",
    })
    .to_string();
    let mut app = composed_router(st);
    let resp = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/internal/mediamtx-auth")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

// `/liveview` cannot be driven here: `ensure_live` provisions a path against a real MediaMTX API and
// waits for it to go ready. So these mint the token through the SAME public function that endpoint
// uses — `live_token::mint` with `Subject::of(principal)` — and then ask the callback what MediaMTX
// asks. The mapping from principal to subject is pinned separately in `live_token`'s own tests; what
// is proven here is the half that was missing entirely: the callback withdrawing the read.

fn stream_token(key_id: &str, path: &str) -> String {
    heldar_kernel::services::live_token::mint(
        path,
        chrono::Utc::now().timestamp(),
        3600,
        &heldar_kernel::services::live_token::Subject::ApiKey(key_id.to_string()),
    )
}

#[tokio::test]
async fn revoking_the_key_that_opened_a_stream_stops_the_stream() {
    let st = test_state_with(|cfg| cfg.auth_enabled = true).await;
    seed_camera(&st, "camera_a").await;
    let (_key, key_id) = mint_key_with_id(
        &st,
        &["camera:read", "video:live", "system:read"],
        Some(&["camera_a"]),
    )
    .await;
    let path = "cam_camera_a";
    let token = stream_token(&key_id, path);

    // While the credential stands, MediaMTX is told to serve.
    assert_eq!(
        mediamtx_auth(&st, path, &token, "read").await,
        StatusCode::OK,
        "a live credential's own stream was refused"
    );

    // The operator burns the key. The token is still cryptographically valid and unexpired — the
    // ONLY thing that changed is the credential behind it.
    revoke_key(&st, &key_id).await;
    assert_eq!(
        mediamtx_auth(&st, path, &token, "read").await,
        StatusCode::UNAUTHORIZED,
        "a revoked key kept streaming: the token outlived the credential that minted it"
    );
}

#[tokio::test]
async fn narrowing_a_scope_off_the_camera_stops_its_stream() {
    let st = test_state_with(|cfg| cfg.auth_enabled = true).await;
    seed_camera(&st, "camera_a").await;
    seed_camera(&st, "camera_b").await;
    let (_key, key_id) = mint_key_with_id(
        &st,
        &["camera:read", "video:live", "system:read"],
        Some(&["camera_a", "camera_b"]),
    )
    .await;
    let path = "cam_camera_b";
    let token = stream_token(&key_id, path);
    assert_eq!(
        mediamtx_auth(&st, path, &token, "read").await,
        StatusCode::OK
    );

    // Re-scope the key off camera_b WITHOUT revoking it: still a perfectly valid credential, it
    // simply no longer holds this camera.
    let admin = seed_key(&st, None).await;
    let body =
        serde_json::json!({ "scope_kind": "cameras", "scope_cameras": ["camera_a"] }).to_string();
    let (status, resp) = call_body(
        &st,
        &admin,
        "PATCH",
        &format!("/api/v1/api-keys/{key_id}"),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-scope failed: {resp}");

    assert_eq!(
        mediamtx_auth(&st, path, &token, "read").await,
        StatusCode::UNAUTHORIZED,
        "the key kept streaming a camera it no longer holds"
    );
    // ...and the camera it DOES still hold keeps working: withdrawal must be per-camera, not a
    // blanket kill of every stream the credential opened.
    let still_held = stream_token(&key_id, "cam_camera_a");
    assert_eq!(
        mediamtx_auth(&st, "cam_camera_a", &still_held, "read").await,
        StatusCode::OK,
        "re-scoping killed a stream on a camera the credential still holds"
    );
}

/// Revoking through the API must withdraw the stream AT THAT MOMENT, not at the reaper's next tick.
///
/// This pins the HOOK, not the mechanism: `live_reaper`'s own tests prove withdrawal works, but the
/// call site in `update_api_key` could be deleted and everything still compiled and passed. That is
/// exactly the shape of a fix that ships inert.
#[tokio::test]
async fn revoking_through_the_api_withdraws_the_stream_immediately() {
    use std::sync::{Arc, Mutex};

    // A stand-in MediaMTX that reports one live session and records what gets kicked.
    let kicked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let k = kicked.clone();
    let mtx = axum::Router::new()
        .route(
            "/v3/{kind}/list",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({ "items": [{ "id": "sess_live" }] }))
            }),
        )
        .route(
            "/v3/{kind}/kick/{id}",
            axum::routing::post(
                move |axum::extract::Path((_k, id)): axum::extract::Path<(String, String)>| {
                    let k = k.clone();
                    async move {
                        k.lock().unwrap().push(id);
                        StatusCode::OK
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, mtx).await;
    });

    let st = test_state_with(move |cfg| {
        cfg.auth_enabled = true;
        cfg.mediamtx_api_url = api.clone();
    })
    .await;
    seed_camera(&st, "camera_a").await;
    let (_key, key_id) = mint_key_with_id(
        &st,
        &["camera:read", "video:live", "system:read"],
        Some(&["camera_a"]),
    )
    .await;

    // A live WebRTC session for that credential — the shape that never re-presents its token.
    sqlx::query(
        "INSERT INTO live_sessions (id, protocol, path, subject_kind, subject_id, created_at, last_seen_at)
         VALUES ('sess_live','webrtc','cam_camera_a','api_key',?,?,?)",
    )
    .bind(&key_id)
    .bind(chrono::Utc::now())
    .bind(chrono::Utc::now())
    .execute(&st.pool)
    .await
    .unwrap();

    revoke_key(&st, &key_id).await;

    // `withdraw_now` is spawned, so poll rather than reading once.
    for _ in 0..60 {
        if !kicked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(
        *kicked.lock().unwrap(),
        vec!["sess_live".to_string()],
        "revoking through the API did not withdraw the live stream; the reaper would eventually, but \
         the operator pressed the button now"
    );
}

// ---- Correlation id, against the REAL layer stack ----
//
// The first version of this test built a one-route toy router with only the id middleware attached.
// It would have stayed green if someone moved `.layer(request_id::layer)` inside the auth floor —
// the exact regression its own comment claimed to prevent. Ordering is the whole property, so the
// test has to use the composed stack.

/// The id must survive the responses raised BEFORE a handler: those are what a caller quotes.
#[tokio::test]
async fn every_response_carries_a_request_id_through_the_real_stack() {
    let st = test_state_with(|cfg| cfg.auth_enabled = true).await;
    seed_camera(&st, "camera_a").await;

    // Same order as `heldar_server::run`: routes, then the auth floor, then the id outermost.
    let base = composed_router(&st)
        .layer(axum::middleware::from_fn_with_state(
            st.clone(),
            heldar_kernel::auth::require_api_auth,
        ))
        .layer(axum::middleware::from_fn(heldar_kernel::request_id::layer));

    // Both are refused BEFORE any handler runs — the auth floor covers the whole `/api/v1` prefix,
    // so even an unrouted path under it answers 401 rather than 404. Which of the two it is does not
    // matter here; that it still carries the id does.
    for path in ["/api/v1/cameras", "/api/v1/no-such-route"] {
        let b = Request::builder().method("GET").uri(path);
        let mut app = base.clone();
        let resp = app.call(b.body(Body::empty()).unwrap()).await.unwrap();
        assert!(
            !resp.status().is_success(),
            "{path} should have been refused without a credential"
        );
        assert!(
            resp.headers().get("x-request-id").is_some(),
            "{path} was refused with no correlation id — the id is layered inside something it \
             should wrap"
        );
    }

    // ...and a caller-supplied id survives the whole stack rather than being replaced.
    let mut app = base.clone();
    let resp = app
        .call(
            Request::builder()
                .uri("/api/v1/cameras")
                .header("x-request-id", "trace-from-caller")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("x-request-id").unwrap(),
        "trace-from-caller"
    );
}

/// Every capability the published contract DECLARES must be one the kernel actually enforces (#120).
///
/// `x-heldar-capability` tells an integrator which credential to present. If it names a capability
/// the handler does not check, the contract is worse than silence: it sends someone to mint a
/// credential that will not work, or — worse — implies a route is gated when it is not.
///
/// So each declaration is driven against the REAL router with a credential holding EVERY capability
/// except the declared one. A refusal proves the gate exists and that this specific capability is
/// what opens it. The control is the same request with the full set, which must NOT be refused for
/// the same reason — otherwise a route that refuses everything would look perfectly declared.
#[tokio::test]
async fn every_declared_capability_is_one_the_kernel_enforces() {
    use heldar_kernel::auth::Cap;
    use heldar_kernel::openapi_security::REQUIREMENTS;

    let st = test_state().await;
    seed_camera(&st, "camera_a").await;

    // A body good enough to reach the capability check on every write route in the table. The gate
    // runs before deserialization matters, so a 422 would still prove nothing — these bodies are
    // valid so a refusal can only be the gate.
    let body_for = |path: &str, method: &str| -> &'static str {
        match (path, method) {
            ("/api/v1/evidence/exports", "post") => {
                r#"{"camera_id":"camera_a","from":"2026-01-01T00:00:00Z","to":"2026-01-01T00:01:00Z"}"#
            }
            ("/api/v1/system/timezone", "put") => r#"{"timezone":"UTC"}"#,
            ("/api/v1/sites", "post") => r#"{"id":"decl","name":"Decl"}"#,
            ("/api/v1/sites/{id}", "patch") => r#"{"name":"x"}"#,
            ("/api/v1/cameras/{id}/clip", "post") => {
                r#"{"from":"2026-01-01T00:00:00Z","to":"2026-01-01T00:01:00Z"}"#
            }
            ("/api/v1/cameras/{id}/playback/sessions", "post") => {
                r#"{"from":"2026-01-01T00:00:00Z","to":"2026-01-01T00:01:00Z"}"#
            }
            // The rest exist ONLY to get past deserialization and reach the capability gate. They
            // do not have to be semantically valid — a 400 from the handler is fine, a 422 from
            // serde is not, because it means the probe never reached the thing being tested.
            ("/api/v1/cameras/config/bulk", "post") => {
                r#"{"camera_ids":[],"action":{"type":"sync_time"}}"#
            }
            ("/api/v1/cameras/{id}/config/time", "put") => {
                r#"{"time_mode":"manual","local_time":"2026-01-01T00:00:00","time_zone":"CST-8:00:00"}"#
            }
            ("/api/v1/cameras/{id}/config/time/ntp", "put") => {
                r#"{"addressing_format":"ipaddress","host_name":"1.2.3.4","port":123,"interval":60}"#
            }
            ("/api/v1/cameras/{id}/config/onvif", "put") => {
                r#"{"onvif_enabled":false,"isapi_enabled":false}"#
            }
            ("/api/v1/cameras/{id}/config/onvif/ensure_user", "post") => {
                r#"{"password":"probe-only-never-used"}"#
            }
            ("/api/v1/cameras/{id}/config/osd", "put") => {
                r#"{"datetime_enabled":false,"channel_name_enabled":false}"#
            }
            ("/api/v1/cameras/{id}/config/reboot", "post") => r#"{"confirm":false}"#,
            ("/api/v1/cameras/{id}/config/video/{channel}", "put") => r#"{}"#,
            ("/api/v1/backup/destinations", "post") => r#"{"name":"p","kind":"local","config":{}}"#,
            ("/api/v1/backup/policies", "post") => r#"{"name":"p","destination_id":"d"}"#,
            ("/api/v1/movement/links", "post") => r#"{"from_camera":"a","to_camera":"b"}"#,
            ("/api/v1/cameras/{id}/control/detections/{kind}", "put") => r#"{"enabled":false}"#,
            ("/api/v1/cameras/{id}/control/line_crossing", "put") => {
                r#"{"enabled":false,"lines":[]}"#
            }
            ("/api/v1/cameras/{id}/control/intrusion", "put") => {
                r#"{"enabled":false,"regions":[]}"#
            }
            ("/api/v1/cameras/{id}/control/motion", "put") => r#"{"enabled":false}"#,
            ("/api/v1/cameras/{id}/ai-tasks", "post") => r#"{"task_type":"detection"}"#,
            ("/api/v1/ai/leases", "post") => r#"{"worker_id":"w-probe"}"#,
            ("/api/v1/ai/events", "post") => {
                r#"{"camera_id":"camera_a","task_type":"detection","detections":[]}"#
            }
            ("/api/v1/ai/embeddings", "post") => {
                r#"{"camera_id":"camera_a","model":"clip","dim":2,"items":[]}"#
            }
            ("/api/v1/cameras/{id}/ptz/goto_preset", "post") => r#"{"token":"1"}"#,
            ("/api/v1/vehicles", "post") => r#"{"plate":"ABC123"}"#,
            ("/api/v1/passes", "post") => r#"{"visitor_name":"P","plate":"ABC123"}"#,
            ("/api/v1/watchlist", "post") => r#"{"plate":"ABC123"}"#,
            ("/api/v1/entry/gate/settings", "put") => r#"{"kill_switch":false}"#,
            ("/api/v1/cameras/{id}/zones", "post") => {
                r#"{"name":"z","polygon":[[0,0],[1,0],[1,1]]}"#
            }
            ("/api/v1/webhooks", "post") => r#"{"name":"w","url":"http://127.0.0.1/x"}"#,
            ("/api/v1/search/nl", "post") => r#"{"query":"red car"}"#,
            ("/api/v1/search/plan", "post") => r#"{"query":"red car"}"#,
            ("/api/v1/modules", "post") => {
                r#"{"id":"m","name":"M","base_url":"http://127.0.0.1:1/","routes":[]}"#
            }
            ("/api/v1/cameras/{id}/schedules", "post") => {
                r#"{"days":[0],"time_start":"01:00","time_end":"02:00"}"#
            }
            ("/api/v1/cameras/{id}/snapshot-schedules", "post") => r#"{"interval_seconds":60}"#,
            ("/api/v1/discover", "post") => r#"{"targets":"127.0.0.1"}"#,
            _ => "{}",
        }
    };

    let mut checked = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for req in REQUIREMENTS {
        let Some(cap) = req.capability else {
            // Admin-gated routes are covered by the fleet-scope assertion list above; a capability
            // claim is what this test is about.
            continue;
        };
        // Substitute EVERY path parameter, not just `{id}`. An unsubstituted `{channel}` reaches
        // the handler as a literal and fails to parse as a u32 — a 400 before the gate, which
        // proves nothing about the declaration either way.
        let path = req
            .path
            .replace("{id}", "camera_a")
            .replace("{camera_id}", "camera_a")
            .replace("{channel}", "1")
            .replace("{port}", "1")
            .replace("{kind}", "motion")
            .replace("{session_id}", "pbs_probe")
            .replace("{task_id}", "task_probe")
            .replace("{zone_id}", "zone_probe")
            .replace("{schedule_id}", "sched_probe")
            .replace("{lease_id}", "lease_probe")
            .replace("{incident_id}", "inc_probe")
            .replace("{plate}", "ABC123")
            .replace("{name}", "probe");
        let body = body_for(req.path, req.method);
        // A GET with a REQUIRED query parameter 400s on the missing param before the gate, the
        // same way a POST 422s on a missing body field.
        let path = match req.path {
            "/api/v1/movement/search/person" => {
                format!("{path}?camera=camera_a&track=t1&at=2026-01-01T00:00:00Z")
            }
            _ => path,
        };
        let method = req.method.to_uppercase();

        // Everything EXCEPT the declared capability. Admin is excluded from the "lacking" set
        // because it implies all of them, which would make every probe vacuous.
        let without: Vec<&str> = Cap::ALL
            .iter()
            .filter(|c| **c != cap && **c != Cap::Admin)
            .map(|c| c.slug())
            .collect();
        let lacking = mint_key(&st, &without, None).await;
        let (status, resp) = call_body(&st, &lacking, &method, &path, body).await;
        if status != StatusCode::FORBIDDEN {
            // COLLECTED, NOT ASSERTED. Failing on the first mismatch means finding them one CI run
            // at a time, and with 149 declarations that is a day of round trips for what is one
            // reading of the same list.
            //
            // A 422 here is a BODY problem, not a declaration problem: deserialization runs before
            // the gate, so the probe never reached it and the declaration is neither proven nor
            // disproven. It is reported distinctly for that reason.
            problems.push(format!(
                "{} {} declares `{}` but a credential holding every OTHER capability got {}{}: {}",
                req.method,
                req.path,
                cap.slug(),
                status,
                if status == StatusCode::UNPROCESSABLE_ENTITY {
                    " (body rejected before the gate — the probe needs a valid body, this is not \
                      necessarily a wrong declaration)"
                } else {
                    ""
                },
                resp.chars().take(160).collect::<String>()
            ));
            continue;
        }

        // The control. Without it, a route that refuses everything would look correctly declared.
        //
        // The control set must INCLUDE the declared capability, which for an Admin-gated route
        // means including Admin — otherwise the control cannot pass and the test reports a
        // contradiction that is its own construction rather than a defect in the contract.
        let holding: Vec<&str> = Cap::ALL
            .iter()
            .filter(|c| **c != Cap::Admin || cap == Cap::Admin)
            .map(|c| c.slug())
            .collect();
        let full = mint_key(&st, &holding, None).await;
        let (status, resp) = call_body(&st, &full, &method, &path, body).await;
        if status == StatusCode::FORBIDDEN {
            problems.push(format!(
                "{} {} refused a credential holding EVERY capability — so its refusal proves \
                 nothing about `{}`: {}",
                req.method,
                req.path,
                cap.slug(),
                resp.chars().take(160).collect::<String>()
            ));
            continue;
        }
        checked += 1;
    }

    assert!(
        problems.is_empty(),
        "{} declaration(s) do not match what the kernel enforces:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
    assert!(
        checked >= 8,
        "only {checked} capability declarations were exercised — if the table shrank, the contract \
         is describing less than it did, and this test would pass by checking almost nothing"
    );
}

/// Every documented path must appear in the requirements table, and vice versa.
///
/// The two are separate lists, which is precisely the shape that drifts — so neither is allowed to
/// grow without the other. A documented route with no declared requirement publishes a surface an
/// integrator cannot authenticate against; a declared requirement for an undocumented route
/// describes something the contract does not contain.
#[test]
fn the_contract_and_the_requirements_table_cover_the_same_routes() {
    use heldar_kernel::openapi_security::REQUIREMENTS;
    use std::collections::BTreeSet;

    let spec = heldar_server::api_document();
    let mut documented: BTreeSet<(String, String)> = BTreeSet::new();
    for (path, item) in spec["paths"].as_object().expect("paths") {
        for method in item.as_object().expect("path item").keys() {
            if ["get", "put", "post", "delete", "patch", "head", "options"]
                .contains(&method.as_str())
            {
                documented.insert((path.clone(), method.clone()));
            }
        }
    }
    let declared: BTreeSet<(String, String)> = REQUIREMENTS
        .iter()
        .map(|r| (r.path.to_string(), r.method.to_string()))
        .collect();

    let undeclared: Vec<_> = documented.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "documented but with no declared requirement: {undeclared:?}"
    );
    let unpublished: Vec<_> = declared.difference(&documented).collect();
    assert!(
        unpublished.is_empty(),
        "a requirement is declared for a route the contract does not document: {unpublished:?}"
    );
}
