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
//!     .probe_body("/api/v1/vertical/thing", r#"{"name":"probe"}"#)
//!     // Routes addressed by a RESOURCE id: seed one owned by a camera the probing credential does
//!     // not hold, and name an id of the same shape that does not exist. The census requires the
//!     // two to be indistinguishable — the property `.unproven()` cannot express.
//!     .fixture("/api/v1/vertical/thing/{thing_id}", &seeded_id, "thing_does_not_exist");
//!
//! let report = census
//!     .run_with_control(
//!         |method, path, body| async move { call(&state, &scoped, &method, &path, &body).await },
//!         |method, path, body| async move { call(&state, &unscoped, &method, &path, &body).await },
//!     )
//!     .await;
//! report.assert_clean();
//! ```
//!
//! Use `.run(probe)` when you have no fixtures; a fixture needs the unscoped control, which is what
//! stops an id that was never seeded from agreeing with itself and reporting a clean census.
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
    /// Probed against a SEEDED out-of-scope resource and answered exactly as it answers for one that
    /// does not exist. The strongest thing this harness can say about a resource-addressed route.
    Indistinguishable,
    /// Refused, but at the CAPABILITY gate, by a capability a camera-scoped credential can never
    /// hold. A real denial — but it says nothing about the scope check behind it.
    CapabilityGated,
    /// Declared to legitimately answer a scoped credential, with a reason.
    Declared(String),
    /// Declared unprovable by this harness, with a reason. Named debt, NOT coverage.
    Unproven(String),
    /// None of the above: it answered a credential whose scope it never consulted.
    Unclassified(String),
}

/// A seeded resource that makes a route addressed by its OWN primary key probeable.
///
/// A route keyed by a resource id rather than a camera id is the shape that hid four defects here:
/// the handler must load the row to learn which camera owns it, and the obvious implementation
/// answers 404 for a missing id and 403 for someone else's — an oracle over the id space. A probe
/// with a synthetic id cannot see any of that, because it 404s before the guard runs.
///
/// So the fixture names two ids of the SAME shape: one a real row owned by a camera the probing
/// credential does NOT hold, one that names nothing at all. The census requires the two answers to
/// be indistinguishable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    /// Id of a REAL resource owned by a camera the probing credential does not hold.
    pub out_of_scope: String,
    /// A plausible id of the same shape, naming nothing.
    pub missing: String,
}

/// The census configuration: where to look, and what is declared safe.
pub struct Census {
    source_roots: Vec<PathBuf>,
    scope_neutral: Vec<(String, String)>,
    unproven: Vec<(String, String)>,
    probe_bodies: Vec<(String, String)>,
    probe_queries: Vec<(String, String)>,
    fixtures: Vec<(String, Fixture)>,
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
            probe_queries: Vec::new(),
            fixtures: Vec::new(),
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

    /// Make a resource-addressed route probeable by naming a SEEDED resource for it.
    ///
    /// `out_of_scope` must be the id of a row you actually seeded, owned by a camera the probing
    /// credential does not hold; `missing` an id of the same shape that names nothing. See
    /// [`Fixture`] for why both are needed, and [`Census::run_with_control`] for how the seeding is
    /// verified — a fixture that was never really seeded makes every assertion about it vacuous,
    /// which is the single most expensive mistake this harness can make.
    pub fn fixture(mut self, path: &str, out_of_scope: &str, missing: &str) -> Self {
        self.fixtures.push((
            path.to_string(),
            Fixture {
                out_of_scope: out_of_scope.to_string(),
                missing: missing.to_string(),
            },
        ));
        self
    }

    /// A query string that satisfies a handler's required parameters, so the probe reaches its GUARD.
    ///
    /// The sibling of [`Census::probe_body`], for handlers gated on the QUERY rather than the body. A
    /// route rejected at `Query<T>` extraction never reached its scope check, so without this it can
    /// only be recorded as unproven — which reads like a gap in the product when it is a gap in the
    /// harness.
    ///
    /// Where the query names a CAMERA, name one the probing credential does not hold: that turns the
    /// probe from a capability test into a scope test.
    pub fn probe_query(mut self, path: &str, query: &str) -> Self {
        self.probe_queries
            .push((path.to_string(), query.to_string()));
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
    ///
    /// Panics if any [`Census::fixture`] is configured — a fixture needs the control credential that
    /// only [`Census::run_with_control`] supplies.
    pub async fn run<F, Fut>(&self, probe: F) -> Report
    where
        F: Fn(String, String, String) -> Fut,
        Fut: Future<Output = (u16, String)>,
    {
        assert!(
            self.fixtures.is_empty(),
            "a seeded fixture needs a CONTROL credential to prove it was really seeded — call \
             run_with_control instead of run"
        );
        self.run_with_control(probe, |_m, _p, _b| async { (0u16, String::new()) })
            .await
    }

    /// [`Census::run`], plus a fleet-wide CONTROL credential used to prove each seeded fixture exists.
    ///
    /// `control` is called the same way as `probe` and must issue the request as an UNSCOPED
    /// credential. Nothing it returns is a pass or a fail on its own; it exists to defeat the one
    /// failure mode a fixture-based probe cannot see by itself. If the resource was never really
    /// seeded — a typo in the id, a table the harness forgot to migrate — then the "out of scope"
    /// probe is secretly a "does not exist" probe, the two answers agree trivially, and the route is
    /// reported as proven while nothing about its guard was exercised. That is not hypothetical
    /// bookkeeping: a fixture asserting against a credential the API refuses to mint is exactly how
    /// an earlier round of this suite shipped a page of vacuous assertions, green.
    ///
    /// So the fixture must be VISIBLE to the control: the two ids must produce different answers for
    /// a credential that holds every camera. If they do not, the census reports the fixture as
    /// unseeded rather than counting it.
    pub async fn run_with_control<F, Fut, G, Gut>(&self, probe: F, control: G) -> Report
    where
        F: Fn(String, String, String) -> Fut,
        Fut: Future<Output = (u16, String)>,
        G: Fn(String, String, String) -> Gut,
        Gut: Future<Output = (u16, String)>,
    {
        // A DECLARATION MUST NOT SILENTLY DISABLE EVIDENCE.
        //
        // `scope_neutral` is checked before `fixtures` below, so declaring a route neutral makes the
        // census SKIP it — fixture and all. That has now happened three times in this repository,
        // each time with the declaration's own text pointing at the very fixture it had disabled,
        // and each time it read as though the route had been checked. The author cannot see it: both
        // halves look present, and the suite is green.
        //
        // Neither is wrong on its own, so this refuses the COMBINATION rather than either piece.
        for (path, _) in &self.fixtures {
            if let Some((_, why)) = self.scope_neutral.iter().find(|(p, _)| p == path) {
                panic!(
                    "{path} has BOTH a fixture and a scope_neutral declaration. The declaration \
                     wins and the fixture never runs, so this route is not being checked at all. \
                     Delete one. If the route is genuinely scope-filtered, the fixture is the \
                     evidence and the declaration is the thing to remove.\n  declared: {why}"
                );
            }
        }

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
            if let Some((_, fx)) = self.fixtures.iter().find(|(p, _)| *p == route.path) {
                self.probe_fixture(route, fx, &probe, &control, &mut report)
                    .await;
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
                let body = self.probe_body_for(&route.path);
                let path = format!("{path}{}", self.probe_query_for(&route.path));
                let (status, resp) = probe(method.clone(), path, body).await;
                // 403 is the refusal; 401 means the auth floor caught it first, which is also a
                // denial.
                //
                // A 404 on a SYNTHETIC id is NOT a refusal, and counting it as one was this harness
                // asserting exactly what its own comment said it could not: "names nothing and
                // carries no signal". A camera-owned row addressed by its own primary key answers 404
                // both when it is out of scope and when it does not exist — that indistinguishability
                // is the property worth proving, and a probe against an id that was never seeded
                // proves only the second half. Five backup routes were counted as coverage on that
                // basis. Such a route now needs a `.fixture()` (which compares seeded-vs-missing) or
                // an explicit declaration; absent both it is reported, not counted.
                let refused = status == 403 || status == 401;
                if status == 404 && route.path.contains('{') {
                    report.needs_fixture.push(format!(
                        "{method} {} -> 404 on a synthetic id, which proves nothing: the row may be \
                         out of scope or may simply not exist. Add a .fixture() naming a real \
                         out-of-scope row and a missing one, or declare it.",
                        route.path
                    ));
                    continue;
                }
                if refused {
                    // Split out the refusals that happened at the CAPABILITY gate. Still a denial —
                    // a camera-scoped credential cannot hold `UNSCOPABLE_CAPS`, so it can never
                    // reach these at all — but the scope check behind the gate was never exercised,
                    // and a single "refuses" figure quietly reads as though it had been.
                    if status == 403 && resp.contains("missing capability") {
                        report
                            .capability_gated
                            .push(format!("{method} {} (capability gate)", route.path));
                    } else {
                        report.refuses += 1;
                    }
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
            .map(|(p, _)| p.clone())
            .chain(self.unproven.iter().map(|(p, _)| p.clone()))
            // A fixture outlives its route the same way a declaration does, and a stale one is worse
            // than useless: it seeds and probes a path nothing serves.
            .chain(self.fixtures.iter().map(|(p, _)| p.clone()))
            .filter(|p| !live.contains(p.as_str()))
            .collect();
        report
    }

    /// The body configured for this route, or the empty object.
    fn probe_body_for(&self, path: &str) -> String {
        self.probe_bodies
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| "{}".to_string())
    }

    fn probe_query_for(&self, path: &str) -> String {
        self.probe_queries
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, q)| format!("?{}", q.trim_start_matches('?')))
            .unwrap_or_default()
    }

    /// Drive one route against its seeded fixture.
    ///
    /// The property asserted is NOT "the scoped credential was refused" — a 404 is also a refusal and
    /// a 404 is precisely the leak. It is that the SEEDED out-of-scope resource and a resource that
    /// does not exist produce the SAME answer, so the route cannot be walked to learn which ids are
    /// real. Everything else here is about making sure that agreement means something.
    async fn probe_fixture<F, Fut, G, Gut>(
        &self,
        route: &Route,
        fx: &Fixture,
        probe: &F,
        control: &G,
        report: &mut Report,
    ) where
        F: Fn(String, String, String) -> Fut,
        Fut: Future<Output = (u16, String)>,
        G: Fn(String, String, String) -> Gut,
        Gut: Future<Output = (u16, String)>,
    {
        let (Some(real), Some(absent)) = (
            fill(&route.path, &fx.out_of_scope),
            fill(&route.path, &fx.missing),
        ) else {
            report.unclassified.push(format!(
                "{} -> a wildcard path parameter cannot be filled by one resource id; declare it",
                route.path
            ));
            return;
        };
        let body = self.probe_body_for(&route.path);

        // The SCOPED probes run first, all of them, before the control touches anything: a refused
        // probe cannot disturb the fixture, but a fleet-wide DELETE genuinely deletes it, and a
        // fixture consumed by an earlier method would make the later ones vacuous.
        let mut proven: Vec<&String> = Vec::new();
        for method in &route.methods {
            let (s_real, b_real) = probe(method.clone(), real.clone(), body.clone()).await;
            let (s_gone, b_gone) = probe(method.clone(), absent.clone(), body.clone()).await;
            // Fold the probed id out of both answers. A handler echoing the caller's OWN input
            // discloses nothing; one echoing the resource still differs after folding.
            let f_real = b_real.replace(&fx.out_of_scope, "{id}");
            let f_gone = b_gone.replace(&fx.missing, "{id}");
            if (s_real, &f_real) != (s_gone, &f_gone) {
                report.unclassified.push(format!(
                    "{method} {} -> a SEEDED out-of-scope resource answers {s_real} but a missing \
                     one answers {s_gone}; that difference is an oracle over the id space. real: {} \
                     | missing: {}",
                    route.path,
                    f_real.chars().take(120).collect::<String>(),
                    f_gone.chars().take(120).collect::<String>(),
                ));
                continue;
            }
            if s_real == 403 && f_real.contains("missing capability") {
                // A real denial, but the credential never reached the scope check behind it. Counted
                // apart so it is never read as evidence about that check.
                report.capability_gated.push(format!(
                    "{method} {} (refused at the capability gate)",
                    route.path
                ));
            } else {
                report.indistinguishable += 1;
            }
            proven.push(method);
        }

        // The control: whatever the scoped credential just proved, prove that there was something
        // there to prove. An unseeded fixture agrees with itself perfectly.
        for method in proven {
            let (c_real, cb_real) = control(method.clone(), real.clone(), body.clone()).await;
            let (c_gone, cb_gone) = control(method.clone(), absent.clone(), body.clone()).await;
            let f_real = cb_real.replace(&fx.out_of_scope, "{id}");
            let f_gone = cb_gone.replace(&fx.missing, "{id}");
            if (c_real, &f_real) == (c_gone, &f_gone) {
                report.unclassified.push(format!(
                    "{method} {} -> fixture `{}` is INDISTINGUISHABLE FROM MISSING for an UNSCOPED \
                     credential too ({c_real}), so the scoped probe agreed with itself about \
                     nothing. Seed the resource, or declare the route with a reason",
                    route.path, fx.out_of_scope,
                ));
            }
        }
    }
}

/// Substitute every `{placeholder}` in a route path with `value`.
///
/// `None` for a wildcard capture (`{*rest}`): it swallows an arbitrary tail, so no single resource id
/// stands in for it and pretending otherwise would probe a path the router never matches.
fn fill(path: &str, value: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let close = open + rest[open..].find('}')?;
        if rest[open + 1..close].starts_with('*') {
            return None;
        }
        out.push_str(&rest[..open]);
        out.push_str(value);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The outcome of a census run.
#[derive(Debug, Default)]
pub struct Report {
    pub total: usize,
    pub camera_keyed: usize,
    pub refuses: usize,
    /// (method, route) pairs where a SEEDED out-of-scope resource answered exactly as a missing one.
    pub indistinguishable: usize,
    /// Refused, but at the capability gate — the scope check behind it was never reached. Listed
    /// rather than counted, because each entry is a claim someone should be able to read and check.
    pub capability_gated: Vec<String>,
    pub declared: Vec<(String, String)>,
    pub unproven: Vec<(String, String)>,
    pub unclassified: Vec<String>,
    /// Resource-addressed routes whose only evidence was a 404 against an id nobody seeded.
    pub needs_fixture: Vec<String>,
    pub stale: Vec<String>,
    min_routes: usize,
}

impl Report {
    /// One line naming what was covered and, separately, what is only DECLARED unprovable. The two
    /// are printed apart on purpose: rounding named debt up into a coverage figure is how a report
    /// starts reassuring instead of informing. The capability-gated count is split out for the same
    /// reason in the other direction: it is a real denial, but not evidence about the scope check.
    pub fn summary(&self) -> String {
        format!(
            "route census: {} routes — {} camera-keyed, {} refuse a scoped credential, {} answer a \
             seeded out-of-scope resource exactly as a missing one, {} refuse at the capability gate \
             (a scoped credential can never hold it — proves nothing about the scope check), {} \
             declared scope-neutral, {} declared UNPROVEN (named debt, not coverage)",
            self.total,
            self.camera_keyed,
            self.refuses,
            self.indistinguishable,
            self.capability_gated.len(),
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
            self.needs_fixture.is_empty(),
            "{} resource-addressed route(s) answered 404 to a synthetic id and nothing else. That is \
             not proof of a scope check — an unguarded route answers it identically. Seed the row and \
             add a .fixture():\n  {}",
            self.needs_fixture.len(),
            self.needs_fixture.join("\n  ")
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
