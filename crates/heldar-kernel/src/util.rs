use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use tokio::process::Command;

/// Hard cap for a single ffprobe invocation so one pathological file cannot wedge the indexer.
const FFPROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Fail fast at startup if the configured ffmpeg/ffprobe binaries aren't runnable. They are required
/// for recording, clip/snapshot export, sampling, and indexing; a missing binary otherwise surfaces
/// only later as silent per-camera failures (cameras stuck "connecting", empty timelines).
pub fn check_media_binaries(cfg: &crate::config::Config) -> anyhow::Result<()> {
    for (label, bin) in [("ffmpeg", &cfg.ffmpeg_bin), ("ffprobe", &cfg.ffprobe_bin)] {
        std::process::Command::new(bin)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| {
                anyhow!(
                    "required media binary `{label}` (`{bin}`) is not runnable: {e}. \
                     Install ffmpeg, or set HELDAR_FFMPEG_BIN / HELDAR_FFPROBE_BIN to its path."
                )
            })?;
    }
    Ok(())
}

/// How the configured ffmpeg identifies itself, e.g. `ffmpeg version 6.1.1-3ubuntu5`.
///
/// Evidence bundles record this. The manifest is SIGNED, so what goes in it has to be a fact the
/// appliance can stand behind: the manifest previously recorded `ffmpeg_bin`, which is a
/// *configured path* and defaults to the bare string `"ffmpeg"`. That identifies no build to anyone
/// verifying a bundle six months later on another machine, while being signed as though it did.
///
/// Returns `None` when the binary cannot be run or says nothing recognisable, and the manifest then
/// records null. An unknown producer is a fact about the evidence; a guessed one is a signed
/// falsehood, and #125 is the standing reminder of what those cost.
///
/// Cached per binary path: ffmpeg does not change underneath a running process, and an export
/// should not pay for a subprocess to re-learn the same string.
pub async fn ffmpeg_version(bin: &str) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

    if let Ok(cache) = CACHE.get_or_init(Default::default).lock() {
        if let Some(hit) = cache.get(bin) {
            return hit.clone();
        }
    }

    let probed = probe_ffmpeg_version(bin).await;
    if let Ok(mut cache) = CACHE.get_or_init(Default::default).lock() {
        cache.insert(bin.to_string(), probed.clone());
    }
    probed
}

async fn probe_ffmpeg_version(bin: &str) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(bin)
            .arg("-version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ffmpeg_version(&String::from_utf8_lossy(&out.stdout))
}

/// First line of `ffmpeg -version`, trimmed. Split out so it can be tested without ffmpeg present.
fn parse_ffmpeg_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    // Only accept something that actually looks like a version banner. A binary that prints
    // anything else is not identified, and saying so beats signing whatever it happened to emit.
    if !line.to_ascii_lowercase().starts_with("ffmpeg version") {
        return None;
    }
    // Cap the length: this string is signed and shown to a human, not a place for a wall of text.
    Some(line.chars().take(200).collect())
}

/// Subset of media properties extracted via ffprobe.
#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub duration_s: f64,
    pub codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
}

/// Parse an ffprobe rational like "20/1" or "30000/1001" into frames-per-second.
fn parse_rational(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let n: f64 = n.parse().ok()?;
    let d: f64 = d.parse().ok()?;
    if d == 0.0 {
        None
    } else {
        Some(n / d)
    }
}

#[derive(Deserialize)]
struct FfprobeOut {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}
#[derive(Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    avg_frame_rate: Option<String>,
}
#[derive(Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

/// Probe a media file for duration and video stream properties.
pub async fn ffprobe_file(ffprobe_bin: &str, path: &Path) -> anyhow::Result<ProbeInfo> {
    let mut cmd = Command::new(ffprobe_bin);
    cmd.kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,avg_frame_rate",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = tokio::time::timeout(FFPROBE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| anyhow!("ffprobe timed out for {}", path.display()))?
        .with_context(|| format!("spawning ffprobe for {}", path.display()))?;

    if !out.status.success() {
        return Err(anyhow!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let parsed: FfprobeOut =
        serde_json::from_slice(&out.stdout).context("parsing ffprobe json output")?;
    let duration_s = parsed
        .format
        .and_then(|f| f.duration)
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);
    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));

    Ok(ProbeInfo {
        duration_s,
        codec: video.and_then(|s| s.codec_name.clone()),
        width: video.and_then(|s| s.width),
        height: video.and_then(|s| s.height),
        fps: video
            .and_then(|s| s.avg_frame_rate.as_deref())
            .and_then(parse_rational),
    })
}

/// Parse a UTC segment start time from a filename like `20260613_120500.mp4`.
pub fn parse_segment_time(filename: &str) -> Option<DateTime<Utc>> {
    let stem = filename.split('.').next().unwrap_or(filename);
    let stem = stem.trim_end_matches('Z');
    NaiveDateTime::parse_from_str(stem, "%Y%m%d_%H%M%S")
        .ok()
        .map(|n| n.and_utc())
}

/// Turn an arbitrary string into a safe lowercase slug for camera ids / path segments.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('_');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "camera".to_string()
    } else {
        trimmed
    }
}

/// Parse an RFC3339 / ISO-8601 timestamp (accepts a trailing `Z`) into UTC.
pub fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Probe a live stream URL (RTSP-aware) to confirm reachability and read codec/dimensions.
pub async fn ffprobe_stream(ffprobe_bin: &str, url: &str) -> anyhow::Result<ProbeInfo> {
    let out = Command::new(ffprobe_bin)
        .kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-rtsp_transport",
            "tcp",
            "-timeout",
            "8000000",
            "-show_entries",
            "format=duration",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,avg_frame_rate",
            "-of",
            "json",
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawning ffprobe for stream")?;
    if !out.status.success() {
        return Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let parsed: FfprobeOut =
        serde_json::from_slice(&out.stdout).context("parsing ffprobe json output")?;
    let duration_s = parsed
        .format
        .and_then(|f| f.duration)
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);
    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    Ok(ProbeInfo {
        duration_s,
        codec: video.and_then(|s| s.codec_name.clone()),
        width: video.and_then(|s| s.width),
        height: video.and_then(|s| s.height),
        fps: video
            .and_then(|s| s.avg_frame_rate.as_deref())
            .and_then(parse_rational),
    })
}

/// Distinguish "field absent" from "field present and null" in a PATCH body.
///
/// `Option<T>` cannot tell them apart — both deserialize to `None` — so a field that can be
/// meaningfully CLEARED needs `Option<Option<T>>` and this deserializer. Without it, "leave it
/// alone" and "unset it" are the same request, and the API silently picks one.
pub fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Gate A 01"), "gate_a_01");
        assert_eq!(slugify("  !!!  "), "camera");
        assert_eq!(slugify("Caf\u{e9}-Cam #2"), "caf_cam_2");
    }

    #[test]
    fn parse_segment_time_reads_utc_filename() {
        let t = parse_segment_time("20260613_050219.mp4").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-06-13T05:02:19+00:00");
    }

    #[test]
    fn parse_rfc3339_accepts_trailing_z() {
        let t = parse_rfc3339("2026-06-13T05:02:19Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-06-13T05:02:19+00:00");
    }
}

#[cfg(test)]
mod ffmpeg_version_tests {
    use super::parse_ffmpeg_version;

    /// The manifest is signed, so this parser's job is to be UNSURE loudly rather than confident
    /// wrongly. Every case below is about which of those two it does.
    #[test]
    fn a_real_banner_is_taken_verbatim() {
        for (stdout, want) in [
            (
                "ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023 the FFmpeg developers\n\
                 built with gcc 13\n",
                "ffmpeg version 6.1.1-3ubuntu5 Copyright (c) 2000-2023 the FFmpeg developers",
            ),
            (
                "ffmpeg version n7.0.2 Copyright (c) 2000-2024\n",
                "ffmpeg version n7.0.2 Copyright (c) 2000-2024",
            ),
            // Static builds and distro rebuilds say all sorts of things after the number; the whole
            // first line is the identification, so it is kept rather than parsed down to a semver.
            (
                "ffmpeg version 2026-01-01-git-abcdef1234-full_build-www.gyan.dev\n",
                "ffmpeg version 2026-01-01-git-abcdef1234-full_build-www.gyan.dev",
            ),
        ] {
            assert_eq!(parse_ffmpeg_version(stdout).as_deref(), Some(want));
        }
    }

    #[test]
    fn anything_that_is_not_a_banner_is_unknown_rather_than_recorded() {
        // A wrapper script, a busybox applet, a shell error on stdout — none of these identify an
        // ffmpeg build, and signing them would assert something false about the evidence.
        for stdout in [
            "",
            "\n",
            "bash: ffmpeg: command not found\n",
            "usage: ffmpeg [options]\n",
            "avconv version 11.4\n",
            "Version: 6.1\n",
            "some wrapper that prints nothing useful\n",
        ] {
            assert_eq!(parse_ffmpeg_version(stdout), None, "stdout = {stdout:?}");
        }
    }

    #[test]
    fn the_banner_match_is_case_insensitive_but_still_anchored() {
        assert!(parse_ffmpeg_version("FFmpeg version 6.1\n").is_some());
        assert!(parse_ffmpeg_version("FFMPEG VERSION 6.1\n").is_some());
        // ...but a line that merely mentions ffmpeg somewhere is not a banner.
        assert_eq!(
            parse_ffmpeg_version("built against ffmpeg version 6.1\n"),
            None
        );
    }

    #[test]
    fn a_pathological_first_line_cannot_bloat_the_signed_manifest() {
        let huge = format!("ffmpeg version {}\n", "x".repeat(10_000));
        let got = parse_ffmpeg_version(&huge).expect("still a banner");
        assert_eq!(got.chars().count(), 200);
    }

    #[test]
    fn leading_whitespace_does_not_hide_the_banner() {
        assert_eq!(
            parse_ffmpeg_version("   ffmpeg version 6.1   \n").as_deref(),
            Some("ffmpeg version 6.1")
        );
    }
}
