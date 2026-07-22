//! On-camera smart-event ingestion (issue #46): consume each opted-in camera's own event
//! notification stream (motion / line-crossing / intrusion fired by the device's built-in
//! detection) and drive the kernel's event machinery from it — the event log (webhooks + email
//! subscribe there) and event-mode recording triggers — exactly like the server-side zone engine,
//! but with the camera's silicon doing the detecting.
//!
//! Transport (HikVision ISAPI today, behind `CameraConfigProvider::open_event_stream`): an endless
//! `multipart/mixed` HTTP response of small XML `<EventNotificationAlert>` blocks. Verified live:
//! an ongoing event re-posts an `active` block ~1/s with an incrementing `activePostCount`, and
//! idle cameras post `videoloss`/`inactive` heartbeats — so the consumer needs a rising-edge
//! debounce (one logged event per activity burst, re-armed after a quiet gap) and an idle watchdog
//! (no bytes for a while = dead connection, reconnect).
//!
//! Supervision: a reconcile loop diffs the opted-in camera set (`native_events_enabled`) every few
//! seconds, spawning one reader task per camera and aborting removed ones; readers reconnect with
//! backoff. Failures are per-camera and never affect the rest.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use tokio::task::JoinHandle;

use crate::models::Camera;
use crate::services::camera_config::hikvision::first_text;
use crate::services::camera_config::{self};
use crate::state::AppState;

/// Reconcile the opted-in camera set this often.
const RECONCILE_SECS: u64 = 10;
/// Reconnect backoff after a stream error.
const RECONNECT_BACKOFF_SECS: u64 = 10;
/// No bytes from the device for this long = dead connection (heartbeats normally arrive every few
/// seconds), tear down and reconnect.
const IDLE_TIMEOUT_SECS: u64 = 60;
/// A parse buffer larger than this means the stream is not the multipart XML we expect — bail.
const MAX_BUFFER_BYTES: usize = 256 * 1024;

/// One parsed `<EventNotificationAlert>` block.
#[derive(Debug, PartialEq)]
struct AlertBlock {
    event_type: String,
    event_state: String,
    device_time: Option<String>,
    description: Option<String>,
}

/// Map the device's event type token onto the same stable kinds the capability probe reports.
/// Unknown-but-active types pass through lowercased, so a device feature we haven't cataloged
/// still surfaces rather than being dropped.
fn map_event_kind(device_type: &str) -> Option<String> {
    match device_type {
        "VMD" => Some("motion".into()),
        "linedetection" => Some("line_crossing".into()),
        "fielddetection" => Some("intrusion".into()),
        "shelteralarm" | "tamperdetection" => Some("tamper".into()),
        // Transport heartbeat, not an event.
        "videoloss" => None,
        other => {
            let t = other.trim();
            (!t.is_empty()).then(|| t.to_ascii_lowercase())
        }
    }
}

/// Incremental multipart parser: append bytes, drain every complete
/// `<EventNotificationAlert>…</EventNotificationAlert>` block. Boundary lines and MIME headers are
/// simply skipped — the XML close tag is the reliable delimiter across firmwares.
fn drain_blocks(buf: &mut String) -> Vec<AlertBlock> {
    const CLOSE: &str = "</EventNotificationAlert>";
    let mut out = Vec::new();
    while let Some(end) = buf.find(CLOSE) {
        let upto = end + CLOSE.len();
        let chunk = &buf[..upto];
        if let Some(event_type) = first_text(chunk, "eventType") {
            out.push(AlertBlock {
                event_type,
                event_state: first_text(chunk, "eventState").unwrap_or_default(),
                device_time: first_text(chunk, "dateTime"),
                description: first_text(chunk, "eventDescription"),
            });
        }
        *buf = buf[upto..].to_string();
    }
    out
}

/// Run the supervisor loop forever (spawned supervised by the composing server). Self-idles when
/// no camera has native events enabled.
pub async fn run(st: AppState) {
    // Dedicated client for the endless streams: NO total timeout (the shared `st.http` carries a
    // 10s one that would kill every stream); bounded connect; no redirects (egress hygiene).
    let stream_http = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "camera_events: cannot build stream client");
            return;
        }
    };

    let mut readers: HashMap<String, JoinHandle<()>> = HashMap::new();
    loop {
        let wanted: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM cameras WHERE enabled = 1 AND native_events_enabled = 1 ORDER BY id",
        )
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();

        // Abort readers for cameras that opted out / were disabled, and reap finished ones.
        readers.retain(|id, handle| {
            if !wanted.contains(id) {
                handle.abort();
                tracing::info!(camera_id = %id, "camera_events: reader stopped (opted out)");
                return false;
            }
            !handle.is_finished()
        });

        // Spawn readers for newly opted-in cameras.
        for id in &wanted {
            if !readers.contains_key(id) {
                let st = st.clone();
                let http = stream_http.clone();
                let camera_id = id.clone();
                tracing::info!(camera_id = %id, "camera_events: reader starting");
                readers.insert(
                    id.clone(),
                    tokio::spawn(async move { reader_loop(st, http, camera_id).await }),
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(RECONCILE_SECS)).await;
    }
}

/// Per-camera reader: connect, consume blocks, reconnect with backoff forever (until aborted by
/// the reconcile loop).
async fn reader_loop(st: AppState, stream_http: reqwest::Client, camera_id: String) {
    loop {
        match read_stream(&st, &stream_http, &camera_id).await {
            Ok(()) => {
                tracing::info!(camera_id = %camera_id, "camera_events: stream ended; reconnecting");
            }
            Err(e) => {
                tracing::warn!(camera_id = %camera_id, error = %e, "camera_events: stream error; reconnecting");
            }
        }
        tokio::time::sleep(Duration::from_secs(RECONNECT_BACKOFF_SECS)).await;
    }
}

/// One connection lifetime: open the stream and pump blocks until it dies or goes idle.
async fn read_stream(
    st: &AppState,
    stream_http: &reqwest::Client,
    camera_id: &str,
) -> crate::error::AppResult<()> {
    let cam: Camera = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| crate::error::AppError::NotFound(format!("camera {camera_id} not found")))?;
    let provider = camera_config::for_camera(&cam, &st.http, st.cfg.isapi_request_timeout_ms)?;
    let resp = provider.open_event_stream(stream_http).await?;
    tracing::info!(camera_id = %camera_id, "camera_events: stream connected");

    let rearm = Duration::from_secs(st.cfg.camera_events_rearm_secs.max(1));
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    // Rising-edge debounce: last time each kind was seen active. A kind quiet for `rearm` logs a
    // fresh event on its next activity; continuous re-posts inside a burst only extend recording.
    let mut last_active: HashMap<String, tokio::time::Instant> = HashMap::new();

    loop {
        let chunk =
            match tokio::time::timeout(Duration::from_secs(IDLE_TIMEOUT_SECS), stream.next()).await
            {
                Err(_) => {
                    return Err(crate::error::AppError::Other(anyhow::anyhow!(
                        "no data for {IDLE_TIMEOUT_SECS}s (heartbeats stopped)"
                    )));
                }
                Ok(None) => return Ok(()), // device closed the stream
                Ok(Some(Err(e))) => {
                    return Err(crate::error::AppError::Other(anyhow::anyhow!(
                        "stream read: {e}"
                    )));
                }
                Ok(Some(Ok(bytes))) => bytes,
            };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if buf.len() > MAX_BUFFER_BYTES {
            return Err(crate::error::AppError::Other(anyhow::anyhow!(
                "parse buffer overflow — not an event stream?"
            )));
        }

        for block in drain_blocks(&mut buf) {
            if !block.event_state.eq_ignore_ascii_case("active") {
                continue;
            }
            let Some(kind) = map_event_kind(&block.event_type) else {
                continue;
            };
            let now = tokio::time::Instant::now();
            let rising = last_active
                .get(&kind)
                .map(|t| now.duration_since(*t) >= rearm)
                .unwrap_or(true);
            last_active.insert(kind.clone(), now);

            // Every active block extends event-mode recording (trigger() no-ops for cameras not
            // in event mode), so recording covers the whole burst, not just its first second.
            let _ = st.recorder.trigger(camera_id, "camera_event").await;

            if rising {
                let _ = crate::repo::log_event(
                    &st.pool,
                    Some(camera_id),
                    &format!("camera_{kind}"),
                    "warning",
                    json!({
                        "source": "camera_native",
                        "device_event_type": block.event_type,
                        "device_time": block.device_time,
                        "description": block.description,
                    }),
                )
                .await;
                tracing::info!(camera_id = %camera_id, kind = %kind, "camera_events: on-camera event");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim block captured live from a DS-2CD3T56WDV3-L alert stream (motion alarm burst).
    const LIVE_BLOCK: &str = "--boundary\r\nContent-Type: application/xml; charset=\"UTF-8\"\r\nContent-Length: 515\r\n\r\n\
<EventNotificationAlert version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\n\
<ipAddress>192.168.0.7</ipAddress>\n<portNo>80</portNo>\n<protocol>HTTP</protocol>\n\
<macAddress>d4:e8:53:87:c0:5c</macAddress>\n<channelID>1</channelID>\n\
<dateTime>2026-07-16T13:48:26+08:00</dateTime>\n<activePostCount>1</activePostCount>\n\
<eventType>VMD</eventType>\n<eventState>active</eventState>\n\
<eventDescription>Motion alarm</eventDescription>\n<DetectionRegionList>\n</DetectionRegionList>\n\
</EventNotificationAlert>\n";

    #[test]
    fn drains_live_block_and_partial_tail() {
        // Feed one complete block plus the start of the next: exactly one drains, tail remains.
        let mut buf = format!("{LIVE_BLOCK}--boundary\r\nContent-Type: application/xml");
        let blocks = drain_blocks(&mut buf);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].event_type, "VMD");
        assert_eq!(blocks[0].event_state, "active");
        assert_eq!(
            blocks[0].device_time.as_deref(),
            Some("2026-07-16T13:48:26+08:00")
        );
        assert_eq!(blocks[0].description.as_deref(), Some("Motion alarm"));
        assert!(
            buf.trim_start().starts_with("--boundary"),
            "partial tail preserved"
        );
        // The tail alone yields nothing until its close tag arrives.
        assert!(drain_blocks(&mut buf).is_empty());
    }

    #[test]
    fn drains_multiple_blocks_split_at_arbitrary_chunk_boundaries() {
        let two = format!("{LIVE_BLOCK}{LIVE_BLOCK}");
        // Split the byte stream at every 7 bytes to simulate arbitrary TCP chunking.
        let mut buf = String::new();
        let mut total = 0;
        for chunk in two.as_bytes().chunks(7) {
            buf.push_str(std::str::from_utf8(chunk).unwrap());
            total += drain_blocks(&mut buf).len();
        }
        assert_eq!(total, 2);
        assert!(!buf.contains("</EventNotificationAlert>"));
    }

    #[test]
    fn maps_device_types_to_stable_kinds() {
        assert_eq!(map_event_kind("VMD").as_deref(), Some("motion"));
        assert_eq!(
            map_event_kind("linedetection").as_deref(),
            Some("line_crossing")
        );
        assert_eq!(
            map_event_kind("fielddetection").as_deref(),
            Some("intrusion")
        );
        assert_eq!(map_event_kind("shelteralarm").as_deref(), Some("tamper"));
        // Heartbeats are transport, not events.
        assert_eq!(map_event_kind("videoloss"), None);
        // Uncataloged types pass through (lowercased) rather than being dropped.
        assert_eq!(
            map_event_kind("unattendedBaggage").as_deref(),
            Some("unattendedbaggage")
        );
    }
}
