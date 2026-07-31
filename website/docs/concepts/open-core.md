---
id: open-core
title: Open-core
sidebar_label: Open-core
sidebar_position: 2
---

# Open-core

Heldar ships as an open-core platform: an Apache-2.0 kernel and a set of generic
reference apps are public, while vertical and client-specific products are
separate proprietary crates in a private repository. The split is enforced at the
crate boundary, not by feature flags inside one codebase.

## What is open (Apache-2.0)

The public `heldar` repository contains:

- **`heldar-kernel`** - the domain-agnostic platform: media/DVR, perception
  ingest and the frame sampler, the zone engine, auth/RBAC, observability,
  retention, and the public seams (the `DetectionConsumer` trait, the
  `Router<AppState>` merge, the self-installed schema pattern, and the auth
  primitive).
- **Generic reference apps**, each a real app built only on the kernel's public
  seams:
  - **`heldar-entry`** - generic access control. Plate authorization (a
    `DetectionConsumer`), a vehicle/visitor/watchlist registry, a guard
    confirm/reject workflow, and entry/exception/audit reports. Domain-neutral:
    any gated-entry deployment uses it as-is.
  - **`heldar-movement`** - cross-camera correlation. A multi-signal ReID
    candidate proposer and a restricted-zone breach rule engine, both running as
    supervised background loops over already-stored kernel facts. An optional,
    off-by-default `appearance_score` (CLIP crop similarity) can add an additive
    ReID signal secondary to the plate anchor.
  - **`heldar-search`** - semantic search. A read-only query layer that turns a
    natural-language question into a structured plan, executes it deterministically
    against stored event facts, and returns the rows as the answer. Now also
    includes CLIP similarity retrieval (text and image queries) over
    kernel-stored crop embeddings — still read-only, still no model-generated
    answers.
- **`heldar-server`** - the reference composing binary that links the kernel and
  the generic apps.
- **`apps/ai`** - the reference Python AI worker.
- **`apps/web`** - the React + Vite + TypeScript dashboard.
- The docs, infra (MediaMTX config), and scripts.

## What is proprietary

Vertical and client-specific products live as separate crates, each in its own
private repository (one per product, so their release cycles stay independent).
They **depend on** the open crates (via crates.io, with a local path patch for
side-by-side development) and layer their domain specifics on top. They are never
merged into this repo, and the kernel never references them.

The composing server is a **library**: `heldar_server::run(impl Verticals)` takes
a four-hook composition trait, so a private product builds its own binary in a few
dozen lines against these crates (the kernel and apps from crates.io; the
composition crate by git tag) — no fork of this tree, no file substitution. The
`heldar-core` binary here composes the open apps and a no-op `Verticals`, so the
reference build links zero proprietary code.

## Why this shape

Owning the kernel means owning the metadata model, the event engine, and the
product logic, while the seams keep apps decoupled from it. A deployment is
composed from the kernel plus whichever apps a client needs (single-tenant per
deployment), so the open generic apps and any proprietary verticals are just
crates linked into a server build. Breaking changes to the kernel seams are a
major version bump that apps opt into.

## Licensing

The kernel and the generic apps are Apache-2.0. Proprietary verticals are
licensed separately. See
[LICENSING.md](https://github.com/Straits-AI/heldar/blob/main/LICENSING.md) for
the boundary.
