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

See the crate docs for usage.
