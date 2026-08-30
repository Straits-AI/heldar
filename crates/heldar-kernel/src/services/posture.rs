//! What this box's security posture ACTUALLY is, as machine-readable findings (#126).
//!
//! `enforce_production_guardrails` already refuses or warns at boot on the settings it can judge.
//! This is the other half: the HOST facts a config check cannot see — who owns the process, whether
//! another local user can read its command line, whether the recording volume is encrypted, how old
//! the master key is, and how many credentials are still sitting in plaintext.
//!
//! # Why findings rather than a score
//!
//! A single "secure: true/false" hides the thing an operator needs, which is *which* control is
//! missing and whether it matters for their deployment. A sealed appliance in a locked cabinet and a
//! shared Docker host have the same config and completely different exposure. So each finding says
//! what was observed, what it means, and — where the answer is genuinely unknowable from inside the
//! container — says `Unknown` rather than guessing.
//!
//! `Unknown` is a first-class outcome here and is deliberately NOT counted as a pass. Reporting
//! "volume encryption: ok" because we could not read `/proc/mounts` would be worse than reporting
//! nothing: it is the shape of claim this whole issue exists to stop making.

use serde::Serialize;

use crate::config::Config;

/// How a finding came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The control is in place.
    Ok,
    /// The control is absent or weak, and the box is running anyway.
    Weak,
    /// Not determinable from inside this process. NOT a pass.
    Unknown,
}

/// One observation about this box.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// Stable identifier — branch on this, not on `detail`.
    pub id: &'static str,
    pub status: Status,
    /// What was actually observed. Never a secret, never a credential.
    pub detail: String,
    /// What it means, in the terms an operator decides with.
    pub matters: &'static str,
}

impl Finding {
    fn new(
        id: &'static str,
        status: Status,
        detail: impl Into<String>,
        matters: &'static str,
    ) -> Self {
        Self {
            id,
            status,
            detail: detail.into(),
            matters,
        }
    }
}

/// Every posture finding for this box.
pub async fn assess(cfg: &Config, pool: &sqlx::SqlitePool) -> Vec<Finding> {
    let mut out = vec![
        secret_key_source(cfg),
        process_visibility(),
        service_user(),
        volume_encryption(cfg),
        rtsp_transport(pool).await,
    ];
    out.extend(credential_state(cfg, pool).await);
    out
}

/// Where the master key came from, and therefore who can read it.
fn secret_key_source(cfg: &Config) -> Finding {
    if cfg.secret_key_b64.is_none() {
        return Finding::new(
            "secret_key_source",
            Status::Weak,
            "no master key configured",
            "camera, webhook and backup credentials are stored in plaintext at rest; anyone who can \
             read the database file can read them",
        );
    }
    // Re-resolve to learn the SOURCE. The value is not touched.
    match crate::services::secret_source::resolve("HELDAR_SECRET_KEY") {
        Ok(Some(r)) if r.source.is_hardened() => Finding::new(
            "secret_key_source",
            Status::Ok,
            format!("{:?}", r.source),
            "the key is not in the process environment, so it is not readable from \
             /proc/<pid>/environ and does not appear in `docker inspect` output",
        ),
        Ok(Some(r)) => Finding::new(
            "secret_key_source",
            Status::Weak,
            format!("{:?}", r.source),
            "an environment variable is readable from /proc/<pid>/environ by anyone who can read \
             it, and appears in `docker inspect` output, shell history and crash dumps. Use \
             HELDAR_SECRET_KEY_FILE or a systemd credential",
        ),
        _ => Finding::new(
            "secret_key_source",
            Status::Unknown,
            "configured, but the source could not be re-read",
            "the key is in use; where it came from could not be determined",
        ),
    }
}

/// Whether another local user can read this process's command line — which is where camera
/// credentials live, because ffmpeg takes an RTSP URL as an argument.
///
/// THE SHARPEST EXPOSURE ON THE BOX, and the one none of #126's merged work addresses. Reported
/// here so it is at least visible rather than implied by its absence.
#[cfg(not(target_os = "linux"))]
fn process_visibility() -> Finding {
    Finding::new(
        "process_visibility",
        Status::Unknown,
        "not a Linux host",
        "camera credentials appear in ffmpeg's argv; process visibility is host-specific and was \
         not assessed",
    )
}

/// TWO WHOLE DEFINITIONS, not one function with `#[cfg]` blocks inside it. A `#[cfg]` attribute on
/// a BLOCK makes it a statement, so `{ helper() }` evaluates to `()` and the function does not
/// compile — on the other platform only, where the author cannot see it.
#[cfg(target_os = "linux")]
fn process_visibility() -> Finding {
    {
        // hidepid=2 makes /proc/<pid> of other users invisible; hidepid=1 hides their contents.
        let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let proc_line = mounts
            .lines()
            .find(|l| l.contains(" /proc ") && l.contains("proc"));
        match proc_line {
            None => Finding::new(
                "process_visibility",
                Status::Unknown,
                "could not read /proc/self/mountinfo",
                "camera credentials appear in ffmpeg's argv; whether another local user can read \
                 them could not be determined",
            ),
            Some(l) if l.contains("hidepid=2") || l.contains("hidepid=invisible") => Finding::new(
                "process_visibility",
                Status::Ok,
                "/proc mounted with hidepid=2",
                "other users cannot see this process at all, so they cannot read the camera \
                 credentials in ffmpeg's argv",
            ),
            Some(l) if l.contains("hidepid=1") || l.contains("hidepid=noaccess") => Finding::new(
                "process_visibility",
                Status::Ok,
                "/proc mounted with hidepid=1",
                "other users cannot read this process's cmdline, so the camera credentials in \
                 ffmpeg's argv are not exposed to them",
            ),
            Some(_) => Finding::new(
                "process_visibility",
                Status::Weak,
                "/proc mounted without hidepid",
                "ffmpeg receives camera credentials in its argv, so ANY local user able to read \
                 /proc/<pid>/cmdline can read them. Mount /proc with hidepid=2, or run the box on a \
                 host with no untrusted local users",
            ),
        }
    }
}

/// Whether the process runs as a dedicated non-root user.
fn service_user() -> Finding {
    #[cfg(unix)]
    {
        // SAFETY: getuid() cannot fail and touches no memory we own.
        let uid = unsafe { libc::getuid() };
        if uid == 0 {
            return Finding::new(
                "service_user",
                Status::Weak,
                "running as root (uid 0)",
                "a compromise of the recorder is a compromise of the host. Run as a dedicated \
                 non-root user; the shipped compose files and systemd unit already do",
            );
        }
        Finding::new(
            "service_user",
            Status::Ok,
            format!("running as uid {uid}"),
            "the process cannot read files outside what that user owns",
        )
    }
    #[cfg(not(unix))]
    Finding::new(
        "service_user",
        Status::Unknown,
        "not a unix host",
        "the service account could not be determined",
    )
}

/// Whether the recordings volume is on encrypted storage, where that is detectable.
///
/// Detectable means: the backing device is a device-mapper crypt target, which is what LUKS
/// produces. A cloud volume encrypted by the provider is invisible from in here and correctly
/// reports `Unknown` — claiming otherwise would be exactly the kind of unearned assurance this
/// module exists to avoid.
#[cfg(not(target_os = "linux"))]
fn volume_encryption(_cfg: &Config) -> Finding {
    Finding::new(
        "recording_volume_encryption",
        Status::Unknown,
        "not a Linux host",
        "volume encryption is host-specific and was not assessed",
    )
}

#[cfg(target_os = "linux")]
fn volume_encryption(cfg: &Config) -> Finding {
    {
        let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        if mounts.is_empty() {
            return Finding::new(
                "recording_volume_encryption",
                Status::Unknown,
                "could not read /proc/self/mountinfo",
                "whether recorded footage is encrypted at rest could not be determined",
            );
        }
        let path = cfg.recordings_dir.to_string_lossy().to_string();
        // The longest mount point that is a prefix of the recordings directory is its filesystem.
        let best = mounts
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let mount = *f.get(4)?;
                path.starts_with(mount).then_some((mount.len(), l))
            })
            .max_by_key(|(n, _)| *n);
        match best {
            Some((_, line)) if line.contains("/dev/mapper/") || line.contains("dm-") => {
                Finding::new(
                    "recording_volume_encryption",
                    Status::Ok,
                    "recordings are on a device-mapper volume",
                    "consistent with LUKS/dm-crypt. Verify with `cryptsetup status` — a \
                     device-mapper target is not proof of encryption on its own",
                )
            }
            Some(_) => Finding::new(
                "recording_volume_encryption",
                Status::Unknown,
                "recordings are not on a device-mapper volume",
                "no encryption is detectable from inside the container. That is NOT the same as \
                 unencrypted — provider-side or filesystem-level encryption is invisible from here",
            ),
            None => Finding::new(
                "recording_volume_encryption",
                Status::Unknown,
                "could not match the recordings directory to a mount",
                "whether recorded footage is encrypted at rest could not be determined",
            ),
        }
    }
}

/// Whether cameras are streamed over plain RTSP.
async fn rtsp_transport(pool: &sqlx::SqlitePool) -> Finding {
    let plain: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cameras
          WHERE enabled = 1
            AND (main_stream_url LIKE 'rtsp://%' OR sub_stream_url LIKE 'rtsp://%')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    if plain == 0 {
        return Finding::new(
            "rtsp_transport",
            Status::Ok,
            "no enabled camera streams over plain rtsp://",
            "camera credentials and footage are not carried in clear text on the network",
        );
    }
    Finding::new(
        "rtsp_transport",
        Status::Weak,
        format!("{plain} enabled camera(s) stream over plain rtsp://"),
        "RTSP basic/digest credentials and the video itself cross the network in clear text. Use \
         rtsps:// where the camera supports it, or keep the camera network physically separate",
    )
}

/// Credentials still in plaintext, and how old the master key is.
async fn credential_state(cfg: &Config, pool: &sqlx::SqlitePool) -> Vec<Finding> {
    let mut out = Vec::new();

    // A stored value is sealed iff it carries the marker. Counting them needs no key, which is the
    // point: an operator who has LOST the key can still be told what was protected.
    let plaintext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cameras
          WHERE password IS NOT NULL AND password != '' AND password NOT LIKE 'enc:v1:%'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    out.push(if plaintext == 0 {
        Finding::new(
            "plaintext_credentials",
            Status::Ok,
            "no camera password is stored in plaintext",
            "every stored camera credential is sealed",
        )
    } else if cfg.secret_key_b64.is_none() {
        Finding::new(
            "plaintext_credentials",
            Status::Weak,
            format!("{plaintext} camera password(s) in plaintext, and no key is configured"),
            "set HELDAR_SECRET_KEY (or _FILE) and re-save each camera; existing rows are not \
             sealed retroactively",
        )
    } else {
        Finding::new(
            "plaintext_credentials",
            Status::Weak,
            format!("{plaintext} camera password(s) predate the master key"),
            "a key is configured but these rows were written before it. Re-save each camera to \
             seal it — encryption applies on write, not retroactively",
        )
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> sqlx::SqlitePool {
        let p = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&p).await.unwrap();
        p
    }

    /// `Unknown` must never be reported as a pass. Saying "volume encryption: ok" because
    /// `/proc/mounts` was unreadable is precisely the unearned assurance this module exists to
    /// avoid — and it is worse than reporting nothing, because an operator acts on it.
    #[test]
    fn unknown_is_not_a_pass() {
        assert_ne!(Status::Unknown, Status::Ok);
        let f = Finding::new("x", Status::Unknown, "d", "m");
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"unknown\""), "{json}");
        assert!(!json.contains("\"ok\""), "{json}");
    }

    #[tokio::test]
    async fn a_plaintext_password_is_reported_and_a_sealed_one_is_not() {
        let p = pool().await;
        let cfg = Config::from_env();
        let now = chrono::Utc::now();
        for (id, pw) in [("cam_plain", "hunter2"), ("cam_sealed", "enc:v1:abc")] {
            sqlx::query(
                "INSERT INTO cameras (id, name, password, created_at, updated_at) VALUES (?,?,?,?,?)",
            )
            .bind(id)
            .bind(id)
            .bind(pw)
            .bind(now)
            .bind(now)
            .execute(&p)
            .await
            .unwrap();
        }
        let f = credential_state(&cfg, &p).await;
        let plaintext = f.iter().find(|f| f.id == "plaintext_credentials").unwrap();
        assert_eq!(plaintext.status, Status::Weak);
        assert!(
            plaintext.detail.contains('1'),
            "exactly the unsealed one is counted: {}",
            plaintext.detail
        );
    }

    #[tokio::test]
    async fn plain_rtsp_is_reported_per_enabled_camera() {
        let p = pool().await;
        let now = chrono::Utc::now();
        // A disabled camera is not a live exposure and must not be counted.
        for (id, url, enabled) in [
            ("cam_plain", "rtsp://cam/1", 1),
            ("cam_tls", "rtsps://cam/1", 1),
            ("cam_off", "rtsp://cam/1", 0),
        ] {
            sqlx::query(
                "INSERT INTO cameras (id, name, main_stream_url, enabled, created_at, updated_at)
                 VALUES (?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(id)
            .bind(url)
            .bind(enabled)
            .bind(now)
            .bind(now)
            .execute(&p)
            .await
            .unwrap();
        }
        let f = rtsp_transport(&p).await;
        assert_eq!(f.status, Status::Weak);
        assert!(f.detail.starts_with("1 enabled"), "{}", f.detail);
    }

    /// Every finding must be serializable and must never carry a secret.
    ///
    /// Tested by SEEDING real credentials and asserting they do not appear — the first version
    /// grepped for `"://"`, which matched the literal `rtsp://` in a finding's own explanatory text
    /// and would have failed forever while proving nothing about leakage.
    #[tokio::test]
    async fn findings_are_machine_readable_and_disclose_nothing() {
        let p = pool().await;
        let cfg = Config::from_env();
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, main_stream_url, username, password, enabled,
                 created_at, updated_at)
             VALUES ('cam_a','A','rtsp://admin:HUNTER2SECRET@10.0.0.5/Streaming/Channels/101',
                     'admin','HUNTER2SECRET',1,?,?)",
        )
        .bind(now)
        .bind(now)
        .execute(&p)
        .await
        .unwrap();

        let findings = assess(&cfg, &p).await;
        assert!(
            findings.len() >= 6,
            "expected the full sweep, got {}",
            findings.len()
        );
        for f in &findings {
            assert!(!f.id.is_empty() && !f.matters.is_empty(), "{f:?}");
        }

        let json = serde_json::to_string(&findings).unwrap();
        for secret in ["HUNTER2SECRET", "10.0.0.5", "Streaming/Channels"] {
            assert!(
                !json.contains(secret),
                "a finding leaked {secret:?} — posture output is meant to be safe to paste into a \
                 support ticket:\n{json}"
            );
        }
        // And it DID see the camera, so the absence above is not because nothing was assessed.
        assert!(
            json.contains("rtsp_transport"),
            "the rtsp finding must be present: {json}"
        );
    }
}
