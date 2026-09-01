#!/usr/bin/env python3
"""Every composite-action output must have a `value`, and it must point at something real.

Run: python3 scripts/check_action_outputs.py

A composite action's `outputs:` entry with a `description` but no `value` is VALID YAML and valid
action syntax. It just always resolves to the empty string. Nothing warns; the consumer receives ""
and fails somewhere else entirely, or — worse — succeeds on the wrong file.

That is not hypothetical. `.github/actions/build-musl-binary` shipped for two merges with:

    outputs:
      asset:
        description: Path to the named heldar-core binary asset      # <- no value
      cli-asset:
        description: Path to the named heldarctl binary asset
        value: ${{ steps.package.outputs.asset }}                    # <- the CORE binary's name

so the core asset path was empty and the CLI's pointed at the core. It went unnoticed because no
release ran in between: the failure would have arrived on release day, in the one workflow nobody can
re-run cheaply.

Checks, per composite action:
  * every declared output has a `value`
  * every `steps.X.outputs.Y` it references names a step `X` that exists in the action
  * that step actually writes `Y` to `$GITHUB_OUTPUT`
  * no two outputs resolve to the same expression (which is how the swap above reads)
"""

import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ACTIONS = os.path.join(ROOT, ".github", "actions")

problems = []


def check(path, doc_text):
    name = os.path.relpath(path, ROOT)

    # The `outputs:` block, to the next top-level key.
    m = re.search(r"^outputs:\n((?:[ \t].*\n|\n)*)", doc_text, re.M)
    if not m:
        return 0
    block = m.group(1)

    # `  key:` at two-space indent starts an output.
    entries = re.findall(r"^  ([A-Za-z0-9_-]+):\n((?:    .*\n|\n)*)", block, re.M)
    if not entries:
        problems.append(f"{name}: an `outputs:` block that declares nothing — is it malformed?")
        return 0

    step_ids = set(re.findall(r"^\s*-?\s*id:\s*(\S+)\s*$", doc_text, re.M))
    written = set(re.findall(r'echo\s+"?([A-Za-z0-9_-]+)=', doc_text))

    seen_values = {}
    for out_name, body in entries:
        vm = re.search(r"^\s*value:\s*(.+?)\s*$", body, re.M)
        if not vm:
            problems.append(
                f"{name}: output {out_name!r} has no `value`, so it always resolves to the empty "
                f"string. Consumers get '' with no warning."
            )
            continue
        value = vm.group(1)
        if value in seen_values:
            problems.append(
                f"{name}: outputs {seen_values[value]!r} and {out_name!r} both resolve to "
                f"{value} — one of them is almost certainly meant to point somewhere else."
            )
        seen_values[value] = out_name

        ref = re.search(r"steps\.([A-Za-z0-9_-]+)\.outputs\.([A-Za-z0-9_-]+)", value)
        if not ref:
            continue
        step, key = ref.group(1), ref.group(2)
        if step_ids and step not in step_ids:
            problems.append(
                f"{name}: output {out_name!r} reads `steps.{step}.outputs.{key}`, but no step in "
                f"this action has `id: {step}`."
            )
        elif key not in written:
            problems.append(
                f"{name}: output {out_name!r} reads `steps.{step}.outputs.{key}`, but no step "
                f'writes `{key}=` to $GITHUB_OUTPUT.'
            )
    return len(entries)


def main():
    if not os.path.isdir(ACTIONS):
        print("no .github/actions directory")
        return 0
    checked = 0
    files = 0
    for root, _, names in os.walk(ACTIONS):
        for n in names:
            if n in ("action.yml", "action.yaml"):
                p = os.path.join(root, n)
                checked += check(p, open(p).read())
                files += 1
    if not files:
        print("no composite actions found — refusing to report success on an empty check")
        return 1
    print(f"checked {checked} output(s) across {files} composite action(s)")
    if problems:
        print(f"\n{len(problems)} problem(s):")
        for p in problems:
            print(f"  {p}")
        return 1
    print("RESULT: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
