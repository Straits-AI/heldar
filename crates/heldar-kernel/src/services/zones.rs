//! Zone engine (Stage 3): evaluates tracked detections against per-camera polygon zones and raises
//! enter / exit / dwell events (with an evidence frame). State is keyed per (camera, zone, track),
//! held in memory, and driven by SERVER time (never the worker-supplied timestamp), so a skewed
//! worker clock cannot corrupt or evict state. A small confirm-frame debounce suppresses boundary
//! jitter, and a track still inside when its state expires gets a synthesized exit. Fed
//! synchronously from detection ingest.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::Config;
use crate::models::{DetectionIngest, Zone};
use crate::repo;

/// How long a track's zone state is retained (server time) without being seen before it is pruned.
const STATE_TTL_SECS: i64 = 120;
/// Default consecutive-observation confirmation before a membership transition (debounce); a zone
/// can override via config.confirm_frames.
const DEFAULT_CONFIRM_FRAMES: u32 = 2;

#[derive(Debug, Clone)]
struct TrackZoneState {
    track: String,
    zone_id: String,
    zone_name: String,
    severity: String,
    inside: bool,
    entered_at: DateTime<Utc>,
    dwell_emitted: bool,
    last_seen: DateTime<Utc>,
    candidate: Option<bool>,
    candidate_count: u32,
}

/// A zone event to persist + log (resolved fields, so prune-time exits need no Zone lookup).
struct ZoneEvt {
    camera_id: String,
    zone_id: String,
    zone_name: String,
    severity: String,
    track: String,
    event_type: &'static str,
    label: String,
    dwell: Option<f64>,
}

pub struct ZoneEngine {
    pool: SqlitePool,
    cfg: Arc<Config>,
    state: Mutex<HashMap<String, TrackZoneState>>,
}

fn point_in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (x, y) = (p[0], p[1]);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn parse_polygon(v: &Value) -> Vec<[f64; 2]> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|pt| {
                    let a = pt.as_array()?;
                    let x = a.first()?.as_f64()?;
                    let y = a.get(1)?.as_f64()?;
                    (x.is_finite() && y.is_finite()).then_some([x, y])
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_labels(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn confirm_frames(zone: &Zone) -> u32 {
    (zone
        .config
        .0
        .get("confirm_frames")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_CONFIRM_FRAMES as u64))
    .clamp(1, 10) as u32
}

/// Ground point of a detection bbox `[x, y, w, h]` (normalized): bottom-center.
fn bbox_ground_point(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    if a.len() < 4 {
        return None;
    }
    let x = a[0].as_f64()?;
    let y = a[1].as_f64()?;
    let w = a[2].as_f64()?;
    let h = a[3].as_f64()?;
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
        return None;
    }
    Some([x + w / 2.0, y + h])
}

#[async_trait::async_trait]
impl crate::services::consumer::DetectionConsumer for ZoneEngine {
    fn name(&self) -> &'static str {
        "zones"
    }
    /// The zone engine evaluates any tracked detection, regardless of task type.
    fn interested_in(&self, _task_type: &str) -> bool {
        true
    }
    async fn consume(&self, batch: &crate::services::consumer::DetectionBatch<'_>) {
        self.process(batch.camera_id, batch.detections).await;
    }
}

impl ZoneEngine {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            state: Mutex::new(HashMap::new()),
        })
    }

    /// Evaluate (tracked) detections for a camera against its zones, raising events. Membership is
    /// driven by server time; the worker-supplied timestamp is not trusted for state/timing.
    pub async fn process(&self, camera_id: &str, detections: &[DetectionIngest]) {
        // Dedup tracked detections by track_id (keep the highest-confidence one per track).
        let mut by_track: HashMap<&str, &DetectionIngest> = HashMap::new();
        for d in detections {
            if let (Some(t), Some(_)) = (d.track_id.as_deref(), d.bbox.as_ref()) {
                let better = by_track
                    .get(t)
                    .map(|p: &&DetectionIngest| {
                        d.confidence.unwrap_or(0.0) > p.confidence.unwrap_or(0.0)
                    })
                    .unwrap_or(true);
                if better {
                    by_track.insert(t, d);
                }
            }
        }
        if by_track.is_empty() {
            return;
        }
        let zones = match sqlx::query_as::<_, Zone>(
            "SELECT * FROM zones WHERE camera_id = ? AND enabled = 1",
        )
        .bind(camera_id)
        .fetch_all(&self.pool)
        .await
        {
            Ok(z) if !z.is_empty() => z,
            _ => return,
        };
        let parsed: Vec<(Vec<[f64; 2]>, Vec<String>, u32)> = zones
            .iter()
            .map(|z| {
                (
                    parse_polygon(&z.polygon.0),
                    parse_labels(&z.labels.0),
                    confirm_frames(z),
                )
            })
            .collect();

        let now = Utc::now();
        let mut emits: Vec<ZoneEvt> = Vec::new();
        {
            let mut state = self.state.lock().await;
            for (track, d) in &by_track {
                let Some(point) = d.bbox.as_ref().and_then(bbox_ground_point) else {
                    continue;
                };
                let label = d.label.as_deref().unwrap_or("");
                for (idx, zone) in zones.iter().enumerate() {
                    let (poly, labels, confirm) = &parsed[idx];
                    if !labels.is_empty() && !labels.iter().any(|l| l == label) {
                        continue;
                    }
                    let raw_inside = point_in_polygon(point, poly);
                    let key = format!("{camera_id}|{}|{track}", zone.id);
                    let entry = state.entry(key).or_insert_with(|| TrackZoneState {
                        track: track.to_string(),
                        zone_id: zone.id.clone(),
                        zone_name: zone.name.clone(),
                        severity: zone.severity.clone(),
                        inside: false,
                        entered_at: now,
                        dwell_emitted: false,
                        last_seen: now,
                        candidate: None,
                        candidate_count: 0,
                    });
                    entry.last_seen = now;

                    // Debounce: require `confirm` consecutive observations to flip membership.
                    if raw_inside == entry.inside {
                        entry.candidate = None;
                        entry.candidate_count = 0;
                    } else {
                        if entry.candidate == Some(raw_inside) {
                            entry.candidate_count += 1;
                        } else {
                            entry.candidate = Some(raw_inside);
                            entry.candidate_count = 1;
                        }
                        if entry.candidate_count >= *confirm {
                            entry.inside = raw_inside;
                            entry.candidate = None;
                            entry.candidate_count = 0;
                            if raw_inside {
                                entry.entered_at = now;
                                entry.dwell_emitted = false;
                                emits.push(make_evt(camera_id, zone, track, "enter", label, None));
                            } else {
                                emits.push(make_evt(camera_id, zone, track, "exit", label, None));
                            }
                        }
                    }

                    if entry.inside && zone.dwell_seconds > 0.0 && !entry.dwell_emitted {
                        let dwell = (now - entry.entered_at).num_milliseconds() as f64 / 1000.0;
                        if dwell >= zone.dwell_seconds {
                            entry.dwell_emitted = true;
                            emits.push(make_evt(
                                camera_id,
                                zone,
                                track,
                                "dwell",
                                label,
                                Some(dwell),
                            ));
                        }
                    }
                }
            }

            // Prune stale state (server time); synthesize an exit for any track still inside.
            let cutoff = now - Duration::seconds(STATE_TTL_SECS);
            let mut survivors: HashMap<String, TrackZoneState> = HashMap::new();
            for (k, s) in state.drain() {
                if s.last_seen >= cutoff {
                    survivors.insert(k, s);
                } else if s.inside {
                    emits.push(ZoneEvt {
                        camera_id: camera_id.to_string(),
                        zone_id: s.zone_id.clone(),
                        zone_name: s.zone_name.clone(),
                        severity: s.severity.clone(),
                        track: s.track.clone(),
                        event_type: "exit",
                        label: String::new(),
                        dwell: None,
                    });
                }
            }
            *state = survivors;
        }

        for e in &emits {
            self.emit(e, now).await;
        }
    }

    async fn emit(&self, evt: &ZoneEvt, now: DateTime<Utc>) {
        let id = format!("zev_{}", Uuid::new_v4().simple());
        let evidence = if evt.event_type == "enter" {
            self.copy_evidence(&evt.camera_id, &id).await
        } else {
            None
        };

        let _ = sqlx::query(
            "INSERT INTO zone_events
               (id, camera_id, zone_id, zone_name, track_id, event_type, label, timestamp,
                dwell_seconds, evidence_path, created_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&id)
        .bind(&evt.camera_id)
        .bind(&evt.zone_id)
        .bind(&evt.zone_name)
        .bind(&evt.track)
        .bind(evt.event_type)
        .bind(&evt.label)
        .bind(now)
        .bind(evt.dwell)
        .bind(&evidence)
        .bind(now)
        .execute(&self.pool)
        .await;

        let _ = repo::log_event(
            &self.pool,
            Some(&evt.camera_id),
            &format!("zone_{}", evt.event_type),
            &evt.severity,
            json!({
                "zone_id": evt.zone_id,
                "zone": evt.zone_name,
                "track_id": evt.track,
                "label": evt.label,
                "dwell_seconds": evt.dwell,
                "evidence": evidence,
            }),
        )
        .await;

        tracing::info!(camera_id = %evt.camera_id, zone = %evt.zone_name, track = %evt.track, event = evt.event_type, "zone event");
    }

    /// Copy the latest sampled sub-stream frame as evidence; returns its served URL.
    async fn copy_evidence(&self, camera_id: &str, id: &str) -> Option<String> {
        let src = self.cfg.camera_frames_dir(camera_id).join("latest_sub.jpg");
        let filename = format!("zoneevt_{id}.jpg");
        let dst = self.cfg.snapshots_dir.join(&filename);
        if tokio::fs::copy(&src, &dst).await.is_ok() {
            Some(format!("/media/snapshots/{filename}"))
        } else {
            None
        }
    }
}

fn make_evt(
    camera_id: &str,
    zone: &Zone,
    track: &str,
    event_type: &'static str,
    label: &str,
    dwell: Option<f64>,
) -> ZoneEvt {
    ZoneEvt {
        camera_id: camera_id.to_string(),
        zone_id: zone.id.clone(),
        zone_name: zone.name.clone(),
        severity: zone.severity.clone(),
        track: track.to_string(),
        event_type,
        label: label.to_string(),
        dwell,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_polygon_basic() {
        let sq = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(point_in_polygon([0.5, 0.5], &sq));
        assert!(!point_in_polygon([1.5, 0.5], &sq));
        assert!(!point_in_polygon([0.5, 1.5], &sq));
    }

    #[test]
    fn bbox_ground_point_is_bottom_center() {
        assert_eq!(
            bbox_ground_point(&json!([0.2, 0.1, 0.4, 0.6])),
            Some([0.4, 0.7])
        );
        assert_eq!(bbox_ground_point(&json!([1, 2, 3])), None);
        assert_eq!(bbox_ground_point(&json!(["x", 0, 0, 0])), None);
    }

    #[test]
    fn parse_polygon_skips_non_finite_and_bad_points() {
        assert_eq!(
            parse_polygon(&json!([[0.0, 0.0], [1.0, 0.5], ["a", 1]])),
            vec![[0.0, 0.0], [1.0, 0.5]]
        );
    }

    #[test]
    fn parse_labels_strings() {
        assert_eq!(
            parse_labels(&json!(["person", "car"])),
            vec!["person", "car"]
        );
    }
}
