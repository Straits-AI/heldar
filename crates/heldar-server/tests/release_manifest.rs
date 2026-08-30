//! The release manifest must describe the release it ships with.
//!
//! Its whole job is to let a verifier refuse a combination that was never tested together. A manifest
//! whose migration ceiling has drifted from the tree does the opposite: it certifies a pairing nobody
//! shipped, and the operator finds out when an older binary opens a newer database.
//!
//! So the ceiling is read from the migrations directory at generation time, and this asserts the
//! generator agrees with the tree — the number cannot be hand-maintained into staleness.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

/// The highest numeric migration prefix actually present in a directory.
fn max_migration(dir: &str) -> i64 {
    std::fs::read_dir(root().join(dir))
        .unwrap_or_else(|e| panic!("reading {dir}: {e}"))
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.strip_suffix(".sql")?
                .split('_')
                .next()?
                .parse::<i64>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

fn manifest() -> serde_json::Value {
    let out = Command::new("bash")
        .arg(root().join("scripts/gen_release_manifest.sh"))
        .arg("v0.0.0-test")
        .current_dir(root())
        .output()
        .expect("running the generator");
    assert!(
        out.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "manifest is not valid JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn the_migration_ceiling_matches_the_tree() {
    let m = manifest();
    assert_eq!(
        m["migrations"]["kernel_max"].as_i64(),
        Some(max_migration("crates/heldar-kernel/migrations")),
        "the manifest's kernel ceiling has drifted from the migrations on disk — it would certify a \
         binary/database pairing that was never shipped"
    );
    assert_eq!(
        m["migrations"]["entry_max"].as_i64(),
        Some(max_migration("crates/heldar-entry/migrations")),
        "the manifest's entry ceiling has drifted from the migrations on disk"
    );
}

#[test]
fn it_pins_every_deployment_file_the_stack_needs() {
    let m = manifest();
    let arts = m["artifacts"].as_object().expect("artifacts object");
    // Anything an operator composes with has to be pinned, or the manifest permits mixing a file
    // from another commit with these images — the exact combination #112 exists to prevent.
    for f in [
        "compose.yml",
        "compose.prod.yml",
        "compose.hardened.yml",
        "compose.tls.yml",
        "mediamtx.yml",
    ] {
        let e = arts
            .get(f)
            .unwrap_or_else(|| panic!("{f} is not pinned by the manifest"));
        let h = e["sha256"].as_str().unwrap_or_default();
        assert_eq!(h.len(), 64, "{f} has no usable sha256 ({h:?})");
    }
}

#[test]
fn it_records_the_commit_and_names_every_image() {
    let m = manifest();
    assert_eq!(
        m["git_sha"].as_str().map(str::len),
        Some(40),
        "the manifest must record the exact source commit"
    );
    for c in ["core", "web", "ai"] {
        assert!(
            m["components"][c]["image"].is_string(),
            "component {c} is missing"
        );
        // The digest is null when the registry was unreachable (a local run). Null is deliberate —
        // a manifest that invents a digest is worse than one admitting it lacks it — but it must be
        // present as a field so a consumer can tell "unknown" from "not applicable".
        assert!(
            m["components"][c].get("digest").is_some(),
            "component {c} must carry a digest field even when unresolved"
        );
    }
}
