# Camera Commissioning Checklist

Field checklist for installing and onboarding cameras into **Heldar Core**
(Stage 0 media kernel). Work top to bottom; do not skip a phase. A camera is
not "live" until **Phase 7 — Verification** passes.

> Why this matters: bad angle, lighting, framing, or stream config silently
> destroys ANPR, ReID, tracking, and behaviour analytics downstream. The cheapest
> place to fix a camera is on the ladder, not in the model. (memo §15.2)

**Legend:** API base = `http://<core-host>:8000`. MediaMTX defaults: API `:9997`,
HLS `:8888`, WebRTC `:8889`, RTSP `:8554`. Default segment length 60 s,
default retention 24 h.

---

## Phase 0 — Pre-site planning (do before you arrive)

- [ ] Site survey done: floor plan / site map with intended camera positions and
      coverage zones marked.
- [ ] Per-camera **purpose** declared up front — this drives everything below:
      - [ ] ANPR (plate capture at a gate/lane)
      - [ ] ReID / body (person re-identification, tracking across cameras)
      - [ ] Face (detection / optional recognition — privacy rules apply, see §15.5)
      - [ ] General overview / situational awareness
- [ ] IP plan ready: static IP or DHCP reservation per camera on the **CCTV VLAN**.
- [ ] Bandwidth + storage budget sanity-checked
      (`cameras × bitrate × 1.0` overhead for net; `1 Mbps ≈ 10.8 GB/day` for storage).
- [ ] Naming convention agreed (e.g. `gate-north-anpr`, `lobby-overview`) — the
      camera `name` becomes the Heldar `id` slug.
- [ ] Credentials provisioned per the rules in **Phase 6** (no shared passwords,
      no brute-forcing the test units).

---

## Phase 1 — Network preparation (memo §12)

PoE cameras → managed switch → media/edge server → core switch → cloud control plane.
Cameras must be on an isolated CCTV VLAN, never on the corp/guest network.

- [ ] PoE budget checked: switch PoE wattage ≥ sum of camera draw (+ IR/heater load).
      Confirm PoE class (802.3af / at / bt) matches each camera.
- [ ] Cable runs tested (link up, no CRC errors); runs within 100 m copper limit
      or fibre/PoE extenders accounted for.
- [ ] VLAN segregation in place (memo §12.3):
      - [ ] **CCTV VLAN** — cameras only
      - [ ] **Media VLAN** — ingest/recording servers
      - [ ] **AI VLAN** — GPU/AI workers (Stage 1+)
      - [ ] **Storage VLAN** — NAS / recording storage
      - [ ] **Operator VLAN** — dashboards
      - [ ] **Management VLAN** — switches / firewall / admin
      - [ ] **Guest/Corp VLAN** — isolated from CCTV
- [ ] Inter-VLAN routing locked down: only the Media VLAN (Heldar core) may
      reach the CCTV VLAN on RTSP (554) and the camera HTTP/ONVIF ports. No path
      from Guest/Corp to CCTV.
- [ ] Cameras have **no internet egress** (block at firewall; disable cloud/P2P
      features on the camera).
- [ ] Switch uplink sized for aggregate camera bitrate with headroom
      (e.g. 32 × 4 Mbps ≈ 128 Mbps → provision 160–200 Mbps; use 10GbE core on
      medium/large sites).
- [ ] NTP reachable by every camera **and** the core host — clock skew corrupts the
      timeline index and playback alignment. Confirm camera time matches server time.
- [ ] Core host can `ping` and reach the camera's RTSP port (verify from the Media
      VLAN, not your laptop on the corp network).

---

## Phase 2 — Physical placement, angle & lighting (memo §15.2)

- [ ] Mounting height and tilt match the purpose (low/frontal for plates & faces;
      higher/wider for overview & tracking).
- [ ] **Angle of incidence** kept shallow for ANPR/face: aim for the plate/face
      roughly square to the lens — keep horizontal and vertical off-axis under
      ~30° each. Steep angles smear characters and faces.
- [ ] Sun path checked: camera not staring into sunrise/sunset; no strong backlight
      behind the target (silhouetting kills plate/face contrast).
- [ ] Headlight wash / glare controlled at vehicle gates (WDR enabled, exposure
      zone set on the plate region, not the sky).
- [ ] No occlusions through the full field (poles, foliage, signage, gate arms in
      the closed position).
- [ ] IR / night tested **on site at night** (not assumed):
      - [ ] Plates and faces still legible under IR; no IR hotspot blowout on
            retroreflective plates (tune IR intensity / use external illuminator).
      - [ ] No fogging/flare on the dome; correct day↔night switch behaviour.
- [ ] Weather/IP rating adequate for the location; lens clean and focused.

---

## Phase 3 — Per-camera stream configuration

Configure on the camera's own web UI before onboarding. Heldar records the
**compressed stream without decode**, so the camera-side encode settings are what
you actually store.

- [ ] **Codec:** prefer **H.265 (HEVC)** for bandwidth/storage savings where the
      ingest + playback path is confirmed to support it; fall back to **H.264** for
      maximum compatibility (browser playback, broad tooling). Pick one per camera
      and record it — mixed-codec sites are fine but note it.
- [ ] **Main stream (record):** full sensor resolution, the fps and bitrate the
      use case needs (see Phase 4). This is the evidence-grade stream.
- [ ] **Sub stream (live/preview & later AI sampling):** lower resolution
      (e.g. 640–720p), modest fps, lower bitrate — keeps live view and analytics cheap.
- [ ] **Stream roles assigned:** decide which stream Heldar records via
      `record_stream` (`main` or `sub`). Default and norm = `main` for evidence;
      use `sub` only for low-value overview cameras.
- [ ] **Keyframe / GOP interval** set to ~1× fps (i.e. 1 IDR per second). Long GOPs
      make segment cuts and clip/snapshot seeks imprecise.
- [ ] **CBR vs VBR:** CBR (or capped VBR) for predictable storage sizing.
- [ ] **fps:** 12–15 fps is plenty for overview; ANPR lanes may want higher with a
      fast shutter (below). Audio off unless required.
- [ ] Note the configured `resolution_main` / `resolution_sub` and `fps_main` /
      `fps_sub` — they go into the registry for sizing/health.

---

## Phase 4 — Use-case framing: pixels-on-target

Frame for the **minimum pixels on the target**, measured where the target actually
appears (entry to the zone), not at the closest point. Useful reference scale
(DORI / EN 62676-4): Detect ~25 px/m, Observe ~62, Recognise ~125, Identify ~250 px/m.

### ANPR / plate
- [ ] **Plate width ≥ ~120–150 px** across the plate at the capture line;
      **character height ≥ ~20–30 px**. Below this OCR degrades fast.
- [ ] **Lane control:** one camera covers **one lane**; constrain the capture point
      (stop line / gate) so the vehicle is near-stationary or slow.
- [ ] **Shutter fast enough to freeze motion** (e.g. ≥ 1/1000 s for moving
      vehicles) — motion blur is unrecoverable. Raise fps only after the shutter is
      fast enough to use it.
- [ ] Dedicated/tuned IR or external illuminator on the plate region for night;
      verify no blowout on retroreflective plates (Phase 2).
- [ ] (Malaysia, memo §15.4) expect motorcycles, rain, mixed plate styles — frame
      for the worst case, not the brochure case.

### ReID / body / tracking
- [ ] **Person height ≥ ~128 px** in frame (≥ ~256 px preferred) where they enter
      the zone — ReID embeddings need body detail.
- [ ] Overlapping fields between adjacent cameras so handoff/tracking has continuity
      (topology matters more than any single great angle).
- [ ] Consistent, even lighting across the walking path; avoid hard shadow bands.

### Face
- [ ] **Face ~120–150 px tall** (or ≥ ~80 px inter-eye distance) for
      recognition-grade; ~40 px is detection-only.
- [ ] Frontal, eye-level framing at a choke point; even frontal lighting.
- [ ] Confirm face recognition is **permitted** for this site before relying on it —
      Heldar default is anonymous/behaviour analysis, **no face recognition by
      default** (memo §15.5).

---

## Phase 5 — Onboard into Heldar

Register the camera via the core API. Credentials are stored server-side and never
returned to clients; stream URLs are masked in all responses.

`POST /api/v1/cameras` body fields:

| field | notes |
| --- | --- |
| `name` | **required**; human label, also basis for the auto `id` slug |
| `id` | optional; explicit slug (else derived from `name`) |
| `site_id` | optional grouping |
| `vendor` | `hikvision`, `dahua`, or `generic` (default `generic`) |
| `model` | optional, free text |
| `address` | camera IP/host (used with the vendor template) |
| `rtsp_port` | default `554` |
| `username` / `password` | RTSP creds (see Phase 6; password write-only) |
| `main_stream_url` / `sub_stream_url` | explicit RTSP override; skips the vendor template |
| `record_stream` | `main` (default) or `sub` |
| `codec` / resolution / fps | metadata for sizing & health |
| `record_enabled` | default `true` |
| `segment_seconds` | default 60 (clamped 2–3600) |
| `retention_hours` | default 24 (min 1) |
| `enabled` | default `true` |

### Vendor URL templates
Heldar builds the RTSP URL from `vendor` + `address` + `rtsp_port` + creds when
no explicit URL is given:

- [ ] **HikVision** → `rtsp://<host>:554/Streaming/Channels/101` (main) /
      `.../102` (sub). Set `vendor: "hikvision"`.
- [ ] **Dahua** → `rtsp://<host>:554/cam/realmonitor?channel=1&subtype=0` (main) /
      `subtype=1` (sub). Set `vendor: "dahua"`.
- [ ] **Other / ONVIF / unusual paths** → set `vendor: "generic"` and supply
      `main_stream_url` (and `sub_stream_url`) explicitly. Generic vendors cannot be
      auto-guessed and will fail the test until a URL is provided.

### Steps
- [ ] `POST /api/v1/cameras` with the fields above. Capture the returned `id`.
- [ ] **Verify reachability:** `POST /api/v1/cameras/{id}/test` (GET also works).
      Expect `{"reachable": true, "codec": ..., "width": ..., "height": ...}`.
      The probe times out at 12 s; URLs/errors in the response are masked.
- [ ] Confirm the probed `codec`/`width`/`height` match what you configured in
      Phase 3. Mismatch = wrong stream selected; fix and re-test.
- [ ] If `reachable: false`: re-check VLAN/firewall path (Phase 1), `address`/port,
      `vendor` template vs actual RTSP path, and credentials — **without** triggering
      lockouts (Phase 6).

Example:

```bash
curl -sX POST http://<core-host>:8000/api/v1/cameras \
  -H 'content-type: application/json' \
  -d '{"name":"gate-north-anpr","vendor":"hikvision",
       "address":"192.168.0.2","username":"viewer","password":"<secret>",
       "record_stream":"main","codec":"h265"}'

curl -sX POST http://<core-host>:8000/api/v1/cameras/gate-north-anpr/test
```

---

## Phase 6 — Credentials & the test-unit rule

- [ ] Use a **dedicated least-privilege viewer/streaming account** per camera (or
      per site), never the admin account, for Heldar RTSP pulls.
- [ ] Passwords entered only via the API body; they are stored server-side, never
      serialized back to clients, and are masked in URLs/logs. Treat them as secrets.
- [ ] No plaintext credentials in tickets, chat, or commits.
- [ ] **HikVision test units `192.168.0.2`–`192.168.0.12`: NO brute-forcing.**
      - [ ] Use only the **known, documented** credentials for these units.
      - [ ] **Do not** credential-spray, dictionary-attack, or retry-loop on auth —
            HikVision locks the account (and can lock the IP) after a few failures,
            bricking the unit for everyone on the shared validation run.
      - [ ] One failed `test` → stop, confirm the correct credential out-of-band,
            then retry deliberately. Never script repeated auth attempts.
- [ ] Rotate any default/factory passwords on real deployments before go-live.

---

## Phase 7 — Verification (a camera is not done until this passes)

Wait at least 2–3 segment lengths (~3+ min at the 60 s default) after enabling, then:

- [ ] **Segments are recording:**
      `GET /api/v1/cameras/{id}/segments` returns recent segments with growing
      `start_time`/`end_time`, sane `duration_s` (≈ `segment_seconds`), non-zero
      `size_bytes`, and the expected `codec`/`width`/`height`.
- [ ] **Camera health is good:**
      `GET /api/v1/cameras/{id}/health` (or `GET /api/v1/health/cameras` for all)
      shows `state: "recording"`, a recent `last_segment_at`, `fps_observed` and
      `bitrate_kbps` near the configured values, and `reconnect_count` not climbing.
      States to watch for: `connecting`, `error`, `offline`, `unknown`.
- [ ] **Timeline has no gaps:**
      `GET /api/v1/cameras/{id}/timeline?from=<rfc3339>&to=<rfc3339>` returns a
      single continuous range covering the window (gaps > 2 s split it into multiple
      `ranges`). `recorded_seconds` should ≈ the wall-clock window with
      `segment_count` matching. Multiple ranges = dropouts → investigate network/IR
      flicker/reconnects before sign-off.
- [ ] **Live view works:**
      `POST /api/v1/cameras/{id}/liveview` returns `hls_url` / `webrtc_url` /
      `rtsp_url`. Open the HLS or WebRTC URL and confirm a live picture with
      acceptable latency.
- [ ] **Snapshot works:** `GET /api/v1/cameras/{id}/snapshot` returns a current
      frame. Inspect it against the Phase 4 targets — is the plate/face/body actually
      big and sharp enough? This is the real-world pixels-on-target check.
- [ ] **Clip export works:** `POST /api/v1/cameras/{id}/clip` over a known window
      produces a playable file — confirms the timeline→playback path end to end.

---

## Phase 8 — Sign-off

- [ ] Camera mapped on the floor plan / site map with its zone(s).
- [ ] Registry metadata complete and correct (vendor, codec, resolution, fps,
      `record_stream`, segment/retention).
- [ ] Day **and** night verification both passed (re-run Phase 7 snapshot at night
      for ANPR/face cameras).
- [ ] Purpose-specific acceptance met (plate/face/body pixels-on-target confirmed
      from a real snapshot, not just on paper).
- [ ] Credentials handed off to the secret store; no secrets left in notes.
- [ ] Camera marked commissioned in the install log with the Heldar `id`.
