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

// =================================================================================================
// Archive-shape attacks.
//
// An adversarial review produced a bundle that exited VALID against the appliance's real key id
// while every extraction tool on the machine wrote a forged manifest and forged footage to disk.
// The verifier resolved manifest keys through a normalising dict that stripped ONE leading "./" and
// silently ignored anything it did not recognise, so `././manifest.json` was invisible to it and
// authoritative to `unzip`. These tests are that attack, and its neighbours.
//
// Each asserts on what an INVESTIGATOR ends up holding, not only on the exit code — the whole defect
// was a gap between what the verifier read and what the extractor wrote, and a test that only reads
// through the verifier is blind to exactly that gap.
// =================================================================================================

/// Build a bundle with extra raw zip entries appended, bypassing any name sanitisation.
fn with_entries(src: &Path, dst: &Path, entries: &[(&str, &str)]) {
    let adds: String = entries
        .iter()
        .map(|(n, body)| format!("    z.writestr({n:?}, {body:?})\n"))
        .collect();
    let script = format!(
        "import sys, zipfile\n\
         zin = zipfile.ZipFile(sys.argv[1])\n\
         with zipfile.ZipFile(sys.argv[2], 'w') as z:\n    \
             for n in zin.namelist():\n        \
                 z.writestr(n, zin.read(n))\n{adds}"
    );
    let o = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(src)
        .arg(dst)
        .output()
        .expect("building the attack bundle");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// Extract with the system `unzip` and return the text of one extracted file.
fn extract_and_read(bundle: &Path, into: &Path, rel: &str) -> String {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).unwrap();
    let o = Command::new("unzip")
        .arg("-oq")
        .arg(bundle)
        .current_dir(into)
        .output()
        .expect("running unzip");
    assert!(
        o.status.success(),
        "unzip failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    std::fs::read_to_string(into.join(rel)).unwrap_or_default()
}

/// THE FORGERY. A second `manifest.json` hidden behind a `.` path component: invisible to a
/// verifier that resolves names, authoritative to every extractor that resolves paths.
#[tokio::test]
async fn a_manifest_hidden_behind_a_path_component_cannot_verify() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    require("unzip");
    let dir = scratch("shadow");
    let (_st, bundle, key_id) = build(&dir).await;

    let forged = r#"{"format":"heldar-evidence/1","camera":{"id":"cam_forged"},"files":{}}"#;
    for shadow in ["././manifest.json", "./manifest.json", "media/./clip.mp4"] {
        let m = dir.join(format!(
            "shadow-{}.heldar-evidence",
            shadow.replace(['/', '.'], "_")
        ));
        with_entries(&bundle, &m, &[(shadow, forged)]);

        let (code, out) = verify(&m, &["--key-id", key_id.as_str()]);
        assert_ne!(
            code, 0,
            "a bundle carrying a shadow copy of {shadow} must NOT verify. The extractor writes it \
             over the attested file, so a VALID here means the verifier and the investigator are \
             looking at different documents:\n{out}"
        );
    }

    // And prove the premise rather than asserting it: `unzip` really does let the shadow win.
    let m = dir.join("shadow-proof.heldar-evidence");
    with_entries(&bundle, &m, &[("././manifest.json", forged)]);
    let extracted = extract_and_read(&m, &dir.join("x"), "manifest.json");
    assert!(
        extracted.contains("cam_forged"),
        "this test is only meaningful if unzip actually resolves `././manifest.json` onto \
         `manifest.json`. It did not, so the attack it guards against is not the attack being \
         built here — fix the test, not the assertion. Extracted: {extracted:.200}"
    );
}

/// Content the manifest does not list is not "harmless extra": it lands in the same folder as the
/// attested files, indistinguishable to whoever opens it, covered by no signature.
#[tokio::test]
async fn entries_the_manifest_does_not_list_are_refused() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("extra");
    let (_st, bundle, key_id) = build(&dir).await;

    let m = dir.join("extra.heldar-evidence");
    with_entries(
        &bundle,
        &m,
        &[
            ("EXHIBIT-B.txt", "the suspect confessed"),
            ("media/clip2.mp4", "FABRICATED SECOND ANGLE"),
        ],
    );
    let (code, out) = verify(&m, &["--key-id", key_id.as_str()]);
    assert_ne!(code, 0, "unlisted files must be refused:\n{out}");
    assert!(
        out.contains("EXHIBIT-B.txt"),
        "the refusal must name what it found:\n{out}"
    );
}

/// Two byte streams under one path. `zipfile` returns the last, `unzip -p` concatenates both — there
/// is no single "that file" for a signature to be about.
#[tokio::test]
async fn duplicate_entry_names_are_refused() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("dup");
    let (_st, bundle, key_id) = build(&dir).await;

    let m = dir.join("dup.heldar-evidence");
    with_entries(&bundle, &m, &[("media/clip.mp4", "FORGED FOOTAGE")]);
    let (code, out) = verify(&m, &["--key-id", key_id.as_str()]);
    assert_ne!(code, 0, "a duplicated entry must be refused:\n{out}");
    assert!(
        out.contains("more than once") || out.contains("twice"),
        "the refusal must say the entry appears more than once:\n{out}"
    );
}

/// The verdict must not depend on which run it is.
///
/// The original resolved names through a dict built from a `set`, and CPython randomises `str`
/// hashing per process — so on a bundle with two colliding spellings the answer flipped between
/// VALID and MODIFIED across runs on the identical file. Which answer is "right" does not matter: a
/// forensic tool that disagrees with itself is unusable as evidence.
#[tokio::test]
async fn the_verdict_is_the_same_every_run() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("determinism");
    let (_st, bundle, key_id) = build(&dir).await;
    let k = ["--key-id", key_id.as_str()];

    let m = dir.join("collide.heldar-evidence");
    with_entries(&bundle, &m, &[("./media/clip.mp4", "FORGED FOOTAGE")]);

    // Each run is a fresh process, so each gets a different PYTHONHASHSEED.
    for target in [&bundle, &m] {
        let first = verify(target, &k).0;
        for run in 2..=12 {
            let again = verify(target, &k).0;
            assert_eq!(
                again, first,
                "run {run} of {target:?} answered {again} where run 1 answered {first} — the same \
                 bytes must always get the same verdict"
            );
        }
    }
}

/// `key_id` is a claim inside the file; the identity is the public key. They must agree, or the
/// document is asserting an identity its own key does not support. (This check existed but had no
/// test — a mutation that stopped enforcing it survived the whole suite.)
#[tokio::test]
async fn a_signature_claiming_a_key_id_its_key_does_not_produce_is_refused() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("keyidlie");
    let (_st, bundle, key_id) = build(&dir).await;

    let m = dir.join("keyidlie.heldar-evidence");
    mutate(
        &bundle,
        &m,
        "signature.json",
        "import json as _j; d = _j.loads(data); d['key_id'] = 'sha256:' + '0'*64; \
         data = _j.dumps(d).encode()",
    );
    // Refused even when the operator asks for exactly the id the file claims — the claim is not the
    // identity, so believing it would let an attacker choose which appliance to impersonate.
    let (code, out) = verify(&m, &["--key-id", &format!("sha256:{}", "0".repeat(64))]);
    assert_ne!(code, 0, "a lying key_id must be refused:\n{out}");
    assert!(out.contains("claims key_id"), "{out}");

    let (code, _) = verify(&m, &["--key-id", key_id.as_str()]);
    assert_ne!(code, 0, "and refused against the real id too");
}

/// A bundle the index cannot record must not survive.
///
/// By the time the index write runs, the file is written, signed and attributed — so it is live at
/// `/media/evidence/<file>`. Returning the error without cleaning up told the operator the export
/// failed while a genuine, appliance-signed evidence document stayed on the box, absent from the
/// list of what left it. An unlisted signed bundle is worse than no bundle: it is real, it verifies,
/// and nothing on the appliance records that it exists or who caused it.
#[tokio::test]
async fn a_bundle_the_index_cannot_record_does_not_survive() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("orphan");
    let st = state(&dir).await;
    let (from, to) = seed(&st, "cam_a").await;

    // Stands in for a locked, full or otherwise unwritable database at exactly the wrong moment.
    sqlx::query("DROP TABLE evidence_bundles")
        .execute(&st.pool)
        .await
        .unwrap();

    let p = heldar_kernel::auth::Principal::system_admin();
    let err = evidence::export(&st, &p, "cam_a", from, to, None, None, None).await;
    assert!(err.is_err(), "the export must report failure");

    let left: Vec<String> = std::fs::read_dir(&st.cfg.evidence_dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| !n.starts_with(".stage-"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        left.is_empty(),
        "a failed export must leave no downloadable bundle behind, found: {left:?}"
    );

    let rows: Vec<(String,)> = sqlx::query_as("SELECT path FROM media_artifacts")
        .fetch_all(&st.pool)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "and no attribution row pointing at a file that is not there: {rows:?}"
    );
}

// =================================================================================================
// Container-level attacks: the file is not what the archive says it is.
//
// A second adversarial pass, run against the HARDENED verifier above, broke it again — and the break
// went straight past every name-level check, because the forged names never appear in the directory
// the verifier reads at all.
//
// A zip is read from the back. `cat forged.zip genuine.zip` gives a file that Python's zipfile,
// `unzip`, and any seeking reader see as the genuine bundle alone, while 7z's default mode and ANY
// streaming read walk local headers from the front and see the forged one. The verifier said VALID
// against the appliance's real key id, three runs running, while a streamed extraction wrote
// `cam_EVIL` and "FORGED CLIP - plate ABC-999" to disk.
//
// The invariant that reaches this is structural, not lexical: a streaming reader and a seeking
// reader must see the same archive, which holds exactly when every byte of the file belongs to an
// entry the central directory names.
// =================================================================================================

/// Concatenate two files.
fn concat(a: &Path, b: &Path, out: &Path) {
    let mut bytes = std::fs::read(a).expect("read a");
    bytes.extend_from_slice(&std::fs::read(b).expect("read b"));
    std::fs::write(out, bytes).expect("write concat");
}

/// Build a small forged archive that claims to be a different camera entirely.
fn forged_archive(dir: &Path) -> PathBuf {
    let tree = dir.join("forged");
    std::fs::create_dir_all(tree.join("media")).unwrap();
    std::fs::write(
        tree.join("manifest.json"),
        r#"{"format":"heldar-evidence/1","camera":{"id":"cam_EVIL","name":"Fabricated"},"files":{}}"#,
    )
    .unwrap();
    std::fs::write(tree.join("media/clip.mp4"), "FORGED CLIP - plate ABC-999").unwrap();
    std::fs::write(tree.join("signature.json"), "{}").unwrap();
    let out = dir.join("forged.zip");
    let o = Command::new("zip")
        .args(["-qXr"])
        .arg(&out)
        .arg(".")
        .current_dir(&tree)
        .output()
        .expect("zip");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    out
}

#[tokio::test]
async fn a_second_archive_concatenated_into_the_file_cannot_verify() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("concat");
    let (_st, bundle, key_id) = build(&dir).await;
    let k = ["--key-id", key_id.as_str()];
    let forged = forged_archive(&dir);

    // Prepended: the genuine EOCD is last, so every seeking reader — including this verifier —
    // sees only the genuine archive, while a streaming reader gets the forged one.
    let combined = dir.join("prepended.heldar-evidence");
    concat(&forged, &bundle, &combined);
    let (code, out) = verify(&combined, &k);
    assert_ne!(
        code, 0,
        "a forged archive concatenated in front of a genuine bundle must NOT verify — a streaming \
         extraction of this exact file writes different footage and a different camera to disk:\n{out}"
    );

    // The obvious repair an attacker makes next: fix up the EOCD's central-directory offset so the
    // arithmetic that catches the naive version no longer does.
    let repaired = dir.join("repaired.heldar-evidence");
    let script = "import struct, sys\n\
                  d = bytearray(open(sys.argv[1],'rb').read())\n\
                  shift = len(open(sys.argv[2],'rb').read())\n\
                  at = d.rfind(b'PK\\x05\\x06')\n\
                  off = int.from_bytes(d[at+16:at+20],'little')\n\
                  d[at+16:at+20] = struct.pack('<I', off + shift)\n\
                  open(sys.argv[3],'wb').write(bytes(d))\n";
    let o = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&combined)
        .arg(&forged)
        .arg(&repaired)
        .output()
        .expect("repairing the EOCD");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (code, out) = verify(&repaired, &k);
    assert_ne!(
        code, 0,
        "repairing the EOCD offset must not rescue it: the walk from byte 0 still meets local \
         headers the directory does not name:\n{out}"
    );

    // Appended data after the end of the archive.
    let appended = dir.join("appended.heldar-evidence");
    let mut bytes = std::fs::read(&bundle).unwrap();
    bytes.extend_from_slice(b"TRAILING FORGED DATA");
    std::fs::write(&appended, bytes).unwrap();
    let (code, out) = verify(&appended, &k);
    assert_ne!(code, 0, "bytes after the archive must be refused:\n{out}");

    // The control that makes the three above mean something.
    let (code, out) = verify(&bundle, &k);
    assert_eq!(code, 0, "the untouched bundle must still verify:\n{out}");
}

/// A crash is not a verdict — and it used to be indistinguishable from one.
///
/// Every uncaught exception exited 1, which IS the MODIFIED code, so a malformed file reported
/// itself as "the evidence was altered": a false accusation carrying the same exit code as a true
/// one, which a caller branching on exit codes cannot tell apart.
#[tokio::test]
async fn hostile_input_gets_a_verdict_not_a_traceback() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("hostile");
    let (_st, bundle, key_id) = build(&dir).await;
    let k = ["--key-id", key_id.as_str()];

    for (name, entry, body) in [
        ("list-manifest", "manifest.json", "[1,2,3]"),
        ("num-manifest", "manifest.json", "42"),
        ("str-signature", "signature.json", "\"a string\""),
        ("null-signature", "signature.json", "null"),
    ] {
        let m = dir.join(format!("{name}.heldar-evidence"));
        mutate(&bundle, &m, entry, &format!("data = {body:?}.encode()"));
        let (code, out) = verify(&m, &k);
        assert_eq!(
            code, 5,
            "{name} must be MALFORMED (5). Anything else means the verifier either crashed or \
             claimed a state it did not establish:\n{out}"
        );
        assert!(
            !out.contains("Traceback"),
            "{name} produced a traceback rather than a verdict:\n{out}"
        );
        // And it must be REFUSED BY NAME, not merely swallowed by the last-resort handler. The
        // catch-all turns any crash into MALFORMED, so asserting only on the exit code would let
        // the specific guards be deleted without a single test noticing — which is exactly how the
        // key_id check ended up untested. This pins the guard, not the safety net.
        assert!(
            out.contains("not a JSON object"),
            "{name} must be identified as a non-object document, not caught by the last-resort \
             handler — a message of 'could not be processed' tells an investigator nothing about \
             what is wrong with the file in front of them:\n{out}"
        );
    }

    // Not a zip at all, and an empty one.
    let junk = dir.join("junk.heldar-evidence");
    std::fs::write(&junk, vec![0u8; 512]).unwrap();
    assert_eq!(verify(&junk, &k).0, 5, "512 zero bytes must be MALFORMED");

    let empty = dir.join("empty.heldar-evidence");
    let o = Command::new("python3")
        .arg("-c")
        .arg("import sys,zipfile;zipfile.ZipFile(sys.argv[1],'w').close()")
        .arg(&empty)
        .output()
        .unwrap();
    assert!(o.status.success());
    let (code, out) = verify(&empty, &k);
    assert_eq!(code, 5, "an empty archive must be MALFORMED:\n{out}");
}

/// Damaged content must be reported as damaged content, naming the file — not as "could not be
/// processed". An investigator needs to know WHICH file would not come out.
#[tokio::test]
async fn a_file_that_will_not_decompress_is_reported_as_modified_and_named() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("crc");
    let (_st, bundle, key_id) = build(&dir).await;

    let broken = dir.join("crc.heldar-evidence");
    let script = "import sys, zipfile\n\
                  z = zipfile.ZipFile(sys.argv[1])\n\
                  i = [x for x in z.infolist() if x.filename == 'media/clip.mp4'][0]\n\
                  d = bytearray(open(sys.argv[1],'rb').read())\n\
                  off = i.header_offset + 30 + len(i.filename) + 2000\n\
                  d[off:off+40] = b'\\x00' * 40\n\
                  open(sys.argv[2],'wb').write(bytes(d))\n";
    let o = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&bundle)
        .arg(&broken)
        .output()
        .expect("corrupting the clip");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let (code, out) = verify(&broken, &["--key-id", key_id.as_str()]);
    assert_eq!(code, 1, "damaged content is MODIFIED:\n{out}");
    assert!(
        out.contains("media/clip.mp4"),
        "and it must name the file that would not come out:\n{out}"
    );
}

// =================================================================================================
// Round three. The container check from round two was itself broken, in BOTH directions.
//
// It asserted an arithmetic stand-in for the invariant — every byte covered by an entry the central
// directory names — rather than the invariant itself. Byte contiguity turns out to be strictly
// weaker than "the two readers agree":
//
//   * FORGERY ACCEPTED. Inflating one entry's CENTRAL-directory compressed size opens slack inside
//     its declared region. The cursor still landed exactly on the directory, so every byte was
//     accounted for — but a front-to-back inflater stops at the DEFLATE end-of-stream, not at the
//     declared size, and read the slack as a whole extra member. VALID three runs running while
//     `cat bundle | tar -x` wrote "FORGED CLIP - plate ABC-999" to disk.
//   * GENUINE EVIDENCE REFUSED. The arithmetic read the classic 32-bit end-of-archive record, so
//     any ZIP64 archive was rejected — including the appliance's OWN output above 4 GiB, which a
//     multi-hour export reaches easily. A verifier that refuses real evidence fails the
//     investigator exactly as badly as one that accepts a forgery.
//
// So the check now compares a streaming parse against the seeking one directly. These tests pin
// both directions, because fixing one by breaking the other is the obvious wrong turn.
// =================================================================================================

/// Repack a bundle's contents with a given command, producing an archive of identical content but a
/// different container shape. Returns the new bundle path.
fn repack(bundle: &Path, dir: &Path, tag: &str, args: &[&str]) -> PathBuf {
    let tree = dir.join(format!("tree-{tag}"));
    let _ = std::fs::remove_dir_all(&tree);
    std::fs::create_dir_all(&tree).unwrap();
    let o = Command::new("unzip")
        .arg("-oq")
        .arg(bundle)
        .current_dir(&tree)
        .output()
        .expect("unzip");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let out = dir.join(format!("repack-{tag}.heldar-evidence"));
    let _ = std::fs::remove_file(&out);
    let o = Command::new("zip")
        .args(args)
        .arg(&out)
        .arg(".")
        .current_dir(&tree)
        .output()
        .expect("zip");
    assert!(
        o.status.success(),
        "zip {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    out
}

/// A bundle whose content is byte-identical must verify however it was packed. The appliance's own
/// producer emits ZIP64 above 4 GiB — routine for a multi-hour export — and that was refused.
#[tokio::test]
async fn honestly_produced_containers_are_not_accused_of_forgery() {
    require("ffmpeg");
    require("zip");
    require("unzip");
    require("openssl");
    let dir = scratch("containers");
    let (_st, bundle, key_id) = build(&dir).await;
    let k = ["--key-id", key_id.as_str()];

    // -D drops directory entries (what the appliance passes); without it they are present; -fz
    // forces ZIP64 on a small archive, standing in for the >4 GiB export that CI cannot afford to
    // build. All three hold the same files.
    for (tag, args, note) in [
        ("appliance", vec!["-qXrD"], "the appliance's own flags"),
        (
            "dirents",
            vec!["-qXr"],
            "the same, keeping directory entries",
        ),
        (
            "zip64",
            vec!["-qXrDfz"],
            "ZIP64 — what the appliance emits above 4 GiB",
        ),
    ] {
        let p = repack(&bundle, &dir, tag, &args);
        let (code, out) = verify(&p, &k);
        assert_eq!(
            code, 0,
            "a bundle repacked with {note} holds exactly the same evidence and must verify. \
             Refusing it accuses the appliance's own output of being a forgery:\n{out}"
        );
    }

    // And the ZIP64 repack must really be ZIP64, or the test above proves nothing about ZIP64.
    let z64 = dir.join("repack-zip64.heldar-evidence");
    let bytes = std::fs::read(&z64).unwrap();
    let has = |sig: &[u8]| bytes.windows(4).any(|w| w == sig);
    assert!(
        has(b"PK\x06\x06") && has(b"PK\x06\x07"),
        "the zip64 fixture carries no ZIP64 records, so it is not exercising the path it names"
    );
}

/// A streamed producer writes data descriptors after each entry — 24 bytes with ZIP64 sizes, 16
/// without. Guessing 16 refused every ZIP64-streamed bundle, so the width is now taken from where
/// the next real record actually begins.
#[tokio::test]
async fn a_streamed_producers_data_descriptors_are_read_at_either_width() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("descriptors");
    let (_st, bundle, key_id) = build(&dir).await;

    let script = "import sys, zipfile\n\
                  src = zipfile.ZipFile(sys.argv[1])\n\
                  class NoSeek:\n    \
                      def __init__(s, f): s.f = f\n    \
                      def write(s, b): return s.f.write(b)\n    \
                      def flush(s): return s.f.flush()\n    \
                      def tell(s): raise OSError('not seekable')\n\
                  force = sys.argv[3] == 'zip64'\n\
                  with open(sys.argv[2], 'wb') as raw, \\\n     \
                          zipfile.ZipFile(NoSeek(raw), 'w', zipfile.ZIP_DEFLATED) as z:\n    \
                      for n in src.namelist():\n        \
                          with z.open(n, 'w', force_zip64=force) as fh:\n            \
                              fh.write(src.read(n))\n";
    for width in ["classic", "zip64"] {
        let out = dir.join(format!("streamed-{width}.heldar-evidence"));
        let o = Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(&bundle)
            .arg(&out)
            .arg(width)
            .output()
            .expect("streaming the bundle");
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let (code, msg) = verify(&out, &["--key-id", key_id.as_str()]);
        assert_eq!(
            code, 0,
            "a bundle streamed with {width} data descriptors holds the same evidence and must \
             verify:\n{msg}"
        );
    }
}

/// THE FORGERY THAT DEFEATED BYTE-COUNTING. Slack inside an entry's declared compressed region is
/// invisible to arithmetic and is a whole extra member to an inflater.
#[tokio::test]
async fn slack_inside_an_entrys_declared_data_cannot_hide_a_member() {
    require("ffmpeg");
    require("zip");
    require("openssl");
    let dir = scratch("slack");
    let (_st, bundle, key_id) = build(&dir).await;

    // Insert a complete STORED local member in the gap before the central directory, charge its
    // bytes to the LAST entry's central-directory compressed size, and move the directory offset.
    // Every byte is then accounted for by the directory — and a streaming reader sees an extra file.
    let script = "import struct, sys, zipfile\n\
                  p = sys.argv[1]\n\
                  d = bytearray(open(p, 'rb').read())\n\
                  z = zipfile.ZipFile(p)\n\
                  last = max(z.infolist(), key=lambda i: i.header_offset)\n\
                  eocd = d.rfind(b'PK\\x05\\x06')\n\
                  cd_off = int.from_bytes(d[eocd+16:eocd+20], 'little')\n\
                  name = sys.argv[3].encode()\n\
                  body = b'FORGED CLIP - plate ABC-999 (a streaming reader saw this)'\n\
                  member = (b'PK\\x03\\x04'\n            \
                            + struct.pack('<HHHHHIIIHH', 20, 0, 0, 0, 0, 0, len(body), len(body), len(name), 0)\n            \
                            + name + body)\n\
                  d[cd_off:cd_off] = member\n\
                  j = cd_off + len(member)\n\
                  while d[j:j+4] == b'PK\\x01\\x02':\n    \
                      nl = int.from_bytes(d[j+28:j+30], 'little')\n    \
                      el = int.from_bytes(d[j+30:j+32], 'little')\n    \
                      cl = int.from_bytes(d[j+32:j+34], 'little')\n    \
                      if bytes(d[j+46:j+46+nl]).decode() == last.filename:\n        \
                          cs = int.from_bytes(d[j+20:j+24], 'little')\n        \
                          d[j+20:j+24] = struct.pack('<I', cs + len(member))\n    \
                      j += 46 + nl + el + cl\n\
                  e2 = d.rfind(b'PK\\x05\\x06')\n\
                  d[e2+16:e2+20] = struct.pack('<I', cd_off + len(member))\n\
                  open(sys.argv[2], 'wb').write(bytes(d))\n";
    // Two shapes, because they are stopped by different things. Overwriting an attested name is
    // caught as a duplicate; a NEW name is not a duplicate of anything and is only caught by the
    // two readers disagreeing about what the archive contains. Testing only the first would leave
    // the general case — hide any file at all — unguarded.
    let attack = dir.join("slack.heldar-evidence");
    let sneaked = dir.join("slack-new-name.heldar-evidence");
    for (out, hidden) in [(&attack, "media/clip.mp4"), (&sneaked, "EXHIBIT-C.txt")] {
        let o = Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(&bundle)
            .arg(out)
            .arg(hidden)
            .output()
            .expect("building the slack attack");
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

        // THE PREMISE, proven rather than assumed: a front-to-back reader really does reach the
        // hidden member. Done with an independent parse rather than a system tool, because which
        // tool reads a zip from a pipe is platform-dependent — macOS `tar` is libarchive and does,
        // GNU `tar` does not — and a premise check that silently cannot run is the exact failure
        // this whole file exists to prevent. (CI caught this: the first version passed on macOS and
        // failed on Linux, which is a premise that was never being checked on one of them.)
        let streaming_parse = "import struct, sys, zlib\n\
                               d = open(sys.argv[1], 'rb').read()\n\
                               off = 0\n\
                               while d[off:off+4] == b'PK\\x03\\x04':\n    \
                                   fl, m = struct.unpack('<HH', d[off+6:off+10])\n    \
                                   cs, = struct.unpack('<I', d[off+18:off+22])\n    \
                                   nl, el = struct.unpack('<HH', d[off+26:off+30])\n    \
                                   name = d[off+30:off+30+nl].decode('utf-8', 'replace')\n    \
                                   at = off + 30 + nl + el\n    \
                                   if m == 8:\n        \
                                       o = zlib.decompressobj(-15)\n        \
                                       body = o.decompress(d[at:])\n        \
                                       used = len(d) - at - len(o.unused_data)\n    \
                                   else:\n        \
                                       body = d[at:at+cs]\n        \
                                       used = cs\n    \
                                   print(name + '\\t' + repr(body[:40]))\n    \
                                   off = at + used\n    \
                                   if fl & 8:\n        \
                                       for w in (16, 24, 12, 20):\n            \
                                           if d[off+w:off+w+4] in (b'PK\\x03\\x04', b'PK\\x01\\x02'):\n                \
                                               off += w; break\n        \
                                       else: break\n";
        let seen = Command::new("python3")
            .arg("-c")
            .arg(streaming_parse)
            .arg(out)
            .output()
            .expect("streaming parse");
        let seen = String::from_utf8_lossy(&seen.stdout).to_string();
        assert!(
            seen.contains("FORGED CLIP"),
            "this test is only meaningful if a front-to-back reader actually reaches the hidden \
             member. It did not, so the attack being guarded against is not the one being built — \
             fix the test, not the assertion. A streaming parse saw:\n{seen}"
        );

        let (code, msg) = verify(out, &["--key-id", key_id.as_str()]);
        assert_ne!(
            code, 0,
            "a member hidden as {hidden} in an entry's declared compressed slack must be refused \
             — every byte is accounted for by the directory, so byte-counting alone passes it:\n{msg}"
        );
    }
}
