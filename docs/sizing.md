# Heldar Sizing Guide — Server, Storage, Bandwidth, AI

This guide turns the sizing model and the deployment topology into worked numbers you can plan a
deployment against. Every formula and the anchor examples are kept internally consistent so the
math lines up end to end.

The four things you size for a camera deployment:

1. **Network bandwidth** — can the LAN/switch/uplink carry the streams.
2. **Storage** — how much disk the recordings consume per day, and over a retention window.
3. **Recording footprint cap** — the Heldar soft ceiling that keeps disk from filling.
4. **AI pixel workload** — how much pixel throughput the AI tier must process (later stages).

---

## 1. Bandwidth

### Formula

```text
Network Mbps ≈ camera_count × bitrate_per_camera_Mbps × overhead
```

- `bitrate_per_camera_Mbps` — the encoded stream bitrate the camera actually emits
  (not its sensor resolution). Use the main stream for recording; a substream is much smaller.
- `overhead` — packetization, retransmits, headroom, and second/sub streams. Plan **1.25–1.5×**.

### Worked example

```text
32 cameras × 4 Mbps              = 128 Mbps raw
× ~1.25–1.5 overhead             → plan 160–200 Mbps
```

A 32-camera 1080p site fits comfortably on a 1 GbE backbone but should not share that link
with bulk file traffic. Past ~64 cameras you are into 10 GbE core territory (see §5, medium site).

### Bandwidth at a glance (raw, before overhead)

| Cameras | 720p @ 2 Mbps | 1080p @ 4 Mbps | 4 MP @ 6 Mbps | 4K @ 8 Mbps |
| ------- | ------------- | -------------- | ------------- | ----------- |
| 4       | 8 Mbps        | 16 Mbps        | 24 Mbps       | 32 Mbps     |
| 8       | 16 Mbps       | 32 Mbps        | 48 Mbps       | 64 Mbps     |
| 16      | 32 Mbps       | 64 Mbps        | 96 Mbps       | 128 Mbps    |
| 32      | 64 Mbps       | **128 Mbps**   | 192 Mbps      | 256 Mbps    |
| 64      | 128 Mbps      | 256 Mbps       | 384 Mbps      | 512 Mbps    |

> Multiply by 1.25–1.5 for the link you actually provision. The bold cell is the worked example above.

---

## 2. Storage

### Rule of thumb

```text
1 Mbps ≈ 10.8 GB/day
```

(1 Mbps = 0.125 MB/s × 86,400 s/day = 10,800 MB ≈ 10.8 GB/day.)

So **per camera per day**: `bitrate_Mbps × 10.8 GB`.

| Stream profile     | Bitrate | Storage / camera / day |
| ------------------ | ------- | ---------------------- |
| 720p substream     | 2 Mbps  | 21.6 GB                |
| 1080p main         | 4 Mbps  | 43.2 GB                |
| 4 MP main          | 6 Mbps  | 64.8 GB                |
| 4K (H.265) main    | 8 Mbps  | 86.4 GB                |

### Worked example

```text
32 cameras × 4 Mbps × 10.8 GB/day ≈ 1.38 TB/day
```

This is why full recording retention must be local and carefully planned.
A month of that 32-camera site is ~41 TB — NAS/array territory, not a single SSD.

### Total recording footprint

`footprint = camera_count × (bitrate_Mbps × 10.8 GB) × retention_days`

**Daily footprint (GB/day)** by camera count and stream profile:

| Cameras | 720p @ 2 Mbps | 1080p @ 4 Mbps | 4 MP @ 6 Mbps | 4K @ 8 Mbps |
| ------- | ------------- | -------------- | ------------- | ----------- |
| 4       | 86.4 GB       | 172.8 GB       | 259.2 GB      | 345.6 GB    |
| 8       | 172.8 GB      | 345.6 GB       | 518.4 GB      | 691.2 GB    |
| 16      | 345.6 GB      | 691.2 GB       | 1.04 TB       | 1.38 TB     |
| 32      | 691.2 GB      | **1.38 TB**    | 2.07 TB       | 2.76 TB     |
| 64      | 1.38 TB       | 2.76 TB        | 4.15 TB       | 5.53 TB     |

**Footprint over a retention window** — multiply the daily figure by the number of days.
Reference table at **1080p @ 4 Mbps** (1 TB = 1000 GB, matching the decimal 10.8 GB/day rule):

| Cameras | 1 day     | 3 days    | 7 days    | 14 days   | 30 days   |
| ------- | --------- | --------- | --------- | --------- | --------- |
| 4       | 172.8 GB  | 518 GB    | 1.21 TB   | 2.42 TB   | 5.18 TB   |
| 8       | 345.6 GB  | 1.04 TB   | 2.42 TB   | 4.84 TB   | 10.4 TB   |
| 16      | 691.2 GB  | 2.07 TB   | 4.84 TB   | 9.68 TB   | 20.7 TB   |
| 32      | 1.38 TB   | 4.15 TB   | 9.68 TB   | 19.4 TB   | 41.5 TB   |
| 64      | 2.76 TB   | 8.29 TB   | 19.4 TB   | 38.7 TB   | 82.9 TB   |

> For another bitrate, scale linearly: 720p ≈ ×0.5, 4 MP ≈ ×1.5, 4K ≈ ×2 of the table above.
> H.265/HEVC roughly halves bitrate vs H.264 at equal quality — record in H.265 where the
> camera supports it to cut these numbers ~50%.

---

## 3. Recording footprint cap in Heldar

Heldar does **not** record blindly until the disk is full. The core
(`crates/heldar-kernel/src/services/retention.rs`) runs a retention sweeper that enforces several
policies on every pass — age, per-camera quota, a global size cap, and a free-disk floor.

### The retention controls

| Control | Where set | Scope | Default |
| ------- | --------- | ----- | ------- |
| `retention_hours` | per camera (DB column, settable via the camera API; falls back to `HELDAR_DEFAULT_RETENTION_HOURS`) | one camera | 24 h |
| `storage_quota_bytes` | per camera (DB column, optional) | one camera | unset (no per-camera cap) |
| `HELDAR_MAX_RECORDINGS_GB` (size cap) | env var, **or runtime override** (see below) | whole install | 20 GB |
| `HELDAR_MIN_FREE_DISK_GB` (free-disk floor) | env var, **or runtime override** (see below) | recordings filesystem | 5 GB |

The free-disk floor is a hard host-protection guard: if the recordings filesystem drops below
`HELDAR_MIN_FREE_DISK_GB` of free space, the sweeper prunes the oldest unlocked segments until back
above it (`0` disables the floor). This is what keeps recordings from filling the disk and corrupting
the system. `HELDAR_MAX_RECORDINGS_GB` / `HELDAR_MIN_FREE_DISK_GB` are parsed in
`crates/heldar-kernel/src/config.rs` into `max_recordings_bytes` / `min_free_disk_bytes` and surfaced
in the system status endpoint as `max_recordings_gb`.

### Setting the limits at runtime

The size cap and free-disk floor can be changed **without restarting** the kernel. The env vars set the
defaults; an operator override stored in the `settings` table (migration `0002_settings`) shadows them,
and the sweeper picks it up on its next pass.

- `GET /api/v1/system/retention` — the effective limits (any authenticated caller); each value flags
  whether it is the env default or an override.
- `PUT /api/v1/system/retention` — set them (admin only): body
  `{ "max_recordings_gb": <gt 0>, "min_free_disk_gb": <ge 0> }` (omit a field to leave it unchanged;
  clearing reverts to the env default).

The dashboard exposes this as the **System → Recording limit** panel.

### How a sweep works

The sweeper runs every `HELDAR_RETENTION_INTERVAL_S` (default 300 s, floor 30 s) and does
four passes in order:

1. **Age-based, per camera.** For each camera, delete unlocked segments whose `end_time` is
   older than that camera's `retention_hours`.
2. **Per-camera quota.** For each camera with a `storage_quota_bytes` set, prune its oldest
   unlocked segments back under that quota.
3. **Global size cap.** If the sum of all segment sizes exceeds `max_recordings_bytes`, delete
   the **globally oldest unlocked** segments (ordered by `end_time` ascending, in batches of 20)
   until the total is back under the cap.
4. **Free-disk floor.** If the recordings filesystem is below `min_free_disk_bytes` of free space,
   prune the globally oldest unlocked segments until back above the floor (a no-op if the floor
   exceeds the whole disk).

**Locked (evidence) segments are never deleted by any pass** — they don't count against you
being able to prune, but they *do* still occupy disk, so a large locked set can keep you above
the cap with nothing left to prune.

### How the two interact (read this carefully)

- They are independent gates. **Effective retention = whichever limit bites first.**
  - If disk is roomy, `retention_hours` governs and each camera keeps exactly its window.
  - If the install is over `HELDAR_MAX_RECORDINGS_GB`, the size cap overrides and footage
    gets pruned *before* it reaches `retention_hours`.
- The size cap is a **global FIFO by end_time, not per camera.** A busy or high-bitrate camera
  can push the install over the cap and cause the oldest segment of a *quiet* camera to be
  deleted. Per-camera `retention_hours` is a maximum age, not a reservation of space.
- Practical rule: **size the cap to comfortably hold the sum of every camera's
  `retention_hours` of footage**, or accept that the cap silently shortens retention.

Required cap to honor everyone's `retention_hours`:

```text
needed_GB ≈ Σ over cameras ( bitrate_Mbps × 10.8 × retention_hours / 24 )
set HELDAR_MAX_RECORDINGS_GB ≥ needed_GB × ~1.15   (headroom for locked clips + variance)
```

Worked: 8 × 1080p cameras at 4 Mbps, all `retention_hours = 12`:
`8 × 43.2 × 12/24 = 172.8 GB` → set the cap to ~**200 GB**. Leaving it at the 20 GB default
means only ~5.5 hours per camera actually survive, regardless of `retention_hours = 12`.

---

## 4. AI pixel workload

This is a Stage 2+ concern (frame sampler / AI tier), but size for it now because it dominates
GPU choice.

### Formula

```text
AI pixels/sec = camera_count × width × height × sampled_fps
```

Note the two big levers that are independent of recording: **decode/inference resolution** and
**sampled FPS**. You record at full bitrate but you can sample AI at 5 FPS and 720p.

### Manageable vs dangerous

```text
Manageable:  32 × 1280 × 720  × 5 FPS  ≈ 147 million pixels/sec
Dangerous:   32 × 3840 × 2160 × 25 FPS ≈ 6.6 billion pixels/sec
```

Same 32 cameras — a **~45×** difference in pixel throughput, and a totally different system. The takeaways:

- Downscale before inference (720p is plenty for most detectors).
- Sample frames (3–8 FPS), don't infer every frame.
- Use a substream for AI where the camera provides one.
- 4K @ 25 FPS on every camera is the trap — it turns a 1-GPU job into a cluster.

| Profile (32 cams)        | Per-camera px/s | Total px/s     | Verdict     |
| ------------------------ | --------------- | -------------- | ----------- |
| 720p @ 5 FPS             | 4.6 M           | ~147 M         | manageable  |
| 1080p @ 10 FPS           | 20.7 M          | ~664 M         | demanding   |
| 4K @ 25 FPS              | 207 M           | ~6.6 B         | dangerous   |

---

## 5. Topology recommendations

### Small site — 4–16 cameras

```text
PoE cameras → managed switch → single edge server (ingest + record + playback + live + AI)
                                                  → cloud control plane
```

- One box does everything. 1 GbE is fine (≤ ~96 Mbps raw at 16 × 6 Mbps).
- Storage: one or two large HDDs/SSD; size from §2 against your retention window.
- AI: a single mid-range GPU handles 720p @ 5 FPS across the fleet.

### Medium site — 16–64 cameras

```text
PoE camera switches → core 10GbE switch → media/recording server
                                         → AI server / GPU node (e.g. DGX Spark)
                                         → local NAS
                                         → cloud control plane
```

- **Split media and AI onto separate servers.** 10 GbE core once you pass ~64 × 4 Mbps.
- Storage on a dedicated NAS/array — a 32-cam 1080p site is ~1.38 TB/day (§2).
- This is where `HELDAR_MAX_RECORDINGS_GB` and per-camera `retention_hours` must be tuned
  deliberately, not left at defaults.

### Large site / campus

```text
Building A cameras → local media node ┐
Building B cameras → local media node ├→ central AI/storage/SOC cluster → cloud control plane
Building C cameras → local media node ┘
```

- Per-building media nodes do local ingest/record; only AI metadata and selected video go to
  the center — keeps multi-TB/day recording traffic off the campus backbone.
- Segment the network into VLANs: CCTV, Media, AI, Storage, Operator, Management,
  Guest/Corp (CCTV isolated from corp/guest).

---

## 6. This dev host (Stage 0)

Measured specs of the development machine:

| Resource | This host |
| -------- | --------- |
| CPU      | 12 cores  |
| RAM      | 30 GB     |
| GPU      | GTX 1080 Ti, 11 GB VRAM |
| Free disk | ~55 GB    |

**Keep retention small here.** With only ~55 GB free, recording is the binding constraint:

- A single 1080p @ 4 Mbps camera burns **43.2 GB/day** (≈ 1.8 GB/hour). The default
  `HELDAR_MAX_RECORDINGS_GB = 20` therefore holds only **~11 camera-hours** at that bitrate —
  the size cap fires long before the default `retention_hours = 24` is reached.
- Rough capacities at the 20 GB default cap:

  | Stream     | GB/hour/cam | 1 cam survives | 4 cams survive |
  | ---------- | ----------- | -------------- | -------------- |
  | 720p @ 2 Mbps | 0.9 GB   | ~22 h          | ~5.5 h each    |
  | 1080p @ 4 Mbps | 1.8 GB  | ~11 h          | ~2.8 h each    |

Recommended dev settings:

- `HELDAR_MAX_RECORDINGS_GB` ≈ **30** (stay well under the ~55 GB free; leave headroom for
  the OS, DB, clips, and snapshots).
- `HELDAR_DEFAULT_RETENTION_HOURS` ≈ **2–6** for a couple of test cameras.
- Prefer a 720p substream and a low sample FPS for any AI experiments — the 1080 Ti is fine for
  the "manageable" 720p @ 5 FPS profile (§4), not for 4K @ 25 FPS.

---

## 7. Sizing worksheet

Fill in the blanks for your deployment.

### Inputs

| Field | Your value |
| ----- | ---------- |
| Camera count `N` | _____ |
| Record bitrate per camera (Mbps) | _____ |
| Network overhead factor (1.25–1.5) | _____ |
| Retention window (hours) | _____ |
| AI sample resolution (w × h) | _____ × _____ |
| AI sampled FPS | _____ |

### Outputs

| Quantity | Formula | Your value |
| -------- | ------- | ---------- |
| Raw bandwidth (Mbps) | `N × bitrate` | _____ |
| Provisioned bandwidth (Mbps) | `raw × overhead` | _____ |
| Storage per camera-day (GB) | `bitrate × 10.8` | _____ |
| Daily footprint (GB) | `N × bitrate × 10.8` | _____ |
| Retention footprint (GB) | `daily × retention_hours / 24` | _____ |
| Recommended `HELDAR_MAX_RECORDINGS_GB` | `retention_footprint × 1.15` | _____ |
| AI pixels/sec | `N × w × h × fps` | _____ |

### Sanity checks

- [ ] Provisioned bandwidth fits the switch/uplink (1 GbE ≈ 1000 Mbps; go 10 GbE past ~64 cams).
- [ ] Retention footprint fits the disk/NAS **with** headroom for clips, snapshots, DB, OS.
- [ ] `HELDAR_MAX_RECORDINGS_GB` ≥ retention footprint, or you accept shortened retention.
- [ ] AI pixels/sec is in the "manageable" range for the available GPU (≈ 147 M/s on one GPU at
      720p @ 5 FPS); downscale and sample if not.

---

*Config behavior: `crates/heldar-kernel/src/config.rs`,
`crates/heldar-kernel/src/services/retention.rs`,
`crates/heldar-kernel/src/services/settings.rs` (the runtime overrides, stored by migration
`0002_settings`), `crates/heldar-kernel/src/routes/system.rs`.*
