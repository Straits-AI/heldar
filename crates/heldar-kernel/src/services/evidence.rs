//! Signed, independently verifiable evidence bundles (#118).
//!
//! Heldar could already lock segments against retention and export a clip. That protects evidence
//! from deletion, which is not the same as making it verifiable once it has left the appliance. A
//! plain MP4 cannot say which camera produced it, which interval was asked for, whether that
//! interval had recording gaps, who exported it, or whether a byte changed afterwards.
//!
//! A bundle is a ZIP with a canonical manifest, a hash for every included byte, and an Ed25519
//! signature over the manifest:
//!
//! ```text
//! manifest.json           the claim: what this is, where it came from, what is missing from it
//! media/clip.mp4          the footage
//! metadata/events.jsonl   operational events in the window (gaps, reconnects, offline)
//! metadata/detections.jsonl
//! metadata/audit.jsonl    the audit trail for this camera in the window, including this export
//! metadata/coverage.json  requested vs actually-recorded seconds, and every gap
//! metadata/camera.json    the camera as configured at export time
//! hashes.sha256           the manifest's hashes, in `sha256sum -c` format
//! signature.json          Ed25519 over the canonical manifest bytes
//! ```
//!
//! WHAT THE MANIFEST REFUSES TO DO.
//!
//! It does not present a gap-free story. `covered_seconds` and the gap list are in the signed
//! manifest, so a bundle spanning an outage says so in the same document that attests to it — an
//! export that quietly concatenated across a gap would produce continuous-looking video of a
//! discontinuous night, which is worse than no export. Low-confidence detections are included with
//! their confidence rather than filtered to make the package look decisive.
//!
//! It does not claim a trusted timestamp. The appliance stamps its own clock; an appliance whose
//! clock is wrong signs the wrong time faithfully. The manifest states this in `attestation.limits`
//! rather than leaving a reader to infer it.
//!
//! It does not claim any detection is correct. An included detection is a record of what the model
//! said, at what confidence, at what time.
//!
//! CANONICAL JSON. The manifest is serialized through `serde_json::Value`, whose maps are
//! `BTreeMap`s — keys are emitted in sorted order, so the same manifest produces the same bytes and
//! the signature is over something a verifier can reconstruct. [`canonical_bytes_are_sorted`] pins
//! that, because it is a property of a dependency's feature flags, not of this file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use uuid::Uuid;

use crate::auth::Principal;
use crate::error::{AppError, AppResult};
use crate::models::Segment;
use crate::services::clip;
use crate::state::AppState;

/// Bumped only for a change a verifier must reject rather than tolerate.
pub const FORMAT: &str = "heldar-evidence/1";
/// The `zip` binary, as `backup.rs` already assumes.
const ZIP_BIN: &str = "/usr/bin/zip";
/// A bundle is bounded by the same ceiling as a clip; the media is the same remux.
const BUNDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// What an export WOULD contain. Returned by the dry run so an operator confirms a real plan rather
/// than discovering the gaps and the size after the export ran.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BundlePlan {
    pub camera_id: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub requested_seconds: f64,
    pub covered_seconds: f64,
    pub gaps: Vec<clip::ClipGap>,
    pub segments: Vec<SegmentRef>,
    /// Sum of the source segments — an upper bound on the media, since the clip is trimmed.
    pub source_bytes: u64,
    pub detection_count: i64,
    pub event_count: i64,
    /// Segments in the window held against retention. An export does not change these; it is here
    /// so the operator can see the window is under hold.
    pub evidence_locked_segments: i64,
    /// The single-bundle byte ceiling this export is measured against.
    pub limit_bytes: u64,
    /// Bytes already in the evidence directory, and its cumulative ceiling. Nothing sweeps bundles,
    /// so this is the number that creeps up until an export starts failing.
    pub dir_used_bytes: u64,
    pub dir_limit_bytes: u64,
    /// True when a real export would be REFUSED. A dry run exists to answer "will this work" before
    /// committing, so it has to say no here rather than succeeding and letting the real call fail.
    pub would_exceed_limits: bool,
}

/// A source segment as recorded, with the hash of the file the clip was built from.
#[derive(Debug, Serialize, Clone, utoipa::ToSchema)]
pub struct SegmentRef {
    pub id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_s: f64,
    pub codec: Option<String>,
    pub size_bytes: i64,
    /// `None` when the file could not be read at export time. Recorded as null rather than omitted:
    /// a segment the appliance listed but could not hash is a fact about the evidence.
    pub sha256: Option<String>,
}

/// A written bundle.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BundleResult {
    pub id: String,
    pub camera_id: String,
    pub filename: String,
    pub url: String,
    pub size_bytes: u64,
    /// sha256 of the bundle file itself.
    pub sha256: String,
    /// sha256 of the canonical manifest — what the signature covers.
    pub manifest_sha256: String,
    pub key_id: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub covered_seconds: f64,
    pub gaps: Vec<clip::ClipGap>,
}

/// Everything the export needs to describe itself, gathered once and shared by the plan and the
/// build so the dry run cannot describe a different export than the one that runs.
struct Gathered {
    segments: Vec<Segment>,
    refs: Vec<SegmentRef>,
    covered_seconds: f64,
    gaps: Vec<clip::ClipGap>,
    requested_seconds: f64,
}

async fn gather(
    st: &AppState,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    hash_sources: bool,
) -> AppResult<Gathered> {
    if to <= from {
        return Err(AppError::BadRequest("`to` must be after `from`".into()));
    }
    let segments: Vec<Segment> = sqlx::query_as::<_, Segment>(
        "SELECT * FROM segments
         WHERE camera_id = ? AND start_time < ? AND end_time > ?
         ORDER BY start_time ASC",
    )
    .bind(camera_id)
    .bind(to)
    .bind(from)
    .fetch_all(&st.pool)
    .await?;

    let (covered_seconds, gaps) = clip::coverage_and_gaps(&segments, from, to);
    let mut refs = Vec::with_capacity(segments.len());
    for s in &segments {
        refs.push(SegmentRef {
            id: s.id.clone(),
            start_time: s.start_time,
            end_time: s.end_time,
            duration_s: s.duration_s,
            codec: s.codec.clone(),
            size_bytes: s.size_bytes,
            sha256: if hash_sources {
                hash_file(Path::new(&s.path)).await
            } else {
                None
            },
        });
    }
    Ok(Gathered {
        segments,
        refs,
        covered_seconds,
        gaps,
        requested_seconds: (to - from).num_milliseconds() as f64 / 1000.0,
    })
}

/// What an export of this window would contain, without producing it.
pub async fn plan(
    st: &AppState,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<BundlePlan> {
    // The plan does NOT hash source segments: it is meant to be cheap enough to call from a
    // confirmation dialog, and hashing gigabytes to answer "how big is this" defeats that.
    let g = gather(st, camera_id, from, to, false).await?;
    let (detection_count, event_count, locked) = counts(st, camera_id, from, to).await?;
    let source_bytes = source_bytes_of(&g.refs);
    // Reported, not enforced, on the plan path: a dry run that refuses cannot show WHY, and the
    // gaps and segment list are exactly what an operator needs in order to pick a smaller window.
    let dir_used_bytes = crate::services::backup::dir_size_bytes(&st.cfg.evidence_dir).await;
    Ok(BundlePlan {
        camera_id: camera_id.to_string(),
        from,
        to,
        requested_seconds: g.requested_seconds,
        covered_seconds: g.covered_seconds,
        gaps: g.gaps,
        source_bytes,
        segments: g.refs,
        detection_count,
        event_count,
        evidence_locked_segments: locked,
        limit_bytes: st.cfg.evidence_max_bytes,
        dir_used_bytes,
        dir_limit_bytes: st.cfg.evidence_dir_max_bytes,
        would_exceed_limits: source_bytes > st.cfg.evidence_max_bytes
            || dir_used_bytes.saturating_add(source_bytes) > st.cfg.evidence_dir_max_bytes,
    })
}

async fn counts(
    st: &AppState,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<(i64, i64, i64)> {
    let d: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM detections WHERE camera_id = ? AND timestamp >= ? AND timestamp <= ?",
    )
    .bind(camera_id)
    .bind(from)
    .bind(to)
    .fetch_one(&st.pool)
    .await?;
    let e: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM events WHERE camera_id = ? AND timestamp >= ? AND timestamp <= ?",
    )
    .bind(camera_id)
    .bind(from)
    .bind(to)
    .fetch_one(&st.pool)
    .await?;
    let l: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM segments
         WHERE camera_id = ? AND start_time < ? AND end_time > ? AND evidence_locked = 1",
    )
    .bind(camera_id)
    .bind(to)
    .bind(from)
    .fetch_one(&st.pool)
    .await?;
    Ok((d.0, e.0, l.0))
}

/// Upper bound on a bundle's media: the sum of its source segments. The clip is trimmed to the
/// requested window, so the finished file is at most this and usually less. Bounding on the upper
/// figure is the point — a limit that only bites after the work is done is not a limit.
fn source_bytes_of(refs: &[SegmentRef]) -> u64 {
    refs.iter().map(|r| r.size_bytes.max(0) as u64).sum()
}

/// One GiB of margin above the estimate, matching the archive path. The estimate is an upper bound
/// on the MEDIA; the container, metadata and the staging copy all cost more.
const DISK_HEADROOM_BYTES: u64 = 1024 * 1024 * 1024;

/// Refuse an export that would not fit, before anything is written.
///
/// Three separate questions, because they fail for different reasons and an operator needs to know
/// which: is this one bundle too big, is there room on the disk right now, and has the directory
/// filled up with previous bundles? The last matters because nothing sweeps them — they are records
/// of what left the appliance, and deleting one is an operator decision, so without a ceiling the
/// directory grows forever on the recording disk.
async fn check_size_limits(st: &AppState, source_bytes: u64) -> AppResult<()> {
    if source_bytes > st.cfg.evidence_max_bytes {
        return Err(AppError::BadRequest(format!(
            "this window covers {source_bytes} bytes of source footage, over the \
             {} byte single-bundle limit (HELDAR_EVIDENCE_MAX_BYTES) — export a shorter window, \
             or raise the limit knowingly",
            st.cfg.evidence_max_bytes
        )));
    }

    tokio::fs::create_dir_all(&st.cfg.evidence_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    if let Some(stats) =
        crate::services::storage::disk_stats_async(st.cfg.evidence_dir.clone()).await
    {
        let needed = source_bytes.saturating_add(DISK_HEADROOM_BYTES);
        if stats.free_bytes < needed {
            return Err(AppError::BadRequest(format!(
                "not enough free disk for this bundle: need ~{needed} bytes, {} free. Refusing \
                 rather than filling the disk the recorder writes to",
                stats.free_bytes
            )));
        }
    }

    let used = crate::services::backup::dir_size_bytes(&st.cfg.evidence_dir).await;
    if used.saturating_add(source_bytes) > st.cfg.evidence_dir_max_bytes {
        return Err(AppError::BadRequest(format!(
            "the evidence directory holds {used} bytes; this bundle would exceed the {} byte cap \
             (HELDAR_EVIDENCE_DIR_MAX_BYTES). Nothing sweeps bundles automatically — they are \
             records of what left the appliance — so remove ones you no longer need",
            st.cfg.evidence_dir_max_bytes
        )));
    }
    Ok(())
}

/// Build and sign a bundle. `audit_id` and `request_id` are recorded in the manifest so the bundle
/// points back at the appliance's own trail.
#[allow(clippy::too_many_arguments)]
pub async fn export(
    st: &AppState,
    principal: &Principal,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    incident_id: Option<&str>,
    audit_id: Option<&str>,
    request_id: Option<&str>,
) -> AppResult<BundleResult> {
    let camera: Option<(String, Option<String>, String)> =
        sqlx::query_as("SELECT id, site_id, name FROM cameras WHERE id = ?")
            .bind(camera_id)
            .fetch_optional(&st.pool)
            .await?;
    let (_, site_id, camera_name) =
        camera.ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))?;

    let g = gather(st, camera_id, from, to, true).await?;
    if g.segments.is_empty() {
        return Err(AppError::NotFound(
            "no recorded footage in the requested range".into(),
        ));
    }

    // Refuse before doing any work, not after filling the disk. Backups have had these three
    // bounds since they were written; evidence export had NONE, and its output lands on the same
    // filesystem as the recordings — so one request for a month-long window could drive the
    // retention sweeper into evicting footage to make room for a copy of that same footage.
    check_size_limits(st, source_bytes_of(&g.refs)).await?;

    // One media-job permit for the whole bundle, taken before any file is written, exactly as clip
    // export does — a bundle is a heavier clip, not a lighter one. Note this bounds CONCURRENCY
    // only: two permitted bundles can still be any size, which is why the byte checks above exist.
    let _permit = st.media_jobs.acquire("evidence_bundle").await?;
    // Hold the source segments for the duration: a retention sweep mid-build would produce a bundle
    // whose manifest hashes segments that no longer exist, which reads as tampering later.
    let _read_lock = crate::repo::SegReadLock::acquire(
        &st.pool,
        g.segments.iter().map(|s| s.id.clone()).collect(),
    )
    .await;

    let id = format!("ev_{}", Uuid::new_v4().simple());
    let stage = st.cfg.evidence_dir.join(format!(".stage-{id}"));
    let filename = format!("{id}.heldar-evidence");
    let out_path = st.cfg.evidence_dir.join(&filename);

    let built = build_bundle(
        st,
        principal,
        &id,
        camera_id,
        &camera_name,
        site_id.as_deref(),
        incident_id,
        audit_id,
        request_id,
        from,
        to,
        &g,
        &stage,
        &out_path,
    )
    .await;

    // The staging tree is scratch on EVERY outcome. Leaving it behind on failure would put unsigned,
    // unhashed media under the data directory with no row describing it.
    let _ = tokio::fs::remove_dir_all(&stage).await;
    let built = match built {
        Ok(b) => b,
        Err(e) => {
            let _ = tokio::fs::remove_file(&out_path).await;
            return Err(e);
        }
    };

    // Attribute BEFORE announcing the URL: an artifact served before its owning camera is recorded
    // is a window in which a camera-scoped credential reads a bundle it does not own.
    crate::services::media_scope::attribute(
        &st.pool,
        &format!("evidence/{filename}"),
        std::slice::from_ref(&camera_id.to_string()),
        crate::services::media_scope::KIND_EVIDENCE_BUNDLE,
    )
    .await;

    let indexed = sqlx::query(
        "INSERT INTO evidence_bundles
           (id, camera_id, site_id, incident_id, filename, from_time, to_time, size_bytes,
            sha256, manifest_sha256, key_id, exported_by, audit_id, request_id, created_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(camera_id)
    .bind(&site_id)
    .bind(incident_id)
    .bind(&filename)
    .bind(from)
    .bind(to)
    .bind(built.size_bytes as i64)
    .bind(&built.sha256)
    .bind(&built.manifest_sha256)
    .bind(&built.key_id)
    .bind(&principal.id)
    .bind(audit_id)
    .bind(request_id)
    .bind(Utc::now())
    .execute(&st.pool)
    .await;

    // A BUNDLE THE INDEX DOES NOT KNOW ABOUT MUST NOT SURVIVE.
    //
    // Everything above this point has already happened: the file is written, signed, and attributed
    // to its camera, so it is downloadable at `/media/evidence/<file>`. If the index write then
    // fails (a locked or full database) and we simply returned the error, the operator would be told
    // the export failed while a genuine, appliance-signed evidence document stayed live on the box
    // and absent from `GET /api/v1/evidence/exports` — which is precisely the "so an operator can
    // list what left the appliance" claim migration 0018 makes.
    //
    // An unlisted signed bundle is worse than no bundle: it is real, it verifies, and nothing on the
    // appliance records that it exists or who caused it.
    if let Err(e) = indexed {
        let _ = tokio::fs::remove_file(&out_path).await;
        crate::services::media_scope::forget(&st.pool, &format!("evidence/{filename}")).await;
        tracing::error!(
            target: "heldar::security",
            error = %e,
            bundle = %id,
            "evidence: could not index a bundle; removed it rather than leave a signed artifact \
             the appliance cannot account for"
        );
        return Err(e.into());
    }

    Ok(BundleResult {
        id,
        camera_id: camera_id.to_string(),
        url: format!("/media/evidence/{filename}"),
        filename,
        size_bytes: built.size_bytes,
        sha256: built.sha256,
        manifest_sha256: built.manifest_sha256,
        key_id: built.key_id,
        from,
        to,
        covered_seconds: g.covered_seconds,
        gaps: g.gaps,
    })
}

struct Built {
    size_bytes: u64,
    sha256: String,
    manifest_sha256: String,
    key_id: String,
}

#[allow(clippy::too_many_arguments)]
async fn build_bundle(
    st: &AppState,
    principal: &Principal,
    id: &str,
    camera_id: &str,
    camera_name: &str,
    site_id: Option<&str>,
    incident_id: Option<&str>,
    audit_id: Option<&str>,
    request_id: Option<&str>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    g: &Gathered,
    stage: &Path,
    out_path: &Path,
) -> AppResult<Built> {
    let io = |e: std::io::Error| AppError::Other(e.into());
    tokio::fs::create_dir_all(stage.join("media"))
        .await
        .map_err(io)?;
    tokio::fs::create_dir_all(stage.join("metadata"))
        .await
        .map_err(io)?;

    // --- media -------------------------------------------------------------------------------
    remux(st, g, from, &stage.join("media/clip.mp4")).await?;

    // --- metadata ----------------------------------------------------------------------------
    write_jsonl(
        &stage.join("metadata/detections.jsonl"),
        rows(
            &st.pool,
            "SELECT id, camera_id, task_type, timestamp, label, confidence, bbox, track_id, \
             attributes FROM detections WHERE camera_id = ? AND timestamp >= ? AND timestamp <= ? \
             ORDER BY timestamp ASC",
            camera_id,
            from,
            to,
        )
        .await?,
    )
    .await?;
    write_jsonl(
        &stage.join("metadata/events.jsonl"),
        rows(
            &st.pool,
            "SELECT id, camera_id, site_id, event_type, severity, timestamp, payload FROM events \
             WHERE camera_id = ? AND timestamp >= ? AND timestamp <= ? ORDER BY timestamp ASC",
            camera_id,
            from,
            to,
        )
        .await?,
    )
    .await?;
    // The audit trail is scoped to THIS camera. A fleet-wide trail in a single-camera bundle would
    // disclose activity on cameras the exporting credential may not even hold.
    //
    // Keyed on `subject_camera_id`, not `target_id`. `target_id` is whatever object the action names
    // — a clip, a schedule, an AI task — so filtering on it returns only the rows where the camera
    // happened to BE the target and silently drops every action taken ON that camera through
    // something else. `subject_camera_id` is the column migration 0014 added to answer exactly the
    // question "which camera does this row concern", and it is what camera-scoped audit reads use.
    //
    // AND this export's own row is included explicitly, by id. It is stamped `now` while the window
    // is almost always in the past, so a range filter alone excludes it — which would have made the
    // header comment above this file ("including this export") a claim the query did not deliver.
    write_jsonl(
        &stage.join("metadata/audit.jsonl"),
        audit_rows(&st.pool, camera_id, from, to, audit_id).await?,
    )
    .await?;

    let coverage = json!({
        "requested_from": from, "requested_to": to,
        "requested_seconds": g.requested_seconds,
        "covered_seconds": g.covered_seconds,
        "gaps": g.gaps.iter().map(|x| json!({"from": x.from, "to": x.to})).collect::<Vec<_>>(),
        "note": "covered_seconds is the part of the requested window for which footage exists. The \
                 clip concatenates across gaps because the missing footage does not exist; the gaps \
                 are listed here so the video is not read as continuous.",
    });
    write_json(&stage.join("metadata/coverage.json"), &coverage).await?;

    let camera_cfg: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(config, '{}') FROM cameras WHERE id = ?")
            .bind(camera_id)
            .fetch_optional(&st.pool)
            .await
            .unwrap_or(None);
    write_json(
        &stage.join("metadata/camera.json"),
        &json!({
            "id": camera_id,
            "name": camera_name,
            "site_id": site_id,
            "config_at_export": camera_cfg
                .and_then(|c| serde_json::from_str::<Value>(&c.0).ok())
                .unwrap_or(Value::Null),
        }),
    )
    .await?;

    // --- manifest ----------------------------------------------------------------------------
    // Hash every file that is IN the bundle, after they are all written. The manifest and the
    // signature cannot cover themselves; `hashes.sha256` is derived from the manifest below and the
    // verifier cross-checks it, so a reader using plain `sha256sum -c` cannot be shown a different
    // set of hashes than the signed ones.
    let payload = [
        "media/clip.mp4",
        "metadata/detections.jsonl",
        "metadata/events.jsonl",
        "metadata/audit.jsonl",
        "metadata/coverage.json",
        "metadata/camera.json",
    ];
    let mut files = BTreeMap::new();
    for rel in payload {
        let p = stage.join(rel);
        let md = tokio::fs::metadata(&p).await.map_err(io)?;
        let h = hash_file(&p).await.ok_or_else(|| {
            AppError::Other(anyhow::anyhow!("could not hash {rel} for the manifest"))
        })?;
        files.insert(rel.to_string(), json!({"sha256": h, "bytes": md.len()}));
    }

    let key = crate::services::evidence_key::EvidenceKey::load_or_create(&st.cfg.data_dir)
        .map_err(AppError::Other)?;

    let (tz, tz_src) = crate::services::tz::site_tz(&st.pool, Some(camera_id)).await;
    let tz_name = tz.map(|t| t.to_string());
    let tz_source = serde_json::to_value(tz_src).unwrap_or(Value::Null);
    let tz_note = match tz {
        Some(_) => {
            "Every timestamp in this bundle is UTC. This zone is the site's, recorded so a \
                    reader can render local wall-clock times without guessing which clock they are."
        }
        None => {
            "No timezone is configured for this site, so all times in this bundle are UTC and \
                 no local wall clock can be derived from it."
        }
    };

    // Asked once per export, cached per binary. Never fails the export: a bundle whose producer
    // could not be identified is still evidence, and saying so is the honest record.
    let ffmpeg_version = crate::util::ffmpeg_version(&st.cfg.ffmpeg_bin).await;

    // Highest applied migration. A read failure records null rather than a number we did not read —
    // this string is signed.
    let schema_version: Option<i64> =
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&st.pool)
            .await
            .unwrap_or(None);

    let manifest = json!({
        "format": FORMAT,
        "bundle_id": id,
        "created_at": Utc::now(),
        "hash_algorithm": "sha256",
        // THE MANIFEST IS SIGNED, SO THIS STATEMENT HAS TO STAY TRUE (#125). It previously attested
        // "not configured on this appliance", which was accurate then and would have become a
        // signed falsehood the moment a zone could be resolved — the box would have gone on
        // asserting it, under its own key, for every bundle.
        //
        // The zone is recorded for DISPLAY only. Every timestamp in this bundle is UTC and stays
        // UTC; naming the zone lets a reader render a local wall clock without guessing, and lets
        // them see that "02:14" in a report meant 02:14 at the site rather than 02:14 somewhere
        // else. Null means genuinely unconfigured, which is a fact about the evidence.
        "site": {
            "id": site_id,
            "timezone": tz_name,
            "timezone_source": tz_source,
            "timezone_note": tz_note,
        },
        "camera": {"id": camera_id, "name": camera_name},
        "incident_id": incident_id,
        "media": {
            "requested_from": from,
            "requested_to": to,
            "requested_seconds": g.requested_seconds,
            "covered_seconds": g.covered_seconds,
            "gaps": g.gaps.iter().map(|x| json!({"from": x.from, "to": x.to})).collect::<Vec<_>>(),
            "source_segments": g.refs.iter().map(|r| json!({
                "id": r.id, "start_time": r.start_time, "end_time": r.end_time,
                "duration_s": r.duration_s, "codec": r.codec, "size_bytes": r.size_bytes,
                "sha256": r.sha256,
            })).collect::<Vec<_>>(),
        },
        "export": {
            "principal_id": principal.id,
            "principal_name": principal.name,
            "principal_kind": format!("{:?}", principal.kind),
            "role": format!("{:?}", principal.role),
            "audit_id": audit_id,
            "request_id": request_id,
        },
        // WHAT PRODUCED THIS, not where it was configured from. `ffmpeg_bin` used to be recorded
        // here — a configured path that defaults to the bare string "ffmpeg", identifying no build
        // to anyone verifying the bundle later on another machine, while being signed as though it
        // did. #118 asks which ffmpeg VERSION produced the media; that is now what is recorded, and
        // null when the binary could not be asked. An unknown producer is a fact about the
        // evidence. A guessed one is a signed falsehood, which is what #125 was about.
        //
        // `schema_version` is the highest applied migration: two appliances on the same release can
        // hold different columns mid-upgrade, and a verifier reading `events.jsonl` needs to know
        // which shape it is looking at.
        "producer": {
            "heldar_version": env!("CARGO_PKG_VERSION"),
            "ffmpeg_version": ffmpeg_version,
            "ffmpeg_bin": st.cfg.ffmpeg_bin,
            "schema_version": schema_version,
        },
        "files": files,
        "attestation": {
            "limits": [
                "The signature establishes that this appliance produced this bundle and that its \
                 bytes have not changed. It does not establish WHEN: the appliance stamps its own \
                 clock, and is not a trusted timestamping authority.",
                "Detections are a record of what a model reported, at the stated confidence. \
                 Nothing here asserts a detection is correct.",
                "Gaps in `media.gaps` are intervals with no recorded footage. The clip is \
                 concatenated across them; it is not continuous video of that period.",
            ],
        },
    });

    let manifest_bytes = canonical(&manifest);
    let manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
    tokio::fs::write(stage.join("manifest.json"), &manifest_bytes)
        .await
        .map_err(io)?;

    // `sha256sum -c` format, derived from the manifest so the two can never disagree by accident.
    let mut sums = String::new();
    for (rel, e) in &files {
        sums.push_str(&format!(
            "{}  {}\n",
            e["sha256"].as_str().unwrap_or(""),
            rel
        ));
    }
    tokio::fs::write(stage.join("hashes.sha256"), sums)
        .await
        .map_err(io)?;

    write_json(
        &stage.join("signature.json"),
        &json!({
            "algorithm": "ed25519",
            "signed": "manifest.json",
            "manifest_sha256": manifest_sha256,
            "key_id": key.key_id,
            "public_key": key.public_key_b64,
            "signature": key.sign(&manifest_bytes),
            "note": "The signature is over the canonical bytes of manifest.json exactly as stored \
                     in this bundle. Verify the manifest first, then the files it lists.",
        }),
    )
    .await?;

    // --- pack --------------------------------------------------------------------------------
    zip_dir(stage, out_path).await?;
    let md = tokio::fs::metadata(out_path).await.map_err(io)?;
    let sha256 = hash_file(out_path)
        .await
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("could not hash the bundle")))?;

    Ok(Built {
        size_bytes: md.len(),
        sha256,
        manifest_sha256,
        key_id: key.key_id,
    })
}

/// Canonical manifest bytes: `serde_json::Value` maps are `BTreeMap`s, so keys serialize sorted.
fn canonical(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

/// The audit rows this bundle carries: everything concerning this camera inside the window, plus the
/// row for this export itself.
async fn audit_rows(
    pool: &sqlx::SqlitePool,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    audit_id: Option<&str>,
) -> AppResult<Vec<Value>> {
    let out = sqlx::query(
        "SELECT id, actor, actor_name, role, action, target_type, target_id, subject_camera_id, \
         detail, created_at AS timestamp FROM audit_log \
         WHERE (subject_camera_id = ? AND created_at >= ? AND created_at <= ?) \
            OR (? IS NOT NULL AND id = ?) \
         ORDER BY created_at ASC",
    )
    .bind(camera_id)
    .bind(from)
    .bind(to)
    .bind(audit_id)
    .bind(audit_id)
    .fetch_all(pool)
    .await?;
    Ok(to_json_rows(&out))
}

async fn rows(
    pool: &sqlx::SqlitePool,
    sql: &str,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<Vec<Value>> {
    let out = sqlx::query(sql)
        .bind(camera_id)
        .bind(from)
        .bind(to)
        .fetch_all(pool)
        .await?;
    Ok(to_json_rows(&out))
}

/// Rows as JSON objects, typed from SQLite's own column types.
fn to_json_rows(out: &[sqlx::sqlite::SqliteRow]) -> Vec<Value> {
    use sqlx::{Column, Row, TypeInfo, ValueRef};
    out.iter()
        .map(|r| {
            let mut m = serde_json::Map::new();
            for (i, c) in r.columns().iter().enumerate() {
                let raw = r.try_get_raw(i).ok();
                let v = match raw {
                    Some(v) if v.is_null() => Value::Null,
                    _ => match c.type_info().name() {
                        "INTEGER" | "BIGINT" => r.try_get::<i64, _>(i).map(Value::from),
                        "REAL" | "FLOAT" | "DOUBLE" => r.try_get::<f64, _>(i).map(Value::from),
                        _ => r.try_get::<String, _>(i).map(Value::from),
                    }
                    .unwrap_or(Value::Null),
                };
                m.insert(c.name().to_string(), v);
            }
            Value::Object(m)
        })
        .collect()
}

async fn write_jsonl(path: &Path, rows: Vec<Value>) -> AppResult<()> {
    let mut s = String::new();
    for r in rows {
        s.push_str(&serde_json::to_string(&r).unwrap_or_default());
        s.push('\n');
    }
    tokio::fs::write(path, s)
        .await
        .map_err(|e| AppError::Other(e.into()))
}

async fn write_json(path: &Path, v: &Value) -> AppResult<()> {
    tokio::fs::write(path, canonical(v))
        .await
        .map_err(|e| AppError::Other(e.into()))
}

/// sha256 of a file, or `None` if it could not be read.
async fn hash_file(path: &Path) -> Option<String> {
    let mut f = tokio::fs::File::open(path).await.ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        use tokio::io::AsyncReadExt;
        let n = f.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(format!("{:x}", h.finalize()))
}

/// Remux the overlapping segments into `out`, trimmed to the window. Same `-c copy` concat as clip
/// export — a bundle must contain the recorded frames, not a re-encode of them.
async fn remux(st: &AppState, g: &Gathered, from: DateTime<Utc>, out: &Path) -> AppResult<()> {
    let list_path = out.with_extension("concat.txt");
    let mut list = String::new();
    for s in &g.segments {
        let escaped = s.path.replace('\'', "'\\''");
        list.push_str(&format!("file '{escaped}'\n"));
    }
    tokio::fs::write(&list_path, list)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let ss = ((from - g.segments[0].start_time).num_milliseconds() as f64 / 1000.0).max(0.0);
    let mut cmd = Command::new(&st.cfg.ffmpeg_bin);
    cmd.kill_on_drop(true)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
        ])
        .arg("-i")
        .arg(&list_path)
        .args(["-ss", &format!("{ss:.3}")])
        .args(["-t", &format!("{:.3}", g.requested_seconds)])
        .args([
            "-c",
            "copy",
            "-avoid_negative_ts",
            "make_zero",
            "-movflags",
            "+faststart",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let result = tokio::time::timeout(BUNDLE_TIMEOUT, cmd.output()).await;
    let _ = tokio::fs::remove_file(&list_path).await;
    let out_res = match result {
        Err(_) => return Err(AppError::Other(anyhow::anyhow!("evidence remux timed out"))),
        Ok(Err(e)) => return Err(AppError::Other(e.into())),
        Ok(Ok(o)) => o,
    };
    if !out_res.status.success() {
        return Err(AppError::Other(anyhow::anyhow!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&out_res.stderr).trim()
        )));
    }
    Ok(())
}

async fn zip_dir(stage: &Path, out: &Path) -> AppResult<()> {
    let out_abs = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| AppError::Other(e.into()))?
            .join(out)
    };
    let res = tokio::time::timeout(
        BUNDLE_TIMEOUT,
        Command::new(ZIP_BIN)
            .kill_on_drop(true)
            .arg("-r")
            .arg("-q")
            // -X drops uid/gid/extra attributes, -D writes no directory entries. Together the
            // archive's entry list is EXACTLY the file list the manifest attests to, which is what
            // the verifier requires: anything else present is refused rather than ignored.
            .arg("-X")
            .arg("-D")
            .arg(&out_abs)
            .arg(".")
            .current_dir(stage)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;
    match res {
        Err(_) => Err(AppError::Other(anyhow::anyhow!("packing timed out"))),
        Ok(Err(e)) => Err(AppError::Other(anyhow::anyhow!(
            "{ZIP_BIN} could not run ({e}) — evidence bundles need it, as archive export does"
        ))),
        Ok(Ok(o)) if !o.status.success() => Err(AppError::Other(anyhow::anyhow!(
            "packing failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))),
        Ok(Ok(_)) => Ok(()),
    }
}

/// Path on disk for a bundle filename, used by the download/delete paths.
pub fn bundle_path(st: &AppState, filename: &str) -> PathBuf {
    st.cfg.evidence_dir.join(filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signature is over `canonical()`'s output, so a verifier that re-serializes the manifest
    /// must get the same bytes. That holds because `serde_json::Value` maps are `BTreeMap`s — a
    /// property of the dependency's feature flags, not of this file. If anything in the workspace
    /// ever turns on `serde_json/preserve_order`, insertion order would leak into the signed bytes
    /// and this catches it here rather than as an unverifiable bundle in someone's evidence locker.
    #[test]
    fn canonical_bytes_are_sorted() {
        let v = json!({"z": 1, "a": {"y": 2, "b": 3}, "m": 4});
        assert_eq!(
            String::from_utf8(canonical(&v)).unwrap(),
            r#"{"a":{"b":3,"y":2},"m":4,"z":1}"#,
            "manifest keys must serialize in sorted order, at every level"
        );
    }
}
