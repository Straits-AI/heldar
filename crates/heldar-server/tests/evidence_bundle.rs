//! Evidence bundles (#118): build a real one, then try to get a "VALID" out of a tampered one.
//!
//! These tests do not stub the media. They generate a synthetic segment with ffmpeg, run the real
//! export (remux, hash, sign, zip), and verify with the real offline script. The whole claim of the
//! feature is that a bundle survives leaving the appliance, so a test that never produced a file
//! would be asserting the claim rather than checking it.
//!
//! The mutation tests are the core of #118's acceptance criteria: "tests mutate the clip, manifest,
//! event metadata and signature independently and prove each mutation fails verification". Each one
//! changes exactly ONE thing so a pass cannot come from some other check firing.
//!
//! They FAIL rather than skip when ffmpeg, zip or openssl are missing. A skipped test reads exactly
//! like a passing one in a CI summary, and the thing being protected here is whether a tampered
//! evidence file can be presented as genuine.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{Duration, Utc};
use heldar_kernel::services::evidence;
use heldar_kernel::state::AppState;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn scratch(name: &str) -> PathBuf {
    let d = repo_root().join("target/evidence-tests").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

fn require(bin: &str) {
    let found = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(
        found,
        "{bin} is required by the evidence-bundle tests and was not found. This test fails rather \
         than skipping on purpose: a skipped test looks like a passing one, and what it protects is \
         whether a tampered evidence file can be presented as genuine."
    );
}

/// A state whose media directories are all under one scratch tree.
async fn state(dir: &Path) -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = true;
    cfg.data_dir = dir.to_path_buf();
    cfg.recordings_dir = dir.join("recordings");
    cfg.evidence_dir = dir.join("evidence");
    cfg.clips_dir = dir.join("clips");
    std::fs::create_dir_all(&cfg.recordings_dir).unwrap();
    std::fs::create_dir_all(&cfg.evidence_dir).unwrap();
    let cfg = std::sync::Arc::new(cfg);
    AppState {
        recorder: heldar_kernel::services::recorder::RecorderManager::new(
            pool.clone(),
            cfg.clone(),
        ),
        sampler: heldar_kernel::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
        live: heldar_kernel::services::live_publisher::LivePublisherManager::new(
            pool.clone(),
            cfg.clone(),
            heldar_kernel::reqwest::Client::new(),
        ),
        mirror: None,
        consumers: std::sync::Arc::new(Vec::new()),
        modules: std::sync::Arc::new(Vec::new()),
        catalog: std::sync::Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
        http: heldar_kernel::reqwest::Client::new(),
        media_jobs: heldar_kernel::services::media_jobs::MediaJobGovernor::new(2),
        started_at: Utc::now(),
        pool,
        cfg,
    }
}

/// Seed a camera with one real 10-second segment, plus a detection and an event inside the window.
async fn seed(st: &AppState, camera: &str) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    let now = Utc::now();
    let start = now - Duration::seconds(10);
    sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
        .bind(camera)
        .bind("Test Camera")
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();

    let seg = st.cfg.recordings_dir.join(format!("{camera}_0001.mp4"));
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x120:rate=10:duration=10",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "10"])
        .arg(&seg)
        .output()
        .expect("running ffmpeg");
    assert!(
        out.status.success(),
        "ffmpeg could not make a fixture segment: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    sqlx::query(
        "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, codec,
             size_bytes, container, created_at)
         VALUES (?,?,?,?,?,?,?,?,'mp4',?)",
    )
    .bind("seg_1")
    .bind(camera)
    .bind(seg.to_string_lossy().to_string())
    .bind(start)
    .bind(now)
    .bind(10.0_f64)
    .bind("h264")
    .bind(std::fs::metadata(&seg).unwrap().len() as i64)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO detections (id, camera_id, task_type, timestamp, label, confidence, created_at)
         VALUES ('det_1', ?, 'detection', ?, 'person', 0.91, ?)",
    )
    .bind(camera)
    .bind(start + Duration::seconds(2))
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO events (id, camera_id, event_type, severity, timestamp, created_at)
         VALUES ('evt_1', ?, 'reconnect', 'warning', ?, ?)",
    )
    .bind(camera)
    .bind(start + Duration::seconds(3))
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();

    (start, now)
}

/// (exit code, stdout+stderr) from the offline verifier.
fn verify(bundle: &Path, extra: &[&str]) -> (i32, String) {
    let out = Command::new("python3")
        .arg(repo_root().join("scripts/verify_evidence_bundle.py"))
        .arg(bundle)
        .args(extra)
        .output()
        .expect("running the verifier");
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), s)
}

/// Rewrite one entry inside a bundle, producing a new file. Everything else is copied byte for byte,
/// so a failure can only be attributed to the entry that changed.
fn mutate(src: &Path, dst: &Path, entry: &str, transform: &str) {
    let script = format!(
        "import shutil, sys, zipfile\n\
         src, dst, entry = sys.argv[1], sys.argv[2], sys.argv[3]\n\
         zin = zipfile.ZipFile(src)\n\
         names = zin.namelist()\n\
         target = entry if entry in names else './' + entry\n\
         assert target in names, 'no such entry: ' + entry + ' in ' + repr(names)\n\
         with zipfile.ZipFile(dst, 'w') as zout:\n    \
             for n in names:\n        \
                 data = zin.read(n)\n        \
                 if n == target:\n            \
                     {transform}\n        \
                 zout.writestr(n, data)\n"
    );
    let out = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(src)
        .arg(dst)
        .arg(entry)
        .output()
        .expect("running the mutator");
    assert!(
        out.status.success(),
        "mutating {entry} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn build(dir: &Path) -> (AppState, PathBuf, String) {
    let st = state(dir).await;
    let (from, to) = seed(&st, "cam_a").await;
    let p = heldar_kernel::auth::Principal::system_admin();
    let r = evidence::export(
        &st,
        &p,
        "cam_a",
        from,
        to,
        None,
        Some("aud_x"),
        Some("req_x"),
    )
    .await
    .expect("exporting a bundle");
    let path = st.cfg.evidence_dir.join(&r.filename);
    assert!(path.is_file(), "the bundle file must exist at {path:?}");
    (st, path, r.key_id)
}

#[tokio::test]
async fn a_freshly_exported_bundle_verifies_against_its_key() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("valid");
    let (_st, bundle, key_id) = build(&dir).await;

    // Without an expected key, the honest answer is UNKNOWN-KEY: a bundle carrying its own public
    // key proves only that whoever made the bundle also made the key.
    let (code, out) = verify(&bundle, &[]);
    assert_eq!(
        code, 3,
        "no expected key must be UNKNOWN-KEY, not VALID:\n{out}"
    );

    let (code, out) = verify(&bundle, &["--key-id", &key_id]);
    assert_eq!(code, 0, "an untouched bundle must be VALID:\n{out}");
    assert!(out.contains("VALID"), "{out}");
    // The limits must be stated in the output, not buried: a reader who only sees "VALID" would
    // reasonably conclude the timestamp is attested. It is not.
    assert!(
        out.contains("trusted timestamping"),
        "the verifier must state what the signature does NOT establish:\n{out}"
    );
}

/// #118's central criterion. Four independent mutations, four refusals — and each names the thing
/// that changed, because "invalid" with no location is not usable by an investigator.
#[tokio::test]
async fn every_single_mutation_is_caught_independently() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("mutations");
    let (_st, bundle, key_id) = build(&dir).await;
    let k = ["--key-id", key_id.as_str()];

    // 1. The MEDIA. One flipped byte deep inside the clip.
    let m = dir.join("m-clip.heldar-evidence");
    mutate(
        &bundle,
        &m,
        "media/clip.mp4",
        "data = data[:400] + bytes([data[400] ^ 0xFF]) + data[401:]",
    );
    let (code, out) = verify(&m, &k);
    assert_eq!(code, 1, "a modified clip must be MODIFIED:\n{out}");
    assert!(out.contains("media/clip.mp4"), "must name the file:\n{out}");

    // 2. The EVENT METADATA. Deleting an inconvenient event is the realistic attack, not corruption.
    let m = dir.join("m-events.heldar-evidence");
    mutate(&bundle, &m, "metadata/events.jsonl", "data = b''");
    let (code, out) = verify(&m, &k);
    assert_eq!(code, 1, "removed events must be MODIFIED:\n{out}");
    assert!(
        out.contains("metadata/events.jsonl"),
        "must name the file:\n{out}"
    );

    // 3. The MANIFEST. Rewriting the claim itself — here, the camera the footage is attributed to.
    let m = dir.join("m-manifest.heldar-evidence");
    mutate(
        &bundle,
        &m,
        "manifest.json",
        "data = data.replace(b'cam_a', b'cam_b')",
    );
    let (code, out) = verify(&m, &k);
    assert_eq!(code, 1, "an altered manifest must be MODIFIED:\n{out}");
    assert!(
        out.contains("signature does not verify"),
        "the manifest is what the signature covers, so THAT is the failure:\n{out}"
    );

    // 4. The SIGNATURE. Corrupting it must not be mistaken for a corrupt manifest.
    let m = dir.join("m-sig.heldar-evidence");
    mutate(
        &bundle,
        &m,
        "signature.json",
        "import json as _j; d = _j.loads(data); d['signature'] = d['signature'][:-4] + 'AAAA'; \
         data = _j.dumps(d).encode()",
    );
    let (code, out) = verify(&m, &k);
    assert_eq!(code, 1, "a forged signature must be MODIFIED:\n{out}");
}

/// The states #118 requires be reported DISTINCTLY. A verifier with one failure mode forces an
/// investigator to guess whether a bundle was tampered with or merely unfamiliar.
#[tokio::test]
async fn the_failure_states_are_distinguishable() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("states");
    let (_st, bundle, key_id) = build(&dir).await;
    let k = ["--key-id", key_id.as_str()];

    // MISSING — a file the manifest lists is gone. Distinct from MODIFIED: the difference between
    // "this was altered" and "part of it was not handed to you" matters to whoever received it.
    let m = dir.join("s-missing.heldar-evidence");
    let script = "import sys, zipfile\n\
                  zin = zipfile.ZipFile(sys.argv[1])\n\
                  with zipfile.ZipFile(sys.argv[2], 'w') as z:\n    \
                      for n in zin.namelist():\n        \
                          if n.endswith('metadata/camera.json'): continue\n        \
                          z.writestr(n, zin.read(n))\n";
    let o = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&bundle)
        .arg(&m)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (code, out) = verify(&m, &k);
    assert_eq!(
        code, 2,
        "a dropped file must be MISSING, not MODIFIED:\n{out}"
    );
    assert!(out.contains("metadata/camera.json"), "{out}");

    // UNKNOWN-KEY — everything checks out, but not against the key you were told to expect.
    let (code, out) = verify(&bundle, &["--key-id", "sha256:0000000000000000"]);
    assert_eq!(code, 3, "a key mismatch must be UNKNOWN-KEY:\n{out}");

    // UNSUPPORTED — a future format. Reported before any other check, because "valid" from a
    // verifier that did not understand the document is the worst possible answer.
    let m = dir.join("s-future.heldar-evidence");
    mutate(
        &bundle,
        &m,
        "manifest.json",
        "data = data.replace(b'heldar-evidence/1', b'heldar-evidence/9')",
    );
    let (code, out) = verify(&m, &k);
    assert_eq!(
        code, 4,
        "an unknown format must be UNSUPPORTED, not MODIFIED:\n{out}"
    );

    // MALFORMED — not a bundle at all.
    let junk = dir.join("s-junk.heldar-evidence");
    std::fs::write(&junk, b"this is not a zip").unwrap();
    let (code, out) = verify(&junk, &k);
    assert_eq!(code, 5, "a non-bundle must be MALFORMED:\n{out}");
}

/// `hashes.sha256` exists so an investigator can check the bundle with coreutils alone. That makes
/// it a second place hashes live, and a second place is a place they can disagree — so a bundle
/// whose convenience file has been edited must fail, or the coreutils reader is verifying hashes
/// nobody signed.
#[tokio::test]
async fn a_doctored_convenience_hash_file_is_not_tolerated() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("sidecar");
    let (_st, bundle, key_id) = build(&dir).await;

    let m = dir.join("d-hashes.heldar-evidence");
    mutate(
        &bundle,
        &m,
        "hashes.sha256",
        "data = data.replace(b'0', b'1', 1)",
    );
    let (code, out) = verify(&m, &["--key-id", key_id.as_str()]);
    assert_eq!(
        code, 1,
        "hashes.sha256 disagreeing with the signed manifest must fail — a reader using \
         `sha256sum -c` alone would otherwise be checking unsigned hashes:\n{out}"
    );
}

/// The bundle must describe the window honestly, including what is NOT in it. An export that
/// concatenated across an outage and reported nothing would produce continuous-looking video of a
/// discontinuous night.
#[tokio::test]
async fn the_signed_manifest_states_the_gaps_rather_than_hiding_them() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("gaps");
    let st = state(&dir).await;
    let (start, end) = seed(&st, "cam_a").await;
    // Ask for a window twice as long as the footage: the second half does not exist.
    let to = end + Duration::seconds(10);

    let p = heldar_kernel::auth::Principal::system_admin();
    let r = evidence::export(&st, &p, "cam_a", start, to, None, None, None)
        .await
        .expect("export");
    assert!(
        !r.gaps.is_empty(),
        "a window extending past the footage must report a gap"
    );

    let bundle = st.cfg.evidence_dir.join(&r.filename);
    let (code, out) = verify(&bundle, &["--key-id", r.key_id.as_str()]);
    assert_eq!(code, 0, "the bundle itself is still valid:\n{out}");
    assert!(
        out.contains("GAP(S)"),
        "the verifier must SAY the window has gaps — a reader who is not told will assume the \
         video is continuous:\n{out}"
    );

    // And the gap must be inside the SIGNED manifest, not only in the verifier's presentation.
    let o = Command::new("python3")
        .arg("-c")
        .arg(
            "import sys,zipfile,json;z=zipfile.ZipFile(sys.argv[1]);\
              m=json.loads(z.read([n for n in z.namelist() if n.endswith('manifest.json')][0]));\
              print(len(m['media']['gaps']), m['media']['covered_seconds'])",
        )
        .arg(&bundle)
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&o.stdout);
    let mut it = s.split_whitespace();
    assert_eq!(it.next(), Some("1"), "one gap in the signed manifest: {s}");
    let covered: f64 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
    assert!(
        (covered - 10.0).abs() < 1.5,
        "covered_seconds must reflect the ~10s that exists, not the 20s requested: {s}"
    );
}

/// The header of `services/evidence.rs` says `audit.jsonl` carries the trail for this camera "in the
/// window, including this export". Both halves were false when written:
///
///   * the export's own audit row is stamped NOW, while the window is almost always in the PAST, so
///     a range filter excluded it;
///   * the query keyed on `target_id`, which is whatever object an action names — a clip, a
///     schedule, an AI task — so every action taken ON the camera through something else was
///     silently dropped. `subject_camera_id` is the column that answers "which camera does this row
///     concern" (migration 0014), and it is what camera-scoped audit reads already use.
///
/// A bundle that quietly ships a thinner chain of custody than its own documentation promises is
/// worse than one that ships none, because the gap is invisible to whoever receives it.
#[tokio::test]
async fn the_audit_trail_holds_what_the_bundle_says_it_holds() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("audit");
    let st = state(&dir).await;
    let (from, to) = seed(&st, "cam_a").await;
    let p = heldar_kernel::auth::Principal::system_admin();

    // An action taken ON the camera, but targeting something else — the shape `target_id` misses.
    let in_window = from + Duration::seconds(4);
    sqlx::query(
        "INSERT INTO audit_log (id, actor, action, target_type, target_id, detail,
             subject_camera_id, created_at)
         VALUES ('aud_inwindow','operator','export_clip','clip','clip_7','{}', 'cam_a', ?)",
    )
    .bind(in_window)
    .execute(&st.pool)
    .await
    .unwrap();
    // A row for a DIFFERENT camera in the same window must not leak into this bundle.
    sqlx::query(
        "INSERT INTO audit_log (id, actor, action, target_type, target_id, detail,
             subject_camera_id, created_at)
         VALUES ('aud_other','operator','export_clip','clip','clip_8','{}', 'cam_b', ?)",
    )
    .bind(in_window)
    .execute(&st.pool)
    .await
    .unwrap();
    // The export's own row, stamped NOW — i.e. after the window closes.
    let export_audit = heldar_kernel::auth::audit(
        &st.pool,
        &p,
        "export_evidence_bundle",
        "camera",
        "cam_a",
        serde_json::json!({}),
    )
    .await
    .expect("the audit row must be written");

    let r = evidence::export(&st, &p, "cam_a", from, to, None, Some(&export_audit), None)
        .await
        .expect("export");
    let bundle = st.cfg.evidence_dir.join(&r.filename);

    let o = Command::new("python3")
        .arg("-c")
        .arg(
            "import sys,zipfile;z=zipfile.ZipFile(sys.argv[1]);\
              print(z.read([n for n in z.namelist() if n.endswith('audit.jsonl')][0]).decode())",
        )
        .arg(&bundle)
        .output()
        .unwrap();
    let trail = String::from_utf8_lossy(&o.stdout).to_string();

    assert!(
        trail.contains("aud_inwindow"),
        "an action on this camera that targeted a CLIP must be in the trail — keying on target_id \
         drops it:\n{trail}"
    );
    assert!(
        trail.contains(&export_audit),
        "this export's own audit row must be in the bundle it describes; it is stamped after the \
         window, so a range filter alone excludes it:\n{trail}"
    );
    assert!(
        !trail.contains("aud_other"),
        "another camera's audit row must NOT be in a single-camera bundle:\n{trail}"
    );

    let (code, out) = verify(&bundle, &["--key-id", r.key_id.as_str()]);
    assert_eq!(code, 0, "the bundle must still verify:\n{out}");
}
