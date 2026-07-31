//! Semantic-retrieval substrate (issue #38): ingest of CLIP crop embeddings posted by the AI
//! worker's `embedding` task, the pull-only query-embedding job queue, and brute-force cosine
//! top-k over the stored vectors.
//!
//! Vectors live as little-endian f32 BLOBs in SQLite (deliberately no vector DB / ANN index): at
//! single-box scale even a million 512-d vectors scan in tens of milliseconds, and the scan
//! streams rows so peak memory stays at one row + the k-sized heap.
//!
//! The query side inverts the worker's pull-only posture: a semantic search cannot call the
//! worker, so it enqueues an `embed_queries` row, the worker claims it on a fast (~1 s) poll,
//! POSTs the vector back, and the search request polls the row until done or its ~3 s budget
//! expires (then 503s — "embedding worker offline").

use std::collections::BinaryHeap;

use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Max embeddings accepted in a single ingest batch (write-amplification bound; vectors are ~2 KB
/// each so batches stay well under the route's body cap).
pub const MAX_INGEST_EMBEDDINGS: usize = 128;
/// Max embedding dimensionality accepted (CLIP ViT-B/32 is 512; leave generous headroom).
pub const MAX_EMBED_DIM: usize = 4096;
/// Max base64 length of a per-item crop thumbnail (~96 KB decoded).
pub const MAX_THUMB_B64_CHARS: usize = 131_072;
/// Pending queries older than this are never delivered to a worker — the enqueuing search request
/// gave up long ago, so computing them would be wasted work.
const QUERY_DELIVERY_TTL_SECS: i64 = 60;
/// Max queries handed to one worker per claim poll.
const CLAIM_BATCH: i64 = 4;
/// Max queries allowed in flight (pending or claimed). Each row can hold a multi-MB image payload
/// inside the size-capped heldar.db, and the route that enqueues is reachable by every viewer —
/// past this bound the enqueue answers 503 instead of letting the queue balloon.
const MAX_QUEUE_DEPTH: i64 = 16;
/// Candidate rows scanned per similarity search before the scan is cut off and the result marked
/// truncated (an honesty signal, mirroring the structured search's fetch cap).
const SCAN_CAP: usize = 100_000;

/// One embedding item in an ingest batch.
#[derive(Debug, Deserialize)]
pub struct EmbeddingItem {
    pub track_id: Option<String>,
    pub detection_id: Option<String>,
    pub label: Option<String>,
    /// RFC3339 observation time; defaults to now.
    pub timestamp: Option<String>,
    /// Normalized `[x, y, w, h]`, like `detections.bbox`.
    pub bbox: Option<Value>,
    pub vec: Vec<f32>,
    /// Optional base64 JPEG crop thumbnail, persisted to the snapshots dir as search evidence.
    pub thumb_b64: Option<String>,
}

/// Batch body of `POST /api/v1/ai/embeddings`.
#[derive(Debug, Deserialize)]
pub struct EmbeddingIngest {
    pub camera_id: String,
    pub model: String,
    pub dim: usize,
    /// Batch idempotency key (same convention as detection ingest: `"{task_id}:{captured_at}"`).
    pub frame_id: Option<String>,
    pub items: Vec<EmbeddingItem>,
}

/// Encode an embedding as the canonical BLOB layout: little-endian f32, `vec.len()` entries.
pub fn encode_vec(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Decode the canonical BLOB layout. Returns `None` if the byte length is not a multiple of 4.
pub fn decode_vec(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.len() % 4 != 0 {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
    )
}

/// Cosine similarity in [-1, 1]. Returns `None` when either vector has zero norm (undefined) or
/// the lengths differ.
pub fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some((dot / (na.sqrt() * nb.sqrt())) as f32)
}

fn validate_vec(vec: &[f32], dim: usize, what: &str) -> AppResult<()> {
    if vec.len() != dim {
        return Err(AppError::BadRequest(format!(
            "{what}: `vec` has {} entries, expected `dim` = {dim}",
            vec.len()
        )));
    }
    if vec.iter().any(|v| !v.is_finite()) {
        return Err(AppError::BadRequest(format!(
            "{what}: `vec` contains a non-finite value"
        )));
    }
    Ok(())
}

/// Persist one embedding batch. Idempotent per (camera, frame_id, track_id) — redelivered rows
/// no-op via the partial unique index. Returns the number of rows actually inserted. Thumbnails
/// are written to the snapshots dir after commit (a failed file write leaves a dangling
/// `evidence_path`; the dashboard hides broken evidence images client-side).
pub async fn ingest_batch(st: &AppState, body: &EmbeddingIngest) -> AppResult<u64> {
    let cam_exists: Option<String> = sqlx::query_scalar("SELECT id FROM cameras WHERE id = ?")
        .bind(&body.camera_id)
        .fetch_optional(&st.pool)
        .await?;
    if cam_exists.is_none() {
        return Err(AppError::NotFound(format!(
            "camera {} not found",
            body.camera_id
        )));
    }
    if body.model.trim().is_empty() {
        return Err(AppError::BadRequest("`model` is required".into()));
    }
    if body.dim == 0 || body.dim > MAX_EMBED_DIM {
        return Err(AppError::BadRequest(format!(
            "`dim` must be 1..={MAX_EMBED_DIM}"
        )));
    }
    if body.items.is_empty() {
        return Err(AppError::BadRequest("`items` must not be empty".into()));
    }
    if body.items.len() > MAX_INGEST_EMBEDDINGS {
        return Err(AppError::BadRequest(format!(
            "too many embeddings in one request ({}); max {MAX_INGEST_EMBEDDINGS}",
            body.items.len()
        )));
    }
    for (i, item) in body.items.iter().enumerate() {
        validate_vec(&item.vec, body.dim, &format!("items[{i}]"))?;
        if let Some(t) = &item.thumb_b64 {
            if t.len() > MAX_THUMB_B64_CHARS {
                return Err(AppError::BadRequest(format!(
                    "items[{i}]: `thumb_b64` exceeds {MAX_THUMB_B64_CHARS} chars"
                )));
            }
        }
        if let Some(raw) = &item.timestamp {
            if crate::util::parse_rfc3339(raw).is_none() {
                return Err(AppError::BadRequest(format!(
                    "items[{i}]: invalid `timestamp`"
                )));
            }
        }
    }

    // Per-row conditional inserts inside one transaction: ON CONFLICT DO NOTHING gives redelivery
    // dedup, and per-row rows_affected tells us which rows are new so thumbnails are only written
    // for rows that actually landed.
    let mut inserted = 0u64;
    let mut thumbs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut tx = st.pool.begin().await?;
    for item in &body.items {
        let ts = item
            .timestamp
            .as_deref()
            .and_then(crate::util::parse_rfc3339)
            .unwrap_or_else(Utc::now);
        let id = format!("emb_{}", Uuid::new_v4().simple());
        let thumb = item
            .thumb_b64
            .as_deref()
            .and_then(|t| base64::engine::general_purpose::STANDARD.decode(t).ok())
            .filter(|b| !b.is_empty());
        let evidence_path = thumb.as_ref().map(|_| format!("/media/snapshots/{id}.jpg"));
        let bbox = item.bbox.clone().map(sqlx::types::Json);
        let res = sqlx::query(
            "INSERT INTO embeddings
               (id, camera_id, detection_id, track_id, label, ts, model, dim, vec, bbox, frame_id, evidence_path, created_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT DO NOTHING",
        )
        .bind(&id)
        .bind(&body.camera_id)
        .bind(&item.detection_id)
        .bind(&item.track_id)
        .bind(&item.label)
        .bind(ts)
        .bind(&body.model)
        .bind(body.dim as i64)
        .bind(encode_vec(&item.vec))
        .bind(bbox)
        .bind(&body.frame_id)
        .bind(&evidence_path)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() > 0 {
            inserted += res.rows_affected();
            if let Some(bytes) = thumb {
                thumbs.push((format!("{id}.jpg"), bytes));
            }
        }
    }
    tx.commit().await?;

    if !thumbs.is_empty() {
        if let Err(e) = tokio::fs::create_dir_all(&st.cfg.snapshots_dir).await {
            tracing::warn!(error = %e, "embeddings: cannot create snapshots dir; thumbs dropped");
        } else {
            for (name, bytes) in thumbs {
                if let Err(e) = tokio::fs::write(st.cfg.snapshots_dir.join(&name), &bytes).await {
                    tracing::warn!(file = %name, error = %e, "embeddings: failed to write crop thumb");
                }
            }
        }
    }
    Ok(inserted)
}

/// A query claimed by (delivered to) a worker.
#[derive(Debug, Serialize)]
pub struct PendingQuery {
    pub id: String,
    pub kind: String,
    pub payload: String,
}

/// Claim up to [`CLAIM_BATCH`] pending queries for a worker (status pending → claimed). Read-only
/// when the queue is empty — this endpoint is polled every ~1 s per worker, and the SQLite writer
/// must not be touched on idle polls (see the debounced `last_used_at` precedent in auth.rs).
/// Two workers racing a claim is possible and harmless: the result POST is first-wins.
pub async fn claim_queries(pool: &SqlitePool, worker_id: &str) -> AppResult<Vec<PendingQuery>> {
    let fresh_cutoff = Utc::now() - Duration::seconds(QUERY_DELIVERY_TTL_SECS);
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, kind, payload FROM embed_queries
         WHERE status = 'pending' AND created_at > ?
         ORDER BY created_at ASC LIMIT ?",
    )
    .bind(fresh_cutoff)
    .bind(CLAIM_BATCH)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut claimed = Vec::with_capacity(rows.len());
    for (id, kind, payload) in rows {
        let res = sqlx::query(
            "UPDATE embed_queries SET status = 'claimed', claimed_at = ?, claimed_by = ?
             WHERE id = ? AND status = 'pending'",
        )
        .bind(Utc::now())
        .bind(worker_id)
        .bind(&id)
        .execute(pool)
        .await?;
        if res.rows_affected() > 0 {
            claimed.push(PendingQuery { id, kind, payload });
        }
    }
    Ok(claimed)
}

/// Worker's answer to a claimed query: a vector, or an error string.
#[derive(Debug, Deserialize)]
pub struct QueryResult {
    pub vec: Option<Vec<f32>>,
    pub model: Option<String>,
    pub dim: Option<usize>,
    pub error: Option<String>,
}

/// Record a worker's query result. First result wins; a late duplicate is a no-op (returns
/// `false`). An `error` result fails the query so the waiting search 503s immediately instead of
/// burning its whole timeout budget.
pub async fn submit_query_result(
    pool: &SqlitePool,
    id: &str,
    result: &QueryResult,
) -> AppResult<bool> {
    let res = if let Some(err) = result.error.as_deref().filter(|e| !e.trim().is_empty()) {
        sqlx::query(
            "UPDATE embed_queries SET status = 'error', error = ?, finished_at = ?
             WHERE id = ? AND status IN ('pending','claimed')",
        )
        .bind(err)
        .bind(Utc::now())
        .bind(id)
        .execute(pool)
        .await?
    } else {
        let vec = result
            .vec
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("either `vec` or `error` is required".into()))?;
        // A success result must name its model: embeddings from different CLIP checkpoints share a
        // dimensionality but are incomparable spaces, and the model id is the search prefilter
        // that keeps them apart.
        if result.model.as_deref().is_none_or(|m| m.trim().is_empty()) {
            return Err(AppError::BadRequest(
                "`model` is required with a `vec` result".into(),
            ));
        }
        let dim = result.dim.unwrap_or(vec.len());
        if dim == 0 || dim > MAX_EMBED_DIM {
            return Err(AppError::BadRequest(format!(
                "`dim` must be 1..={MAX_EMBED_DIM}"
            )));
        }
        validate_vec(vec, dim, "result")?;
        sqlx::query(
            "UPDATE embed_queries
             SET status = 'done', vec = ?, model = ?, dim = ?, finished_at = ?
             WHERE id = ? AND status IN ('pending','claimed')",
        )
        .bind(encode_vec(vec))
        .bind(&result.model)
        .bind(dim as i64)
        .bind(Utc::now())
        .bind(id)
        .execute(pool)
        .await?
    };
    Ok(res.rows_affected() > 0)
}

/// Enqueue a text/image query for embedding by a worker. Returns the queue row id. Refuses
/// (503, retryable) when [`MAX_QUEUE_DEPTH`] queries are already in flight — backpressure, so a
/// burst of image searches can never bloat heldar.db past its cap with transient payloads.
pub async fn enqueue_query(pool: &SqlitePool, kind: &str, payload: &str) -> AppResult<String> {
    let in_flight: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM embed_queries WHERE status IN ('pending','claimed')",
    )
    .fetch_one(pool)
    .await?;
    if in_flight >= MAX_QUEUE_DEPTH {
        return Err(AppError::Unavailable(
            "semantic search is busy; retry shortly".into(),
        ));
    }
    let id = format!("embq_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO embed_queries (id, kind, payload, status, created_at) VALUES (?,?,?,'pending',?)")
        .bind(&id)
        .bind(kind)
        .bind(payload)
        .bind(Utc::now())
        .execute(pool)
        .await?;
    Ok(id)
}

/// A completed query embedding.
#[derive(Debug)]
pub struct QueryEmbedding {
    pub vec: Vec<f32>,
    pub model: Option<String>,
}

/// One `(status, vec, model, error)` row polled by [`await_query`].
type QueryRow = (String, Option<Vec<u8>>, Option<String>, Option<String>);

/// Poll a queue row until a worker answers or the budget expires. Errors and timeouts both map to
/// 503 ("embedding worker offline") — the caller cannot distinguish a dead worker from a missing
/// CLIP backend, and both are operator problems, not client ones.
pub async fn await_query(
    pool: &SqlitePool,
    id: &str,
    timeout: std::time::Duration,
) -> AppResult<QueryEmbedding> {
    const POLL_EVERY: std::time::Duration = std::time::Duration::from_millis(100);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let row: Option<QueryRow> =
            sqlx::query_as("SELECT status, vec, model, error FROM embed_queries WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        match row {
            None => {
                return Err(AppError::Unavailable(
                    "embedding query vanished; retry".into(),
                ))
            }
            Some((status, vec, model, error)) => match status.as_str() {
                "done" => {
                    let vec = vec
                        .as_deref()
                        .and_then(decode_vec)
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| {
                            AppError::Unavailable("embedding worker returned no vector".into())
                        })?;
                    return Ok(QueryEmbedding { vec, model });
                }
                "error" => {
                    return Err(AppError::Unavailable(format!(
                        "embedding worker error: {}",
                        error.unwrap_or_else(|| "unknown".into())
                    )));
                }
                _ => {}
            },
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::Unavailable(
                "embedding worker offline or not ready".into(),
            ));
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Filters for a similarity search. Time bounds are required (the caller defaults them).
pub struct SimilarFilters {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub cameras: Option<Vec<String>>,
    pub label: Option<String>,
    /// Only rows embedded by this model are comparable to the query vector. `None` skips the
    /// filter (used when the worker didn't report its model id).
    pub model: Option<String>,
    /// Zone scope (issue #77): keep only candidates whose bbox **ground point (bottom-center)**
    /// falls inside this normalized polygon — the zone engine's exact containment semantics, so
    /// "in the zone" means the same thing here as everywhere else. Rows without a bbox are
    /// excluded while this is set (nothing to test). Applied during the scan, before top-k.
    pub zone_polygon: Option<Vec<[f64; 2]>>,
}

/// A zone resolved for retrieval scoping (issue #77): the geometry plus the camera it pins.
#[derive(Debug)]
pub struct ZoneScope {
    pub zone_id: String,
    pub name: String,
    pub camera_id: String,
    pub enabled: bool,
    pub polygon: Vec<[f64; 2]>,
}

/// Resolve a zone id to its retrieval scope. `NotFound` for an unknown id; a degenerate polygon
/// (< 3 points) is a `BadRequest` — silently matching nothing would read as "no results".
pub async fn resolve_zone_scope(pool: &SqlitePool, zone_id: &str) -> AppResult<ZoneScope> {
    let row: Option<(String, String, i64, sqlx::types::Json<Value>)> =
        sqlx::query_as("SELECT name, camera_id, enabled, polygon FROM zones WHERE id = ?")
            .bind(zone_id)
            .fetch_optional(pool)
            .await?;
    let (name, camera_id, enabled, polygon) =
        row.ok_or_else(|| AppError::NotFound(format!("zone {zone_id} not found")))?;
    let polygon = crate::services::zones::parse_polygon(&polygon.0);
    if polygon.len() < 3 {
        return Err(AppError::BadRequest(format!(
            "zone {zone_id} has a degenerate polygon (fewer than 3 points)"
        )));
    }
    Ok(ZoneScope {
        zone_id: zone_id.to_string(),
        name,
        camera_id,
        enabled: enabled != 0,
        polygon,
    })
}

/// One similarity hit, newest-scan-order broken by score.
#[derive(Debug, Serialize)]
pub struct SimilarHit {
    pub id: String,
    pub score: f32,
    pub camera_id: String,
    pub detection_id: Option<String>,
    pub track_id: Option<String>,
    pub label: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub bbox: Option<Value>,
    pub evidence_path: Option<String>,
    pub model: String,
}

/// Outcome of a similarity scan.
pub struct SimilarOutcome {
    pub hits: Vec<SimilarHit>,
    /// True when the scan hit [`SCAN_CAP`] before exhausting candidates — the honesty signal that
    /// older matches inside the window may exist but were not ranked.
    pub truncated: bool,
}

// Min-heap entry so the heap root is always the worst of the current top-k.
struct HeapHit(SimilarHit);
impl PartialEq for HeapHit {
    fn eq(&self, other: &Self) -> bool {
        self.0.score == other.0.score
    }
}
impl Eq for HeapHit {}
impl PartialOrd for HeapHit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapHit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reversed: BinaryHeap is a max-heap, we want the LOWEST score at the root to evict.
        other.0.score.total_cmp(&self.0.score)
    }
}

/// Brute-force cosine top-k over the stored embeddings. Streams candidate rows newest-first (so a
/// truncated scan drops the OLDEST candidates) and keeps a k-sized heap — peak memory is one row's
/// BLOB plus k hits, regardless of table size.
pub async fn search_similar(
    pool: &SqlitePool,
    query_vec: &[f32],
    filters: &SimilarFilters,
    k: usize,
) -> AppResult<SimilarOutcome> {
    let mut sql = String::from(
        "SELECT id, camera_id, detection_id, track_id, label, ts, model, dim, vec, bbox, evidence_path
         FROM embeddings WHERE ts >= ? AND ts <= ? AND dim = ?",
    );
    if let Some(cams) = filters.cameras.as_deref().filter(|c| !c.is_empty()) {
        sql.push_str(&format!(
            " AND camera_id IN ({})",
            vec!["?"; cams.len()].join(",")
        ));
    }
    if filters.label.is_some() {
        sql.push_str(" AND label = ?");
    }
    if filters.model.is_some() {
        sql.push_str(" AND model = ?");
    }
    sql.push_str(" ORDER BY ts DESC");

    let mut q = sqlx::query(&sql)
        .bind(filters.from)
        .bind(filters.to)
        .bind(query_vec.len() as i64);
    if let Some(cams) = filters.cameras.as_deref().filter(|c| !c.is_empty()) {
        for c in cams {
            q = q.bind(c);
        }
    }
    if let Some(label) = &filters.label {
        q = q.bind(label);
    }
    if let Some(model) = &filters.model {
        q = q.bind(model);
    }

    let k = k.max(1);
    let mut heap: BinaryHeap<HeapHit> = BinaryHeap::with_capacity(k + 1);
    let mut scanned = 0usize;
    let mut truncated = false;
    let mut rows = q.fetch(pool);
    while let Some(row) = rows.try_next().await? {
        if scanned >= SCAN_CAP {
            truncated = true;
            break;
        }
        scanned += 1;
        let blob: Vec<u8> = row.get("vec");
        let Some(vec) = decode_vec(&blob) else {
            continue;
        };
        let Some(score) = cosine(query_vec, &vec) else {
            continue;
        };
        if !score.is_finite() {
            continue;
        }
        let ts: DateTime<Utc> = row.get("ts");
        let bbox: Option<sqlx::types::Json<Value>> = row.get("bbox");
        // Zone scope: ground-point-in-polygon on the crop's bbox (the zone engine's containment
        // semantics). Must run BEFORE the top-k heap — filtering after would break k.
        if let Some(poly) = &filters.zone_polygon {
            let inside = bbox
                .as_ref()
                .and_then(|b| crate::services::zones::bbox_ground_point(&b.0))
                .map(|p| crate::services::zones::point_in_polygon(p, poly))
                .unwrap_or(false);
            if !inside {
                continue;
            }
        }
        heap.push(HeapHit(SimilarHit {
            id: row.get("id"),
            score,
            camera_id: row.get("camera_id"),
            detection_id: row.get("detection_id"),
            track_id: row.get("track_id"),
            label: row.get("label"),
            timestamp: ts,
            bbox: bbox.map(|b| b.0),
            evidence_path: row.get("evidence_path"),
            model: row.get("model"),
        }));
        if heap.len() > k {
            heap.pop();
        }
    }
    drop(rows);

    let mut hits: Vec<SimilarHit> = heap.into_iter().map(|h| h.0).collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(SimilarOutcome { hits, truncated })
}

/// One appearance to score for visual similarity — a moment observed on a camera. The join to
/// crop embeddings is TEMPORAL-SPATIAL (camera + time window), never by `track_id`: the embedding
/// task and the detection/ANPR tasks run independent ByteTrack instances, so their track-id spaces
/// are disjoint (see the worker docs). `label` optionally narrows to a class the embedder indexed.
pub struct AppearanceRef {
    pub camera_id: String,
    pub ts: DateTime<Utc>,
    pub label: Option<String>,
}

/// Owner decision (issue #51): at least this many comparable crop embeddings must exist on EACH
/// side before an appearance-similarity score is claimed — one lucky crop is not evidence.
pub const APPEARANCE_MIN_VECTORS: usize = 2;
/// Cap on crops fetched per side (a dense multi-second window still bounds the cross-product).
const APPEARANCE_MAX_VECTORS: usize = 32;

/// Crop embeddings observed within `±window_secs` of an appearance, newest-of-window first.
async fn vectors_near(
    pool: &SqlitePool,
    a: &AppearanceRef,
    window_secs: i64,
) -> AppResult<Vec<(String, Vec<f32>)>> {
    let win = Duration::seconds(window_secs.max(1));
    let mut sql = String::from(
        "SELECT model, vec FROM embeddings WHERE camera_id = ? AND ts >= ? AND ts <= ?",
    );
    if a.label.is_some() {
        sql.push_str(" AND label = ?");
    }
    sql.push_str(" ORDER BY ts DESC LIMIT ?");
    let mut q = sqlx::query_as::<_, (String, Vec<u8>)>(&sql)
        .bind(&a.camera_id)
        .bind(a.ts - win)
        .bind(a.ts + win);
    if let Some(label) = &a.label {
        q = q.bind(label);
    }
    let rows = q
        .bind(APPEARANCE_MAX_VECTORS as i64)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(model, blob)| decode_vec(&blob).map(|v| (model, v)))
        .collect())
}

/// Visual-similarity score in [-1, 1] between two appearances, for the movement app's cross-camera
/// link scorer (issue #51). Fetches each side's crop embeddings within `±window_secs` and returns
/// the **best pairwise cosine** across vectors sharing the same model (embeddings from different
/// CLIP checkpoints are incomparable, so cross-model pairs are never compared). Returns `None` —
/// an honest ABSENCE, never a low score — when either side has fewer than [`APPEARANCE_MIN_VECTORS`]
/// comparable vectors (no embedding task on that camera, retention pruned them, or the object class
/// isn't indexed, e.g. the privacy-excluded `person`). Max-of-pairs is robust to a few off-angle or
/// occluded crops dragging a mean down.
pub async fn appearance_similarity(
    pool: &SqlitePool,
    a: &AppearanceRef,
    b: &AppearanceRef,
    window_secs: i64,
) -> AppResult<Option<f32>> {
    let va = vectors_near(pool, a, window_secs).await?;
    if va.len() < APPEARANCE_MIN_VECTORS {
        return Ok(None);
    }
    let vb = vectors_near(pool, b, window_secs).await?;
    if vb.len() < APPEARANCE_MIN_VECTORS {
        return Ok(None);
    }
    // A model is only usable if BOTH sides have the minimum count in it (else "≥2 per side" isn't met
    // for any comparable space). Compute the best cosine within each such shared model.
    let mut best: Option<f32> = None;
    let models: std::collections::HashSet<&str> = va.iter().map(|(m, _)| m.as_str()).collect();
    for model in models {
        let sa: Vec<&Vec<f32>> = va
            .iter()
            .filter(|(m, _)| m == model)
            .map(|(_, v)| v)
            .collect();
        let sb: Vec<&Vec<f32>> = vb
            .iter()
            .filter(|(m, _)| m == model)
            .map(|(_, v)| v)
            .collect();
        if sa.len() < APPEARANCE_MIN_VECTORS || sb.len() < APPEARANCE_MIN_VECTORS {
            continue;
        }
        for x in &sa {
            for y in &sb {
                if let Some(c) = cosine(x, y) {
                    if c.is_finite() && best.map(|prev| c > prev).unwrap_or(true) {
                        best = Some(c);
                    }
                }
            }
        }
    }
    Ok(best)
}

/// Delete one queue row (best-effort). Called by the search route as soon as its request
/// completes — successfully or not — so a query's payload (possibly a multi-MB image) lives in
/// heldar.db only for the seconds the search is actually waiting. A worker's late result for a
/// deleted row is a harmless `updated: false` no-op.
pub async fn delete_query(pool: &SqlitePool, id: &str) {
    let _ = sqlx::query("DELETE FROM embed_queries WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await;
}

/// Prune finished and stale queue rows. Queue rows are transient by design (the payload can be a
/// multi-megabyte image inside the size-capped heldar.db) — anything older than an hour is gone.
pub async fn prune_queries(pool: &SqlitePool) -> AppResult<u64> {
    let cutoff = Utc::now() - Duration::hours(1);
    let n = sqlx::query("DELETE FROM embed_queries WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_blob_roundtrip() {
        let v = vec![0.0f32, 1.5, -2.25, f32::MIN_POSITIVE, 1e30];
        let blob = encode_vec(&v);
        assert_eq!(blob.len(), v.len() * 4);
        assert_eq!(decode_vec(&blob).unwrap(), v);
    }

    #[test]
    fn decode_rejects_ragged_blob() {
        assert!(decode_vec(&[0u8, 1, 2]).is_none());
        assert_eq!(decode_vec(&[]).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn cosine_basics() {
        let a = [1.0f32, 0.0];
        let b = [0.0f32, 1.0];
        let c = [2.0f32, 0.0];
        assert!((cosine(&a, &c).unwrap() - 1.0).abs() < 1e-6);
        assert!(cosine(&a, &b).unwrap().abs() < 1e-6);
        assert!((cosine(&b, &[0.0, -3.0]).unwrap() + 1.0).abs() < 1e-6);
        // Undefined cases: zero vector, mismatched lengths.
        assert!(cosine(&a, &[0.0, 0.0]).is_none());
        assert!(cosine(&a, &[1.0]).is_none());
    }

    #[test]
    fn validate_vec_enforces_dim_and_finiteness() {
        assert!(validate_vec(&[1.0, 2.0], 2, "t").is_ok());
        assert!(validate_vec(&[1.0], 2, "t").is_err());
        assert!(validate_vec(&[1.0, f32::NAN], 2, "t").is_err());
        assert!(validate_vec(&[1.0, f32::INFINITY], 2, "t").is_err());
    }

    async fn mem_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn queue_lifecycle_claim_answer_await() {
        let pool = mem_pool().await;
        let id = enqueue_query(&pool, "text", "red pickup truck")
            .await
            .unwrap();

        // Claim delivers the payload and flips status; a second claim gets nothing.
        let claimed = claim_queries(&pool, "w1").await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, id);
        assert_eq!(claimed[0].kind, "text");
        assert!(claim_queries(&pool, "w2").await.unwrap().is_empty());

        // First result wins; the duplicate is a no-op.
        let ok = submit_query_result(
            &pool,
            &id,
            &QueryResult {
                vec: Some(vec![0.5, 0.25]),
                model: Some("m".into()),
                dim: Some(2),
                error: None,
            },
        )
        .await
        .unwrap();
        assert!(ok);
        let dup = submit_query_result(
            &pool,
            &id,
            &QueryResult {
                vec: Some(vec![9.0, 9.0]),
                model: Some("m2".into()),
                dim: Some(2),
                error: None,
            },
        )
        .await
        .unwrap();
        assert!(!dup);

        // A success result without a model id is a contract violation (mixed embedding spaces
        // would become mutually rankable) — rejected as 400.
        let id_nm = enqueue_query(&pool, "text", "x").await.unwrap();
        assert!(matches!(
            submit_query_result(
                &pool,
                &id_nm,
                &QueryResult {
                    vec: Some(vec![1.0, 0.0]),
                    model: None,
                    dim: Some(2),
                    error: None,
                },
            )
            .await,
            Err(AppError::BadRequest(_))
        ));

        let got = await_query(&pool, &id, std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(got.vec, vec![0.5, 0.25]);
        assert_eq!(got.model.as_deref(), Some("m"));
    }

    #[tokio::test]
    async fn await_times_out_as_unavailable_and_error_results_fail_fast() {
        let pool = mem_pool().await;
        let id = enqueue_query(&pool, "text", "q").await.unwrap();
        match await_query(&pool, &id, std::time::Duration::from_millis(120)).await {
            Err(AppError::Unavailable(m)) => assert!(m.contains("offline"), "{m}"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
        let id2 = enqueue_query(&pool, "image", "abc").await.unwrap();
        submit_query_result(
            &pool,
            &id2,
            &QueryResult {
                vec: None,
                model: None,
                dim: None,
                error: Some("clip backend unavailable".into()),
            },
        )
        .await
        .unwrap();
        match await_query(&pool, &id2, std::time::Duration::from_secs(1)).await {
            Err(AppError::Unavailable(m)) => assert!(m.contains("clip backend"), "{m}"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn enqueue_backpressure_and_delete_query() {
        let pool = mem_pool().await;
        for i in 0..MAX_QUEUE_DEPTH {
            enqueue_query(&pool, "text", &format!("q{i}"))
                .await
                .unwrap();
        }
        // Queue full → 503-shaped refusal.
        match enqueue_query(&pool, "text", "one too many").await {
            Err(AppError::Unavailable(m)) => assert!(m.contains("busy"), "{m}"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
        // Completing (deleting) a row frees a slot.
        let victim: String = sqlx::query_scalar("SELECT id FROM embed_queries LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        delete_query(&pool, &victim).await;
        assert!(enqueue_query(&pool, "text", "fits again").await.is_ok());
    }

    #[tokio::test]
    async fn prune_queries_drops_old_rows() {
        let pool = mem_pool().await;
        let id = enqueue_query(&pool, "text", "old").await.unwrap();
        sqlx::query("UPDATE embed_queries SET created_at = ? WHERE id = ?")
            .bind(Utc::now() - Duration::hours(2))
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        let keep = enqueue_query(&pool, "text", "fresh").await.unwrap();
        assert_eq!(prune_queries(&pool).await.unwrap(), 1);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM embed_queries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1);
        let _ = keep;
    }

    #[tokio::test]
    async fn stale_pending_queries_are_not_delivered() {
        let pool = mem_pool().await;
        let id = enqueue_query(&pool, "text", "stale").await.unwrap();
        sqlx::query("UPDATE embed_queries SET created_at = ? WHERE id = ?")
            .bind(Utc::now() - Duration::seconds(QUERY_DELIVERY_TTL_SECS + 5))
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(claim_queries(&pool, "w1").await.unwrap().is_empty());
    }

    async fn seed_camera(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?, ?, 1, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_embedding(
        pool: &SqlitePool,
        cam: &str,
        label: &str,
        vec: &[f32],
        ts: DateTime<Utc>,
    ) -> String {
        let id = format!("emb_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO embeddings (id, camera_id, label, ts, model, dim, vec, created_at)
             VALUES (?,?,?,?,'m',?,?,?)",
        )
        .bind(&id)
        .bind(cam)
        .bind(label)
        .bind(ts)
        .bind(vec.len() as i64)
        .bind(encode_vec(vec))
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn search_similar_ranks_filters_and_respects_k() {
        let pool = mem_pool().await;
        seed_camera(&pool, "cam1").await;
        seed_camera(&pool, "cam2").await;
        let now = Utc::now();
        // cam1: two cars at increasing distance from the query; cam2: an exact match we filter out.
        let best = insert_embedding(&pool, "cam1", "car", &[1.0, 0.0], now).await;
        let mid = insert_embedding(&pool, "cam1", "car", &[1.0, 0.5], now).await;
        insert_embedding(&pool, "cam1", "person", &[1.0, 0.1], now).await;
        insert_embedding(&pool, "cam2", "car", &[1.0, 0.0], now).await;

        let filters = SimilarFilters {
            from: now - Duration::hours(1),
            to: now + Duration::hours(1),
            cameras: Some(vec!["cam1".into()]),
            label: Some("car".into()),
            model: Some("m".into()),
            zone_polygon: None,
        };
        let out = search_similar(&pool, &[1.0, 0.0], &filters, 10)
            .await
            .unwrap();
        assert!(!out.truncated);
        assert_eq!(
            out.hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec![best.as_str(), mid.as_str()]
        );
        assert!(out.hits[0].score > out.hits[1].score);

        // k=1 keeps only the best.
        let out1 = search_similar(&pool, &[1.0, 0.0], &filters, 1)
            .await
            .unwrap();
        assert_eq!(out1.hits.len(), 1);
        assert_eq!(out1.hits[0].id, best);

        // Dim mismatch rows are excluded by the SQL prefilter (query vec of len 3 matches nothing).
        let none = search_similar(&pool, &[1.0, 0.0, 0.0], &filters, 5)
            .await
            .unwrap();
        assert!(none.hits.is_empty());
    }

    #[tokio::test]
    async fn ingest_batch_dedups_on_frame_and_track() {
        let pool = mem_pool().await;
        seed_camera(&pool, "cam1").await;
        let cfg = std::sync::Arc::new(crate::config::Config::from_env());
        let st = AppState {
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
            started_at: Utc::now(),
            pool: pool.clone(),
            cfg,
        };
        let body = EmbeddingIngest {
            camera_id: "cam1".into(),
            model: "m".into(),
            dim: 2,
            frame_id: Some("t1:f1".into()),
            items: vec![
                EmbeddingItem {
                    track_id: Some("7".into()),
                    detection_id: None,
                    label: Some("car".into()),
                    timestamp: None,
                    bbox: None,
                    vec: vec![1.0, 0.0],
                    thumb_b64: None,
                },
                EmbeddingItem {
                    track_id: Some("8".into()),
                    detection_id: None,
                    label: Some("truck".into()),
                    timestamp: None,
                    bbox: None,
                    vec: vec![0.0, 1.0],
                    thumb_b64: None,
                },
            ],
        };
        assert_eq!(ingest_batch(&st, &body).await.unwrap(), 2);
        // Redelivery of the same batch is a no-op.
        assert_eq!(ingest_batch(&st, &body).await.unwrap(), 0);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM embeddings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);

        // Oversized batches and dim mismatches are rejected.
        let mut too_big = EmbeddingIngest {
            camera_id: "cam1".into(),
            model: "m".into(),
            dim: 2,
            frame_id: None,
            items: Vec::new(),
        };
        for _ in 0..=MAX_INGEST_EMBEDDINGS {
            too_big.items.push(EmbeddingItem {
                track_id: None,
                detection_id: None,
                label: None,
                timestamp: None,
                bbox: None,
                vec: vec![0.0, 0.0],
                thumb_b64: None,
            });
        }
        assert!(matches!(
            ingest_batch(&st, &too_big).await,
            Err(AppError::BadRequest(_))
        ));
    }

    async fn insert_embedding_with_bbox(
        pool: &SqlitePool,
        cam: &str,
        vec: &[f32],
        ts: DateTime<Utc>,
        bbox: Option<Value>,
    ) -> String {
        let id = format!("emb_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO embeddings (id, camera_id, label, ts, model, dim, vec, bbox, created_at)
             VALUES (?,?,'car',?,'m',?,?,?,?)",
        )
        .bind(&id)
        .bind(cam)
        .bind(ts)
        .bind(vec.len() as i64)
        .bind(encode_vec(vec))
        .bind(bbox.map(sqlx::types::Json))
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Zone scoping (issue #77): only candidates whose bbox GROUND POINT (bottom-center) is inside
    /// the polygon survive; bbox-less rows are excluded while the filter is active.
    #[tokio::test]
    async fn search_similar_zone_polygon_filters_by_ground_point() {
        let pool = mem_pool().await;
        seed_camera(&pool, "camZ").await;
        let t = Utc::now();
        // Left-half zone: x in [0, 0.5], full height.
        let poly = vec![[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]];
        // bbox [x,y,w,h] -> ground point (x + w/2, y + h). Inside: (0.2, 0.5).
        let inside = insert_embedding_with_bbox(
            &pool,
            "camZ",
            &[1.0, 0.0],
            t,
            Some(serde_json::json!([0.1, 0.3, 0.2, 0.2])),
        )
        .await;
        // Ground point (0.8, 0.5) — right half, outside the zone.
        insert_embedding_with_bbox(
            &pool,
            "camZ",
            &[1.0, 0.0],
            t,
            Some(serde_json::json!([0.7, 0.3, 0.2, 0.2])),
        )
        .await;
        // Straddling bbox whose ground point lands inside: box spans the middle but bottom-center
        // is (0.45, 0.9) -> inside.
        let straddle = insert_embedding_with_bbox(
            &pool,
            "camZ",
            &[0.9, 0.1],
            t,
            Some(serde_json::json!([0.3, 0.5, 0.3, 0.4])),
        )
        .await;
        // No bbox: excluded while the zone filter is active.
        insert_embedding_with_bbox(&pool, "camZ", &[1.0, 0.0], t, None).await;

        let filters = SimilarFilters {
            from: t - Duration::hours(1),
            to: t + Duration::hours(1),
            cameras: None,
            label: None,
            model: Some("m".into()),
            zone_polygon: Some(poly),
        };
        let out = search_similar(&pool, &[1.0, 0.0], &filters, 10)
            .await
            .unwrap();
        let ids: Vec<&str> = out.hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec![inside.as_str(), straddle.as_str()]);

        // Without the filter all four rank (the bbox-less row included).
        let all = search_similar(
            &pool,
            &[1.0, 0.0],
            &SimilarFilters {
                from: t - Duration::hours(1),
                to: t + Duration::hours(1),
                cameras: None,
                label: None,
                model: Some("m".into()),
                zone_polygon: None,
            },
            10,
        )
        .await
        .unwrap();
        assert_eq!(all.hits.len(), 4);
    }

    #[tokio::test]
    async fn resolve_zone_scope_paths() {
        let pool = mem_pool().await;
        seed_camera(&pool, "camZ").await;
        sqlx::query(
            "INSERT INTO zones (id, camera_id, name, polygon, created_at, updated_at)
             VALUES ('zone_ok','camZ','Patio','[[0,0],[1,0],[1,1],[0,1]]',?,?),
                    ('zone_bad','camZ','Line','[[0,0],[1,1]]',?,?)",
        )
        .bind(Utc::now())
        .bind(Utc::now())
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();

        let z = resolve_zone_scope(&pool, "zone_ok").await.unwrap();
        assert_eq!(
            (z.camera_id.as_str(), z.name.as_str(), z.polygon.len()),
            ("camZ", "Patio", 4)
        );
        assert!(matches!(
            resolve_zone_scope(&pool, "zone_missing").await,
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            resolve_zone_scope(&pool, "zone_bad").await,
            Err(AppError::BadRequest(_))
        ));
    }

    #[tokio::test]
    async fn appearance_similarity_gates_on_min_vectors_and_takes_best_pair() {
        let pool = mem_pool().await;
        seed_camera(&pool, "camA").await;
        seed_camera(&pool, "camB").await;
        let t = Utc::now();
        let a = AppearanceRef {
            camera_id: "camA".into(),
            ts: t,
            label: None,
        };
        let b = AppearanceRef {
            camera_id: "camB".into(),
            ts: t,
            label: None,
        };

        // Only ONE vector on each side yet → below APPEARANCE_MIN_VECTORS → honest None.
        insert_embedding(&pool, "camA", "car", &[1.0, 0.0], t).await;
        insert_embedding(&pool, "camB", "car", &[1.0, 0.0], t).await;
        assert_eq!(
            appearance_similarity(&pool, &a, &b, 5).await.unwrap(),
            None,
            "one vector per side is not enough evidence"
        );

        // Second vector each: camB has an off-angle crop plus one that matches camA well. Max-of-
        // pairs must find the good pair (~1.0), not be dragged down by the bad one.
        insert_embedding(&pool, "camA", "car", &[0.9, 0.1], t).await;
        insert_embedding(&pool, "camB", "car", &[0.0, 1.0], t).await; // orthogonal, off-angle
        let score = appearance_similarity(&pool, &a, &b, 5)
            .await
            .unwrap()
            .expect("both sides now have >= 2 vectors");
        assert!(
            score > 0.99,
            "best pair is the identical [1,0] crops: {score}"
        );

        // A vector outside the ±window is not gathered → back under the min → None.
        let far = AppearanceRef {
            camera_id: "camA".into(),
            ts: t + Duration::hours(1),
            label: None,
        };
        assert_eq!(
            appearance_similarity(&pool, &far, &b, 5).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn appearance_similarity_never_compares_across_models() {
        let pool = mem_pool().await;
        seed_camera(&pool, "camA").await;
        seed_camera(&pool, "camB").await;
        let t = Utc::now();
        // camA vectors are model 'm' (helper default); camB vectors are a DIFFERENT model.
        insert_embedding(&pool, "camA", "car", &[1.0, 0.0], t).await;
        insert_embedding(&pool, "camA", "car", &[1.0, 0.0], t).await;
        for _ in 0..2 {
            let id = format!("emb_{}", Uuid::new_v4().simple());
            sqlx::query(
                "INSERT INTO embeddings (id, camera_id, label, ts, model, dim, vec, created_at)
                 VALUES (?,?,?,?,'other-model',?,?,?)",
            )
            .bind(&id)
            .bind("camB")
            .bind("car")
            .bind(t)
            .bind(2i64)
            .bind(encode_vec(&[1.0, 0.0]))
            .bind(Utc::now())
            .execute(&pool)
            .await
            .unwrap();
        }
        let a = AppearanceRef {
            camera_id: "camA".into(),
            ts: t,
            label: None,
        };
        let b = AppearanceRef {
            camera_id: "camB".into(),
            ts: t,
            label: None,
        };
        // Identical vectors but incomparable checkpoints → no shared-model pair → None.
        assert_eq!(appearance_similarity(&pool, &a, &b, 5).await.unwrap(), None);
    }
}
