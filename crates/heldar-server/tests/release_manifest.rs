//! The release manifest must describe the release it ships with.
//!
//! Its whole job is to let a verifier refuse a combination that was never tested together. A manifest
//! whose migration ceiling has drifted from the tree does the opposite: it certifies a pairing nobody
//! shipped, and the operator finds out when an older binary opens a newer database.
//!
//! So the ceiling is read from the migrations directory at generation time, and this asserts the
//! generator agrees with the tree — the number cannot be hand-maintained into staleness.
//!
//! The same argument applies to WHICH components have ceilings. The first version named kernel and
//! entry by hand and silently omitted movement and search, so their schemas shipped with nothing to
//! compare against: a database six migrations ahead on `movement` verified clean. A hardcoded list
//! of components goes stale exactly the way a hardcoded number does, so the generator discovers
//! them, and [`every_component_with_a_schema_has_a_ceiling`] holds it to the tree.

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

/// Every `crates/heldar-*/migrations` directory in the tree, as (component, dir).
fn components_with_schemas() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(root().join("crates"))
        .expect("crates/")
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let comp = name.strip_prefix("heldar-")?.to_string();
            let dir = format!("crates/{name}/migrations");
            root().join(&dir).is_dir().then_some((comp, dir))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_migration_ceiling_matches_the_tree() {
    let m = manifest();
    for (comp, dir) in components_with_schemas() {
        assert_eq!(
            m["migrations"][&comp].as_i64(),
            Some(max_migration(&dir)),
            "the manifest's {comp} ceiling has drifted from the migrations on disk — it would \
             certify a binary/database pairing that was never shipped"
        );
    }
}

/// The gap that made the ceiling check a half-measure: a component whose schema moves but whose
/// ceiling is absent is not "unchecked", it is a database the verifier waves through.
#[test]
fn every_component_with_a_schema_has_a_ceiling() {
    let m = manifest();
    let declared = m["migrations"].as_object().expect("migrations object");
    let found = components_with_schemas();
    assert!(
        found.len() >= 4,
        "expected at least kernel/entry/movement/search to carry schemas, found {found:?} — if a \
         crate was renamed this test is looking in the wrong place, not proving an absence"
    );
    for (comp, dir) in &found {
        assert!(
            declared.contains_key(comp),
            "{dir} carries migrations but the manifest declares no ceiling for {comp} — a release \
             that moves this schema ships a database nothing can refuse"
        );
    }
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

// ---------------------------------------------------------------------------------------------
// The verifier. A manifest nothing enforces is a text file, so these drive the real script and
// assert on its EXIT CODE — the thing an operator's `set -e` actually reads.
//
// Every case below was a silent `RESULT: PASS` at some point in this feature's history. They are
// here because the failure they describe was not hypothetical.
// ---------------------------------------------------------------------------------------------

/// A scratch directory for one test, under `target/` so it needs no dependency and no cleanup.
fn scratch(name: &str) -> PathBuf {
    let d = root().join("target/release-manifest-tests").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

/// Run the verifier; returns (exit_ok, combined output).
fn verify(
    manifest: &std::path::Path,
    deploy: &std::path::Path,
    db: Option<&std::path::Path>,
) -> (bool, String) {
    let mut c = Command::new("python3");
    c.arg(root().join("scripts/verify_release_manifest.py"))
        .arg(manifest)
        .arg(deploy)
        .env_remove("HELDAR_DB")
        .current_dir(root());
    if let Some(db) = db {
        c.env("HELDAR_DB", db);
    }
    let out = c.output().expect("running the verifier");
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

fn write(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, body).expect("write fixture");
    p
}

#[test]
fn the_verifier_accepts_the_tree_it_shipped_with() {
    let tmp = scratch("clean");
    let m = write(&tmp, "m.json", &manifest().to_string());
    let (okd, out) = verify(&m, &root().join("deploy"), None);
    assert!(okd, "an untouched tree must verify clean:\n{out}");
    assert!(out.contains("RESULT: PASS"), "{out}");
}

#[test]
fn the_verifier_refuses_a_modified_deployment_file() {
    let tmp = scratch("modified");
    let m = write(&tmp, "m.json", &manifest().to_string());
    let deploy = tmp.join("deploy");
    std::fs::create_dir_all(&deploy).expect("deploy dir");
    for f in std::fs::read_dir(root().join("deploy"))
        .expect("deploy")
        .flatten()
    {
        if f.path().is_file() {
            std::fs::copy(f.path(), deploy.join(f.file_name())).expect("copy");
        }
    }
    let target = deploy.join("compose.yml");
    let mut body = std::fs::read_to_string(&target).expect("read");
    body.push_str("\n# one byte of drift\n");
    std::fs::write(&target, body).expect("tamper");

    let (okd, out) = verify(&m, &deploy, None);
    assert!(!okd, "a modified compose.yml must fail:\n{out}");
    assert!(out.contains("compose.yml has been modified"), "{out}");
}

/// The bug that shipped: one malformed entry made the reader crash mid-stream, the loop verified the
/// entries BEFORE it, found them good, and reported PASS. A partial check must never read as a
/// complete one — which means refusing before verifying anything, not after.
#[test]
fn the_verifier_refuses_a_manifest_it_can_only_partly_read() {
    let tmp = scratch("partial");
    let mut m = manifest();
    m["artifacts"]["compose.prod.yml"] = serde_json::json!({});
    let p = write(&tmp, "m.json", &m.to_string());
    let (okd, out) = verify(&p, &root().join("deploy"), None);
    assert!(
        !okd,
        "a manifest with an unreadable entry must fail:\n{out}"
    );
    assert!(
        !out.contains("PASS compose.yml"),
        "it must refuse BEFORE verifying anything — reporting individual passes from a manifest it \
         cannot fully read is how a partial check gets read as a complete one:\n{out}"
    );
}

#[test]
fn the_verifier_refuses_an_empty_manifest() {
    let tmp = scratch("empty");
    for (name, body) in [
        (
            "empty.json",
            r#"{"artifacts":{},"migrations":{"kernel":17}}"#,
        ),
        (
            "nomig.json",
            r#"{"artifacts":{"compose.yml":{"sha256":"x"}},"migrations":{}}"#,
        ),
        ("garbage.json", "not json at all"),
    ] {
        let p = write(&tmp, name, body);
        let (okd, out) = verify(&p, &root().join("deploy"), None);
        assert!(!okd, "{name} must not verify clean:\n{out}");
    }
}

/// Build a database whose recorded migrations are exactly `versions`.
fn db_at(dir: &std::path::Path, name: &str, kernel: i64, apps: &[(&str, i64)]) -> PathBuf {
    let p = dir.join(name);
    let script = format!(
        "import sqlite3,sys\n\
         c=sqlite3.connect(sys.argv[1])\n\
         c.execute('CREATE TABLE _sqlx_migrations(version INTEGER, description TEXT)')\n\
         c.execute('INSERT INTO _sqlx_migrations VALUES (?, \\'x\\')', ({kernel},))\n\
         c.execute('CREATE TABLE _heldar_app_migrations(component TEXT, version INTEGER, name TEXT, checksum TEXT, applied_at TEXT)')\n\
         for comp, v in {apps:?}:\n    \
             c.execute('INSERT INTO _heldar_app_migrations VALUES (?,?,\\'n\\',\\'c\\',\\'t\\')', (comp, v))\n\
         c.commit()\n"
    );
    let out = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&p)
        .output()
        .expect("building fixture db");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    p
}

/// The check that only half existed: the kernel's ceiling was enforced, every other component's was
/// carried in the manifest and compared against nothing. A database ahead on `movement` verified
/// clean, which is the precise upgrade failure the ceiling exists to refuse.
#[test]
fn the_verifier_refuses_a_database_ahead_of_the_ceiling_on_any_component() {
    let tmp = scratch("ceiling");
    let m = manifest();
    let mp = write(&tmp, "m.json", &m.to_string());
    let ceil = |c: &str| {
        m["migrations"][c]
            .as_i64()
            .unwrap_or_else(|| panic!("no ceiling for {c}"))
    };
    let at_ceiling: Vec<(&str, i64)> = ["entry", "movement", "search"]
        .iter()
        .map(|c| (*c, ceil(c)))
        .collect();

    let good = db_at(&tmp, "ok.db", ceil("kernel"), &at_ceiling);
    let (okd, out) = verify(&mp, &root().join("deploy"), Some(&good));
    assert!(okd, "a database exactly at the ceiling must pass:\n{out}");

    // One component at a time, so a pass here can only come from that component being checked.
    for comp in ["kernel", "entry", "movement", "search"] {
        let kernel = if comp == "kernel" {
            ceil("kernel") + 5
        } else {
            ceil("kernel")
        };
        let apps: Vec<(&str, i64)> = at_ceiling
            .iter()
            .map(|(c, v)| if *c == comp { (*c, v + 5) } else { (*c, *v) })
            .collect();
        let db = db_at(&tmp, &format!("{comp}.db"), kernel, &apps);
        let (okd, out) = verify(&mp, &root().join("deploy"), Some(&db));
        assert!(
            !okd,
            "a database 5 migrations AHEAD on {comp} must be refused — it was written by a newer \
             release and this binary cannot serve it:\n{out}"
        );
        assert!(
            out.contains(&format!("{comp} schema is at migration")),
            "the refusal must name {comp}, or the operator cannot tell what is wrong:\n{out}"
        );
    }
}

/// An unreadable database is the state in which skipping the schema check is least defensible. It
/// used to report "fresh install": four kilobytes of random bytes verified clean.
#[test]
fn the_verifier_refuses_a_database_it_cannot_read() {
    let tmp = scratch("unreadable");
    let m = write(&tmp, "m.json", &manifest().to_string());
    let corrupt = tmp.join("corrupt.db");
    std::fs::write(&corrupt, vec![0xAB; 4096]).expect("write garbage");
    let (okd, out) = verify(&m, &root().join("deploy"), Some(&corrupt));
    assert!(
        !okd,
        "a corrupt database must not be reported as a fresh install:\n{out}"
    );

    // And a HELDAR_DB pointing nowhere must not report "no HELDAR_DB set" — a check that did not
    // run for the wrong stated reason is how an operator concludes it ran.
    let (okd, out) = verify(&m, &root().join("deploy"), Some(&tmp.join("absent.db")));
    assert!(!okd, "HELDAR_DB set to a missing path must fail:\n{out}");
    assert!(
        !out.contains("no HELDAR_DB set"),
        "wrong reason reported:\n{out}"
    );
}
