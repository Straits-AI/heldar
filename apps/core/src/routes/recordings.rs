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
}

#[derive(Debug, Deserialize)]
struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct SegmentView {
    #[serde(flatten)]
    seg: Segment,
    /// Browser-playable URL for the segment file.
    url: String,
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
    Ok((parse(&q.from, "from")?, parse(&q.to, "to")?))
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

    let views = segments
        .into_iter()
        .map(|s| {
            let url = segment_url(&id, &s.path);
            SegmentView { seg: s, url }
        })
        .collect();
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

/// Coalesce contiguous segments into availability ranges (gaps > 2s split a range).
async fn timeline(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> AppResult<Json<Timeline>> {
    let _ = load_camera(&st.pool, &id).await?;
    let (from, to) = parse_range(&q)?;

    // Honor either or both bounds; with neither, returns the full timeline (bounded by retention).
    let segments: Vec<Segment> = sqlx::query_as::<_, Segment>(
        "SELECT * FROM segments
         WHERE camera_id = ?
           AND (? IS NULL OR start_time < ?)
           AND (? IS NULL OR end_time > ?)
         ORDER BY start_time ASC",
    )
    .bind(&id)
    .bind(to)
    .bind(to)
    .bind(from)
    .bind(from)
    .fetch_all(&st.pool)
    .await?;

    let segment_count = segments.len();
    let mut ranges: Vec<TimelineRange> = Vec::new();
    const GAP_TOLERANCE_S: i64 = 2;

    for s in &segments {
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
