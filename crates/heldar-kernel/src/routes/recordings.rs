use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::models::Segment;
use crate::routes::cameras::load_camera;
use crate::state::AppState;
use crate::util;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/cameras/{id}/segments", get(list_segments))
        .route("/api/v1/cameras/{id}/timeline", get(timeline))
        .route("/api/v1/cameras/{id}/gaps", get(gaps))
}

#[derive(Debug, Deserialize)]
struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
    limit: Option<i64>,
}

/// A segment row plus its browser-playable media URL. Flattens the full [`Segment`] (so new model
/// fields like `evidence_locked` flow through automatically). Reused by the incidents API.
#[derive(Debug, Serialize)]
pub struct SegmentView {
    #[serde(flatten)]
    seg: Segment,
    /// Browser-playable URL for the segment file.
    url: String,
}

impl SegmentView {
    /// Build a view from a segment row, deriving its media URL from the stored path.
    pub fn new(seg: Segment) -> Self {
        let url = segment_url(&seg.camera_id, &seg.path);
        SegmentView { seg, url }
    }
}

fn segment_url(camera_id: &str, path: &str) -> String {
    let file = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    format!("/media/recordings/{camera_id}/{file}")
}

type OptTimeRange = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

fn parse_range(q: &RangeQuery) -> AppResult<OptTimeRange> {
    let parse = |s: &Option<String>, field: &str| -> AppResult<Option<DateTime<Utc>>> {
        match s {
            Some(v) => util::parse_rfc3339(v)
                .map(Some)
                .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
            None => Ok(None),
        }
    };
    let from = parse(&q.from, "from")?;
    let to = parse(&q.to, "to")?;
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err(AppError::BadRequest("`from` must be <= `to`".into()));
        }
    }
    Ok((from, to))
}

async fn list_segments(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<Vec<SegmentView>>> {
    let _ = load_camera(&st.pool, &id).await?;
    let (from, to) = parse_range(&q)?;
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);

    let segments: Vec<Segment> = if from.is_none() && to.is_none() {
        // No range: return the most recent N segments (ascending for display).
        let mut rows = sqlx::query_as::<_, Segment>(
            "SELECT * FROM segments WHERE camera_id = ? ORDER BY start_time DESC LIMIT ?",
        )
        .bind(&id)
        .bind(limit)
        .fetch_all(&st.pool)
        .await?;
        rows.reverse();
        rows
    } else {
        // Honor either or both bounds (open-ended when one side is absent).
        sqlx::query_as::<_, Segment>(
            "SELECT * FROM segments
             WHERE camera_id = ?
               AND (? IS NULL OR start_time < ?)
               AND (? IS NULL OR end_time > ?)
             ORDER BY start_time ASC LIMIT ?",
        )
        .bind(&id)
        .bind(to)
        .bind(to)
        .bind(from)
        .bind(from)
        .bind(limit)
        .fetch_all(&st.pool)
        .await?
    };

    let views = segments.into_iter().map(SegmentView::new).collect();
    Ok(Json(views))
}

#[derive(Debug, Serialize)]
struct TimelineRange {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    seconds: f64,
}

#[derive(Debug, Serialize)]
struct Timeline {
    camera_id: String,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    ranges: Vec<TimelineRange>,
    recorded_seconds: f64,
    segment_count: usize,
}

/// Gaps below this many seconds between segments are treated as contiguous.
const GAP_TOLERANCE_S: i64 = 2;

/// Fetch a camera's segments, honoring either or both optional bounds (open-ended otherwise).
async fn fetch_segments_in_range(
    pool: &sqlx::SqlitePool,
    id: &str,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> AppResult<Vec<Segment>> {
    let segments = sqlx::query_as::<_, Segment>(
        "SELECT * FROM segments
         WHERE camera_id = ?
           AND (? IS NULL OR start_time < ?)
           AND (? IS NULL OR end_time > ?)
         ORDER BY start_time ASC",
    )
    .bind(id)
    .bind(to)
    .bind(to)
    .bind(from)
    .bind(from)
    .fetch_all(pool)
    .await?;
    Ok(segments)
}

/// Coalesce contiguous segments into availability ranges (gaps > tolerance split a range).
fn coalesce(segments: &[Segment]) -> Vec<TimelineRange> {
    let mut ranges: Vec<TimelineRange> = Vec::new();
    for s in segments {
        if let Some(last) = ranges.last_mut() {
            if (s.start_time - last.end).num_seconds() <= GAP_TOLERANCE_S {
                if s.end_time > last.end {
                    last.end = s.end_time;
                    last.seconds = (last.end - last.start).num_milliseconds() as f64 / 1000.0;
                }
                continue;
            }
        }
        ranges.push(TimelineRange {
            start: s.start_time,
            end: s.end_time,
            seconds: (s.end_time - s.start_time).num_milliseconds() as f64 / 1000.0,
        });
    }
    ranges
}

async fn timeline(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<Timeline>> {
    let _ = load_camera(&st.pool, &id).await?;
    let (from, to) = parse_range(&q)?;
    let segments = fetch_segments_in_range(&st.pool, &id, from, to).await?;
    let segment_count = segments.len();
    let ranges = coalesce(&segments);
    let recorded_seconds = ranges.iter().map(|r| r.seconds).sum();
    Ok(Json(Timeline {
        camera_id: id,
        from,
        to,
        ranges,
        recorded_seconds,
        segment_count,
    }))
}

#[derive(Debug, Serialize)]
struct Gaps {
    camera_id: String,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    gaps: Vec<TimelineRange>,
    gap_count: usize,
    total_gap_seconds: f64,
}

/// Report holes in recording coverage (the spans between coalesced availability ranges).
async fn gaps(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<Gaps>> {
    let _ = load_camera(&st.pool, &id).await?;
    let (from, to) = parse_range(&q)?;
    let segments = fetch_segments_in_range(&st.pool, &id, from, to).await?;
    let ranges = coalesce(&segments);

    let mk = |start: DateTime<Utc>, end: DateTime<Utc>| -> Option<TimelineRange> {
        let seconds = (end - start).num_milliseconds() as f64 / 1000.0;
        (seconds > GAP_TOLERANCE_S as f64).then_some(TimelineRange {
            start,
            end,
            seconds,
        })
    };

    let mut gaps = Vec::new();
    // Leading edge: a hole between the requested window start and the first coverage (or the whole
    // window when there is no coverage at all).
    if let Some(f) = from {
        match ranges.first() {
            None => gaps.extend(to.and_then(|t| mk(f, t))),
            Some(first) => gaps.extend(mk(f, first.start)),
        }
    }
    // Interior holes between coalesced coverage ranges.
    for w in ranges.windows(2) {
        gaps.extend(mk(w[0].end, w[1].start));
    }
    // Trailing edge: a hole between the last coverage and the requested window end.
    if let (Some(t), Some(last)) = (to, ranges.last()) {
        gaps.extend(mk(last.end, t));
    }
    let total_gap_seconds = gaps.iter().map(|g| g.seconds).sum();
    let gap_count = gaps.len();
    Ok(Json(Gaps {
        camera_id: id,
        from,
        to,
        gaps,
        gap_count,
        total_gap_seconds,
    }))
}
