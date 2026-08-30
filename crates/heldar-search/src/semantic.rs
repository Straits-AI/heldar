//! `POST /api/v1/search/semantic` — CLIP similarity retrieval over the kernel's stored crop
//! embeddings (issue #38). The query (text or image) is embedded by a pull-only AI worker via the
//! kernel's `embed_queries` job queue, then ranked brute-force by cosine in the kernel
//! (`heldar_kernel::services::embeddings::search_similar`).
//!
//! Unlike the structured/NL routes, results here are SIMILARITY-RANKED, not facts: the score is a
//! closeness estimate from a learned embedding space, and the proof ladder marks the whole ranking
//! as a fallible inference. Searches are logged to `search_log` (mode `semantic`) and
//! identity-bearing text queries are audited exactly like plate searches.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::Json;
use base64::Engine as _;
use chrono::{TimeDelta, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use heldar_kernel::auth::{Cap, Principal};
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::services::embeddings::{self, SimilarFilters, SimilarHit};
use heldar_kernel::state::AppState;

use crate::config::SearchConfig;
use crate::query::QueryPlan;

/// Body cap for the semantic route (enforced pre-deserialization in routes.rs): fits
/// [`MAX_IMAGE_B64_CHARS`] of query image plus filters.
pub const SEMANTIC_BODY_LIMIT_BYTES: usize = 12 * 1024 * 1024;
/// Max base64 length of a query image (~7.5 MB decoded).
pub const MAX_IMAGE_B64_CHARS: usize = 10_000_000;
/// Hard bound on requested result count.
const MAX_K: usize = 100;
const DEFAULT_K: usize = 24;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SemanticBody {
    pub text: Option<String>,
    /// Base64 image (JPEG/PNG). A `data:` URL prefix is tolerated and stripped.
    pub image_b64: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub cameras: Vec<String>,
    /// Exact detection label filter (e.g. "car").
    pub label: Option<String>,
    /// Zone scope (issue #77): a zone id — only crops whose bbox ground point falls inside the
    /// zone's polygon are ranked. Zones are per-camera, so this pins the camera implicitly.
    pub zone: Option<String>,
    pub k: Option<usize>,
}

pub async fn search_semantic(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(body): Json<SemanticBody>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "semantic search")?;

    let text = body
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let image = body
        .image_b64
        .as_deref()
        .map(strip_data_url)
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let (kind, payload) = match (text, image) {
        (Some(t), None) => ("text", t.to_string()),
        (None, Some(img)) => {
            if img.len() > MAX_IMAGE_B64_CHARS {
                return Err(AppError::BadRequest(format!(
                    "`image_b64` exceeds {MAX_IMAGE_B64_CHARS} chars"
                )));
            }
            // Reject undecodable or non-image payloads here as a 400 — otherwise the worker fails
            // them and the caller sees a misleading 503 "worker offline".
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(img)
                .map_err(|_| AppError::BadRequest("`image_b64` is not valid base64".into()))?;
            if !looks_like_image(&decoded) {
                return Err(AppError::BadRequest(
                    "`image_b64` is not a supported image (JPEG, PNG, WebP, GIF, or BMP)".into(),
                ));
            }
            ("image", img.to_string())
        }
        (Some(_), Some(_)) => {
            return Err(AppError::BadRequest(
                "provide either `text` or `image_b64`, not both".into(),
            ))
        }
        (None, None) => {
            return Err(AppError::BadRequest(
                "one of `text` or `image_b64` is required".into(),
            ))
        }
    };

    let now = Utc::now();
    let from_parsed = parse_ts(&body.from, "from")?;
    let to_parsed = parse_ts(&body.to, "to")?;
    // The proof's honesty flag: the server chose (part of) the window, not the caller. Computed
    // from the PARSED values so a whitespace-only bound counts as defaulted too.
    let window_defaulted = from_parsed.is_none() || to_parsed.is_none();
    let from = from_parsed.unwrap_or_else(|| now - TimeDelta::try_days(7).expect("const"));
    let to = to_parsed.unwrap_or_else(|| now + TimeDelta::try_minutes(1).expect("const"));
    if from >= to {
        return Err(AppError::BadRequest("`from` must be before `to`".into()));
    }
    let k = body.k.unwrap_or(DEFAULT_K).clamp(1, MAX_K);
    let label = body
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty());
    // Zone scope (issue #77): resolve id -> (camera, polygon, name). The zone pins its camera; a
    // caller-supplied `cameras` list that doesn't include it is a contradiction we refuse rather
    // than silently answer "no results" for.
    let zone = match body
        .zone
        .as_deref()
        .map(str::trim)
        .filter(|z| !z.is_empty())
    {
        Some(zid) => Some(embeddings::resolve_zone_scope(&st.pool, zid).await?),
        None => None,
    };
    let mut cameras = body.cameras.clone();
    if let Some(z) = &zone {
        if !cameras.is_empty() && !cameras.iter().any(|c| c == &z.camera_id) {
            return Err(AppError::BadRequest(format!(
                "zone `{}` belongs to camera `{}`, which is not in `cameras`",
                z.zone_id, z.camera_id
            )));
        }
        cameras = vec![z.camera_id.clone()];
    }

    // Embed the query via the pull-only worker queue; a missing/slow worker is a clean 503. The
    // queue row (which may hold a multi-MB image payload) is deleted as soon as this request is
    // done with it — success or not — so transient payloads never accumulate in heldar.db.
    let query_id = embeddings::enqueue_query(&st.pool, kind, &payload).await?;
    let awaited = embeddings::await_query(
        &st.pool,
        &query_id,
        std::time::Duration::from_millis(cfg.embed_timeout_ms),
    )
    .await;
    embeddings::delete_query(&st.pool, &query_id).await;
    let embedded = awaited?;

    let filters = SimilarFilters {
        from,
        to,
        cameras: (!cameras.is_empty()).then(|| cameras.clone()),
        label: label.map(str::to_string),
        model: embedded.model.clone(),
        zone_polygon: zone.as_ref().map(|z| z.polygon.clone()),
    };
    let outcome = embeddings::search_similar(&st.pool, &embedded.vec, &filters, k).await?;
    let detections = detection_meta(&st.pool, &outcome.hits).await;

    // Same accountability as the other search modes: log to search_log, and audit plate-like text
    // queries. The plan snapshot records the effective filters; the raw image is NEVER logged.
    let query_text = text.map(str::to_string).unwrap_or_else(|| "[image]".into());
    let plan = QueryPlan {
        from: Some(from.to_rfc3339()),
        to: Some(to.to_rfc3339()),
        cameras: cameras.clone(),
        text: text.map(str::to_string),
        zone: zone.as_ref().map(|z| z.zone_id.clone()),
        limit: Some(k as i64),
        ..QueryPlan::default()
    };
    crate::routes::log_search(
        &st,
        &principal,
        "semantic",
        Some(&query_text),
        &plan,
        "clip",
        outcome.hits.len(),
    )
    .await;

    let model = embedded
        .model
        .clone()
        .or_else(|| outcome.hits.first().map(|h| h.model.clone()));
    let hits: Vec<Value> = outcome
        .hits
        .iter()
        .map(|h| {
            let det = h
                .detection_id
                .as_deref()
                .and_then(|id| detections.get(id).cloned());
            let mut hit = json!({
                "id": h.id,
                "score": h.score,
                "camera_id": h.camera_id,
                "timestamp": h.timestamp.to_rfc3339(),
                "label": h.label,
                "track_id": h.track_id,
                "bbox": h.bbox,
                "evidence_path": h.evidence_path,
            });
            // `detection` is present ONLY when the embedding row carries a `detection_id` that
            // joins to a live detection row. The reference embedding analyzer runs its OWN
            // ByteTrack tracker, whose track/detection ids live in a different id-space from the
            // `detection` task's, so it never sets `detection_id` — its hits carry their own
            // bbox/label/track_id instead, and this key is simply omitted rather than emitted as
            // a permanently-null field. (A future frame+bbox-replay correlation would populate it.)
            if let Some(det) = det {
                hit.as_object_mut()
                    .expect("json! object")
                    .insert("detection".into(), det);
            }
            hit
        })
        .collect();
    let proof = semantic_proof(
        &query_text,
        model.as_deref(),
        &outcome.hits,
        outcome.truncated,
        from.to_rfc3339(),
        to.to_rfc3339(),
        window_defaulted,
        zone.as_ref().map(|z| z.name.as_str()),
    );
    Ok(Json(json!({
        "query": query_text,
        "mode": "semantic",
        "model": model,
        // `enabled` is echoed so a caller scoping by a disabled zone sees it deliberately: retrieval
        // is geometry over stored history, unlike the live zone engine which skips disabled zones.
        "zone": zone.as_ref().map(|z| json!({ "id": z.zone_id, "name": z.name, "enabled": z.enabled })),
        "count": outcome.hits.len(),
        "truncated": outcome.truncated,
        "hits": hits,
        "proof": proof,
    })))
}

/// Magic-byte sniff for the image formats the worker's PIL decoder handles. Anything else gets a
/// 400 up front instead of a worker-side decode failure surfacing as 503 "worker offline".
fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF]) // JPEG
        || bytes.starts_with(&[0x89, b'P', b'N', b'G']) // PNG
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP") // WebP
        || bytes.starts_with(b"GIF8") // GIF
        || bytes.starts_with(b"BM") // BMP
}

/// Strip a `data:image/...;base64,` prefix if the client sent a full data URL.
fn strip_data_url(s: &str) -> &str {
    if s.starts_with("data:") {
        s.split_once(',').map(|(_, rest)| rest).unwrap_or(s)
    } else {
        s
    }
}

fn parse_ts(s: &Option<String>, field: &str) -> AppResult<Option<chrono::DateTime<Utc>>> {
    match s.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => heldar_kernel::util::parse_rfc3339(v)
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
        None => Ok(None),
    }
}

/// Fetch `{confidence, attributes}` for the hits that carry a `detection_id` (correlation is
/// best-effort: detections are pruned on their own TTL, so a missing row is expected, not an error).
async fn detection_meta(
    pool: &sqlx::SqlitePool,
    hits: &[SimilarHit],
) -> std::collections::HashMap<String, Value> {
    let ids: Vec<&str> = hits
        .iter()
        .filter_map(|h| h.detection_id.as_deref())
        .collect();
    if ids.is_empty() {
        return Default::default();
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql =
        format!("SELECT id, confidence, attributes FROM detections WHERE id IN ({placeholders})");
    let mut q = sqlx::query_as::<_, (String, Option<f64>, sqlx::types::Json<Value>)>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    match q.fetch_all(pool).await {
        Ok(rows) => rows
            .into_iter()
            .map(|(id, confidence, attrs)| {
                (
                    id,
                    json!({ "confidence": confidence, "attributes": attrs.0 }),
                )
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "semantic: detection metadata join failed");
            Default::default()
        }
    }
}

/// The semantic claim ladder. Unlike the structured routes, the RANKING ITSELF is the inference:
/// every hit is a real stored observation (event level), but its relevance to the query is a
/// similarity estimate from a learned embedding space — fallible by construction.
#[allow(clippy::too_many_arguments)]
fn semantic_proof(
    query: &str,
    model: Option<&str>,
    hits: &[SimilarHit],
    truncated: bool,
    eff_from: String,
    eff_to: String,
    defaulted: bool,
    zone_name: Option<&str>,
) -> Value {
    let n = hits.len();
    let levels = vec![
        json!({
            "level": "inference",
            "statement": format!(
                "Ranked stored detection-crop embeddings by cosine similarity to the query \"{query}\"."
            ),
            "confidence": "medium",
            "fallible": true,
            "evidence": { "model": model, "ranking": "cosine similarity, brute-force over stored vectors" },
            "caveat": "Similarity-ranked, NOT verified facts: a high score means the crop looks like \
                       the query to the embedding model, nothing more. Verify each hit via its \
                       evidence frame and footage.",
        }),
        json!({
            "level": "aggregate",
            "statement": if truncated {
                format!("Top {n} of the newest candidates in the window (scan hit its cap — older \
                         in-window crops were not ranked; narrow the window for exhaustive coverage).")
            } else {
                format!("Top {n} matches over every stored crop embedding in the queried window.")
            },
            "confidence": if truncated { "partial (truncated)" } else { "high" },
            "complete": !truncated,
            "evidence": {
                "count": n,
                "truncated": truncated,
                "window": { "from": eff_from, "to": eff_to, "defaulted": defaulted },
                // Zone scope: containment = the crop bbox's ground point inside the zone polygon
                // (the zone engine's semantics), tested per candidate during the scan.
                "zone": zone_name,
            },
        }),
        json!({
            "level": "event",
            "statement": format!("{n} observation claim(s); each is a real stored detection crop."),
            "confidence": "per-hit (see score)",
            "provenance": "Each hit is a crop embedding produced by the AI worker's `embedding` task \
                           from a tracked detection; pull footage via the kernel clip API \
                           (POST /api/v1/cameras/{camera_id}/clip) at the hit's timestamp, and the \
                           crop via its evidence_path.",
            "evidence": { "hit_ids": hits.iter().take(50).map(|h| json!({
                "id": h.id, "score": h.score, "evidence_path": h.evidence_path
            })).collect::<Vec<_>>() },
        }),
    ];
    json!({
        "claim_levels": levels,
        "note": "Semantic results are similarity-ranked retrievals, not facts. The ranking is the \
                 inference; the underlying crops are stored observations.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_data_url_variants() {
        assert_eq!(strip_data_url("abc"), "abc");
        assert_eq!(strip_data_url("data:image/jpeg;base64,abc"), "abc");
        assert_eq!(strip_data_url("data:"), "data:");
    }

    #[test]
    fn image_sniff_accepts_real_formats_rejects_garbage() {
        assert!(looks_like_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\n"));
        assert!(looks_like_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "));
        assert!(looks_like_image(b"GIF89a"));
        assert!(looks_like_image(b"BM\x00\x00"));
        assert!(!looks_like_image(b"%PDF-1.7"));
        assert!(!looks_like_image(b"hello world"));
        assert!(!looks_like_image(b""));
    }

    #[test]
    fn parse_ts_validates() {
        assert!(parse_ts(&None, "from").unwrap().is_none());
        assert!(parse_ts(&Some("  ".into()), "from").unwrap().is_none());
        assert!(parse_ts(&Some("2026-07-16T00:00:00Z".into()), "from")
            .unwrap()
            .is_some());
        assert!(matches!(
            parse_ts(&Some("nope".into()), "to"),
            Err(AppError::BadRequest(_))
        ));
    }
}
