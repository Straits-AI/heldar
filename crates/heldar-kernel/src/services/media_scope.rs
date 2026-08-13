//! Camera scope for the recorded-media plane (`/media/*`).
//!
//! `/media/*` serves the same footage the API gates, so it must be gated the same way. Two of the
//! five subtrees name their camera in the path (`recordings/<camera_id>/…`, and the SCHEDULED
//! snapshots at `snapshots/<camera_id>/<ts>.jpg`) and are scopable by string alone. The other three
//! — clips, playback sessions and archives — are FLAT: `clips/clip_<uuid>.mp4`,
//! `playback/pbs_<uuid>/…`, `archives/<job>.zip` carry no camera anywhere, which is why they used to
//! be gated by capability only and were readable by any credential holding that capability.
//!
//! This module adds the missing attribution as a sidecar table keyed by the artifact's PATH
//! (`media_artifacts`, migration 0013), so artifacts keep their current names and locations. A
//! producer registers its output with [`attribute`]; the [`guard`] middleware resolves it with
//! [`owners`] and refuses anything it cannot attribute to a camera the caller holds.
//!
//! Fail-closed by construction, in both directions:
//! - a producer that never registers its artifact leaves it `Unattributed`, which is a 403 for a
//!   camera-scoped credential and unchanged for everyone else;
//! - a `/media/*` prefix this module does not recognise is refused for EVERY credential, before the
//!   unscoped fast path, so adding a sixth `nest_service` cannot silently serve it ungated.
//!
//! Cost: auth disabled returns at the first line; an unscoped credential (every human role, the
//! dashboard, every `<video>` byte-range request) pays one discriminant compare and no I/O. Only a
//! camera-scoped credential reaches the database, and then only on the flat subtrees.

use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use sqlx::SqlitePool;

use crate::auth::{Cap, Principal};
use crate::state::AppState;

/// `media_artifacts.kind` values. Descriptive only — the guard keys on `path`.
pub const KIND_CLIP: &str = "clip";
pub const KIND_PLAYBACK_SESSION: &str = "playback_session";
pub const KIND_ZONE_EVIDENCE: &str = "zone_evidence";
pub const KIND_EMBED_THUMB: &str = "embed_thumb";
pub const KIND_ENTRY_EVIDENCE: &str = "entry_evidence";
pub const KIND_ARCHIVE: &str = "archive";

/// How a `/media/*` path is scoped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    /// The camera id is the first path segment of the subtree — scopable without a lookup.
    Partitioned,
    /// A flat artifact whose owning camera(s) come from `media_artifacts`.
    Artifact,
    /// An internal file that is never a viewer surface (`clips/<id>.txt` holds absolute recording
    /// paths; `playback/<id>/session.json` discloses the camera, window and source segment ids).
    /// Refused for every credential — nothing in the product fetches these over HTTP.
    Denied,
}

/// The cameras an artifact belongs to, or the absence of any attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owners {
    Cameras(Vec<String>),
    /// No row: either the artifact predates migration 0013, or its producer failed to register it.
    /// Both are refused for a camera-scoped credential.
    Unattributed,
}

/// Register an artifact's owning camera(s). The ONLY public write path into `media_artifacts`.
///
/// Deliberately infallible from the caller's point of view: a producer (clip export, playback session
/// build, evidence copy, archive export) must never fail its job because attribution could not be
/// written. A dropped row leaves the artifact `Unattributed`, which fails CLOSED at read time.
pub async fn attribute(pool: &SqlitePool, key: &str, cameras: &[String], kind: &str) {
    let now = Utc::now();
    for cam in cameras {
        if let Err(e) = sqlx::query(
            "INSERT INTO media_artifacts (path, camera_id, kind, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(key)
        .bind(cam)
        .bind(kind)
        .bind(now)
        .execute(pool)
        .await
        {
            tracing::warn!(
                artifact = %key, camera = %cam, kind = %kind, error = %e,
                "media_scope: could not attribute artifact; it will be refused for scoped credentials"
            );
        }
    }
}

/// Drop an artifact's attribution when the artifact itself is deleted.
pub async fn forget(pool: &SqlitePool, key: &str) {
    if let Err(e) = sqlx::query("DELETE FROM media_artifacts WHERE path = ?")
        .bind(key)
        .execute(pool)
        .await
    {
        tracing::warn!(artifact = %key, error = %e, "media_scope: could not forget artifact row");
    }
}

/// The cameras owning the artifact at `key`. A database error reads as `Unattributed`, i.e. refused.
pub async fn owners(pool: &SqlitePool, key: &str) -> Owners {
    match sqlx::query_scalar::<_, String>("SELECT camera_id FROM media_artifacts WHERE path = ?")
        .bind(key)
        .fetch_all(pool)
        .await
    {
        Ok(v) if !v.is_empty() => Owners::Cameras(v),
        Ok(_) => Owners::Unattributed,
        Err(e) => {
            tracing::warn!(artifact = %key, error = %e, "media_scope: attribution lookup failed; refusing");
            Owners::Unattributed
        }
    }
}

/// The part of a request path inside the media root, e.g. `/media/clips/x.mp4` -> `clips/x.mp4`.
fn media_rel(path: &str) -> Option<&str> {
    path.trim_start_matches('/').strip_prefix("media/")
}

fn segments(rel: &str) -> Vec<&str> {
    rel.split('/').filter(|s| !s.is_empty()).collect()
}

/// The capability a `/media/*` path requires, and how it is scoped.
///
/// `None` means "not a media path this module recognises" and the guard refuses it outright — for
/// every credential, scoped or not. The previous `_ => None` fallback *served* such a path, so a new
/// `nest_service` was ungated until someone remembered to extend the match.
pub fn requirement(path: &str) -> Option<(Cap, MediaKind)> {
    let rel = media_rel(path)?;
    let segs = segments(rel);
    let (subtree, rest) = segs.split_first()?;
    match *subtree {
        // recordings/<camera_id>/<segment> — camera-partitioned on disk since day one.
        "recordings" => Some((Cap::VideoPlayback, MediaKind::Partitioned)),
        // snapshots is a MIXED subtree: the scheduler writes snapshots/<camera_id>/<ts>.jpg
        // (partitioned, resolvable by prefix), while zone/entry evidence and embedding thumbs are
        // flat single files (attributed).
        "snapshots" => match rest.len() {
            0 => None,
            1 => Some((Cap::VideoPlayback, MediaKind::Artifact)),
            _ => Some((Cap::VideoPlayback, MediaKind::Partitioned)),
        },
        // clips/<id>.mp4 is the export; clips/<id>.txt is the ffmpeg concat list, which holds
        // ABSOLUTE recording paths for the source camera inside the served tree.
        "clips" => match rest {
            [f] if f.ends_with(".mp4") => Some((Cap::VideoExport, MediaKind::Artifact)),
            _ => Some((Cap::VideoExport, MediaKind::Denied)),
        },
        // playback/<session>/{index.m3u8,init.mp4,seg_*.m4s} is the HLS VOD; session.json is the
        // sidecar and concat.txt the temp list — neither is a viewer surface.
        "playback" => match rest {
            [_id, f]
                if *f == "index.m3u8"
                    || *f == "init.mp4"
                    || (f.starts_with("seg_") && f.ends_with(".m4s")) =>
            {
                Some((Cap::VideoPlayback, MediaKind::Artifact))
            }
            _ => Some((Cap::VideoPlayback, MediaKind::Denied)),
        },
        // Backup archives are a whole-system export — admin only, never a viewer surface.
        "archives" => match rest.len() {
            1 => Some((Cap::Admin, MediaKind::Artifact)),
            _ => Some((Cap::Admin, MediaKind::Denied)),
        },
        _ => None,
    }
}

/// The `media_artifacts.path` key for a flat-artifact request, or `None` if the path is not one.
///
/// A directory artifact is keyed by its DIRECTORY: `playback/pbs_abc` covers `index.m3u8`,
/// `init.mp4` and every `seg_*.m4s` beneath it, so a scrub through a session shares one row.
pub fn artifact_key(path: &str) -> Option<String> {
    let rel = media_rel(path)?;
    match segments(rel).as_slice() {
        ["clips", f] => Some(format!("clips/{f}")),
        ["snapshots", f] => Some(format!("snapshots/{f}")),
        ["archives", f] => Some(format!("archives/{f}")),
        ["playback", id, ..] => Some(format!("playback/{id}")),
        _ => None,
    }
}

/// The camera id owning a partitioned path: the first segment after the subtree.
fn partition_camera(path: &str) -> Option<&str> {
    let rel = media_rel(path)?;
    let segs = segments(rel);
    segs.get(1).copied().filter(|s| !s.is_empty())
}

/// True if any path segment could escape the subtree it appears to name. Only consulted for a
/// camera-scoped credential, whose scope decision is derived FROM the path — the key must be honest.
fn dishonest_path(path: &str) -> bool {
    if path.split('/').any(|s| s == ".." || s == ".") {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    // Percent-encoded separators/dots would be decoded by the file server after we keyed on them.
    lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c")
}

/// Auth + camera-scope guard for the recorded-media plane.
///
/// Ordering is load-bearing:
/// 1. auth disabled -> untouched pass-through (the LAN-appliance default);
/// 2. no credential -> 401;
/// 3. unrecognised `/media/*` prefix -> 403 **before** the unscoped fast path, so a subtree added
///    without extending [`requirement`] is refused rather than served to everyone;
/// 4. missing capability -> 403;
/// 5. internal (`Denied`) filenames -> 403 for every credential;
/// 6. unscoped credential -> pass, having paid one discriminant compare;
/// 7. camera-scoped credential -> path honesty, then partition prefix or artifact attribution.
pub async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if !st.cfg.auth_enabled {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    let (mut parts, body) = req.into_parts();
    let principal = match Principal::from_request_parts(&mut parts, &st).await {
        Ok(p) => p,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let Some((cap, kind)) = requirement(&path) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if !principal.has(cap) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if kind == MediaKind::Denied {
        return StatusCode::FORBIDDEN.into_response();
    }
    if principal.camera_scope().is_none() {
        return next.run(Request::from_parts(parts, body)).await;
    }
    // ---- below here runs ONLY for a camera-scoped credential ----
    if dishonest_path(&path) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let allowed = match kind {
        MediaKind::Denied => false,
        MediaKind::Partitioned => {
            matches!(partition_camera(&path), Some(cam) if principal.camera_allowed(cam))
        }
        MediaKind::Artifact => match artifact_key(&path) {
            Some(key) => match owners(&st.pool, &key).await {
                Owners::Cameras(v) => v.iter().all(|c| principal.camera_allowed(c)),
                Owners::Unattributed => false,
            },
            None => false,
        },
    };
    if !allowed {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_maps_every_served_prefix() {
        assert_eq!(
            requirement("/media/recordings/cam_a/seg.mp4"),
            Some((Cap::VideoPlayback, MediaKind::Partitioned))
        );
        assert_eq!(
            requirement("/media/snapshots/cam_a/1700000000.jpg"),
            Some((Cap::VideoPlayback, MediaKind::Partitioned))
        );
        assert_eq!(
            requirement("/media/snapshots/zoneevt_x.jpg"),
            Some((Cap::VideoPlayback, MediaKind::Artifact))
        );
        assert_eq!(
            requirement("/media/clips/clip_x.mp4"),
            Some((Cap::VideoExport, MediaKind::Artifact))
        );
        assert_eq!(
            requirement("/media/clips/clip_x.txt"),
            Some((Cap::VideoExport, MediaKind::Denied))
        );
        assert_eq!(
            requirement("/media/playback/pbs_x/index.m3u8"),
            Some((Cap::VideoPlayback, MediaKind::Artifact))
        );
        assert_eq!(
            requirement("/media/playback/pbs_x/seg_00001.m4s"),
            Some((Cap::VideoPlayback, MediaKind::Artifact))
        );
        assert_eq!(
            requirement("/media/playback/pbs_x/session.json"),
            Some((Cap::VideoPlayback, MediaKind::Denied))
        );
        assert_eq!(
            requirement("/media/playback/pbs_x/concat.txt"),
            Some((Cap::VideoPlayback, MediaKind::Denied))
        );
        assert_eq!(
            requirement("/media/archives/bkp_x.zip"),
            Some((Cap::Admin, MediaKind::Artifact))
        );
    }

    #[test]
    fn an_unrecognised_prefix_is_refused_not_served() {
        // The regression test for the old `_ => None` fallback: a sixth nest_service added without
        // extending `requirement` must not be servable to anyone.
        assert_eq!(requirement("/media/newthing/x"), None);
        assert_eq!(requirement("/media/"), None);
        assert_eq!(requirement("/api/v1/cameras"), None);
    }

    #[test]
    fn artifact_key_folds_a_session_directory_to_one_row() {
        assert_eq!(
            artifact_key("/media/playback/pbs_x/seg_00007.m4s").as_deref(),
            Some("playback/pbs_x")
        );
        assert_eq!(
            artifact_key("/media/playback/pbs_x/index.m3u8").as_deref(),
            Some("playback/pbs_x")
        );
        assert_eq!(
            artifact_key("/media/clips/clip_x.mp4").as_deref(),
            Some("clips/clip_x.mp4")
        );
        assert_eq!(
            artifact_key("/media/snapshots/zoneevt_x.jpg").as_deref(),
            Some("snapshots/zoneevt_x.jpg")
        );
        assert_eq!(
            artifact_key("/media/archives/bkp_x.zip").as_deref(),
            Some("archives/bkp_x.zip")
        );
        assert_eq!(artifact_key("/media/recordings/cam_a/s.mp4"), None);
    }

    #[test]
    fn partition_camera_reads_the_first_segment_only() {
        assert_eq!(
            partition_camera("/media/recordings/cam_a/2026/seg.mp4"),
            Some("cam_a")
        );
        assert_eq!(
            partition_camera("/media/snapshots/cam_b/1.jpg"),
            Some("cam_b")
        );
        assert_eq!(partition_camera("/media/recordings"), None);
    }

    #[test]
    fn traversal_and_encoded_separators_are_dishonest() {
        assert!(dishonest_path("/media/clips/../recordings/cam_b/x.mp4"));
        assert!(dishonest_path("/media/clips/%2e%2e/x.mp4"));
        assert!(dishonest_path("/media/clips/a%2Fb.mp4"));
        assert!(!dishonest_path("/media/clips/clip_x.mp4"));
    }

    async fn test_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn attribute_then_owners_round_trips_and_is_idempotent() {
        let pool = test_pool().await;
        assert_eq!(
            owners(&pool, "clips/clip_x.mp4").await,
            Owners::Unattributed
        );
        attribute(&pool, "clips/clip_x.mp4", &["cam_a".to_string()], KIND_CLIP).await;
        attribute(&pool, "clips/clip_x.mp4", &["cam_a".to_string()], KIND_CLIP).await;
        assert_eq!(
            owners(&pool, "clips/clip_x.mp4").await,
            Owners::Cameras(vec!["cam_a".to_string()])
        );
        forget(&pool, "clips/clip_x.mp4").await;
        assert_eq!(
            owners(&pool, "clips/clip_x.mp4").await,
            Owners::Unattributed
        );
    }

    #[tokio::test]
    async fn an_archive_spanning_cameras_keeps_one_row_per_camera() {
        let pool = test_pool().await;
        attribute(
            &pool,
            "archives/bkp_x.zip",
            &["cam_a".to_string(), "cam_b".to_string()],
            KIND_ARCHIVE,
        )
        .await;
        match owners(&pool, "archives/bkp_x.zip").await {
            Owners::Cameras(mut v) => {
                v.sort();
                assert_eq!(v, vec!["cam_a".to_string(), "cam_b".to_string()]);
            }
            Owners::Unattributed => panic!("expected attribution"),
        }
    }
}
