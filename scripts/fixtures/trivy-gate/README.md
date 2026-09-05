# Trivy gate self-test fixtures

**The versions in `vulnerable/` are deliberately, knowingly vulnerable. Do not "fix" them.**
Nothing installs or imports them; they exist only to be scanned.

## Why

`.github/workflows/security.yml` runs Trivy with `exit-code: "1"` so a HIGH/CRITICAL finding fails
the build. That flag was **absent** for a long time: the job produced SARIF, the dashboard filled up,
and every PR showed green. Adding the flag fixed it — but nothing proves the flag is still there and
still doing something, and a security gate whose failure path has never once executed is a gate
nobody has tested.

So the workflow scans these two directories with the *same* action and the *same* inputs as the real
scan, and asserts the outcome:

| fixture | expected | proves |
| --- | --- | --- |
| `vulnerable/` | scan **fails** | the gate blocks on a real finding |
| `clean/` | scan **passes** | the gate is not simply always red |

Both matter. Without the clean case, a Trivy that failed on everything would pass the self-test.

## Why four packages

Any one advisory can be re-scored, withdrawn, or re-classified below HIGH, which would silently make
a single-package fixture stop proving anything. These four carry ~48 advisories between them, all
with fixed versions available — so `ignore-unfixed: true`, which the real scan sets, cannot hide
them either.

If the self-test ever reports the vulnerable fixture as clean, the fixture has gone stale rather than
the world having gotten safer. Add an older package; do not delete the check.

## Why the real repo scan does not trip over this

`scripts/fixtures` is in the real scan's `skip-dirs`. `scripts/check_trivy_gate.py` asserts that it
stays there — without it these fixtures would fail every unrelated PR, and the first person to hit
that would quite reasonably delete them.
