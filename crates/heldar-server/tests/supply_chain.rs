//! The release workflows must keep signing what they publish.
//!
//! An attestation is easy to delete by accident: a workflow refactor drops a step, releases keep
//! succeeding, and nobody notices until someone tries to verify a binary months later and finds
//! nothing to verify against. Nothing else in CI fails when signing quietly stops, because signing is
//! not what any other job is checking.
//!
//! So this asserts the steps are present, and — more importantly — the properties that make them
//! meaningful: images attested by DIGEST rather than tag, and no long-lived signing key.

use std::path::PathBuf;

fn workflow(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join(".github/workflows")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

#[test]
fn published_binaries_are_attested_and_carry_an_sbom() {
    let w = workflow("release.yml");
    for needed in [
        "actions/attest-build-provenance",
        "actions/attest-sbom",
        "anchore/sbom-action",
    ] {
        assert!(
            w.contains(needed),
            "release.yml no longer runs `{needed}` — published binaries would ship unverifiable"
        );
    }
    // Sigstore's keyless flow needs both, and the failure without them is a confusing permissions
    // error at release time rather than anything obvious here.
    assert!(
        w.contains("id-token: write"),
        "no OIDC identity to sign with"
    );
    assert!(
        w.contains("attestations: write"),
        "cannot store attestations"
    );
}

#[test]
fn images_are_attested_by_digest_not_by_tag() {
    let w = workflow("docker-open.yml");
    assert!(
        w.contains("actions/attest-build-provenance"),
        "images would ship unverifiable"
    );
    assert!(
        w.contains("subject-digest:"),
        "an image attestation must name a DIGEST"
    );
    // A tag is mutable: attesting one certifies whatever it points at today, which is precisely the
    // substitution attestation exists to detect.
    assert!(
        !w.contains("subject-tag:"),
        "an image is being attested by TAG, which certifies nothing durable"
    );
    assert!(
        w.contains("push-to-registry: true"),
        "attestations should travel with the image as OCI referrers, so a mirror can be checked"
    );
}

/// The whole reason for keyless signing: a private key in repository secrets forges everything a
/// signature is meant to prevent, and #115 asks explicitly for it not to exist.
#[test]
fn no_long_lived_signing_key_is_referenced() {
    for f in ["release.yml", "docker-open.yml"] {
        let w = workflow(f);
        for banned in ["COSIGN_PRIVATE_KEY", "COSIGN_KEY", "SIGNING_KEY"] {
            assert!(
                !w.contains(banned),
                "{f} references {banned}: signing should be keyless OIDC, not a stored key"
            );
        }
    }
}
