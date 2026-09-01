//! Withdraw live MediaMTX sessions whose credential no longer stands.
//!
//! `/internal/mediamtx-auth` re-resolves a token's subject on every read, which bounds a transport to
//! the rate at which it RE-PRESENTS the token. HLS re-presents per segment, so revocation bites in
//! seconds and this reaper has little left to do. **WebRTC never re-presents**: it authorizes once at
//! WHEP negotiation, then media flows over the established peer connection and MediaMTX asks nothing
//! further. RTSP readers behave the same way. For those, "the token is bound to its subject" bought
//! nothing at all — the session simply outlived the credential.
//!
//! So the callback records who opened each session (`live_sessions`, migration 0016) and this loop
//! closes it: list what MediaMTX still holds, re-ask the SAME question the callback asks, and kick
//! whoever fails it.
//!
//! # Why it reuses `media_auth::subject_still_stands`
//!
//! Because "does this credential still stand" must have exactly one definition. Two copies of that
//! question is how the mint-time and request-time capability checks drifted apart until an `admin`
//! grant could carry a camera scope past the very refusal aimed at it. A reaper with its own,
//! subtly-different idea of withdrawal would kick sessions the callback would have allowed, or spare
//! ones it would have refused — and nobody would notice until an operator did.
//!
//! # What it will not do
//!
//! - **Publishes are never recorded, so they can never be kicked.** The cameras feed the box over
//!   RTSP, and their sessions look exactly like a reader's to this API. That separation is structural
//!   (the callback records only the `read`/`playback` arm) rather than a filter here that someone
//!   could later forget.
//! - **A session it cannot attribute is left alone**, and logged. It is unattributable, not
//!   known-bad: it predates this table, or survived a database loss. Kicking on ignorance would make
//!   a restart look like an outage.
//! - **`Subject::Site` is never withdrawn** — the WebRTC rendezvous holds a site token rather than a
//!   principal, and a remote viewer must not lose video because an unrelated key was revoked.

use std::time::Duration;

use crate::state::AppState;

/// MediaMTX session lists that correspond to READS we can kick. HLS is deliberately absent: it
/// re-presents its token per segment, so the callback already stops it within seconds and there is no
/// long-lived session object to kick.
const READER_KINDS: [&str; 2] = ["webrtcsessions", "rtspsessions"];

/// One sweep: for each live session MediaMTX holds, is its credential still good?
///
/// Returns the number kicked. Never returns an error — an unreachable MediaMTX or a busy database is
/// a reason to try again next tick, not to take the loop down.
pub async fn sweep(state: &AppState) -> u64 {
    let api = state.cfg.mediamtx_api_url.trim_end_matches('/');
    let mut kicked = 0u64;
    for kind in READER_KINDS {
        let listed = match list_sessions(state, api, kind).await {
            Some(v) => v,
            None => continue,
        };
        for id in listed {
            let Some(rec) = load_session(state, &id).await else {
                // Not ours to judge — see the module note on unattributable sessions.
                tracing::debug!(session = %id, kind, "live_reaper: session has no recorded subject; leaving it");
                continue;
            };
            let subject = match rec.subject_kind.as_str() {
                "api_key" => match rec.subject_id {
                    Some(sid) => crate::services::live_token::Subject::ApiKey(sid),
                    None => continue,
                },
                "user" => match rec.subject_id {
                    Some(sid) => crate::services::live_token::Subject::User(sid),
                    None => continue,
                },
                // Site subjects are not recorded; anything else is a row we do not understand.
                _ => continue,
            };
            if crate::routes::media_auth::subject_still_stands(state, &subject, &rec.path).await {
                continue;
            }
            if kick(state, api, kind, &id).await {
                kicked += 1;
                tracing::warn!(
                    target: "heldar::security",
                    session = %id, kind, path = %rec.path,
                    "live_reaper: withdrew a live stream whose credential no longer stands"
                );
                let _ = sqlx::query("DELETE FROM live_sessions WHERE id = ?")
                    .bind(&id)
                    .execute(&state.pool)
                    .await;
            }
        }
    }
    prune_stale(state).await;
    kicked
}

struct SessionRow {
    path: String,
    subject_kind: String,
    subject_id: Option<String>,
}

async fn load_session(state: &AppState, id: &str) -> Option<SessionRow> {
    sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT path, subject_kind, subject_id FROM live_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .map(|(path, subject_kind, subject_id)| SessionRow {
        path,
        subject_kind,
        subject_id,
    })
}

/// The session ids MediaMTX currently holds for `kind`, or `None` if it could not be asked.
async fn list_sessions(state: &AppState, api: &str, kind: &str) -> Option<Vec<String>> {
    let resp = state
        .http
        .get(format!("{api}/v3/{kind}/list"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    Some(
        body.get("items")?
            .as_array()?
            .iter()
            .filter_map(|i| i.get("id")?.as_str().map(String::from))
            .collect(),
    )
}

async fn kick(state: &AppState, api: &str, kind: &str, id: &str) -> bool {
    match state
        .http
        .post(format!("{api}/v3/{kind}/kick/{id}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        // A session that ended between the list and the kick answers 404; treat that as done, not
        // as a failure to retry forever.
        Ok(r) => r.status().is_success() || r.status().as_u16() == 404,
        Err(e) => {
            tracing::warn!(session = %id, error = %e, "live_reaper: kick failed; will retry next sweep");
            false
        }
    }
}

/// Drop records for sessions MediaMTX has not re-authorized in a long while.
///
/// Generous on purpose: an established WebRTC session may never re-authorize at all, so `last_seen_at`
/// stops advancing while the session is still very much alive. The cutoff has to outlast a plausible
/// viewing session, or the reaper would forget who is watching and then decline to withdraw them.
async fn prune_stale(state: &AppState) {
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let _ = sqlx::query("DELETE FROM live_sessions WHERE last_seen_at < ?")
        .bind(cutoff)
        .execute(&state.pool)
        .await;
}

/// Withdraw NOW, for one subject, without waiting for the next tick.
///
/// The periodic loop is a backstop for changes this process never sees — a key expiring, a row edited
/// out of band, a kick that failed and needs retrying. But a revocation made THROUGH the API is an
/// event we are holding in our hands, and making the operator wait a poll interval for it is a choice,
/// not a constraint.
///
/// Spawned, never awaited by the request: the operator's `PATCH` must not block on MediaMTX being
/// reachable, and the loop will catch anything this misses.
pub fn withdraw_now(state: &AppState, subject_kind: &'static str, subject_id: String) {
    let state = state.clone();
    tokio::spawn(async move {
        let sessions: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, protocol, path FROM live_sessions WHERE subject_kind = ? AND subject_id = ?",
        )
        .bind(subject_kind)
        .bind(&subject_id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        if sessions.is_empty() {
            return;
        }
        let api = state.cfg.mediamtx_api_url.trim_end_matches('/');
        for (id, protocol, path) in sessions {
            let subject = match subject_kind {
                "api_key" => crate::services::live_token::Subject::ApiKey(subject_id.clone()),
                "user" => crate::services::live_token::Subject::User(subject_id.clone()),
                _ => return,
            };
            // Re-ask rather than assume: an update that NARROWED a scope leaves other cameras'
            // sessions perfectly valid, and this must not cut them.
            if crate::routes::media_auth::subject_still_stands(&state, &subject, &path).await {
                continue;
            }
            // `protocol` decides the endpoint; anything not in READER_KINDS has no session to kick
            // (HLS re-presents its token and is stopped by the callback instead).
            let kind = match protocol.as_str() {
                "webrtc" => "webrtcsessions",
                "rtsp" => "rtspsessions",
                _ => continue,
            };
            if kick(&state, api, kind, &id).await {
                tracing::warn!(
                    target: "heldar::security",
                    session = %id, kind, path = %path,
                    "live_reaper: withdrew a live stream immediately on credential change"
                );
                let _ = sqlx::query("DELETE FROM live_sessions WHERE id = ?")
                    .bind(&id)
                    .execute(&state.pool)
                    .await;
            }
        }
    });
}

/// Run [`sweep`] forever. Interval trades revocation latency against MediaMTX API chatter.
pub async fn run(state: AppState) {
    let every = Duration::from_secs(state.cfg.live_reaper_interval_s.max(2));
    loop {
        tokio::time::sleep(every).await;
        let n = sweep(&state).await;
        if n > 0 {
            tracing::info!(kicked = n, "live_reaper: withdrew live streams");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::live_token::Subject;
    use std::sync::{Arc, Mutex};

    /// A stand-in MediaMTX: serves one session in `/v3/{kind}/list` and records what got kicked.
    ///
    /// A real listener rather than a stubbed client, because the thing most likely to be wrong here is
    /// the SHAPE of the exchange — the list JSON, the kick path, the 404-means-gone case — and none of
    /// that is exercised by faking the transport.
    struct FakeMediamtx {
        base: String,
        kicked: Arc<Mutex<Vec<String>>>,
        _handle: tokio::task::JoinHandle<()>,
    }

    async fn fake_mediamtx(session_ids: Vec<String>) -> FakeMediamtx {
        use axum::extract::Path;
        use axum::routing::{get, post};
        let kicked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let listed = Arc::new(session_ids);
        let k = kicked.clone();
        let l = listed.clone();
        let app = axum::Router::new()
            .route(
                "/v3/{kind}/list",
                get(move |Path(kind): Path<String>| {
                    let l = l.clone();
                    async move {
                        // Only the webrtc list is populated, so a test also proves the reaper asks
                        // for more than one kind without double-counting.
                        let items: Vec<serde_json::Value> = if kind == "webrtcsessions" {
                            l.iter().map(|id| serde_json::json!({ "id": id })).collect()
                        } else {
                            Vec::new()
                        };
                        axum::Json(serde_json::json!({ "itemCount": items.len(), "items": items }))
                    }
                }),
            )
            .route(
                "/v3/{kind}/kick/{id}",
                post(move |Path((_kind, id)): Path<(String, String)>| {
                    let k = k.clone();
                    async move {
                        k.lock().unwrap().push(id);
                        axum::http::StatusCode::OK
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeMediamtx {
            base,
            kicked,
            _handle: handle,
        }
    }

    async fn state_with(api: &str) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut c = crate::config::Config::from_env();
        c.auth_enabled = true;
        c.mediamtx_api_url = api.to_string();
        let cfg = std::sync::Arc::new(c);
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: std::sync::Arc::new(Vec::new()),
            modules: std::sync::Arc::new(Vec::new()),
            catalog: std::sync::Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    async fn seed_key(st: &AppState, id: &str, cameras: &[&str], revoked: bool) {
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at,
                                   capabilities, scope_kind, scope_cameras, revoked_at)
             VALUES (?,?,?,?,'integration',1,?,?,'cameras',?,?)",
        )
        .bind(id)
        .bind(id)
        .bind(format!("hash_{id}"))
        .bind("vok_test")
        .bind(now)
        .bind(serde_json::json!(["camera:read", "video:live"]).to_string())
        .bind(serde_json::json!(cameras).to_string())
        .bind(if revoked { Some(now) } else { None })
        .execute(&st.pool)
        .await
        .unwrap();
    }

    async fn record(st: &AppState, session: &str, path: &str, key_id: &str) {
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO live_sessions (id, protocol, path, subject_kind, subject_id, created_at, last_seen_at)
             VALUES (?,'webrtc',?,'api_key',?,?,?)",
        )
        .bind(session)
        .bind(path)
        .bind(key_id)
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
    }

    /// The whole point: a WebRTC session never re-presents its token, so only this loop can end it.
    #[tokio::test]
    async fn a_revoked_credentials_established_session_is_kicked() {
        let mtx = fake_mediamtx(vec!["sess_revoked".into()]).await;
        let st = state_with(&mtx.base).await;
        seed_key(&st, "key_gone", &["cam_a"], true).await;
        record(&st, "sess_revoked", "cam_cam_a", "key_gone").await;

        assert_eq!(
            sweep(&st).await,
            1,
            "the revoked credential's stream survived"
        );
        assert_eq!(
            *mtx.kicked.lock().unwrap(),
            vec!["sess_revoked".to_string()]
        );
        // The record goes with it, so a later sweep does not keep re-kicking a dead id.
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM live_sessions")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    /// The control that stops the above from being satisfied by a reaper that kicks everything.
    #[tokio::test]
    async fn a_live_credentials_session_is_left_alone() {
        let mtx = fake_mediamtx(vec!["sess_ok".into()]).await;
        let st = state_with(&mtx.base).await;
        seed_key(&st, "key_good", &["cam_a"], false).await;
        record(&st, "sess_ok", "cam_cam_a", "key_good").await;

        assert_eq!(sweep(&st).await, 0, "a valid credential's stream was cut");
        assert!(mtx.kicked.lock().unwrap().is_empty());
    }

    /// Re-scoping is the other half: the credential is perfectly valid, it just lost this camera.
    #[tokio::test]
    async fn a_session_on_a_camera_the_credential_lost_is_kicked() {
        let mtx = fake_mediamtx(vec!["sess_rescoped".into()]).await;
        let st = state_with(&mtx.base).await;
        seed_key(&st, "key_narrow", &["cam_a"], false).await;
        // Watching cam_b, which the key does not hold.
        record(&st, "sess_rescoped", "cam_cam_b", "key_narrow").await;

        assert_eq!(sweep(&st).await, 1);
        assert_eq!(
            *mtx.kicked.lock().unwrap(),
            vec!["sess_rescoped".to_string()]
        );
    }

    /// A session with no recorded subject is unattributable, not known-bad: it predates the table or
    /// survived a database loss. Kicking on ignorance would turn a restart into an outage.
    #[tokio::test]
    async fn an_unattributable_session_is_left_alone() {
        let mtx = fake_mediamtx(vec!["sess_unknown".into()]).await;
        let st = state_with(&mtx.base).await;
        assert_eq!(sweep(&st).await, 0);
        assert!(mtx.kicked.lock().unwrap().is_empty());
    }

    /// An unreachable MediaMTX is a reason to try again, never to take the loop down.
    #[tokio::test]
    async fn an_unreachable_mediamtx_is_survived() {
        let st = state_with("http://127.0.0.1:1").await;
        seed_key(&st, "key_gone", &["cam_a"], true).await;
        record(&st, "sess_revoked", "cam_cam_a", "key_gone").await;
        assert_eq!(sweep(&st).await, 0);
    }

    /// The immediate path: a revocation made through the API should not wait for the next tick.
    ///
    /// `withdraw_now` is spawned so the operator's request never blocks on MediaMTX, so the assertion
    /// polls rather than reading straight after — testing a spawned effect by looking once is how you
    /// write a test that passes on a fast machine and flakes on a loaded one.
    #[tokio::test]
    async fn withdraw_now_cuts_a_revoked_credentials_session_without_waiting() {
        let mtx = fake_mediamtx(vec!["sess_immediate".into()]).await;
        let st = state_with(&mtx.base).await;
        seed_key(&st, "key_burned", &["cam_a"], true).await;
        record(&st, "sess_immediate", "cam_cam_a", "key_burned").await;

        super::withdraw_now(&st, "api_key", "key_burned".to_string());

        for _ in 0..50 {
            if !mtx.kicked.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            *mtx.kicked.lock().unwrap(),
            vec!["sess_immediate".to_string()],
            "the revoked credential's stream was not withdrawn immediately"
        );
    }

    /// ...and it re-asks per session rather than cutting everything the credential opened. Narrowing
    /// a scope leaves the cameras it still holds perfectly valid.
    #[tokio::test]
    async fn withdraw_now_spares_the_cameras_the_credential_still_holds() {
        let mtx = fake_mediamtx(vec!["sess_kept".into(), "sess_lost".into()]).await;
        let st = state_with(&mtx.base).await;
        seed_key(&st, "key_narrowed", &["cam_a"], false).await;
        record(&st, "sess_kept", "cam_cam_a", "key_narrowed").await;
        record(&st, "sess_lost", "cam_cam_b", "key_narrowed").await;

        super::withdraw_now(&st, "api_key", "key_narrowed".to_string());

        for _ in 0..50 {
            if !mtx.kicked.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(
            *mtx.kicked.lock().unwrap(),
            vec!["sess_lost".to_string()],
            "withdrawal must be per camera: the session on the camera it STILL holds was cut"
        );
    }

    /// `Subject::Site` is never recorded, so the rendezvous path can never be kicked by this loop.
    #[test]
    fn site_subjects_are_not_withdrawable() {
        assert!(matches!(
            Subject::of(&crate::auth::Principal::system_admin()),
            Subject::Site
        ));
    }
}
