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
            // Camera-keyed means the FIRST path parameter is the camera id.
            if !path.starts_with("/api/v1/cameras/{id}") {
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

async fn test_state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
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
    // `admin` + no explicit capability grant: the STRONGEST adversary. Capabilities are orthogonal to
    // scope (`camera_allowed` does not exempt Cap::Admin), so this isolates the scope boundary — if a
    // route lets this key through, only the missing scope check can be responsible.
    sqlx::query(
        "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at,
                               capabilities, scope_kind, scope_cameras, expires_at)
         VALUES (?,?,?,?,'admin',1,?,NULL,?,?,NULL)",
    )
    .bind(&id)
    .bind("matrix")
    .bind(heldar_kernel::auth::token_hash(&token))
    .bind(&token[..8])
    .bind(chrono::Utc::now())
    .bind(kind)
    .bind(list)
    .execute(&st.pool)
    .await
    .unwrap();
    token
}

async fn call(st: &AppState, token: &str, method: &str, path: &str) -> StatusCode {
    let mut app = heldar_kernel::routes::api_router().with_state(st.clone());
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

    for route in &routes {
        for method in &route.methods {
            let for_cam = |cam: &str| route.path.replace("{id}", cam);

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
            let a = call(&st, &scoped, method, &for_cam("camera_a")).await;
            if a == StatusCode::FORBIDDEN {
                vacuous.push(format!("{method} {} -> camera_a wrongly 403", route.path));
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
    for must in [
        "/api/v1/cameras/{id}/liveview",
        "/api/v1/cameras/{id}/clip",
        "/api/v1/cameras/{id}/snapshot",
    ] {
        assert!(
            paths.contains(must),
            "route discovery missed {must} — it is one of the routes this whole repair exists for.\n\
             found: {paths:#?}"
        );
    }
}
