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
        // Backup archives are an export of footage, so they gate on `VideoExport` exactly as `clips`
        // does — NOT on `Cap::Admin`.
        //
        // Admin here made the attribution below unreachable and the whole subtree a permanent false
        // deny: a camera scope can no longer be combined with an admin grant (`validate_grant`
        // refuses it, since Admin implies the unscopable caps), so NO credential the API can mint is
        // both camera-scoped and Admin-capable. Every scoped caller failed this line before the
        // owners check ever ran — including on the archive it had just been authorised to create.
        //
        // This is not a widening: the `Artifact` arm below requires the caller to hold EVERY camera
        // the archive is attributed to, so a fleet-wide archive still needs a fleet-wide credential.
        // An archive of one camera is readable by exactly the credential that could have exported it.
        "archives" => match rest.len() {
            1 => Some((Cap::VideoExport, MediaKind::Artifact)),
            _ => Some((Cap::VideoExport, MediaKind::Denied)),
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
/// # This runs on EVERY request, and that is the expiry story for the whole recorded-media plane
///
/// `/media/*` sits OUTSIDE the `/api/v1` auth floor, so `Principal::from_request_parts` finds no
/// pre-resolved principal in the request extensions and goes to the database — every playlist poll,
/// every `seg_*.m4s`, every byte-range of a clip. Two consequences worth stating so they are not
/// re-litigated:
///
/// - A playback session, clip export or archive CANNOT outlive the scope that created it. Its URL is
///   not a bearer capability: re-scope the credential and the next fetch is 403; revoke it and the
///   next fetch is 401. That is why none of those producers needs an expiry, a session token, or a
///   revocation list of its own. Pinned by
///   `a_media_artifact_stops_being_readable_the_moment_its_credential_changes`.
/// - The cost is one credential lookup per media request. It is an indexed seek on `key_hash`/session
///   id and only the scoped path adds the `media_artifacts` read; do not "optimize" it into a cache
///   without replacing the property above with something equivalent.
///
/// The LIVE plane is deliberately different and weaker — MediaMTX streams direct from a signed URL
/// with no credential to re-check. See `services::live_token`.
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

/// Forget attribution rows whose artifact is no longer on disk.
///
/// Migration 0013 promised this sweep and nothing implemented it, so `media_artifacts` grew one
/// permanent row per clip, evidence frame and archive for the life of the box: retention deletes
/// those files by mtime (or with their owning event row) and has no way to forget them by name.
///
/// It sweeps by EXISTENCE rather than by the `(kind, created_at)` the migration sketched, because
/// the kinds do not share a horizon — clips die at `CLIP_RETENTION`, scheduled snapshots at
/// `snapshot_retention_hours`, evidence frames whenever their zone/entry event is pruned, archives at
/// `archive_retention_hours`. Any single age would sweep some kind EARLY, and an early sweep is not a
/// harmless leak: `owners` would report `Unattributed`, which the guard fails closed on, so a
/// camera-scoped credential would get a 403 on its own live evidence. Keying on "the file is gone"
/// makes that unrepresentable — once the bytes are gone every credential gets a 404 anyway, so the
/// attribution cannot matter.
///
/// `min_age` only keeps the sweep clear of artifacts still being written (a clip export or playback
/// build attributes before the file lands). Rows younger than it are never examined.
///
/// Best-effort and non-fatal, like the rest of retention: a failure here must not stop the sweep that
/// also frees disk.
pub async fn sweep_orphans(
    pool: &SqlitePool,
    cfg: &crate::config::Config,
    min_age: chrono::Duration,
) -> u64 {
    let cutoff = Utc::now() - min_age;
    let rows = match sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT path FROM media_artifacts WHERE created_at < ?",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "media_scope: orphan sweep query failed");
            return 0;
        }
    };
    let mut removed = 0u64;
    for (key,) in rows {
        let Some(path) = artifact_path(cfg, &key) else {
            // A key no subtree claims can never be resolved by the guard either, so it is dead by
            // construction. Dropping it is what keeps a renamed subtree from leaking rows forever.
            continue;
        };
        if tokio::fs::metadata(&path).await.is_ok() {
            continue;
        }
        match sqlx::query("DELETE FROM media_artifacts WHERE path = ?")
            .bind(&key)
            .execute(pool)
            .await
        {
            Ok(r) => removed += r.rows_affected(),
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "media_scope: orphan sweep delete failed")
            }
        }
    }
    removed
}

/// Resolve an attribution key back to the file (or session directory) it names.
///
/// The exact inverse of [`artifact_key`] — they must be changed together, which is why they sit next
/// to each other and share a test. A key shape not listed here is unresolvable and is treated as
/// dead by the sweep.
fn artifact_path(cfg: &crate::config::Config, key: &str) -> Option<std::path::PathBuf> {
    let (subtree, rest) = key.split_once('/')?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    match subtree {
        "clips" => Some(cfg.clips_dir.join(rest)),
        "snapshots" => Some(cfg.snapshots_dir.join(rest)),
        "archives" => Some(cfg.archive_dir.join(rest)),
        "playback" => Some(cfg.playback_dir.join(rest)),
        _ => None,
    }
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
        // Archives gate on VideoExport, NOT Admin. Admin made the subtree unreachable for every
        // camera-scoped credential — a scope can no longer be combined with an admin grant, so the
        // cap check refused before the attribution below could ever allow. Pin it: reverting this to
        // Admin silently restores a permanent false deny that no attribution fix can reach past.
        assert_eq!(
            requirement("/media/archives/bkp_x.zip"),
            Some((Cap::VideoExport, MediaKind::Artifact))
        );
        assert_eq!(
            requirement("/media/archives/nested/x.zip"),
            Some((Cap::VideoExport, MediaKind::Denied))
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

    /// A Config whose media subtrees point at a fresh scratch dir, so the sweep's existence checks
    /// see only what the test puts there.
    fn sweep_cfg(root: &std::path::Path) -> crate::config::Config {
        let mut cfg = crate::config::Config::from_env();
        cfg.clips_dir = root.join("clips");
        cfg.snapshots_dir = root.join("snapshots");
        cfg.archive_dir = root.join("archives");
        cfg.playback_dir = root.join("playback");
        for d in [
            &cfg.clips_dir,
            &cfg.snapshots_dir,
            &cfg.archive_dir,
            &cfg.playback_dir,
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        cfg
    }

    async fn backdate(pool: &SqlitePool, key: &str, hours: i64) {
        sqlx::query("UPDATE media_artifacts SET created_at = ? WHERE path = ?")
            .bind(Utc::now() - chrono::Duration::hours(hours))
            .bind(key)
            .execute(pool)
            .await
            .unwrap();
    }

    /// `artifact_path` must invert `artifact_key` for every subtree — the sweep decides a row is dead
    /// from this mapping, so a wrong arm here deletes attribution for files that still exist.
    #[test]
    fn artifact_path_inverts_artifact_key() {
        let root = std::env::temp_dir().join("heldar_inv_test");
        let cfg = sweep_cfg(&root);
        for (url, expect) in [
            ("/media/clips/clip_x.mp4", cfg.clips_dir.join("clip_x.mp4")),
            (
                "/media/snapshots/zoneevt_x.jpg",
                cfg.snapshots_dir.join("zoneevt_x.jpg"),
            ),
            (
                "/media/archives/bkp_x.zip",
                cfg.archive_dir.join("bkp_x.zip"),
            ),
            (
                "/media/playback/pbs_x/index.m3u8",
                cfg.playback_dir.join("pbs_x"),
            ),
        ] {
            let key = artifact_key(url).expect("guard derives a key");
            assert_eq!(
                artifact_path(&cfg, &key),
                Some(expect),
                "artifact_path must invert artifact_key for {url}"
            );
        }
        // A key naming no subtree is unresolvable rather than silently mapped somewhere.
        assert_eq!(artifact_path(&cfg, "recordings/cam_a"), None);
        assert_eq!(artifact_path(&cfg, "clips"), None);
        // ..and a key that would climb out of its subtree resolves nowhere.
        assert_eq!(artifact_path(&cfg, "clips/../../etc/passwd"), None);
    }

    /// Regression: zone/entry evidence used to attribute under the BARE filename while the guard
    /// looks the row up under `snapshots/<file>`. The row existed and was never found, so a
    /// camera-scoped credential got a 403 on its own evidence — the exact false deny attribution is
    /// there to prevent. Assert the producer's key is the one the guard derives from its URL.
    #[tokio::test]
    async fn evidence_is_attributed_under_the_key_the_guard_derives() {
        let pool = test_pool().await;
        for (filename, kind) in [
            ("zoneevt_abc.jpg", KIND_ZONE_EVIDENCE),
            ("entryevt_abc.jpg", KIND_ENTRY_EVIDENCE),
        ] {
            // Exactly what zones.rs / anpr.rs write, and the URL they hand back.
            attribute(
                &pool,
                &format!("snapshots/{filename}"),
                &["cam_a".to_string()],
                kind,
            )
            .await;
            let url = format!("/media/snapshots/{filename}");
            let key = artifact_key(&url).expect("guard derives a key from the served URL");
            assert_eq!(
                owners(&pool, &key).await,
                Owners::Cameras(vec!["cam_a".to_string()]),
                "the guard must resolve the row the producer wrote for {url}"
            );
        }
    }

    /// The sweep forgets a row only once its file is gone — never on age alone.
    #[tokio::test]
    async fn sweep_forgets_only_rows_whose_file_has_left_the_disk() {
        let pool = test_pool().await;
        let root = std::env::temp_dir().join(format!("heldar_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = sweep_cfg(&root);
        std::fs::write(cfg.clips_dir.join("live.mp4"), b"x").unwrap();
        std::fs::create_dir_all(cfg.playback_dir.join("pbs_live")).unwrap();

        for key in [
            "clips/live.mp4",
            "clips/gone.mp4",
            "playback/pbs_live",
            "playback/pbs_gone",
        ] {
            attribute(&pool, key, &["cam_a".to_string()], KIND_CLIP).await;
            backdate(&pool, key, 48).await;
        }
        // A row whose file is ALSO gone but which is younger than min_age: still in flight.
        attribute(
            &pool,
            "clips/inflight.mp4",
            &["cam_a".to_string()],
            KIND_CLIP,
        )
        .await;

        let removed = sweep_orphans(&pool, &cfg, chrono::Duration::hours(1)).await;
        assert_eq!(
            removed, 2,
            "only the two rows with no file may be forgotten"
        );

        // Live artifacts keep their attribution — losing it is a 403 on footage that still serves.
        for key in ["clips/live.mp4", "playback/pbs_live", "clips/inflight.mp4"] {
            assert_eq!(
                owners(&pool, key).await,
                Owners::Cameras(vec!["cam_a".to_string()]),
                "{key} still exists (or is too young to judge) and must stay attributed"
            );
        }
        for key in ["clips/gone.mp4", "playback/pbs_gone"] {
            assert_eq!(owners(&pool, key).await, Owners::Unattributed, "{key}");
        }

        // Once the live clip is deleted too, the next sweep collects it — the table is bounded.
        std::fs::remove_file(cfg.clips_dir.join("live.mp4")).unwrap();
        assert_eq!(
            sweep_orphans(&pool, &cfg, chrono::Duration::hours(1)).await,
            1
        );
        let _ = std::fs::remove_dir_all(&root);
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
