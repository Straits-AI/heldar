---
id: intro
title: Introduction
sidebar_label: Introduction
sidebar_position: 1
slug: /
---

# Heldar

Heldar is a visual event-intelligence operating system for physical spaces. It
turns camera streams into structured events, events into workflows, and
workflows into operational intelligence. Rather than wrapping an existing
DVR/NVR or starting from AI features, Heldar builds its own **media kernel**
first (camera registry, RTSP ingest, recording, playback, live view), then
layers perception, a zone engine, and apps on top as *consumers*. FFmpeg and
MediaMTX do the low-level media work; Heldar owns the metadata model, the event
engine, and the product logic.

## Open-core

Heldar is open-core:

- An Apache-2.0 **kernel** (`heldar-kernel`) plus **generic reference apps**
  (`heldar-entry`, `heldar-movement`, `heldar-search`), a reference composing
  server, a reference AI worker, and a React dashboard. This is the public
  `heldar` repository.
- **Vertical / client products** live as separate proprietary crates in a
  private repository and depend on the open crates. The kernel never references
  them.

Apps plug into the kernel only through a small set of public seams, so the
kernel has **no** dependency on any app. A deployment is *composed* from the
kernel plus whichever apps a client needs (single-tenant per deployment). See
[Open-core](./concepts/open-core.md) for the boundary and
[Architecture](./concepts/architecture.md) for the seams.

## Architecture at a glance

```text
                    +-----------------------------------------------+
   cameras --RTSP-->|  heldar-kernel  (Apache-2.0)                  |
                    |  media/DVR . perception ingest + sampler .    |
                    |  zone engine . auth/RBAC . observability .    |
                    |  retention . the DetectionConsumer + worker   |
                    |  SDK seams                                    |
                    +----------------+--------------+---------------+
   AI worker --/ai/events--> (perception)           | composed by heldar-server
   (apps/ai, HTTP client)                           |
                    +--------------------------------+---------------+
   OPEN generic apps (Apache-2.0)        PROPRIETARY verticals       |
   heldar-entry    (access control)      separate private crates     |
   heldar-movement (ReID / breach)       (depend on the open crates) |
   heldar-search   (semantic search)                                 |
                    +------------------------------------------------+
```

The kernel is the **only** component that talks to cameras. The 24/7 recorder
keeps the compressed stream decode-free; a budgeted sampler is the only thing
that decodes, writing one current frame per camera. AI workers are pure HTTP
clients: they pull sampled frames and post detections back. Apps interpret those
detections into domain events.

## Where to go next

- [Quickstart](./getting-started/quickstart.md) - build, run, add a camera, and
  run the AI worker.
- [Deploy](./getting-started/deploy.md) - one binary, one URL.
- [Architecture](./concepts/architecture.md) - the kernel and its four public
  seams.
- [Open-core](./concepts/open-core.md) - what is open versus proprietary.
- [Build a module](./develop/build-a-module.md) - build your own app against the
  open kernel.
- [Build an AI worker](./develop/ai-worker.md) - the perception worker SDK
  contract.
- [Operate](./operate/index.md) - the in-repo operator and integrator guides.

Source lives at
[github.com/Straits-AI/heldar](https://github.com/Straits-AI/heldar).
