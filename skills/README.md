# Heldar skills

*Issue [#124](https://github.com/Straits-AI/heldar/issues/124).*

## Tools, skills, and model reasoning

Three different things, and conflating them is how an agent ends up confidently wrong about a
recording.

- **Tools** are verbs. `heldar-mcp` and `heldarctl` expose them. A tool either works or errors; it
  has no opinion about what you should do next.
- **Skills** are operating procedure. They say which tools to use in which order, what counts as
  sufficient evidence, when to stop, and what must never be claimed. They are versioned, linted, and
  pinned to API versions.
- **Model reasoning** fills the gaps between steps. It is the least reliable layer and the one that
  invents continuity when data is missing, which is exactly what these skills exist to constrain.

**A skill grants no authority.** The kernel decides what a credential may do; a skill saying an
action is permitted does not make it so, and a skill saying an action is forbidden does not stop a
differently-instructed agent. Skills reduce the chance of a *well-behaved* agent doing the wrong
thing competently. They are not a security boundary — #123's structural read-only and the kernel's
capability checks are.

## Format

Each skill is a directory with a `SKILL.md` whose YAML frontmatter is machine-checked by
`scripts/validate_skills.py` (run in CI):

```yaml
---
name: heldar-incident-triage
version: 1.0.0
summary: One line, shown when an agent is choosing a skill.
compatible:
  core_api: ">=0.1.0 <1.0.0"     # the contract version from GET /api/v1/system
permitted_tools:                  # MUST exist in heldar-mcp or heldarctl
  - get_timeline
  - get_recording_gaps
prohibited_actions:               # MUST include every rule in the common safety set
  - actuate a gate, relay or PTZ
  - ...
---
```

The validator refuses:

- a `permitted_tools` entry that **does not exist** in `heldar-mcp`'s tool table or `heldarctl`'s
  commands — a skill naming a tool that is not there teaches an agent to hallucinate one
- a `permitted_tools` entry that is a **mutation**, unless the skill declares `mutating: true`
- a missing safety rule from the common set
- a missing required section
- a `compatible.core_api` range that does not parse, or that excludes the contract version this
  repository currently ships

## Required sections

`Purpose`, `Inputs`, `Prerequisites`, `Workflow`, `Stop conditions`, `Output`.

`Stop conditions` is the one that matters most and the one an author is most tempted to skip: it is
where the skill says *stop and ask a human* rather than proceeding on thin evidence.

## The common safety rules

Every skill must carry all of these in `prohibited_actions`. The validator enforces the list, so a
skill cannot be published having quietly dropped one:

- actuate a gate, relay or PTZ
- delete recordings, evidence or weaken retention
- create, modify or retrieve credentials
- identify a person from appearance similarity alone
- assert that nothing happened without first checking recording gaps
- present a correlation or hypothesis as an observation
