# heldar-testkit

Test harness for composed Heldar deployments.

Its one job is the **route census**: enumerate every registered route and fail unless each is
camera-keyed, provably refuses a camera-scoped credential, or is declared safe with a written
reason. An unguarded route becomes a CI failure by default, rather than waiting for someone to
notice it.

That default matters more than it sounds. Camera scope in this codebase was audited four times.
Three separate rounds found gaps that were invisible because the test only examined routes it
already knew about — and each fix named the missing routes by hand, which cannot catch the next one.

This lives in a crate rather than an integration test so a private workspace composing proprietary
verticals over the `Verticals` seam runs the *same* rule over the *same* composed router. Point it
at both workspaces' source roots: enumerating only your own routes while the kernel enumerates only
its own leaves the union — the thing that actually serves traffic — checked by nobody.

## Routes addressed by a resource id

A route keyed by its own primary key rather than a camera id cannot be probed with a made-up id: the
handler 404s on the missing row before the guard runs. Hand the census a **fixture** instead — a real
resource owned by a camera the probing credential does not hold, plus an id of the same shape that
names nothing:

```rust
census.fixture("/api/v1/thing/{thing_id}", &seeded_id, "thing_does_not_exist")
```

It then requires the two answers to be **indistinguishable**. "Refused" is not the property: a 404 is
also a refusal, and answering 404 for a missing resource while answering 403 for someone else's turns
the route into an oracle over the id space. That exact shape hid four defects in this repository.

Fixtures are used through `run_with_control`, which also takes an **unscoped** credential. Nothing it
returns is a pass or a fail; it exists so a fixture that was never really seeded is caught. Without it
the "out of scope" probe is secretly a "does not exist" probe, the two answers agree trivially, and the
route is reported as proven while nothing was exercised.

See the crate docs for usage.
