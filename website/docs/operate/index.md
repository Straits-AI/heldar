---
id: operate
title: Operate
sidebar_label: Operate
sidebar_position: 1
slug: /operate
---

# Operate

Running, securing, and maintaining a Heldar deployment. For getting a deployment
up in the first place, start with [Deploy](../getting-started/deploy.md).

The detailed operator and integrator guides currently live in the repository.
Each link below opens the in-repo guide on GitHub:

- [Access Control](https://github.com/Straits-AI/heldar/blob/main/docs/ACCESS-CONTROL.md)
  - the plate-authorization entry engine, the vehicle/visitor/watchlist registry,
  the guard confirm/reject workflow, RBAC, and reports.
- [Movement](https://github.com/Straits-AI/heldar/blob/main/docs/MOVEMENT.md)
  - multi-signal cross-camera ReID candidates (human-reviewed) and restricted-zone
  breach incidents.
- [Search](https://github.com/Straits-AI/heldar/blob/main/docs/SEARCH.md)
  - deterministic query over stored event facts, with the natural-language plan as
  the single fallible step and a proof layer over every answer.
- [Observability](https://github.com/Straits-AI/heldar/blob/main/docs/OBSERVABILITY.md)
  - the health/metrics/events APIs, Prometheus exposition, the alert webhook,
  storage monitoring, and recording-gap reporting.
- [Remote Access](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md)
  - reaching a site behind CGNAT over a WireGuard overlay, peer-to-peer first and
  end-to-end encrypted, with no proxy seeing the video.
- [Sizing](https://github.com/Straits-AI/heldar/blob/main/docs/sizing.md)
  - capacity planning for cameras, storage, and the AI frame budget.
- [Commissioning](https://github.com/Straits-AI/heldar/blob/main/docs/commissioning-checklist.md)
  - the checklist for bringing a new site online.

For the architecture behind these, see
[ARCHITECTURE.md](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)
and the [Architecture](../concepts/architecture.md) overview.
