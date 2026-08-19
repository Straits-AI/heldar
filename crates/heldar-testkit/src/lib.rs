//! Test harness for composed Heldar deployments.
//!
//! # Why this is a crate and not a test
//!
//! Camera scope is enforced per handler. The auth floor is structural — [`heldar_server::Verticals`]
//! merges vertical routers *before* `require_api_auth` is layered on, so no composed route can be
//! unauthenticated by accident — but nothing comparable forces a handler to consult
//! `principal.camera_scope()`. In this workspace 47 routes across three app crates shipped with no
//! scope call at all, with the primitives public in a crate they already depended on.
//!
//! Four rounds of adversarial hunting fixed those, but the durable lesson was not any individual
//! gap: it was that the test only ever examined routes it already knew about. Three separate times a
//! surface was invisible — `/metrics` because it mounts a layer up, then `/backup/*`, `/system`,
//! `/api-keys` and `/audit` because they carry no camera id in the path — and three separate times
//! the fix was to name the missing routes by hand, which cannot catch the next one.
//!
//! [`Census`] inverts that default: it enumerates EVERY registered route and fails unless each is
//! camera-keyed, provably refuses a camera-scoped credential, or is declared safe with a written
//! reason. An unguarded route becomes a CI failure without anyone remembering it exists.
//!
//! It lives here, rather than in `heldar-server/tests/`, because an integration test cannot be
//! imported. A private workspace composing proprietary verticals over the seam needs the SAME rule
//! over the SAME composed router — and a reimplementation drifts, which for this particular rule
//! means drifting back to "we test the routes we thought of".
//!
//! # Using it from a private workspace
//!
//! ```ignore
//! let census = Census::new(vec![repo_root().join("crates")])
//!     // Routes that legitimately answer a scoped credential. Each entry is a security
//!     // assertion — adding one should feel heavier than adding a guard.
//!     .scope_neutral(&[("/api/v1/vertical/thing", "camera_scope_filter on camera_id")])
//!     // Bodies that satisfy a handler's extractor so the probe reaches its GUARD rather than
//!     // dying at 422 — an unprobed route is not a proven one.
//!     .probe_body("/api/v1/vertical/thing", r#"{"name":"probe"}"#);
//!
//! let report = census
//!     .run(|method, path, body| async move { call(&state, &token, &method, &path, &body).await })
//!     .await;
//! report.assert_clean();
//! ```
//!
//! Scan the roots of BOTH workspaces so the census sees the composed surface. Enumerating only your
//! own routes while the kernel enumerates only its own leaves the union — the thing that actually
//! serves traffic — checked by nobody.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};

/// A route registration discovered in source: the path and the HTTP methods on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Route {
    pub path: String,
    pub methods: Vec<String>,
}

/// How a route satisfied the census, or why it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// Parameterised by a camera id — covered by the caller's camera-scope sweep.
    CameraKeyed,
    /// Probed, and it refused a camera-scoped credential. Proven here, not asserted elsewhere.
    Refuses,
    /// Declared to legitimately answer a scoped credential, with a reason.
    Declared(String),
    /// Declared unprovable by this harness, with a reason. Named debt, NOT coverage.
    Unproven(String),
    /// None of the above: it answered a credential whose scope it never consulted.
    Unclassified(String),
}

/// The census configuration: where to look, and what is declared safe.
pub struct Census {
    source_roots: Vec<PathBuf>,
    scope_neutral: Vec<(String, String)>,
    unproven: Vec<(String, String)>,
    probe_bodies: Vec<(String, String)>,
    camera_keys: Vec<String>,
    min_routes: usize,
}

impl Census {
    /// Scan these directory trees for `.route("<path>", …)` registrations.
    pub fn new(source_roots: Vec<PathBuf>) -> Self {
        Self {
            source_roots,
            scope_neutral: Vec::new(),
            unproven: Vec::new(),
            probe_bodies: Vec::new(),
            // The kernel keys on `/api/v1/cameras/{id}`; app crates use an explicit `{camera_id}`
            // under their own prefix. Missing the latter is why the app crates went unscoped.
            camera_keys: vec!["/api/v1/cameras/{id}".into(), "{camera_id}".into()],
            min_routes: 1,
        }
    }

    /// Routes that legitimately answer a camera-scoped credential, each with the reason it is safe.
    pub fn scope_neutral(mut self, entries: &[(&str, &str)]) -> Self {
        self.scope_neutral
            .extend(entries.iter().map(|(p, r)| (p.to_string(), r.to_string())));
        self
    }

    /// Routes this harness cannot reach the guard of, each with the reason. Named debt.
    pub fn unproven(mut self, entries: &[(&str, &str)]) -> Self {
        self.unproven
            .extend(entries.iter().map(|(p, r)| (p.to_string(), r.to_string())));
        self
    }

    /// A body that satisfies a handler's extractor so the probe reaches its guard.
    pub fn probe_body(mut self, path: &str, body: &str) -> Self {
        self.probe_bodies.push((path.to_string(), body.to_string()));
        self
    }

    /// Extra substrings marking a path as camera-keyed (for verticals using their own convention).
    pub fn camera_key(mut self, marker: &str) -> Self {
        self.camera_keys.push(marker.to_string());
        self
    }

    /// Fail if fewer than `n` routes are discovered — a scan that silently finds nothing would
    /// otherwise report a perfect census over an empty set.
    pub fn min_routes(mut self, n: usize) -> Self {
        self.min_routes = n;
        self
    }

    fn is_camera_keyed(&self, path: &str) -> bool {
        self.camera_keys
            .iter()
            .any(|k| path.starts_with(k.as_str()) || path.contains(k.as_str()))
    }

    /// Every `.route("<path>", …)` in production code under the configured roots.
    ///
    /// A source scan rather than a hand-maintained list, because the failure being guarded is
    /// someone adding a route and not scoping it — and a list they would also have to remember to
    /// update cannot catch that.
    pub fn discover(&self) -> Vec<Route> {
        let mut found: BTreeSet<Route> = BTreeSet::new();
        for file in self.sources() {
            let full = std::fs::read_to_string(&file).unwrap_or_default();
            // Routers declared inside `#[cfg(test)]` are fixtures, not product surface.
            let src = match full.find("#[cfg(test)]") {
                Some(i) => &full[..i],
                None => &full[..],
            };
            for (idx, _) in src.match_indices(".route(") {
                let tail = &src[idx..];
                // Bound to THIS call by balancing parens: a fixed window bleeds into the next
                // registration and attributes its methods to this path.
                let mut depth = 0i32;
                let mut end = tail.len();
                for (i, c) in tail.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let chunk = &tail[..end];
                let Some(q1) = chunk.find('"') else { continue };
                let Some(q2) = chunk[q1 + 1..].find('"') else {
                    continue;
                };
                let path = &chunk[q1 + 1..q1 + 1 + q2];
                if !path.starts_with('/') {
                    continue;
                }
                let mut methods = Vec::new();
                for m in ["get", "post", "patch", "delete", "put"] {
                    if chunk[q1 + q2..].contains(&format!("{m}(")) {
                        methods.push(m.to_uppercase());
                    }
                }
                if methods.is_empty() {
                    methods.push("GET".into());
                }
                found.insert(Route {
                    path: path.to_string(),
                    methods,
                });
            }
        }
        found.into_iter().collect()
    }

    fn sources(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    // `target/` is build output; scanning it finds vendored sources and is slow.
                    if p.file_name().map(|n| n == "target").unwrap_or(false) {
                        continue;
                    }
                    walk(&p, out);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    out.push(p);
                }
            }
        }
        let mut out = Vec::new();
        for root in &self.source_roots {
            walk(root, &mut out);
        }
        out
    }

    /// Classify every discovered route, probing the ones that are neither camera-keyed nor declared.
    ///
    /// `probe` is called as `(method, path, body)` and must issue the request AS A CAMERA-SCOPED
    /// CREDENTIAL, returning `(status, body)`. Mint that credential through the real key-creation
    /// endpoint: a fixture written straight into the credentials table can be a shape the product
    /// refuses to issue, and an assertion against a principal no deployment can hold is vacuous.
    pub async fn run<F, Fut>(&self, probe: F) -> Report
    where
        F: Fn(String, String, String) -> Fut,
        Fut: Future<Output = (u16, String)>,
    {
        let routes = self.discover();
        let mut report = Report {
            total: routes.len(),
            min_routes: self.min_routes,
            ..Default::default()
        };

        for route in &routes {
            if self.is_camera_keyed(&route.path) {
                report.camera_keyed += 1;
                continue;
            }
            if let Some((_, why)) = self.scope_neutral.iter().find(|(p, _)| *p == route.path) {
                report.declared.push((route.path.clone(), why.clone()));
                continue;
            }
            if let Some((_, why)) = self.unproven.iter().find(|(p, _)| *p == route.path) {
                report.unproven.push((route.path.clone(), why.clone()));
                continue;
            }
            for method in &route.methods {
                let path = route
                    .path
                    .replace("{id}", "probe_id")
                    .replace("{name}", "probe")
                    .replace("{key}", "probe");
                if path.contains('{') {
                    report.unclassified.push(format!(
                        "{method} {} -> UNPROBEABLE (unfillable path parameter; declare it, or key \
                         it by camera)",
                        route.path
                    ));
                    continue;
                }
                let body = self
                    .probe_bodies
                    .iter()
                    .find(|(p, _)| *p == route.path)
                    .map(|(_, b)| b.clone())
                    .unwrap_or_else(|| "{}".to_string());
                let (status, resp) = probe(method.clone(), path, body).await;
                // 403 is the refusal; 401 means the auth floor caught it first, which is also a
                // denial. 404 on a filled placeholder names nothing and carries no signal.
                let refused =
                    status == 403 || status == 401 || (status == 404 && route.path.contains('{'));
                if refused {
                    report.refuses += 1;
                } else {
                    report.unclassified.push(format!(
                        "{method} {} -> {status} for a camera-scoped credential (not camera-keyed, \
                         not declared scope-neutral, and does not refuse): {}",
                        route.path,
                        resp.chars().take(160).collect::<String>()
                    ));
                }
            }
        }

        // A declaration outliving its route silently pre-authorises whatever later claims that path.
        let live: BTreeSet<&str> = routes.iter().map(|r| r.path.as_str()).collect();
        report.stale = self
            .scope_neutral
            .iter()
            .chain(self.unproven.iter())
            .map(|(p, _)| p.clone())
            .filter(|p| !live.contains(p.as_str()))
            .collect();
        report
    }
}

/// The outcome of a census run.
#[derive(Debug, Default)]
pub struct Report {
    pub total: usize,
    pub camera_keyed: usize,
    pub refuses: usize,
    pub declared: Vec<(String, String)>,
    pub unproven: Vec<(String, String)>,
    pub unclassified: Vec<String>,
    pub stale: Vec<String>,
    min_routes: usize,
}

impl Report {
    /// One line naming what was covered and, separately, what is only DECLARED unprovable. The two
    /// are printed apart on purpose: rounding named debt up into a coverage figure is how a report
    /// starts reassuring instead of informing.
    pub fn summary(&self) -> String {
        format!(
            "route census: {} routes — {} camera-keyed, {} refuse a scoped credential, {} declared \
             scope-neutral, {} declared UNPROVEN (named debt, not coverage)",
            self.total,
            self.camera_keyed,
            self.refuses,
            self.declared.len(),
            self.unproven.len()
        )
    }

    /// Panic unless every route is accounted for. Prints [`Report::summary`] either way.
    pub fn assert_clean(&self) {
        eprintln!("{}", self.summary());
        assert!(
            self.total >= self.min_routes,
            "census discovered only {} routes (expected at least {}) — the SCAN is broken, not the \
             product; a census over an empty set passes trivially",
            self.total,
            self.min_routes
        );
        assert!(
            self.stale.is_empty(),
            "the census declares routes that no longer exist, so the declaration now pre-authorises \
             whatever next claims that path:\n  {}",
            self.stale.join("\n  ")
        );
        assert!(
            self.unclassified.is_empty(),
            "{} route(s) are neither camera-keyed, nor refuse a camera-scoped credential, nor \
             declared. Each answered a credential whose scope it never consulted — guard it, or \
             declare it with a reason:\n  {}",
            self.unclassified.len(),
            self.unclassified.join("\n  ")
        );
    }
}
