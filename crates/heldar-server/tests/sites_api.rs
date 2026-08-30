//! Sites (#125): the CRUD that makes the timezone resolver's first arm reachable.
//!
//! Two behaviours here are operational rather than cosmetic and are the reason this file exists:
//! changing a site's zone MOVES the recording windows of every camera on it, and deleting a site
//! would drop those cameras to the box default and reinterpret their windows with nothing to notice.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use heldar_kernel::state::AppState;
use tower::Service;

async fn state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = false; // the synthetic admin; scope behaviour is covered by the route census
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

async fn call(
    st: &AppState,
    method: &str,
    path: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let mut app = heldar_kernel::routes::api_router().with_state(st.clone());
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.call(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

async fn add_camera(st: &AppState, id: &str, site: Option<&str>) {
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO cameras (id, site_id, name, created_at, updated_at) VALUES (?,?,?,?,?)",
    )
    .bind(id)
    .bind(site)
    .bind(id)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_site_can_be_created_read_and_listed() {
    let st = state().await;
    let (s, v) = call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"Kuala Lumpur","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["timezone"], "Asia/Kuala_Lumpur");

    let (s, v) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["name"], "Kuala Lumpur");

    let (s, v) = call(&st, "GET", "/api/v1/sites", "").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["sites"].as_array().map(Vec::len), Some(1), "{v}");
}

/// The whole point of migration 0019. A site created without a zone must READ BACK as null, not as
/// "this site chose UTC" — the resolver treats those differently and only one of them is true.
#[tokio::test]
async fn a_site_created_without_a_zone_has_none_rather_than_utc() {
    let st = state().await;
    let (s, v) = call(&st, "POST", "/api/v1/sites", r#"{"id":"s1","name":"S"}"#).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["timezone"].is_null(),
        "an unspecified zone must be null, not UTC: {v}"
    );

    add_camera(&st, "cam_a", Some("s1")).await;
    let (tz, src) = heldar_kernel::services::tz::site_tz(&st.pool, Some("cam_a")).await;
    assert_eq!(tz, None);
    assert_eq!(src, heldar_kernel::services::tz::TzSource::Unset);
}

/// A rename must not wipe the zone. `timezone` absent means "leave it"; `timezone: null` means
/// "clear it". Serde collapses those into the same value unless asked not to.
#[tokio::test]
async fn omitting_the_timezone_leaves_it_and_sending_null_clears_it() {
    let st = state().await;
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;

    let (s, v) = call(&st, "PATCH", "/api/v1/sites/kl", r#"{"name":"KL HQ"}"#).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["timezone_changed"], false);
    // READ IT BACK FROM THE DATABASE. The PATCH response is constructed from the handler's own
    // inputs, so asserting on it proves only that the handler can echo. Breaking the UPDATE's bind
    // left this whole suite green while the API reported a zone change that never landed.
    let (_, stored) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    assert_eq!(
        stored["timezone"], "Asia/Kuala_Lumpur",
        "a rename must not silently wipe the zone: {stored}"
    );

    let (s, v) = call(&st, "PATCH", "/api/v1/sites/kl", r#"{"timezone":null}"#).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["timezone_changed"], true);
    let (_, stored) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    assert!(
        stored["timezone"].is_null(),
        "explicit null clears it: {stored}"
    );
    assert_eq!(v["timezone_changed"], true);
}

/// Changing a zone moves recording windows. The response has to say so, or an operator relabels a
/// site at 5pm and finds out at midnight that nothing recorded.
#[tokio::test]
async fn changing_the_zone_reports_how_many_cameras_it_moved() {
    let st = state().await;
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;
    add_camera(&st, "cam_a", Some("kl")).await;
    add_camera(&st, "cam_b", Some("kl")).await;
    add_camera(&st, "cam_elsewhere", None).await;

    let (s, v) = call(
        &st,
        "PATCH",
        "/api/v1/sites/kl",
        r#"{"timezone":"America/New_York"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["timezone_changed"], true);
    assert_eq!(
        v["cameras_affected"], 2,
        "only the site's own cameras moved: {v}"
    );
    assert_eq!(v["previous_timezone"], "Asia/Kuala_Lumpur");
    assert!(
        v["note"]
            .as_str()
            .unwrap_or_default()
            .contains("wall-clock"),
        "the response must say what a zone change actually does: {v}"
    );

    // And the cameras' clocks actually moved. A report is only worth having if it describes reality.
    let (_, stored) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    assert_eq!(stored["timezone"], "America/New_York", "{stored}");
    let (tz, src) = heldar_kernel::services::tz::site_tz(&st.pool, Some("cam_a")).await;
    assert_eq!(
        tz.map(|t| t.to_string()).as_deref(),
        Some("America/New_York")
    );
    assert_eq!(src, heldar_kernel::services::tz::TzSource::Site);
}

/// `cameras.site_id` is ON DELETE SET NULL, so deleting a populated site would drop its cameras to
/// the box default and reinterpret their windows with no event and nothing to notice.
#[tokio::test]
async fn a_site_with_cameras_cannot_be_deleted_out_from_under_them() {
    let st = state().await;
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;
    add_camera(&st, "cam_a", Some("kl")).await;

    let (s, v) = call(&st, "DELETE", "/api/v1/sites/kl", "").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(
        v["error"].as_str().unwrap_or_default().contains("reassign"),
        "the refusal must say what to do instead: {v}"
    );

    // The site is still there, and the camera still resolves to its zone.
    let (tz, _) = heldar_kernel::services::tz::site_tz(&st.pool, Some("cam_a")).await;
    assert_eq!(
        tz.map(|t| t.to_string()).as_deref(),
        Some("Asia/Kuala_Lumpur")
    );

    // Reassign, then it deletes.
    sqlx::query("UPDATE cameras SET site_id = NULL WHERE id = 'cam_a'")
        .execute(&st.pool)
        .await
        .unwrap();
    let (s, v) = call(&st, "DELETE", "/api/v1/sites/kl", "").await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn a_bad_timezone_is_refused_on_both_write_paths() {
    let st = state().await;
    for body in [
        r#"{"id":"x","name":"X","timezone":"Asia/KL"}"#,
        r#"{"id":"x","name":"X","timezone":"GMT+8"}"#,
        r#"{"id":"x","name":"X","timezone":"+08:00"}"#,
    ] {
        let (s, v) = call(&st, "POST", "/api/v1/sites", body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body} must be refused: {v}");
    }
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"UTC"}"#,
    )
    .await;
    let (s, _) = call(
        &st,
        "PATCH",
        "/api/v1/sites/kl",
        r#"{"timezone":"Asia/KL"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "PATCH validates too");
}

#[tokio::test]
async fn a_duplicate_id_is_a_conflict_not_a_silent_overwrite() {
    let st = state().await;
    let body = r#"{"id":"kl","name":"KL","timezone":"UTC"}"#;
    assert_eq!(
        call(&st, "POST", "/api/v1/sites", body).await.0,
        StatusCode::OK
    );
    let (s, v) = call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"Other"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (_, v) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    assert_eq!(v["name"], "KL", "the original must survive: {v}");
}

// -------------------------------------------------------------------------------------------------
// The fleet-scope guard, tested where it actually lives.
//
// The route census asserts these routes 403 for a camera-scoped credential — but that credential
// reaches the handler WITHOUT admin, so the 403 comes from `require(can_admin())`, which fires
// first. `require_fleet_scope` is never evaluated, and deleting it from all three handlers leaves
// the census entirely green. The assertion is true and proves the wrong thing.
//
// An admin credential that is ALSO camera-scoped is the case that separates them. The API refuses to
// MINT one (admin implies the unscopable capabilities), so it cannot be exercised over HTTP — which
// is exactly why the guard needs testing here instead of being assumed. The handlers are plain async
// functions, so they can be called with a constructed principal.
// -------------------------------------------------------------------------------------------------

use axum::extract::{Path, State};
use axum::Json;
use heldar_kernel::auth::{Principal, Scope};
use heldar_kernel::routes::sites;

/// Admin caps, camera scope. Only `scope` differs from the synthetic admin, so any difference in
/// outcome is attributable to the scope and nothing else.
fn admin_but_scoped() -> Principal {
    let mut set = std::collections::HashSet::new();
    set.insert("cam_a".to_string());
    Principal {
        scope: Scope::Cameras(std::sync::Arc::new(set)),
        ..Principal::system_admin()
    }
}

#[tokio::test]
async fn every_site_write_refuses_a_camera_scoped_credential_on_scope_alone() {
    let st = state().await;
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;

    let p = admin_but_scoped();
    assert!(
        p.can_admin(),
        "this principal must pass the admin gate, or the test proves the admin gate again \
         instead of the scope guard"
    );

    let create = sites::create(
        State(st.clone()),
        p.clone(),
        Json(serde_json::from_str(r#"{"id":"x","name":"X"}"#).unwrap()),
    )
    .await;
    assert!(create.is_err(), "creating a site must be fleet-only");

    let update = sites::update(
        State(st.clone()),
        p.clone(),
        Path("kl".to_string()),
        Json(serde_json::from_str(r#"{"timezone":"America/New_York"}"#).unwrap()),
    )
    .await;
    assert!(
        update.is_err(),
        "moving a site's clock moves recording windows for every camera on it — fleet-only"
    );

    let del = sites::delete_site(State(st.clone()), p, Path("kl".to_string())).await;
    assert!(del.is_err(), "deleting a site must be fleet-only");

    // The control: the same calls succeed for a fleet-wide admin, so the refusals above are the
    // scope and not something else rejecting every request.
    let fleet = Principal::system_admin();
    assert!(sites::create(
        State(st.clone()),
        fleet.clone(),
        Json(serde_json::from_str(r#"{"id":"ok","name":"OK"}"#).unwrap()),
    )
    .await
    .is_ok());
    assert!(sites::delete_site(State(st), fleet, Path("ok".to_string()))
        .await
        .is_ok());
}

/// The box-wide timezone PUT has the same shape and the same gap: the census proves it 403s, but via
/// the admin gate. This is where `require_fleet_scope` on it is actually proven.
#[tokio::test]
async fn the_box_timezone_write_refuses_a_camera_scoped_credential_on_scope_alone() {
    let st = state().await;
    let p = admin_but_scoped();
    assert!(p.can_admin());

    let scoped = heldar_kernel::routes::system::put_timezone(
        State(st.clone()),
        p,
        Json(serde_json::from_str(r#"{"timezone":"America/New_York"}"#).unwrap()),
    )
    .await;
    assert!(
        scoped.is_err(),
        "a zone reinterprets every camera on the box — a camera-scoped credential must not set it"
    );

    let fleet = heldar_kernel::routes::system::put_timezone(
        State(st),
        Principal::system_admin(),
        Json(serde_json::from_str(r#"{"timezone":"America/New_York"}"#).unwrap()),
    )
    .await;
    assert!(fleet.is_ok(), "and a fleet-wide admin must be able to");
}

// -------------------------------------------------------------------------------------------------
// Moving a camera between sites moves the hours it records. Reaching that effect through the CAMERA
// required only `RegistryManage` and a camera scope, said nothing, and audited `{}` — while the
// identical effect through `PATCH /api/v1/sites/{id}` was admin + fleet and announced itself.
// -------------------------------------------------------------------------------------------------

#[tokio::test]
async fn moving_a_camera_between_sites_is_fleet_only_and_leaves_a_record() {
    let st = state().await;
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;
    add_camera(&st, "cam_a", None).await;

    // A camera-scoped credential cannot move it — which also closes the site-enumeration oracle,
    // since a real site id and an invented one now get the same refusal.
    let scoped = admin_but_scoped();
    let moved = heldar_kernel::routes::cameras::update_camera(
        State(st.clone()),
        Path("cam_a".to_string()),
        scoped.clone(),
        Json(serde_json::from_str(r#"{"site_id":"kl"}"#).unwrap()),
    )
    .await;
    assert!(
        moved.is_err(),
        "a scoped credential must not move a camera's clock"
    );
    let ghost = heldar_kernel::routes::cameras::update_camera(
        State(st.clone()),
        Path("cam_a".to_string()),
        scoped,
        Json(serde_json::from_str(r#"{"site_id":"no_such_site"}"#).unwrap()),
    )
    .await;
    assert!(
        ghost.is_err(),
        "and an invented site id must be refused the SAME way, or the difference enumerates sites"
    );

    // A fleet-wide admin can, and it is recorded.
    let fleet = Principal::system_admin();
    assert!(heldar_kernel::routes::cameras::update_camera(
        State(st.clone()),
        Path("cam_a".to_string()),
        fleet.clone(),
        Json(serde_json::from_str(r#"{"site_id":"kl"}"#).unwrap()),
    )
    .await
    .is_ok());

    let (tz, _) = heldar_kernel::services::tz::site_tz(&st.pool, Some("cam_a")).await;
    assert_eq!(
        tz.map(|t| t.to_string()).as_deref(),
        Some("Asia/Kuala_Lumpur")
    );

    let detail: (String,) = sqlx::query_as(
        "SELECT detail FROM audit_log WHERE action = 'update_camera' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&st.pool)
    .await
    .unwrap();
    let d: serde_json::Value = serde_json::from_str(&detail.0).unwrap();
    assert_eq!(d["site_changed"], true, "the audit row must record it: {d}");
    assert_eq!(d["timezone_now"], "Asia/Kuala_Lumpur", "{d}");
    assert!(d["previous_site_id"].is_null(), "{d}");
}

/// `DELETE /api/v1/sites/{id}` tells the operator to reassign the cameras first. That has to be
/// possible: `site_id` absent means "leave it", and only an explicit `null` detaches.
#[tokio::test]
async fn a_camera_can_actually_be_detached_from_its_site() {
    let st = state().await;
    call(
        &st,
        "POST",
        "/api/v1/sites",
        r#"{"id":"kl","name":"KL","timezone":"Asia/Kuala_Lumpur"}"#,
    )
    .await;
    add_camera(&st, "cam_a", Some("kl")).await;
    let fleet = Principal::system_admin();

    // Absent leaves it where it is.
    let _ = heldar_kernel::routes::cameras::update_camera(
        State(st.clone()),
        Path("cam_a".to_string()),
        fleet.clone(),
        Json(serde_json::from_str(r#"{"name":"Renamed"}"#).unwrap()),
    )
    .await
    .expect("rename");
    let (tz, _) = heldar_kernel::services::tz::site_tz(&st.pool, Some("cam_a")).await;
    assert!(tz.is_some(), "a rename must not detach the camera");

    // Explicit null detaches, and the site then deletes.
    let _ = heldar_kernel::routes::cameras::update_camera(
        State(st.clone()),
        Path("cam_a".to_string()),
        fleet,
        Json(serde_json::from_str(r#"{"site_id":null}"#).unwrap()),
    )
    .await
    .expect("detach");
    let (tz, src) = heldar_kernel::services::tz::site_tz(&st.pool, Some("cam_a")).await;
    assert_eq!(tz, None, "explicit null must detach");
    assert_eq!(src, heldar_kernel::services::tz::TzSource::Unset);

    let (s, v) = call(&st, "DELETE", "/api/v1/sites/kl", "").await;
    assert_eq!(
        s,
        StatusCode::OK,
        "the 409's advice must be followable: {v}"
    );
}

/// A filtered list is something the route census cannot check — it can only tell a refusal from an
/// answer, and this route legitimately answers. So the confinement needs its own test, and without
/// one the read half of this API was asserted by nothing at all.
#[tokio::test]
async fn a_scoped_credential_sees_only_its_own_sites() {
    let st = state().await;
    for (id, tz) in [("kl", "Asia/Kuala_Lumpur"), ("ldn", "Europe/London")] {
        call(
            &st,
            "POST",
            "/api/v1/sites",
            &format!(r#"{{"id":"{id}","name":"{id}","timezone":"{tz}"}}"#),
        )
        .await;
    }
    add_camera(&st, "cam_a", Some("kl")).await;
    add_camera(&st, "cam_secret", Some("ldn")).await;

    let scoped = admin_but_scoped(); // holds cam_a only
    let listed = sites::list(State(st.clone()), scoped.clone())
        .await
        .unwrap();
    let ids: Vec<&str> = listed.0["sites"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["kl"],
        "a scoped credential must see only the sites its own cameras are on"
    );

    // ...and a site it holds no camera on answers exactly as a site that does not exist, so an id
    // cannot be used to discover the fleet's structure.
    let hidden = sites::get_one(State(st.clone()), scoped.clone(), Path("ldn".to_string())).await;
    let missing = sites::get_one(State(st.clone()), scoped, Path("nope".to_string())).await;
    let shape = |r: &Result<_, heldar_kernel::error::AppError>| match r {
        Err(e) => format!("{e:?}")
            .split('(')
            .next()
            .unwrap_or("?")
            .to_string(),
        Ok(_) => "Ok".to_string(),
    };
    assert_eq!(
        shape(&hidden),
        shape(&missing),
        "an out-of-scope site and an unknown one must be indistinguishable"
    );
    assert!(hidden.is_err());

    // The control: a fleet-wide credential sees both, so the filtering above is the scope and not
    // something else hiding everything.
    let all = sites::list(State(st), Principal::system_admin())
        .await
        .unwrap();
    assert_eq!(all.0["sites"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn ids_and_names_are_validated_rather_than_stored_as_given() {
    let st = state().await;
    for (body, why) in [
        (r#"{"id":"","name":"X"}"#, "empty id"),
        (r#"{"id":"has space","name":"X"}"#, "space in id"),
        (r#"{"id":"a/b","name":"X"}"#, "slash in id"),
        (r#"{"id":"kl","name":"   "}"#, "whitespace-only name"),
    ] {
        let (s, v) = call(&st, "POST", "/api/v1/sites", body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{why} must be refused: {v}");
    }
    let long = "a".repeat(65);
    let (s, _) = call(
        &st,
        "POST",
        "/api/v1/sites",
        &format!(r#"{{"id":"{long}","name":"X"}}"#),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a 65-char id is not a slug");

    // A whitespace-only rename keeps the old name rather than storing "   ".
    call(&st, "POST", "/api/v1/sites", r#"{"id":"kl","name":"KL"}"#).await;
    call(&st, "PATCH", "/api/v1/sites/kl", r#"{"name":"   "}"#).await;
    let (_, v) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    assert_eq!(
        v["name"], "KL",
        "a blank rename must not blank the name: {v}"
    );
}

/// Every timestamp the API emits should look the same. sqlx hands back `+00:00` from a raw column
/// while typed models re-serialize as `Z`, and one box speaking two dialects is a trap a generated
/// client will not catch — OpenAPI types both as plain `string`.
#[tokio::test]
async fn timestamps_match_the_rest_of_the_api() {
    let st = state().await;
    call(&st, "POST", "/api/v1/sites", r#"{"id":"kl","name":"KL"}"#).await;
    let (_, v) = call(&st, "GET", "/api/v1/sites/kl", "").await;
    let ts = v["created_at"].as_str().unwrap_or_default();
    assert!(
        ts.ends_with('Z'),
        "site timestamps must be Z-form like every other model, got {ts:?}"
    );
}
