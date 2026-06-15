# High Availability (HA) for Heldar appliances

Heldar is an **edge DVR**: each node records its own cameras to its own local disks. There is no
clustering, no consensus, and **no shared storage** inside the application — and deliberately so. A
shared filesystem (NFS/iSCSI/Ceph) under a video write path is a single point of failure and a
performance cliff; in-app RAID/clustering would re-implement, badly, what the OS and the network
already do well. Heldar instead exposes the **health and readiness signals** an external HA layer
(keepalived, a load balancer, or a fleet controller) needs to fail over between **independent nodes**.

This document covers the two operational primitives the kernel provides for HA, and the recommended
**N+1 hot-spare** topology that uses them.

---

## 1. Building blocks the kernel exposes

### Liveness — `GET /healthz`
Process-up check. Returns `200 {"status":"ok"}` as soon as the HTTP server is accepting connections.
Use it for container/orchestrator liveness probes (restart-on-failure), **not** for failover.

### Readiness — `GET /readyz`
Returns `200 {"ready":true}` when the node is fit to serve, `503` otherwise. Two layers:

1. **Database connectivity** (always on): a `503 {"ready":false,"reason":"database"}` if the SQLite
   store is unreachable.
2. **Recorder quorum** (opt-in, HA): when `HELDAR_READYZ_MIN_RECORDING_PERCENT > 0`, the probe also
   requires at least that percent of **enabled cameras** to be in the `recording` state. Below the
   threshold it returns:

   ```json
   503 { "ready": false, "reason": "insufficient_recorders",
         "recording_pct": 40.0, "required_pct": 75.0 }
   ```

   This turns `/readyz` into a failover trigger: a node that has lost too many of its recorders (NIC
   flap, ffmpeg storm, disk wedged) demotes itself so a hot spare can take the VIP. Default `0`
   preserves the DB-only behaviour, so existing single-node deployments are unaffected.

   > Note: the denominator is **enabled** cameras. Event-mode cameras (`event` / `scheduled_event`)
   > are armed-but-idle most of the time and are *not* in the `recording` state, so a fleet of mostly
   > event-mode cameras should pick a low threshold (or leave it `0` and rely on per-camera alerting).

### Disk / array health — `GET /api/v1/system`
The background health loop runs an opt-in **disk-health pass** on its own cadence
(`HELDAR_SMART_CHECK_INTERVAL_S`, default 300s):

- **SMART** (`HELDAR_SMART_CHECK_ENABLED=true`, `HELDAR_SMART_DEVICES=/dev/sda,/dev/sdb`): runs
  `smartctl -H <dev>` and emits a `disk_smart_warning` event when a drive does not report PASSED/OK.
  Requires `smartmontools` on `PATH`; if `smartctl` is missing, the loop logs **once** and skips (no
  crash) — so the build and tests never require it.
- **md/RAID** (`HELDAR_MDSTAT_CHECK_ENABLED=true`): reads `/proc/mdstat` (Linux) and emits a
  `raid_degraded` event for any array showing a down member (e.g. `[U_]`).

`GET /api/v1/system` rolls these up:

```json
{ "disk_health_ok": true, "last_disk_alert_at": null, "live_transcode_engine": "software", ... }
```

`disk_health_ok` is `false` while a disk alert has fired within the last few check cycles;
`last_disk_alert_at` is the timestamp of the most recent alert. Wire these into your dashboard and
paging — a degraded array on a recording node is an early failover/drain signal even before
`/readyz` trips.

---

## 2. Recommended topology: N+1 hot spare

Run **N independent recording nodes** plus **one idle spare**, each with its own cameras configured
and its own local disks. No node depends on another node's storage.

```
            keepalived VRRP VIP (per node group)
   ┌───────────────┬───────────────┬───────────────┐
 node-1          node-2          node-3          spare
 cams 1-16       cams 17-32      cams 33-48      (armed, idle)
 local disks     local disks     local disks     local disks
```

- Each active node owns a **VIP** managed by keepalived. Clients (the SPA, MediaMTX consumers, the
  fleet uplink) reach a node through its VIP.
- keepalived runs a **`vrrp_script`** that curls `/readyz`. When a node fails the script
  (DB down, or recorder quorum lost), keepalived lowers its priority and the VIP migrates to the
  spare, which is pre-provisioned with that node's camera set and starts recording on takeover.
- "N+1" means one spare backs several actives: the spare only needs the camera credentials/config
  for whichever node it may assume (provision all, or scope per group).

### keepalived health_script

```conf
# /etc/keepalived/keepalived.conf  (on each active node)
vrrp_script chk_heldar {
    script  "/usr/bin/curl -sf -o /dev/null http://127.0.0.1:8000/readyz"
    interval 5          # probe every 5s
    timeout  3
    fall     3          # 3 consecutive failures => unhealthy
    rise     2          # 2 consecutive successes => healthy
    weight  -40         # drop priority by 40 while failing
}

vrrp_instance heldar_node1 {
    state            MASTER
    interface        eth0
    virtual_router_id 51
    priority         150          # spare runs BACKUP at e.g. 100
    advert_int       1
    virtual_ipaddress { 192.168.0.50/24 }
    track_script     { chk_heldar }
}
```

Set `HELDAR_READYZ_MIN_RECORDING_PERCENT` on the active nodes (e.g. `75`) so the script reacts to
**recorder loss**, not just process death:

```bash
HELDAR_READYZ_MIN_RECORDING_PERCENT=75
```

The spare runs the same config at a lower VRRP `priority`; when an active node's priority drops below
the spare's, the VIP moves and the spare takes over. Because storage is local, there is **no
split-brain over data** — at most a brief recording gap on the failed node's cameras during takeover,
which the indexer records as a gap (and ANR can backfill from camera-onboard storage if enabled).

### What is intentionally NOT here

- **No shared storage / clustered filesystem.** Each node records locally; this is a feature.
- **No in-app RAID.** Use OS md/RAID (monitored via `HELDAR_MDSTAT_CHECK_ENABLED`) or a hardware
  controller. Heldar *observes* array health, it does not manage it.
- **No leader election / quorum in the app.** keepalived (VRRP) owns failover; Heldar only reports
  whether a node is fit to serve.

---

## 3. Off-box durability: the fleet outbox

Independent nodes still need a path to ship evidence/events off the edge. The durable, ordered
**outbox** (`GET /api/v1/outbox?since_seq=&limit=`, admin-only) is that seam: a fleet controller
drains each node's committed detection batches in `seq` order and correlates them with the node's
identity from `GET /api/v1/site` (`HELDAR_SITE_ID`). Combined with scheduled **backup policies**
(local/NAS or rclone remotes) for the video itself, this gives off-box durability without coupling
the recording nodes to each other. See `docs/OBSERVABILITY.md` for the event/metrics surface and the
backup subsystem for footage replication.
