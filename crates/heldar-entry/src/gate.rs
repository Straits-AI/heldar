//! Barrier/gate actuation (issue #44): close the loop from a `matched` entry decision to a physical
//! barrier opening, by pulsing the alarm/relay output most ANPR barrier cameras wire to the boom.
//!
//! Policy is per-camera and dashboard-managed (`gate_policies`); a single global kill-switch
//! (`gate_settings.kill_switch`) halts ALL actuation, auto and manual. The actual device write is
//! the kernel's [`heldar_kernel::services::camera_control::pulse_output_with`] primitive.
//!
//! Safety posture:
//! - Actuation NEVER blocks or fails event recording — the ANPR engine spawns [`GateActuator::
//!   auto_open`] fire-and-forget after the entry event is committed.
//! - Every actuation (auto or manual) is written to the kernel event log (`gate_opened` /
//!   `gate_open_failed`); manual opens are additionally audited with the acting principal by the
//!   route layer.
//! - Failures surface as `gate_open_failed` warning events (webhook/email subscribable); there is
//!   no retry queue — a late pulse at a gate is worse than no pulse.

use std::sync::Arc;

use serde_json::json;
use sqlx::SqlitePool;

use heldar_kernel::config::Config;
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::repo;
use heldar_kernel::services::camera_control;

/// A camera's gate-actuation policy row.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct GatePolicy {
    pub camera_id: String,
    /// Auto-open on `matched` entry events (manual guard-open works whenever a policy row exists).
    pub enabled: bool,
    pub output_port: i64,
    pub pulse_ms: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Drives gate relays from entry decisions. Cheap to clone (pool/client handles + Arc).
#[derive(Clone)]
pub struct GateActuator {
    pool: SqlitePool,
    http: heldar_kernel::reqwest::Client,
    cfg: Arc<Config>,
}

impl GateActuator {
    pub fn new(pool: SqlitePool, http: heldar_kernel::reqwest::Client, cfg: Arc<Config>) -> Self {
        Self { pool, http, cfg }
    }

    /// The global kill-switch state.
    pub async fn kill_switch(pool: &SqlitePool) -> bool {
        sqlx::query_scalar::<_, bool>("SELECT kill_switch FROM gate_settings WHERE id = 1")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    /// The policy row for a camera, if configured.
    pub async fn policy(pool: &SqlitePool, camera_id: &str) -> Option<GatePolicy> {
        sqlx::query_as::<_, GatePolicy>("SELECT * FROM gate_policies WHERE camera_id = ?")
            .bind(camera_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
    }

    /// Auto-actuation hook, called by the ANPR engine AFTER an entry event is committed. Opens the
    /// barrier only for `auth_status = "matched"` on a camera whose policy is enabled, and only
    /// while the kill-switch is off. All failures are logged/evented, never propagated.
    pub async fn auto_open(&self, camera_id: &str, entry_event_id: &str, auth_status: &str) {
        self.auto_open_with(
            camera_id,
            entry_event_id,
            auth_status,
            serde_json::Value::Null,
        )
        .await;
    }

    /// [`Self::auto_open`] plus the PROVENANCE of the reads that produced the decision.
    ///
    /// This is the first time a barrier opening is attributable to a credential: the `gate_opened`
    /// event now carries which producer's reads voted it open (`kernel:native_anpr`, or the api key id
    /// of the worker), and how many votes there were. An incident responder asking "who opened lane 1
    /// at 03:14?" previously had `{"mode":"auto"}` and nothing else.
    ///
    /// The actuation POLICY checks above are deliberately untouched — provenance is recorded, never
    /// consulted, so nothing about which reads open a barrier changes here.
    pub async fn auto_open_with(
        &self,
        camera_id: &str,
        entry_event_id: &str,
        auth_status: &str,
        provenance: serde_json::Value,
    ) {
        if auth_status != "matched" {
            return;
        }
        let Some(policy) = Self::policy(&self.pool, camera_id).await else {
            return;
        };
        if !policy.enabled {
            return;
        }
        if Self::kill_switch(&self.pool).await {
            tracing::info!(%camera_id, %entry_event_id, "gate: kill-switch on; auto-open suppressed");
            return;
        }
        let outcome = self
            .pulse(camera_id, policy.output_port, policy.pulse_ms)
            .await;
        self.log_actuation(
            camera_id,
            entry_event_id,
            "auto",
            &policy,
            outcome,
            provenance,
        )
        .await;
    }

    /// Manual guard-open: same pulse + event trail, but bypasses the `enabled` (auto) flag — a
    /// configured policy row is still required (it carries the port/pulse), and the kill-switch
    /// still wins. Returns the pulse width for the API response.
    pub async fn manual_open(&self, camera_id: &str, principal_id: &str) -> AppResult<u64> {
        let policy = Self::policy(&self.pool, camera_id).await.ok_or_else(|| {
            AppError::BadRequest(
                "no gate policy configured for this camera; set its output port first".into(),
            )
        })?;
        if Self::kill_switch(&self.pool).await {
            return Err(AppError::BadRequest(
                "the gate kill-switch is on; all actuation is disabled".into(),
            ));
        }
        let outcome = self
            .pulse(camera_id, policy.output_port, policy.pulse_ms)
            .await;
        // Surface the device's failure reason to the operator (a 500 would render as a generic
        // "internal error", hiding e.g. "Invalid Operation" from a camera with no relay port —
        // observed live on a DS-2CD3T56WDV3-L, which has none).
        let res = match &outcome {
            Ok(ms) => Ok(*ms),
            Err(e) => Err(AppError::BadRequest(format!("gate open failed: {e}"))),
        };
        // A manual open is already attributable — the acting principal is the `reference`, and the
        // route layer audits it separately.
        self.log_actuation(
            camera_id,
            principal_id,
            "manual",
            &policy,
            outcome,
            json!({ "source": "operator", "principal": principal_id }),
        )
        .await;
        res
    }

    async fn pulse(&self, camera_id: &str, port: i64, pulse_ms: i64) -> AppResult<u64> {
        camera_control::pulse_output_with(
            &self.pool,
            &self.http,
            &self.cfg,
            camera_id,
            port,
            pulse_ms.max(0) as u64,
        )
        .await
    }

    /// Write the actuation outcome to the kernel event log (the alert notifier / webhooks see it).
    ///
    /// `provenance` is the server-authored trail of what drove the decision — carried through so the
    /// `gate_opened` record answers "which credential's reads opened this barrier?" without a join.
    async fn log_actuation(
        &self,
        camera_id: &str,
        reference: &str,
        mode: &str,
        policy: &GatePolicy,
        outcome: AppResult<u64>,
        provenance: serde_json::Value,
    ) {
        match outcome {
            Ok(ms) => {
                tracing::info!(%camera_id, mode, port = policy.output_port, pulse_ms = ms, provenance = %provenance, "gate: opened");
                let _ = repo::log_event(
                    &self.pool,
                    Some(camera_id),
                    "gate_opened",
                    "info",
                    json!({ "mode": mode, "reference": reference, "port": policy.output_port, "pulse_ms": ms, "provenance": provenance }),
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(%camera_id, mode, error = %e, "gate: open FAILED");
                let _ = repo::log_event(
                    &self.pool,
                    Some(camera_id),
                    "gate_open_failed",
                    "warning",
                    json!({ "mode": mode, "reference": reference, "port": policy.output_port, "error": e.to_string(), "provenance": provenance }),
                )
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();
        crate::schema::init(&pool).await.unwrap();
        pool
    }

    fn actuator(pool: &SqlitePool) -> GateActuator {
        GateActuator::new(
            pool.clone(),
            heldar_kernel::reqwest::Client::new(),
            Arc::new(heldar_kernel::config::Config::from_env()),
        )
    }

    #[tokio::test]
    async fn kill_switch_defaults_off_and_flips() {
        let pool = test_pool().await;
        assert!(!GateActuator::kill_switch(&pool).await);
        sqlx::query("UPDATE gate_settings SET kill_switch = 1 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert!(GateActuator::kill_switch(&pool).await);
    }

    #[tokio::test]
    async fn manual_open_requires_a_policy_and_respects_kill_switch() {
        let pool = test_pool().await;
        let act = actuator(&pool);

        // No policy row → BadRequest, and nothing was attempted against a device.
        let err = act.manual_open("lane-1", "guard-a").await.unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)), "got {err:?}");

        // With a policy but the kill-switch ON → refused before any device I/O.
        sqlx::query(
            "INSERT INTO gate_policies (camera_id, enabled, output_port, pulse_ms, updated_at)
             VALUES ('lane-1', 1, 1, 500, ?)",
        )
        .bind(chrono::Utc::now())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE gate_settings SET kill_switch = 1 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let err = act.manual_open("lane-1", "guard-a").await.unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("kill-switch"), "message: {m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    /// auto_open is a no-op (no device I/O, no event rows) for non-matched statuses, for cameras
    /// without a policy, and for disabled policies — the paths a mis-ANPR must never actuate on.
    #[tokio::test]
    async fn auto_open_gates_on_status_policy_and_enabled() {
        let pool = test_pool().await;
        let act = actuator(&pool);

        // Unknown camera / no policy: returns silently.
        act.auto_open("lane-1", "evt-1", "matched").await;
        // Non-matched statuses never actuate, even with an enabled policy.
        sqlx::query(
            "INSERT INTO gate_policies (camera_id, enabled, output_port, pulse_ms, updated_at)
             VALUES ('lane-1', 0, 1, 500, ?)",
        )
        .bind(chrono::Utc::now())
        .execute(&pool)
        .await
        .unwrap();
        for status in ["exception", "unmatched", "blocked"] {
            act.auto_open("lane-1", "evt-2", status).await;
        }
        // Disabled policy + matched: still a no-op.
        act.auto_open("lane-1", "evt-3", "matched").await;

        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type LIKE 'gate_%'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(events, 0, "no actuation events for gated-off paths");
    }
}
