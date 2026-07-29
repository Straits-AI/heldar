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
use crate::services::recorder::RecorderManager;

/// How long a track's zone state is retained (server time) without being seen before it is pruned.
const STATE_TTL_SECS: i64 = 120;
/// Default consecutive-observation confirmation before a membership transition (debounce); a zone
/// can override via config.confirm_frames.
const DEFAULT_CONFIRM_FRAMES: u32 = 2;
/// Default per-zone confidence floor (see `min_confidence`).
const DEFAULT_MIN_CONFIDENCE: f64 = 0.5;
/// Static-object suppression defaults (see `static_params`): a track whose ground point never
/// strays more than `EPSILON` (normalized units) from where it first appeared for
/// `AFTER_SECS` is not a moving subject — suppress its zone events until it actually moves.
const DEFAULT_STATIC_EPSILON: f64 = 0.02;
const DEFAULT_STATIC_AFTER_SECS: i64 = 120;

/// Per-zone precomputed evaluation parameters: polygon, label filter, confirm-frames, confidence
/// floor, static-suppression (epsilon, after-seconds) when enabled.
type ZoneParams = (Vec<[f64; 2]>, Vec<String>, u32, f64, Option<(f64, i64)>);

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
    // ---- static-object suppression (issue #47) ----
    /// When this state entry was created (server time).
    first_seen: DateTime<Utc>,
    /// Ground point at first sight — displacement is measured from here.
    origin: [f64; 2],
    /// Watermark of the largest displacement from `origin` ever observed.
    max_displacement: f64,
    /// Static suppression active: membership is still tracked, but no events are emitted.
    suppressed: bool,
    /// The one-time `zone_static_suppressed` notice was already emitted (inherited by reborn
    /// tracks at the same position, so flicker can't re-announce).
    static_evented: bool,
    /// Previous observation's ground point (for line-crossing tests on `kind = "line"` zones).
    last_point: Option<[f64; 2]>,
}

/// Outcome of the per-observation static-suppression update (see `update_static`).
#[derive(Debug, PartialEq, Eq)]
enum StaticTransition {
    None,
    /// The track just crossed the static threshold: suppress + announce once.
    BecameStatic,
    /// A suppressed track moved beyond epsilon: unsuppress (and re-announce presence).
    BecameActive,
}

/// Update a state entry's displacement watermark and suppression flag for one observation.
/// Pure state-machine step, factored out for unit testing.
fn update_static(
    entry: &mut TrackZoneState,
    point: [f64; 2],
    now: DateTime<Utc>,
    epsilon: f64,
    after_secs: i64,
) -> StaticTransition {
    let dx = point[0] - entry.origin[0];
    let dy = point[1] - entry.origin[1];
    let dist = (dx * dx + dy * dy).sqrt();
    if dist > entry.max_displacement {
        entry.max_displacement = dist;
    }
    if entry.suppressed {
        if entry.max_displacement >= epsilon {
            entry.suppressed = false;
            return StaticTransition::BecameActive;
        }
        return StaticTransition::None;
    }
    if !entry.static_evented
        && entry.max_displacement < epsilon
        && (now - entry.first_seen).num_seconds() >= after_secs
    {
        entry.suppressed = true;
        entry.static_evented = true;
        return StaticTransition::BecameStatic;
    }
    StaticTransition::None
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
    /// Recorder handle: a committed zone event triggers event-mode recording (no-op for cameras not
    /// in `event` / `scheduled_event` mode — [`RecorderManager::trigger`] guards on the mode).
    recorder: Arc<RecorderManager>,
    state: Mutex<HashMap<String, TrackZoneState>>,
    /// Last occupancy written per zone (write-behind cache: `zone_occupancy` rows are upserted
    /// only when a zone's live count changes, keeping the hot path off the SQLite writer).
    occupancy: Mutex<HashMap<String, i64>>,
}

/// 2D cross product of (b-a) × (c-a): sign tells which side of line a→b the point c lies on.
fn cross_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Whether segment p1→p2 properly intersects segment q1→q2 (shared endpoints/colinear overlaps do
/// not count — a ground point sitting exactly ON the line must not fire on jitter).
fn segments_cross(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let d1 = cross_sign(q1, q2, p1);
    let d2 = cross_sign(q1, q2, p2);
    let d3 = cross_sign(p1, p2, q1);
    let d4 = cross_sign(p1, p2, q2);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Directional crossing test for a `kind = "line"` zone (polyline A→B): did the track's ground
/// point move across the line between `prev` and `cur`, and in which direction? Returns
/// `Some("cross_ab")` when the movement crosses with A→B's LEFT side first (i.e. travelling from
/// the left of A→B to its right), `Some("cross_ba")` for the opposite, `None` when no crossing.
fn line_crossing(prev: [f64; 2], cur: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<&'static str> {
    if !segments_cross(prev, cur, a, b) {
        return None;
    }
    // Side of the line the movement STARTED on decides the direction label.
    if cross_sign(a, b, prev) > 0.0 {
        Some("cross_ab")
    } else {
        Some("cross_ba")
    }
}

/// The direction filter of a line zone: `any` (default) | `ab` | `ba` (`config.direction`).
fn line_direction_filter(zone: &Zone) -> String {
    zone.config
        .0
        .get("direction")
        .and_then(|v| v.as_str())
        .filter(|d| matches!(*d, "any" | "ab" | "ba"))
        .unwrap_or("any")
        .to_string()
}

pub(crate) fn point_in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
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

pub(crate) fn parse_polygon(v: &Value) -> Vec<[f64; 2]> {
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

/// Per-zone minimum detection confidence for zone membership. Detections below it are invisible
/// to the zone (they neither enter nor sustain presence). Default 0.5: verified live that
/// borderline detector output (a 0.26–0.33 "person" that was actually laundry on a line) streams
/// continuous presence/dwell events from a full-frame zone — near-threshold detections are noise
/// far more often than signal. Override per zone via `config.min_confidence` (0.0 restores the
/// old accept-everything behavior).
fn min_confidence(zone: &Zone) -> f64 {
    zone.config
        .0
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite())
        .unwrap_or(DEFAULT_MIN_CONFIDENCE)
        .clamp(0.0, 1.0)
}

/// Per-zone static-object suppression parameters, or `None` when disabled. A track that has never
/// moved more than `epsilon` (normalized units, Euclidean on the ground point) from where it first
/// appeared, for at least `after_secs`, is treated as a static object (laundry on a line, a parked
/// object, a poster): its zone events are suppressed until it actually moves. Verified live: a
/// detector-confused static object re-alarms a presence zone indefinitely, and confidence floors
/// cannot catch it when its score peaks with the light. Defaults on; disable per zone via
/// `config.static_suppression: false`, tune via `config.static_epsilon` /
/// `config.static_after_seconds`.
fn static_params(zone: &Zone) -> Option<(f64, i64)> {
    let cfg = &zone.config.0;
    if cfg
        .get("static_suppression")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
    {
        let eps = cfg
            .get("static_epsilon")
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(DEFAULT_STATIC_EPSILON)
            .clamp(0.001, 0.5);
        let after = cfg
            .get("static_after_seconds")
            .and_then(|v| v.as_i64())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_STATIC_AFTER_SECS)
            .clamp(5, 86_400);
        Some((eps, after))
    } else {
        None
    }
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

/// Ground point of a detection bbox `[x, y, w, h]` (normalized): bottom-center. `pub(crate)`:
/// the embeddings service reuses it so zone-scoped retrieval means EXACTLY what the zone engine
/// means by "in the zone" (issue #77).
pub(crate) fn bbox_ground_point(v: &Value) -> Option<[f64; 2]> {
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
    pub fn new(pool: SqlitePool, cfg: Arc<Config>, recorder: Arc<RecorderManager>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            cfg,
            recorder,
            state: Mutex::new(HashMap::new()),
            occupancy: Mutex::new(HashMap::new()),
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
        let parsed: Vec<ZoneParams> = zones
            .iter()
            .map(|z| {
                (
                    parse_polygon(&z.polygon.0),
                    parse_labels(&z.labels.0),
                    confirm_frames(z),
                    min_confidence(z),
                    static_params(z),
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
                    let (poly, labels, confirm, min_conf, static_cfg) = &parsed[idx];
                    if !labels.is_empty() && !labels.iter().any(|l| l == label) {
                        continue;
                    }
                    // Below the zone's confidence floor: invisible to this zone (no enter, no
                    // dwell sustain) — borderline detections must not drive zone state.
                    if d.confidence.unwrap_or(0.0) < *min_conf {
                        continue;
                    }
                    let raw_inside = point_in_polygon(point, poly);
                    let key = format!("{camera_id}|{}|{track}", zone.id);
                    if !state.contains_key(&key) {
                        // A detector flickering around its threshold kills and re-births tracks of
                        // the SAME static object under fresh ids — each rebirth would restart the
                        // static clock and fire a fresh enter. If a suppressed sibling of this
                        // zone sits within epsilon of this point, the "new" track IS that object:
                        // inherit its suppression + origin instead of starting clean.
                        let inherited = static_cfg.and_then(|(eps, _)| {
                            find_suppressed_sibling(&state, camera_id, &zone.id, point, eps)
                        });
                        let (origin, suppressed, static_evented) = match inherited {
                            Some(origin) => (origin, true, true),
                            None => (point, false, false),
                        };
                        state.insert(
                            key.clone(),
                            TrackZoneState {
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
                                first_seen: now,
                                origin,
                                max_displacement: 0.0,
                                suppressed,
                                static_evented,
                                last_point: None,
                            },
                        );
                    }
                    let entry = state.get_mut(&key).expect("just inserted");
                    entry.last_seen = now;

                    // Static-object suppression (issue #47): update the displacement watermark and
                    // suppression state BEFORE membership so this observation's events are gated.
                    if let Some((eps, after)) = static_cfg {
                        match update_static(entry, point, now, *eps, *after) {
                            StaticTransition::BecameStatic => {
                                // One-time operator notice (info): why this zone went quiet.
                                emits.push(ZoneEvt {
                                    camera_id: camera_id.to_string(),
                                    zone_id: zone.id.clone(),
                                    zone_name: zone.name.clone(),
                                    severity: "info".into(),
                                    track: track.to_string(),
                                    event_type: "static_suppressed",
                                    label: label.to_string(),
                                    dwell: None,
                                });
                            }
                            StaticTransition::BecameActive => {
                                // It moved: re-announce presence so operators see the now-live
                                // object (its original enter fired long ago or was inherited).
                                if entry.inside {
                                    entry.entered_at = now;
                                    entry.dwell_emitted = false;
                                    emits.push(make_evt(
                                        camera_id, zone, track, "enter", label, None,
                                    ));
                                }
                            }
                            StaticTransition::None => {}
                        }
                    }

                    // Line zones: directional crossing test on the movement segment, no
                    // membership/dwell machinery (a line has no interior).
                    if zone.kind == "line" {
                        if let (Some(prev), [a, b, ..]) = (entry.last_point, poly.as_slice()) {
                            if let Some(dir) = line_crossing(prev, point, *a, *b) {
                                if !entry.suppressed {
                                    let filter = line_direction_filter(zone);
                                    if filter == "any" || dir.ends_with(filter.as_str()) {
                                        emits.push(make_evt(
                                            camera_id, zone, track, dir, label, None,
                                        ));
                                    }
                                }
                            }
                        }
                        entry.last_point = Some(point);
                        continue;
                    }
                    entry.last_point = Some(point);

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
                            // A suppressed (static) track still updates membership silently.
                            if !entry.suppressed {
                                if raw_inside {
                                    entry.entered_at = now;
                                    entry.dwell_emitted = false;
                                    emits.push(make_evt(
                                        camera_id, zone, track, "enter", label, None,
                                    ));
                                } else {
                                    emits.push(make_evt(
                                        camera_id, zone, track, "exit", label, None,
                                    ));
                                }
                            }
                        }
                    }

                    if entry.inside
                        && !entry.suppressed
                        && zone.dwell_seconds > 0.0
                        && !entry.dwell_emitted
                    {
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
                } else if s.inside && !s.suppressed {
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

        self.flush_occupancy(now).await;
    }

    /// Recompute live per-zone occupancy from track state (inside, not static-suppressed, seen
    /// recently) and upsert only the zones whose count CHANGED — a write-behind aggregate for
    /// `GET /api/v1/cameras/{id}/zones/occupancy` that stays off the hot path when steady.
    async fn flush_occupancy(&self, now: DateTime<Utc>) {
        let fresh_cutoff = now - Duration::seconds(STATE_TTL_SECS);
        let mut counts: HashMap<(String, String), i64> = HashMap::new();
        {
            let state = self.state.lock().await;
            for (key, s) in state.iter() {
                if s.inside && !s.suppressed && s.last_seen >= fresh_cutoff {
                    let camera_id = key.split('|').next().unwrap_or("").to_string();
                    *counts.entry((camera_id, s.zone_id.clone())).or_insert(0) += 1;
                }
            }
        }
        let mut cache = self.occupancy.lock().await;
        // Zones that had a nonzero cached count but no inside tracks now drop to 0.
        let mut changed: Vec<(String, String, i64)> = Vec::new();
        for ((camera_id, zone_id), count) in &counts {
            if cache.get(zone_id).copied().unwrap_or(0) != *count {
                changed.push((camera_id.clone(), zone_id.clone(), *count));
            }
        }
        let zeroed: Vec<String> = cache
            .iter()
            .filter(|(z, c)| **c != 0 && !counts.keys().any(|(_, zid)| zid == *z))
            .map(|(z, _)| z.clone())
            .collect();
        for (camera_id, zone_id, count) in changed {
            let _ = sqlx::query(
                "INSERT INTO zone_occupancy (zone_id, camera_id, count, updated_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(zone_id) DO UPDATE SET count = excluded.count, updated_at = excluded.updated_at",
            )
            .bind(&zone_id)
            .bind(&camera_id)
            .bind(count)
            .bind(now)
            .execute(&self.pool)
            .await;
            cache.insert(zone_id, count);
        }
        for zone_id in zeroed {
            let _ = sqlx::query(
                "UPDATE zone_occupancy SET count = 0, updated_at = ? WHERE zone_id = ?",
            )
            .bind(now)
            .bind(&zone_id)
            .execute(&self.pool)
            .await;
            cache.insert(zone_id, 0);
        }
    }

    async fn emit(&self, evt: &ZoneEvt, now: DateTime<Utc>) {
        // The one-time static-suppression notice is operator telemetry, not zone analytics: log it
        // to the event feed and skip the zone_events row + the recording trigger (a static object
        // must not extend event-mode recording).
        if evt.event_type == "static_suppressed" {
            let _ = repo::log_event(
                &self.pool,
                Some(&evt.camera_id),
                "zone_static_suppressed",
                "info",
                json!({
                    "zone_id": evt.zone_id,
                    "zone": evt.zone_name,
                    "track_id": evt.track,
                    "label": evt.label,
                    "note": "stationary object; zone events from this track suppressed until it moves",
                }),
            )
            .await;
            tracing::info!(camera_id = %evt.camera_id, zone = %evt.zone_name, track = %evt.track, "zone: static object suppressed");
            return;
        }
        let id = format!("zev_{}", Uuid::new_v4().simple());
        // Evidence frame for the event types that start an episode (enters + line crossings).
        let evidence = if matches!(evt.event_type, "enter" | "cross_ab" | "cross_ba") {
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

        // Event-triggered recording: extend the camera's trigger window. A no-op unless the camera's
        // record_mode is `event` / `scheduled_event` (the recorder guards on the mode).
        let _ = self.recorder.trigger(&evt.camera_id, "zone_event").await;
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

/// A suppressed state entry of the same (camera, zone) whose ORIGIN lies within `epsilon` of
/// `point` — i.e. the static object a "new" track was re-born from. Returns its origin so the new
/// entry measures displacement from the object's true anchor.
fn find_suppressed_sibling(
    state: &HashMap<String, TrackZoneState>,
    camera_id: &str,
    zone_id: &str,
    point: [f64; 2],
    epsilon: f64,
) -> Option<[f64; 2]> {
    let prefix = format!("{camera_id}|{zone_id}|");
    state
        .iter()
        .filter(|(k, s)| s.suppressed && k.starts_with(&prefix))
        .find(|(_, s)| {
            let dx = point[0] - s.origin[0];
            let dy = point[1] - s.origin[1];
            (dx * dx + dy * dy).sqrt() < epsilon
        })
        .map(|(_, s)| s.origin)
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

    fn zone_with_config(config: Value) -> Zone {
        Zone {
            id: "z1".into(),
            camera_id: "cam".into(),
            name: "test".into(),
            kind: "presence".into(),
            polygon: sqlx::types::Json(json!([[0, 0], [1, 0], [1, 1], [0, 1]])),
            dwell_seconds: 0.0,
            labels: sqlx::types::Json(json!([])),
            severity: "info".into(),
            config: sqlx::types::Json(config),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// The confidence floor defaults to 0.5 and clamps the per-zone override into 0..=1. A zero
    /// override restores the old accept-everything behavior.
    #[test]
    fn min_confidence_default_override_and_clamp() {
        assert_eq!(min_confidence(&zone_with_config(json!({}))), 0.5);
        assert_eq!(
            min_confidence(&zone_with_config(json!({ "min_confidence": 0.25 }))),
            0.25
        );
        assert_eq!(
            min_confidence(&zone_with_config(json!({ "min_confidence": 0.0 }))),
            0.0
        );
        assert_eq!(
            min_confidence(&zone_with_config(json!({ "min_confidence": 7.0 }))),
            1.0
        );
        // Non-numeric / non-finite → default.
        assert_eq!(
            min_confidence(&zone_with_config(json!({ "min_confidence": "high" }))),
            0.5
        );
    }

    fn fresh_state(origin: [f64; 2], first_seen: DateTime<Utc>) -> TrackZoneState {
        TrackZoneState {
            track: "t1".into(),
            zone_id: "z1".into(),
            zone_name: "test".into(),
            severity: "info".into(),
            inside: true,
            entered_at: first_seen,
            dwell_emitted: false,
            last_seen: first_seen,
            candidate: None,
            candidate_count: 0,
            first_seen,
            origin,
            max_displacement: 0.0,
            suppressed: false,
            static_evented: false,
            last_point: None,
        }
    }

    /// Replays the live laundry case (issue #47): a "person" whose ground point never moves.
    /// Before the time threshold nothing happens; once it ages past `after_secs` without moving
    /// beyond epsilon it becomes static exactly once; when it finally moves it becomes active.
    #[test]
    fn update_static_lifecycle() {
        let t0 = Utc::now();
        let origin = [0.535, 0.41]; // laundry ground point, cam3
        let mut st = fresh_state(origin, t0);

        // 60s of pixel-frozen observations: below after_secs (120) — no transition.
        let r = update_static(
            &mut st,
            [0.5351, 0.4101],
            t0 + Duration::seconds(60),
            0.02,
            120,
        );
        assert_eq!(r, StaticTransition::None);
        assert!(!st.suppressed);

        // Past 120s, still frozen: suppressed, once.
        let r = update_static(
            &mut st,
            [0.5349, 0.4099],
            t0 + Duration::seconds(121),
            0.02,
            120,
        );
        assert_eq!(r, StaticTransition::BecameStatic);
        assert!(st.suppressed && st.static_evented);
        let r = update_static(&mut st, origin, t0 + Duration::seconds(122), 0.02, 120);
        assert_eq!(r, StaticTransition::None, "no repeat announcement");

        // It moves (someone takes the laundry / a real person walks): unsuppressed.
        let r = update_static(
            &mut st,
            [0.60, 0.50],
            t0 + Duration::seconds(200),
            0.02,
            120,
        );
        assert_eq!(r, StaticTransition::BecameActive);
        assert!(!st.suppressed);
        // Watermark keeps it from ever re-suppressing.
        let r = update_static(
            &mut st,
            [0.60, 0.50],
            t0 + Duration::seconds(999),
            0.02,
            120,
        );
        assert_eq!(r, StaticTransition::None);
        assert!(!st.suppressed);
    }

    /// A moving track never suppresses, even after the time threshold.
    #[test]
    fn update_static_moving_track_never_suppresses() {
        let t0 = Utc::now();
        let mut st = fresh_state([0.2, 0.8], t0);
        for i in 0..10 {
            let p = [0.2 + 0.01 * i as f64, 0.8];
            let r = update_static(&mut st, p, t0 + Duration::seconds(30 * (i + 1)), 0.02, 120);
            assert_eq!(r, StaticTransition::None);
        }
        assert!(!st.suppressed);
        assert!(st.max_displacement > 0.02);
    }

    /// A re-born track at a suppressed object's position inherits that object's suppression
    /// (via its origin); a track elsewhere does not.
    #[test]
    fn reborn_track_folds_into_suppressed_sibling() {
        let t0 = Utc::now();
        let mut old = fresh_state([0.535, 0.41], t0);
        old.suppressed = true;
        old.static_evented = true;
        let mut map = HashMap::new();
        map.insert("cam3|z1|48156".to_string(), old);

        assert_eq!(
            find_suppressed_sibling(&map, "cam3", "z1", [0.536, 0.412], 0.02),
            Some([0.535, 0.41])
        );
        // Different zone or far away: no fold.
        assert_eq!(
            find_suppressed_sibling(&map, "cam3", "z2", [0.536, 0.412], 0.02),
            None
        );
        assert_eq!(
            find_suppressed_sibling(&map, "cam3", "z1", [0.20, 0.80], 0.02),
            None
        );
    }

    /// Config parsing: on by default, per-zone off switch, clamped overrides.
    #[test]
    fn static_params_defaults_and_overrides() {
        assert_eq!(
            static_params(&zone_with_config(json!({}))),
            Some((0.02, 120))
        );
        assert_eq!(
            static_params(&zone_with_config(json!({ "static_suppression": false }))),
            None
        );
        assert_eq!(
            static_params(&zone_with_config(
                json!({ "static_epsilon": 0.05, "static_after_seconds": 300 })
            )),
            Some((0.05, 300))
        );
        // Nonsense values fall back / clamp.
        assert_eq!(
            static_params(&zone_with_config(
                json!({ "static_epsilon": -1.0, "static_after_seconds": 0 })
            )),
            Some((0.02, 120))
        );
    }

    /// Directional line crossing (#40): vertical line A(0.5,0)→B(0.5,1); movement left→right
    /// starts on A→B's positive side → cross_ab; right→left → cross_ba; parallel movement and
    /// movement that stops short never fire.
    #[test]
    fn line_crossing_directions() {
        let a = [0.5, 0.0];
        let b = [0.5, 1.0];
        assert_eq!(
            line_crossing([0.3, 0.5], [0.7, 0.5], a, b),
            Some("cross_ab")
        );
        assert_eq!(
            line_crossing([0.7, 0.5], [0.3, 0.5], a, b),
            Some("cross_ba")
        );
        assert_eq!(
            line_crossing([0.3, 0.2], [0.4, 0.8], a, b),
            None,
            "stops short"
        );
        assert_eq!(
            line_crossing([0.6, 0.2], [0.8, 0.8], a, b),
            None,
            "same side"
        );
        // Movement crossing the line's EXTENSION (beyond B) does not fire.
        assert_eq!(line_crossing([0.3, 1.5], [0.7, 1.5], a, b), None);
        // Jitter exactly on the line does not fire (proper intersection only).
        assert_eq!(line_crossing([0.5, 0.4], [0.5, 0.6], a, b), None);
    }

    #[test]
    fn line_direction_filter_parses_config() {
        assert_eq!(line_direction_filter(&zone_with_config(json!({}))), "any");
        assert_eq!(
            line_direction_filter(&zone_with_config(json!({"direction": "ab"}))),
            "ab"
        );
        assert_eq!(
            line_direction_filter(&zone_with_config(json!({"direction": "sideways"}))),
            "any"
        );
    }

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
