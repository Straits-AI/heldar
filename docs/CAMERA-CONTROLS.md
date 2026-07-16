# Camera device controls, on-board ANPR, and barrier actuation

Heldar prefers a camera's **native capabilities** over reimplementing them server-side: if the
device can recognize plates, switch day/night modes, drive a relay, or pan/tilt/zoom, the kernel
exposes that through the dashboard instead of leaving it to the vendor's own web UI. This guide
covers the three surfaces that implement that principle (issues #43/#44/#45):

1. **Device controls** — day/night (IR-cut), image/lighting, relay outputs, PTZ (the Device panel).
2. **Camera-native ANPR** — ingesting the device's on-board plate reads as the ANPR source.
3. **Barrier/gate actuation** — pulsing the lane camera's relay on a matched entry.

Everything is configured in the dashboard (kernel settings + UI, per the no-box-side-scripts
principle) and gated by the kernel's RBAC.

## 1. The capability map (why the UI is vendor-neutral)

The dashboard renders per-camera device controls strictly from a **normalized capability map** the
kernel persists under the `device_control` key of `cameras.capabilities`:

```json
{
  "vendor": "hikvision",
  "day_night": true,
  "image": true,
  "io_outputs": [{ "id": 1, "name": "Gate relay", "default_state": "low" }],
  "native_anpr": true,
  "ptz": false,
  "supplement_light_modes": ["eventIntelligence", "colorVuWhiteLight", "irLight", "close"],
  "built_in_detections": [
    { "kind": "motion", "enabled": true },
    { "kind": "line_crossing", "enabled": false },
    { "kind": "intrusion", "enabled": false }
  ],
  "probed_at": "2026-07-16T04:12:00Z"
}
```

`supplement_light_modes` is the device's own option list (a hybrid-light model reports the four
modes above — `eventIntelligence` is the "smart night" mode: IR normally, white light on events;
an IR-only model reports just `irLight`/`close`). `built_in_detections` are the camera's OWN
smart-event features (motion / line-crossing / intrusion / …) with their arm state where readable —
shown informationally on the Device panel today; configuring them and ingesting their events into
the kernel is tracked as a follow-up (they are distinct from Heldar's server-side zone engine,
which works on any camera).

- **Probing is automatic**: the kernel fires a best-effort background probe when a camera is
  created or updated (address/credential changes), and the Device panel auto-probes once on first
  view if the camera has never been probed — so features appear without anyone pressing a button.
  `POST /api/v1/cameras/{id}/control/probe` (manager+) is the same probe on demand — the
  **"Re-detect"** button. Probing asks the device itself (capability documents), not a model-name
  lookup table, so it stays correct across firmware variants.
- `GET /api/v1/cameras/{id}/control/capabilities` returns the persisted map (a DB read; the UI
  never talks to the device just to render).
- Probes are best-effort per surface: an endpoint that errors leaves its capability `false`; a
  probe failure never affects camera add, streaming, or recording.
- `ptz` is sourced from the ONVIF probe (`camera_onvif.ptz_enabled`), so it is vendor-neutral; the
  other surfaces come from the vendor device-control provider (HikVision ISAPI today — the
  `CameraConfigProvider` trait has default "unsupported" answers, so adding Dahua/Uniview is a
  second impl, not a redesign).

## 2. Device controls (Device panel on the camera page)

| Surface | Endpoints | RBAC | ISAPI backing |
| --- | --- | --- | --- |
| Day/night mode (auto/day/night/schedule) | `GET/PUT .../control/day_night` | view / manager+ | `/ISAPI/Image/channels/1/ircutFilter` |
| Image & lighting (brightness/contrast/saturation, WDR, BLC, supplement light) | `GET/PUT .../control/image` | view / manager+ | `/ISAPI/Image/channels/1/{color,WDR,BLC,supplementLight}` |
| Relay outputs (list + test pulse) | `GET .../control/io/outputs`, `POST .../io/outputs/{port}/pulse` | view / manager+ | `/ISAPI/System/IO/outputs` |
| PTZ (existing) | `/api/v1/cameras/{id}/ptz/*` | view / manager+ | ONVIF Profile S |

Writes are **read-modify-write** against the device (the current XML is fetched, only the changed
fields are spliced in, and the result is PUT back), so device-managed fields are preserved and
settings the device does not expose are never blind-written. Every mutation lands in the immutable
audit log.

The raw output **pulse** endpoint is manager+ and intended for wiring verification ("test pulse").
The guard-facing gate-open goes through the entry app's policy (below), not this primitive.

## 2b. On-camera smart events (issue #46)

The camera's own detection engine (motion / line-crossing / intrusion) can drive Heldar directly —
the low-CPU alternative to server-side zones where the hardware supports it:

- **Arm/disarm from the Device panel**: the "Built-in detections" chips are toggles for the kinds
  that carry a device config resource (`PUT /api/v1/cameras/{id}/control/detections/{kind}`,
  manager+, audited).
- **Geometry editors (Device panel → Configure)**: draw **line-crossing lines** (up to the device's
  slot count, two clicks per line, direction + sensitivity per line) and **intrusion regions**
  (click-to-add polygon, dwell threshold + sensitivity per region) directly on the camera frame —
  written to the device via `GET/PUT .../control/line_crossing` and `.../control/intrusion`
  (read-modify-write: the item list is rebuilt inside the device's own document shell; slots you
  don't touch keep their device state). **Motion** exposes the arm switch + sensitivity
  (`GET/PUT .../control/motion`); the motion grid layout itself stays on-device (full-frame by
  default). Coordinates are normalized 0..1 in our API; the device speaks 0..1000.
- **Ingest events** (per-camera `native_events_enabled`, the "Ingest events" toggle): the kernel
  keeps one connection to the device's event notification stream
  (`/ISAPI/Event/notification/alertStream`) per opted-in camera (`services/camera_events.rs`).
  Active events are mapped to stable kinds (`VMD`→motion, `linedetection`→line_crossing,
  `fielddetection`→intrusion, tamper; unknown types pass through) and logged as
  `camera_<kind>` warning events — so webhooks, email, and the events feed see them — and every
  active block extends **event-mode recording** via the same trigger the zone engine uses.
- A rising-edge debounce logs ONE event per activity burst (the device re-posts `active` blocks
  ~1/s during motion; `HELDAR_CAMERA_EVENTS_REARM_SECS`, default 10, is the quiet gap that re-arms
  logging), while recording is extended continuously for the whole burst.
- Reliability: per-camera reader tasks with reconnect backoff, an idle watchdog (heartbeats stop →
  reconnect), and a reconcile loop that starts/stops readers as cameras opt in/out. One camera's
  failure never affects another.

**When to use which:** server-side zones work on ANY camera and support Heldar-drawn polygons,
confidence floors, and static suppression; on-camera events cost zero server CPU and use the
vendor's tuned detector, but geometry is configured on the device. Both feed the same event
machinery, so alerts/recording behave identically downstream.

## 3. Camera-native ANPR (issue #43)

Dedicated ANPR barrier cameras (e.g. HikVision iDS-series) recognize plates **on-device** with
specialized optics and illumination — at a gate lane this is usually more accurate than server-side
OCR, and it costs zero GPU. Enable it per camera on the Device panel (**On-board ANPR → Enable**),
or via `PATCH /api/v1/cameras/{id}` with `{"native_anpr_enabled": true}`.

How it works (`services/native_anpr.rs`):

- A supervised kernel poller queries each enabled camera's plate-results endpoint
  (`/ISAPI/Traffic/channels/1/vehicleDetect/plates`) every `HELDAR_NATIVE_ANPR_POLL_MS`
  (default 1000 ms), with a durable per-camera cursor (`camera_native_anpr_state`) so restarts
  resume where they left off. Poll failures are recorded on the state row and never affect other
  cameras.
- Each read becomes an ordinary `task_type = "anpr"` detection batch through the **same ingest
  path** the AI worker uses (outbox idempotency, all-or-nothing transaction, durable consumer
  fan-out) — the entry app's voting/identity/guard workflow consumes it unchanged.
- Reads carry `attributes.source = "camera_native"`, which the entry engine weights as
  **authoritative**: the device already consolidated multiple frames itself, so one read satisfies
  the vote threshold (worker OCR reads still need `HELDAR_ANPR_MIN_VOTES`, default 3).
- The device picture name is the idempotency key, so a crash between ingest and cursor advance can
  never double-count a vehicle.

**Source selection per camera:** *camera-native* = `native_anpr_enabled` on; *AI worker* = an
`anpr` AI task on the camera (unchanged existing behavior); *off* = neither. When enabling
camera-native on a lane, disable the camera's server-side `anpr` AI task to avoid double sources —
the Device panel reminds you.

## 4. Barrier/gate actuation (issue #44)

The entry pipeline stops at a recorded decision; actuation closes the loop. Most ANPR barrier
cameras have an alarm/relay output wired to the boom — Heldar pulses it through the kernel's
device-control primitive.

**Policy (Entry module → Gate tab):** per lane camera — auto-open on `matched` entries (on/off),
the relay output port, and the pulse width (100–30000 ms). Lanes start **manual-only**: verify the
wiring with "Open gate", then enable auto. A single global **kill-switch** halts all actuation,
automatic and manual.

**Safety posture** (`heldar-entry/src/gate.rs`):

- Auto-actuation runs fire-and-forget **after** the entry event is durably recorded — a slow or
  unreachable relay can never stall the perception pipeline or lose an event.
- Only `auth_status = "matched"` auto-opens; exceptions/unmatched/blocked never actuate (guards
  use the manual button after review). Manual open is guard+ (`can_operate_gate`) and audited with
  the acting principal.
- The relay release (set-low) is always attempted and retried once — a failed pulse must not leave
  the barrier relay latched open. There is deliberately **no retry queue**: a late gate pulse is
  worse than no pulse; failures surface as `gate_open_failed` warning events.
- Every actuation writes a `gate_opened` / `gate_open_failed` event to the kernel event log —
  subscribable by webhooks and the email notifier.

**External gate controllers (no camera relay):** subscribe a webhook to the entry events
(`entry_matched` etc. in the kernel event log carry plate, auth status, camera, direction) and
drive your controller from the HMAC-signed delivery — see `docs/ACCESS-CONTROL.md` §6 and the
webhook engine in `ARCHITECTURE.md` §14.3. The `gate_opened` events flow through the same channel,
so an external system can also observe actuations Heldar performed itself.

## 5. Environment knobs

| Variable | Default | Meaning |
| --- | --- | --- |
| `HELDAR_NATIVE_ANPR_POLL_MS` | `1000` | Poll cadence of the camera-native ANPR poller. |
| `HELDAR_CAMERA_EVENTS_REARM_SECS` | `10` | Quiet gap after which a new on-camera event burst logs a fresh event. |
| `HELDAR_ISAPI_REQUEST_TIMEOUT_MS` | `8000` | Per-request timeout for all ISAPI device calls (shared with camera config). |
| `HELDAR_ANPR_MIN_VOTES` | `3` | Worker-OCR vote threshold; camera-native reads are weighted to meet it in one read. |

Gate policy and the kill-switch are **runtime settings** (dashboard/API), not environment
variables — they live in the entry app's `gate_policies` / `gate_settings` tables.
