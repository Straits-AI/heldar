//! ANPR entry engine (Stage 4): consolidates per-frame plate reads from the AI worker into one
//! authoritative entry/exit event per vehicle track via temporal voting, resolves the plate against
//! the registered-vehicle / visitor-pass / watchlist registry, classifies authorization
//! (matched / exception / unmatched / blocked), and writes a canonical entry event with
//! an evidence frame.
//!
//! Like the zone engine, all timing is driven by SERVER time and state is keyed per (camera, track)
//! in memory. A track commits once its WINNING plate (plausible-preferred, majority vote) has
//! accumulated `min_votes` reads, or on TTL prune if a vehicle passed too quickly to reach the
//! threshold but did produce at least one plate read. Plate is the PRIMARY identity
//! anchor; vehicle attributes (type/color/make/model) are SECONDARY — an attribute mismatch against
//! a registered plate raises an *exception for guard review*, never an automatic rejection
//! (by policy: no hard access decision on make/model without local benchmarking).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use uuid::Uuid;

use heldar_kernel::config::Config;
use heldar_kernel::models::DetectionIngest;
use heldar_kernel::repo;

/// Retain a track's voting state this long (server time) after it was last seen before pruning.
const STATE_TTL_SECS: i64 = 30;

/// Normalize a plate string to its lookup key: ASCII alphanumerics only, uppercased.
pub fn normalize_plate(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Loose plausibility gate for a normalized plate (Malaysian plates mix letters and digits, 3–10
/// chars). Used to flag likely-unreadable OCR rather than to hard-reject — guards review the rest.
pub fn is_plausible_plate(norm: &str) -> bool {
    let len = norm.len();
    if !(3..=10).contains(&len) {
        return false;
    }
    let has_alpha = norm.bytes().any(|b| b.is_ascii_alphabetic());
    let has_digit = norm.bytes().any(|b| b.is_ascii_digit());
    has_alpha && has_digit
}

#[derive(Default, Clone)]
struct PlateVote {
    count: u32,
    conf_sum: f64,
}

#[derive(Default, Clone)]
struct ObservedAttrs {
    vehicle_type: Option<(String, f64)>,
    color: Option<(String, f64)>,
    make: Option<(String, f64)>,
    model: Option<(String, f64)>,
}

impl ObservedAttrs {
    /// Keep the highest-confidence observation for each attribute.
    fn observe(slot: &mut Option<(String, f64)>, val: Option<&str>, conf: f64) {
        if let Some(v) = val.filter(|s| !s.trim().is_empty()) {
            let better = slot.as_ref().map(|(_, c)| conf > *c).unwrap_or(true);
            if better {
                *slot = Some((v.trim().to_string(), conf));
            }
        }
    }
}

struct TrackVoteState {
    camera_id: String,
    site_id: Option<String>,
    track: Option<String>,
    direction: String,
    votes: HashMap<String, PlateVote>,
    raw_by_norm: HashMap<String, String>,
    attrs: ObservedAttrs,
    last_seen: DateTime<Utc>,
    committed: bool,
    model_versions: Value,
    /// Server-authored `attributes.source` of the most recent read (`camera_native` / `worker`).
    source: String,
    /// Server-authored producer identity from `attributes._prov` — the credential (or kernel
    /// producer) answerable for these reads. Written into the entry event's audit blob, replacing the
    /// old hard-coded `"system"`.
    produced_by: String,
    /// Unique per instance (see [`AnprEngine::next_uid`]); distinguishes a track from a successor
    /// that reused the same map key.
    uid: u64,
}

/// A consolidated track ready to resolve + emit (built under the lock, processed after release).
struct CommitJob {
    /// State-map key, so a failed insert can clear `committed` and let the track retry.
    key: String,
    /// Identity of the track-state this job was built from. A failed insert clears `committed` only
    /// if the live state entry STILL has this uid (else the key was reused by a different vehicle).
    uid: u64,
    camera_id: String,
    site_id: Option<String>,
    track: Option<String>,
    direction: String,
    plate_norm: String,
    plate_raw: String,
    plate_conf: f64,
    vehicle_type: Option<String>,
    color: Option<String>,
    make: Option<String>,
    model: Option<String>,
    model_versions: Value,
    /// Votes the winning plate actually accumulated.
    votes: u32,
    /// Whether this commit may ACTUATE the barrier.
    ///
    /// False for a commit-on-prune below the vote threshold. The entry event is still written — the
    /// audit record is the entire point of commit-on-prune — but it is marked for review and the gate
    /// is not touched. Without this, a single accepted plate read still opened the barrier ~30 s later
    /// when the track was pruned, so hardening the vote path alone left the gate capability intact.
    actuate: bool,
    source: String,
    produced_by: String,
}

pub struct AnprEngine {
    pool: SqlitePool,
    /// Kernel config — used for media paths (evidence frames).
    cfg: Arc<Config>,
    /// Entry-app config — voting threshold.
    ecfg: Arc<crate::config::EntryConfig>,
    /// Barrier actuator (issue #44): fired-and-forgotten after a committed `matched` event so a
    /// slow/unreachable relay can never stall or fail event recording.
    gate: crate::gate::GateActuator,
    state: Mutex<HashMap<String, TrackVoteState>>,
    /// Monotonic id stamped on each track-state instance. Lets a failed commit clear `committed`
    /// only on the SAME track it committed — never on a successor that reused the (track-id) key.
    next_uid: std::sync::atomic::AtomicU64,
}

fn attr_str<'a>(attrs: &'a Value, key: &str) -> Option<&'a str> {
    attrs.get(key).and_then(|v| v.as_str())
}

#[async_trait::async_trait]
impl heldar_kernel::services::consumer::DetectionConsumer for AnprEngine {
    fn name(&self) -> &'static str {
        "anpr"
    }
    /// Only the ANPR task feeds the entry engine.
    fn interested_in(&self, task_type: &str) -> bool {
        task_type.eq_ignore_ascii_case("anpr")
    }
    async fn consume(&self, batch: &heldar_kernel::services::consumer::DetectionBatch<'_>) {
        self.process(batch.camera_id, batch.site_id, batch.detections)
            .await;
    }
}

impl AnprEngine {
    pub fn new(
        pool: SqlitePool,
        cfg: Arc<Config>,
        ecfg: Arc<crate::config::EntryConfig>,
        http: heldar_kernel::reqwest::Client,
    ) -> Arc<Self> {
        let gate = crate::gate::GateActuator::new(pool.clone(), http, cfg.clone());
        Arc::new(Self {
            pool,
            cfg,
            ecfg,
            gate,
            state: Mutex::new(HashMap::new()),
            next_uid: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Feed a batch of ANPR detections for a camera. Each detection carries vehicle + plate fields
    /// in `attributes` (plate, plate_confidence, vehicle_type, color, make, model, direction,
    /// model_versions). Commits tracks that reach the vote threshold and prunes/commits stale ones.
    pub async fn process(
        &self,
        camera_id: &str,
        site_id: Option<&str>,
        detections: &[DetectionIngest],
    ) {
        let now = Utc::now();
        let min_votes = self.ecfg.anpr_min_votes;
        let mut jobs: Vec<CommitJob> = Vec::new();
        {
            let mut state = self.state.lock().await;
            // ONE VOTE PER FRAME. A batch is ONE frame, so it may contribute at most one vote per
            // (track, plate) — otherwise a caller reaching `min_votes` needed only to repeat the same
            // detection N times in a single body, which the vote loop below happily counted because it
            // runs over every element before the commit scan. Combined with server-derived frame ids,
            // reaching the threshold now requires `min_votes` distinct tickets, i.e. `min_votes`
            // distinct sampled frames off that physical camera.
            let mut voted_this_frame: HashSet<(String, String)> = HashSet::new();
            for d in detections {
                let attrs = match d.attributes.as_ref() {
                    Some(a) if a.is_object() => a,
                    _ => continue,
                };
                let plate_raw = attr_str(attrs, "plate").map(|s| s.to_string());
                let plate_norm = plate_raw
                    .as_deref()
                    .map(normalize_plate)
                    .unwrap_or_default();
                let plate_conf = attrs
                    .get("plate_confidence")
                    .and_then(|v| v.as_f64())
                    .filter(|c| c.is_finite())
                    .unwrap_or(0.0);

                // Key per (camera, track). Without a track id, fall back to the plate so repeated
                // reads of the same plate still consolidate (and dedupe) within the TTL window.
                let track = d.track_id.clone();
                let sub = track
                    .clone()
                    .unwrap_or_else(|| format!("plate:{plate_norm}"));
                if track.is_none() && plate_norm.is_empty() {
                    continue; // nothing to key on
                }
                let key = format!("{camera_id}|{sub}");
                let already_voted = !plate_norm.is_empty()
                    && !voted_this_frame.insert((key.clone(), plate_norm.clone()));
                let entry = state.entry(key).or_insert_with(|| TrackVoteState {
                    camera_id: camera_id.to_string(),
                    site_id: site_id.map(|s| s.to_string()),
                    track: track.clone(),
                    direction: "unknown".into(),
                    votes: HashMap::new(),
                    raw_by_norm: HashMap::new(),
                    attrs: ObservedAttrs::default(),
                    last_seen: now,
                    committed: false,
                    model_versions: json!({}),
                    source: "worker".into(),
                    produced_by: "system".into(),
                    uid: self
                        .next_uid
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                });
                entry.last_seen = now;
                // `source` and `_prov` are written by the kernel's ingest path from the batch's
                // Provenance — a client cannot set either, so what is read here is trustworthy.
                if let Some(src) = attr_str(attrs, "source") {
                    entry.source = src.to_string();
                }
                if let Some(p) = attrs.get("_prov") {
                    entry.produced_by = producer_label(p);
                }

                if !plate_norm.is_empty() && !already_voted {
                    // Weighted voting: a read from a camera's ON-BOARD ANPR engine
                    // (`source = "camera_native"`, see the kernel's native_anpr poller) is
                    // authoritative — the device already consolidated multiple frames itself —
                    // so one read satisfies the vote threshold. Worker OCR reads stay at 1.
                    let weight = if attr_str(attrs, "source") == Some("camera_native") {
                        min_votes.max(1)
                    } else {
                        1
                    };
                    let v = entry.votes.entry(plate_norm.clone()).or_default();
                    v.count += weight;
                    v.conf_sum += plate_conf.max(0.0) * f64::from(weight);
                    if let Some(raw) = &plate_raw {
                        entry.raw_by_norm.insert(plate_norm.clone(), raw.clone());
                    }
                }
                let vc = d.confidence.unwrap_or(plate_conf).max(0.0);
                ObservedAttrs::observe(
                    &mut entry.attrs.vehicle_type,
                    attr_str(attrs, "vehicle_type").or(d.label.as_deref()),
                    vc,
                );
                ObservedAttrs::observe(&mut entry.attrs.color, attr_str(attrs, "color"), vc);
                ObservedAttrs::observe(&mut entry.attrs.make, attr_str(attrs, "make"), vc);
                ObservedAttrs::observe(&mut entry.attrs.model, attr_str(attrs, "model"), vc);
                if let Some(dir) = attr_str(attrs, "direction") {
                    if matches!(dir, "inbound" | "outbound") {
                        entry.direction = dir.to_string();
                    }
                }
                if let Some(mv) = attrs.get("model_versions") {
                    if mv.is_object() {
                        entry.model_versions = mv.clone();
                    }
                }
            }

            // Commit tracks whose WINNING plate has reached the vote threshold (temporal voting on
            // the plate itself — not the raw detection count, which would let a single noisy read or
            // a plateless track trip the gate).
            for (key, st) in state.iter_mut() {
                if st.committed {
                    continue;
                }
                if let Some((_, count, _)) = winning_plate(&st.votes) {
                    if count >= min_votes {
                        // Threshold reached: this commit MAY actuate.
                        if let Some(job) = build_job(key, st, true) {
                            jobs.push(job);
                        }
                        st.committed = true;
                    }
                }
            }

            // Prune stale tracks; commit-on-prune for vehicles that passed too quickly to reach the
            // threshold but DID produce at least one plate read. Tracks that never yielded any plate
            // (pure vehicle detections) are dropped silently so the entry log is not flooded with
            // "unmatched" events for every transient background vehicle.
            let cutoff = now - Duration::seconds(STATE_TTL_SECS);
            let mut survivors: HashMap<String, TrackVoteState> = HashMap::new();
            for (k, st) in state.drain() {
                if st.last_seen >= cutoff {
                    survivors.insert(k, st);
                } else if !st.committed && winning_plate(&st.votes).is_some() {
                    // Commit-on-prune: record the event, but only ACTUATE if the plate actually
                    // reached the threshold. A vehicle that passed too fast to be voted through is a
                    // guard-review item, not a barrier open.
                    let reached = winning_plate(&st.votes)
                        .map(|(_, c, _)| c >= min_votes)
                        .unwrap_or(false);
                    if let Some(job) = build_job(&k, &st, reached) {
                        jobs.push(job);
                    }
                }
            }
            *state = survivors;
        }

        for job in jobs {
            let (key, uid) = (job.key.clone(), job.uid);
            // If the insert fails, clear `committed` so a still-live track retries next batch
            // instead of silently dropping the event — but ONLY if the live state entry is still the
            // same track (matching uid). A concurrent batch may have pruned it and a reused track-id
            // key now points to a different vehicle; clearing that one would duplicate its event.
            if !self.commit(job, now).await {
                let mut state = self.state.lock().await;
                if let Some(s) = state.get_mut(&key) {
                    if s.uid == uid {
                        s.committed = false;
                    }
                }
            }
        }
    }

    /// Returns true on a successful entry-event insert.
    async fn commit(&self, job: CommitJob, now: DateTime<Utc>) -> bool {
        let mut resolution = self.resolve(&job).await;
        // A commit that may not actuate is, by definition, a guard-review item: force the workflow
        // status regardless of how the identity resolved, so the dashboard surfaces it as one.
        if !job.actuate {
            resolution.workflow_status = "review".into();
        }
        let id = format!("evt_{}", Uuid::new_v4().simple());
        let evidence_path = self.copy_evidence(&job.camera_id, &id).await;

        let event_type = if job.direction == "outbound" {
            "vehicle_exit"
        } else {
            "vehicle_entry"
        };
        let make_model = match (&job.make, &job.model) {
            (Some(mk), Some(md)) => Some(format!("{mk} {md}")),
            (Some(mk), None) => Some(mk.clone()),
            (None, Some(md)) => Some(md.clone()),
            (None, None) => None,
        };
        let plate_out = (!job.plate_norm.is_empty()).then(|| job.plate_raw.clone());
        let subject = json!({
            "type": "vehicle",
            "plate": plate_out,
            "plate_confidence": job.plate_conf,
            "plate_valid": is_plausible_plate(&job.plate_norm),
            "vehicle_type": job.vehicle_type,
            "color": job.color,
            "make_model": make_model,
        });
        let evidence = json!({ "snapshot_path": evidence_path });
        let workflow = json!({ "status": resolution.workflow_status });
        // `created_by` was hard-coded "system", which was true of nothing: the reads came from a
        // worker credential or the camera's own engine. It is now the server-authored producer, and
        // the evidence block records what actually carried the decision.
        let audit = json!({
            "created_by": job.produced_by,
            "model_versions": job.model_versions,
            "evidence": {
                "votes": job.votes,
                "min_votes": self.ecfg.anpr_min_votes,
                "source": job.source,
                "key": job.produced_by,
                "actuated": job.actuate,
            },
        });
        let plate_db = (!job.plate_norm.is_empty()).then(|| job.plate_norm.clone());

        let res = sqlx::query(
            "INSERT INTO entry_events
               (id, site_id, camera_id, event_type, timestamp, direction, plate, plate_confidence,
                subject, authorization, auth_status, evidence, workflow_status, workflow, audit,
                track_id, created_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&id)
        .bind(&job.site_id)
        .bind(&job.camera_id)
        .bind(event_type)
        .bind(now)
        .bind(&job.direction)
        .bind(&plate_db)
        .bind(job.plate_conf)
        .bind(sqlx::types::Json(&subject))
        .bind(sqlx::types::Json(&resolution.authorization))
        .bind(&resolution.auth_status)
        .bind(sqlx::types::Json(&evidence))
        .bind(&resolution.workflow_status)
        .bind(sqlx::types::Json(&workflow))
        .bind(sqlx::types::Json(&audit))
        .bind(&job.track)
        .bind(now)
        .execute(&self.pool)
        .await;
        if let Err(e) = res {
            tracing::error!(error = %e, "anpr: failed to insert entry event");
            return false;
        }

        // Mirror into the generic event log so the alert notifier + metrics see exceptions/blocks.
        let _ = repo::log_event(
            &self.pool,
            Some(&job.camera_id),
            &format!("entry_{}", resolution.auth_status),
            &resolution.severity,
            json!({
                "entry_event_id": id,
                "plate": plate_db,
                "auth_status": resolution.auth_status,
                "source": resolution.source,
                "event_type": event_type,
                "evidence": evidence_path,
            }),
        )
        .await;

        tracing::info!(
            camera_id = %job.camera_id,
            plate = %job.plate_norm,
            auth = %resolution.auth_status,
            source = %resolution.source,
            "entry event"
        );

        // A below-threshold commit-on-prune NEVER actuates. The audit row above is written (that is
        // the whole point of commit-on-prune), and an operator-visible event says so, but the barrier
        // is not touched — the vote threshold is the gate's authorization, and 30 seconds later is
        // not a second chance at it.
        if !job.actuate {
            let _ = repo::log_event(
                &self.pool,
                Some(&job.camera_id),
                "gate_review_not_actuated",
                "warning",
                json!({
                    "entry_event_id": id,
                    "plate": plate_db,
                    "votes": job.votes,
                    "min_votes": self.ecfg.anpr_min_votes,
                    "source": job.source,
                    "produced_by": job.produced_by,
                    "auth_status": resolution.auth_status,
                    "reason": "below_vote_threshold_on_track_prune",
                }),
            )
            .await;
            return true;
        }

        // Barrier actuation (issue #44): fire-and-forget AFTER the event is durably recorded, so a
        // slow or unreachable relay never blocks the consumer or fails the entry event. The
        // actuator itself re-checks policy/kill-switch and logs the outcome.
        {
            let gate = self.gate.clone();
            let camera_id = job.camera_id.clone();
            let auth_status = resolution.auth_status.clone();
            let event_id = id.clone();
            let provenance = json!({
                "votes": job.votes,
                "min_votes": self.ecfg.anpr_min_votes,
                "source": job.source,
                "produced_by": job.produced_by,
            });
            tokio::spawn(async move {
                gate.auto_open_with(&camera_id, &event_id, &auth_status, provenance)
                    .await;
            });
        }
        true
    }

    /// Identity resolver: classify a consolidated plate against the registry. Precedence:
    /// active block-watchlist (security) → registered vehicle (attribute check) → active visitor
    /// pass → alert/vip watchlist → unmatched. Mutates a matched pass to checked_in on entry.
    async fn resolve(&self, job: &CommitJob) -> Resolution {
        let plate = &job.plate_norm;
        let now = Utc::now();

        // Unreadable plate: nothing to look up — emit for guard review.
        if plate.is_empty() || !is_plausible_plate(plate) {
            return Resolution::unmatched(json!({
                "status": "unmatched",
                "source": "none",
                "note": if plate.is_empty() { "no_plate_read" } else { "plate_unreadable" },
            }));
        }

        // 1) Block watchlist wins outright. This is the only security-critical lookup, so it must
        //    FAIL CLOSED: a DB error here must not silently fall through to an "allow" branch — flag
        //    the event for guard review instead.
        match sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT reason, severity FROM watchlist WHERE plate_norm = ? AND active = 1 AND kind = 'block'",
        )
        .bind(plate)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some((reason, severity))) => {
                return Resolution {
                    auth_status: "blocked".into(),
                    workflow_status: "pending".into(),
                    severity: if severity.is_empty() {
                        "critical".into()
                    } else {
                        severity
                    },
                    source: "watchlist".into(),
                    authorization: json!({
                        "status": "blocked", "source": "watchlist", "kind": "block", "reason": reason,
                    }),
                };
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, plate = %plate, "anpr: block-watchlist lookup failed; failing closed to exception");
                return Resolution {
                    auth_status: "exception".into(),
                    workflow_status: "pending".into(),
                    severity: "warning".into(),
                    source: "system".into(),
                    authorization: json!({
                        "status": "exception", "source": "system", "note": "watchlist_lookup_failed",
                    }),
                };
            }
        }

        // 2) Registered vehicle (within validity window, if set). We deliberately only compare
        //    color + vehicle_type for mismatch — make/model is assistive metadata only and not
        //    reliable enough to drive an exception.
        let vehicle = sqlx::query_as::<_, RegVehicle>(
            "SELECT id, vehicle_type, color, valid_from, valid_until
               FROM vehicles WHERE plate_norm = ? AND active = 1",
        )
        .bind(plate)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        if let Some(v) = vehicle {
            let in_window = v.valid_from.map(|t| t <= now).unwrap_or(true)
                && v.valid_until.map(|t| t >= now).unwrap_or(true);
            if !in_window {
                return Resolution {
                    auth_status: "exception".into(),
                    workflow_status: "pending".into(),
                    severity: "warning".into(),
                    source: "registered_vehicle".into(),
                    authorization: json!({
                        "status": "exception", "source": "registered_vehicle",
                        "vehicle_id": v.id, "note": "outside_validity_window",
                    }),
                };
            }
            // Secondary verification: attribute mismatch → exception (never an auto-reject).
            let mut mismatches: Vec<String> = Vec::new();
            check_mismatch(
                &mut mismatches,
                "color",
                v.color.as_deref(),
                job.color.as_deref(),
            );
            check_mismatch(
                &mut mismatches,
                "vehicle_type",
                v.vehicle_type.as_deref(),
                job.vehicle_type.as_deref(),
            );
            if mismatches.is_empty() {
                // An alert listing downgrades a clean match to a review exception; keep the
                // denormalized column and the embedded authorization JSON in lock-step.
                let alert = self.has_alert(plate).await;
                let status = if alert { "exception" } else { "matched" };
                return Resolution {
                    auth_status: status.into(),
                    workflow_status: if alert {
                        "pending".into()
                    } else {
                        "auto".into()
                    },
                    severity: if alert {
                        "warning".into()
                    } else {
                        "info".into()
                    },
                    source: "registered_vehicle".into(),
                    authorization: json!({
                        "status": status, "source": "registered_vehicle",
                        "vehicle_id": v.id, "alert": alert,
                    }),
                };
            } else {
                return Resolution {
                    auth_status: "exception".into(),
                    workflow_status: "pending".into(),
                    severity: "warning".into(),
                    source: "registered_vehicle".into(),
                    authorization: json!({
                        "status": "exception", "source": "registered_vehicle",
                        "vehicle_id": v.id, "mismatches": mismatches,
                    }),
                };
            }
        }

        // 3) Active visitor pass that is CURRENTLY within its validity window. The window is filtered
        //    in SQL (a plate may have several active passes — e.g. a future-dated one with a later
        //    valid_until — and we must not let that mask a presently-valid pass).
        let pass = sqlx::query_as::<_, (String, String)>(
            "SELECT id, status FROM visitor_passes
              WHERE plate_norm = ? AND status IN ('active','checked_in')
                AND valid_from <= ? AND valid_until >= ?
              ORDER BY valid_until DESC LIMIT 1",
        )
        .bind(plate)
        .bind(now)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        if let Some((pass_id, status)) = pass {
            // Auto check-in on an inbound match.
            if status == "active" && job.direction != "outbound" {
                let _ = sqlx::query(
                    "UPDATE visitor_passes SET status='checked_in', checked_in_at=?, updated_at=? WHERE id=?",
                )
                .bind(now)
                .bind(now)
                .bind(&pass_id)
                .execute(&self.pool)
                .await;
            }
            let alert = self.has_alert(plate).await;
            let st = if alert { "exception" } else { "matched" };
            return Resolution {
                auth_status: st.into(),
                workflow_status: if alert {
                    "pending".into()
                } else {
                    "auto".into()
                },
                severity: if alert {
                    "warning".into()
                } else {
                    "info".into()
                },
                source: "visitor_pass".into(),
                authorization: json!({
                    "status": st, "source": "visitor_pass", "pass_id": pass_id, "alert": alert,
                }),
            };
        }
        // 3b) A pass exists for this plate but is outside its validity window → exception for review.
        if let Ok(Some((pass_id,))) = sqlx::query_as::<_, (String,)>(
            "SELECT id FROM visitor_passes WHERE plate_norm = ? AND status IN ('active','checked_in')
              ORDER BY valid_until DESC LIMIT 1",
        )
        .bind(plate)
        .fetch_optional(&self.pool)
        .await
        {
            return Resolution {
                auth_status: "exception".into(),
                workflow_status: "pending".into(),
                severity: "warning".into(),
                source: "visitor_pass".into(),
                authorization: json!({
                    "status": "exception", "source": "visitor_pass",
                    "pass_id": pass_id, "note": "pass_outside_validity_window",
                }),
            };
        }

        // 4) VIP watchlist (informational allow) — only reached when not registered/passed.
        if let Ok(Some((reason,))) = sqlx::query_as::<_, (Option<String>,)>(
            "SELECT reason FROM watchlist WHERE plate_norm = ? AND active = 1 AND kind = 'vip'",
        )
        .bind(plate)
        .fetch_optional(&self.pool)
        .await
        {
            return Resolution {
                auth_status: "matched".into(),
                workflow_status: "auto".into(),
                severity: "info".into(),
                source: "watchlist".into(),
                authorization: json!({
                    "status": "matched", "source": "watchlist", "kind": "vip", "reason": reason,
                }),
            };
        }

        // 5) Unknown plate. If alert-listed, escalate to exception; else simply unmatched.
        if self.has_alert(plate).await {
            return Resolution {
                auth_status: "exception".into(),
                workflow_status: "pending".into(),
                severity: "warning".into(),
                source: "watchlist".into(),
                authorization: json!({ "status": "exception", "source": "watchlist", "kind": "alert" }),
            };
        }
        Resolution::unmatched(json!({ "status": "unmatched", "source": "none" }))
    }

    /// True if the plate is on an active alert watchlist (flag-for-review without blocking).
    async fn has_alert(&self, plate: &str) -> bool {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM watchlist WHERE plate_norm = ? AND active = 1 AND kind = 'alert'",
        )
        .bind(plate)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
            > 0
    }

    /// Copy the latest sampled frame (prefer main stream) as evidence; return its served URL.
    async fn copy_evidence(&self, camera_id: &str, id: &str) -> Option<String> {
        let dir = self.cfg.camera_frames_dir(camera_id);
        let filename = format!("entryevt_{id}.jpg");
        let dst = self.cfg.snapshots_dir.join(&filename);
        for profile in ["main", "sub"] {
            let src = dir.join(format!("latest_{profile}.jpg"));
            if tokio::fs::copy(&src, &dst).await.is_ok() {
                return Some(format!("/media/snapshots/{filename}"));
            }
        }
        None
    }
}

#[derive(sqlx::FromRow)]
struct RegVehicle {
    id: String,
    vehicle_type: Option<String>,
    color: Option<String>,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
}

struct Resolution {
    auth_status: String,
    workflow_status: String,
    severity: String,
    source: String,
    authorization: Value,
}

impl Resolution {
    fn unmatched(authorization: Value) -> Self {
        Resolution {
            auth_status: "unmatched".into(),
            workflow_status: "pending".into(),
            severity: "warning".into(),
            source: "none".into(),
            authorization,
        }
    }
}

/// Record an attribute mismatch only when BOTH sides are known and differ (case-insensitive).
fn check_mismatch(
    out: &mut Vec<String>,
    field: &str,
    registered: Option<&str>,
    detected: Option<&str>,
) {
    if let (Some(r), Some(d)) = (registered, detected) {
        if !r.trim().is_empty() && !d.trim().is_empty() && !r.trim().eq_ignore_ascii_case(d.trim())
        {
            out.push(format!("{field}: registered={r}, detected={d}"));
        }
    }
}

/// Pick the winning plate for a track: most votes, tie-broken by summed confidence. Plausible plates
/// are preferred over implausible ones (so a noisy digits-only reading can't mask a real plate); the
/// overall vote leader is used only when no candidate is plausible. Returns (plate_norm, votes, avg_conf).
fn winning_plate(votes: &HashMap<String, PlateVote>) -> Option<(String, u32, f64)> {
    let leader = |plausible_only: bool| {
        votes
            .iter()
            .filter(|(norm, _)| !plausible_only || is_plausible_plate(norm))
            .max_by(|a, b| {
                a.1.count.cmp(&b.1.count).then(
                    a.1.conf_sum
                        .partial_cmp(&b.1.conf_sum)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
            })
    };
    let (norm, vote) = leader(true).or_else(|| leader(false))?;
    let avg = if vote.count > 0 {
        vote.conf_sum / vote.count as f64
    } else {
        0.0
    };
    Some((norm.clone(), vote.count, avg))
}

/// Flatten the server-authored `_prov` object into a single attributable label.
fn producer_label(prov: &Value) -> String {
    if let Some(p) = prov.get("producer").and_then(|v| v.as_str()) {
        return format!("kernel:{p}");
    }
    if let Some(k) = prov.get("key").and_then(|v| v.as_str()) {
        return format!("apikey:{k}");
    }
    "system".to_string()
}

fn build_job(key: &str, st: &TrackVoteState, actuate: bool) -> Option<CommitJob> {
    let (plate_norm, count, plate_conf) = winning_plate(&st.votes)?;
    let plate_raw = st
        .raw_by_norm
        .get(&plate_norm)
        .cloned()
        .unwrap_or_else(|| plate_norm.clone());
    Some(CommitJob {
        key: key.to_string(),
        uid: st.uid,
        camera_id: st.camera_id.clone(),
        site_id: st.site_id.clone(),
        track: st.track.clone(),
        direction: st.direction.clone(),
        plate_norm,
        plate_raw,
        plate_conf,
        vehicle_type: st.attrs.vehicle_type.as_ref().map(|(v, _)| v.clone()),
        color: st.attrs.color.as_ref().map(|(v, _)| v.clone()),
        make: st.attrs.make.as_ref().map(|(v, _)| v.clone()),
        model: st.attrs.model.as_ref().map(|(v, _)| v.clone()),
        model_versions: st.model_versions.clone(),
        votes: count,
        actuate,
        source: st.source.clone(),
        produced_by: st.produced_by.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn votes(pairs: &[(&str, u32, f64)]) -> HashMap<String, PlateVote> {
        pairs
            .iter()
            .map(|(p, c, s)| {
                (
                    p.to_string(),
                    PlateVote {
                        count: *c,
                        conf_sum: *s,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn normalize_strips_and_uppercases() {
        assert_eq!(normalize_plate("abc 1234"), "ABC1234");
        assert_eq!(normalize_plate("W-XY 88.88"), "WXY8888");
        assert_eq!(normalize_plate(""), "");
    }

    #[test]
    fn winning_plate_prefers_plausible_over_higher_voted_implausible() {
        // "12345" has more votes but is implausible; the plausible "ABC1234" must win.
        let v = votes(&[("12345", 5, 4.0), ("ABC1234", 2, 1.8)]);
        let (norm, count, _) = winning_plate(&v).unwrap();
        assert_eq!(norm, "ABC1234");
        assert_eq!(count, 2);
    }

    #[test]
    fn winning_plate_picks_top_votes_among_plausible() {
        let v = votes(&[("ABC1234", 2, 1.8), ("ABD1234", 5, 4.5)]);
        let (norm, count, _) = winning_plate(&v).unwrap();
        assert_eq!(norm, "ABD1234");
        assert_eq!(count, 5);
    }

    #[test]
    fn winning_plate_none_when_no_votes() {
        assert!(winning_plate(&HashMap::new()).is_none());
    }

    #[test]
    fn plausibility_requires_alpha_and_digit() {
        assert!(is_plausible_plate("ABC1234"));
        assert!(is_plausible_plate("WA12B"));
        assert!(!is_plausible_plate("1234"));
        assert!(!is_plausible_plate("ABCDE"));
        assert!(!is_plausible_plate("A1"));
        assert!(!is_plausible_plate("ABCDEFGHIJK1"));
    }

    async fn test_engine() -> (Arc<AnprEngine>, SqlitePool) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();
        crate::schema::init(&pool).await.unwrap();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,?,?,?)",
        )
        .bind("gate-cam")
        .bind("gate-cam")
        .bind(1)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let cfg = Arc::new(heldar_kernel::config::Config::from_env());
        // Default entry config: min_votes = 3, so a single worker read must NOT commit.
        let ecfg = Arc::new(crate::config::EntryConfig {
            anpr_min_votes: 3,
            entry_retention_days: 365,
        });
        let engine = AnprEngine::new(
            pool.clone(),
            cfg,
            ecfg,
            heldar_kernel::reqwest::Client::new(),
        );
        (engine, pool)
    }

    fn detection(attrs: Value) -> DetectionIngest {
        DetectionIngest {
            label: Some("vehicle".into()),
            confidence: Some(0.9),
            bbox: None,
            track_id: None,
            attributes: Some(attrs),
        }
    }

    /// Build the kernel state the ingest path needs, with THIS engine registered as the perception
    /// consumer — so a test can drive the real `ingest_batch` and get the real fan-out.
    async fn state_with_engine(
        pool: &SqlitePool,
        engine: Arc<AnprEngine>,
    ) -> heldar_kernel::state::AppState {
        use heldar_kernel::services::consumer::DetectionConsumer;
        let cfg = Arc::new(heldar_kernel::config::Config::from_env());
        let consumers: Vec<Arc<dyn DetectionConsumer>> = vec![engine];
        heldar_kernel::state::AppState {
            recorder: heldar_kernel::services::recorder::RecorderManager::new(
                pool.clone(),
                cfg.clone(),
            ),
            sampler: heldar_kernel::services::sampler::SamplerManager::new(
                pool.clone(),
                cfg.clone(),
            ),
            live: heldar_kernel::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                heldar_kernel::reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(consumers),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
            http: heldar_kernel::reqwest::Client::new(),
            started_at: Utc::now(),
            pool: pool.clone(),
            cfg,
        }
    }

    fn ingest_body(frame: &str, attrs: Value) -> heldar_kernel::models::AiIngest {
        heldar_kernel::models::AiIngest {
            camera_id: "gate-cam".into(),
            task_type: "anpr".into(),
            timestamp: None,
            frame_id: Some(frame.into()),
            detections: vec![detection(attrs)],
            event: None,
            frame_ticket: None,
        }
    }

    async fn entry_event_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM entry_events")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// THE NEGATIVE CONTROL, end to end. This test used to assert the vulnerability: it hand-built
    /// `attributes.source = "camera_native"` and expected a single read to commit. Now it drives the
    /// REAL ingest path, so the attribute is whatever the kernel authored — and a worker-provenance
    /// batch that FORGES `camera_native` gets no weight at all.
    #[tokio::test]
    async fn a_forged_camera_native_read_does_not_commit_but_the_real_one_does() {
        use heldar_kernel::models::{KernelProducer, Provenance};

        let (engine, pool) = test_engine().await;
        let st = state_with_engine(&pool, engine.clone()).await;

        // A credential posting `source: camera_native` over the API. One read, min_votes = 3.
        heldar_kernel::services::perception_ingest::ingest_batch(
            &st,
            &ingest_body(
                "f_forged",
                json!({
                    "plate": "ABC1234",
                    "plate_confidence": 0.8,
                    "source": "camera_native",
                    "_prov": { "producer": "native_anpr" },
                }),
            ),
            &Provenance::Worker {
                api_key_id: "key_attacker".into(),
                task_id: Some("ai_1".into()),
                worker_id: Some("w1".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            entry_event_count(&pool).await,
            0,
            "a forged camera_native read must carry NO extra weight — one worker read stays below \
             min_votes=3"
        );

        // ...and the persisted attribute says `worker`, not what the client asked for.
        let attrs: sqlx::types::Json<Value> =
            sqlx::query_scalar("SELECT attributes FROM detections WHERE frame_id = 'f_forged'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(attrs.0["source"], "worker");

        // THE POSITIVE CONTROL: the kernel's own native-ANPR producer is still authoritative, so a
        // single on-board read still commits immediately. Over-correcting breaks this.
        heldar_kernel::services::perception_ingest::ingest_batch(
            &st,
            &ingest_body(
                "f_native",
                json!({ "plate": "WXY8888", "direction": "inbound" }),
            ),
            &Provenance::Kernel {
                producer: KernelProducer::NativeAnpr,
            },
        )
        .await
        .unwrap();
        let rows: Vec<(Option<String>, String)> =
            sqlx::query_as("SELECT plate, auth_status FROM entry_events")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 1, "one genuine native read commits immediately");
        assert_eq!(rows[0].0.as_deref(), Some("WXY8888"));
        assert_eq!(rows[0].1, "unmatched", "unknown plate resolves unmatched");
    }

    /// ONE VOTE PER FRAME. A batch is one frame, so repeating the same plate inside it must not add
    /// up to the threshold. This passes trivially before the per-call dedupe is added — the vote loop
    /// runs over every element before the commit scan — which is exactly why it is worth pinning.
    #[tokio::test]
    async fn repeating_a_plate_within_one_batch_is_a_single_vote() {
        let (engine, pool) = test_engine().await;
        let same = || DetectionIngest {
            label: Some("vehicle".into()),
            confidence: Some(0.9),
            bbox: None,
            track_id: Some("trk-1".into()),
            attributes: Some(json!({ "plate": "ABC1234", "plate_confidence": 0.9 })),
        };
        engine
            .process("gate-cam", None, &[same(), same(), same()])
            .await;
        assert_eq!(
            entry_event_count(&pool).await,
            0,
            "three copies of one read in ONE batch must not reach min_votes=3"
        );

        // Three SEPARATE frames do reach it — the threshold still means what it says.
        for _ in 0..3 {
            engine.process("gate-cam", None, &[same()]).await;
        }
        assert_eq!(
            entry_event_count(&pool).await,
            1,
            "three distinct frames reach min_votes=3"
        );
    }

    /// PRUNE-COMMIT NEVER ACTUATES. A track that never reached the threshold is still RECORDED when
    /// it is pruned (the audit row is the point), but it is marked for review and the barrier is not
    /// touched. Without this, a single accepted read still opened the gate ~30 s later.
    #[tokio::test]
    async fn a_below_threshold_prune_records_for_review_and_never_actuates() {
        let (engine, pool) = test_engine().await;
        // Give the camera an ENABLED gate policy, so "no actuation" cannot be an artefact of there
        // being nothing to actuate.
        sqlx::query(
            "INSERT INTO gate_policies (camera_id, enabled, output_port, pulse_ms, updated_at)
             VALUES ('gate-cam', 1, 1, 500, ?)",
        )
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();

        // One read (below min_votes = 3), then age the track past STATE_TTL_SECS and poke the engine.
        engine
            .process(
                "gate-cam",
                None,
                &[detection(
                    json!({ "plate": "ABC1234", "plate_confidence": 0.8 }),
                )],
            )
            .await;
        {
            let mut state = engine.state.lock().await;
            for st in state.values_mut() {
                st.last_seen = Utc::now() - Duration::seconds(STATE_TTL_SECS + 5);
            }
        }
        engine.process("gate-cam", None, &[]).await;

        let (workflow_status, audit): (String, sqlx::types::Json<Value>) =
            sqlx::query_as("SELECT workflow_status, audit FROM entry_events")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            workflow_status, "review",
            "a below-threshold prune commit is a guard-review item"
        );
        assert_eq!(audit.0["evidence"]["actuated"], false);

        let reviewed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type = 'gate_review_not_actuated'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            reviewed, 1,
            "the operator gets an explicit not-actuated event"
        );

        let opened: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'gate_opened'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(opened, 0, "commit-on-prune must NEVER open the barrier");
    }

    /// The entry event names the producer that actually drove it, replacing the hard-coded "system".
    #[tokio::test]
    async fn the_audit_trail_names_the_producer_and_the_vote_count() {
        use heldar_kernel::models::{KernelProducer, Provenance};
        let (engine, pool) = test_engine().await;
        let st = state_with_engine(&pool, engine.clone()).await;

        heldar_kernel::services::perception_ingest::ingest_batch(
            &st,
            &ingest_body("f_a", json!({ "plate": "WXY8888" })),
            &Provenance::Kernel {
                producer: KernelProducer::NativeAnpr,
            },
        )
        .await
        .unwrap();

        let audit: sqlx::types::Json<Value> = sqlx::query_scalar("SELECT audit FROM entry_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(audit.0["created_by"], "kernel:native_anpr");
        assert_eq!(audit.0["evidence"]["source"], "camera_native");
        assert_eq!(audit.0["evidence"]["votes"], 3);
        assert_eq!(audit.0["evidence"]["actuated"], true);
    }

    #[test]
    fn producer_label_flattens_the_server_authored_prov_block() {
        assert_eq!(
            producer_label(&json!({ "producer": "native_anpr" })),
            "kernel:native_anpr"
        );
        assert_eq!(
            producer_label(&json!({ "key": "ak_1", "task": "ai_1" })),
            "apikey:ak_1"
        );
        assert_eq!(producer_label(&json!({})), "system");
    }

    #[test]
    fn mismatch_only_when_both_known_and_differ() {
        let mut m = Vec::new();
        check_mismatch(&mut m, "color", Some("white"), Some("black"));
        check_mismatch(&mut m, "color", Some("white"), Some("WHITE"));
        check_mismatch(&mut m, "vehicle_type", None, Some("suv"));
        check_mismatch(&mut m, "vehicle_type", Some("car"), None);
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("color"));
    }
}
