import type * as Contract from "./contract";

// Shapes the server publishes are ALIASED from the generated contract, not re-declared here.
// Re-declaring them is what let five real drifts through: a field the server returns that the
// dashboard had never heard of, caught by a test only after the fact. An alias cannot drift.
//
// Types below that are NOT aliased are dashboard-only — view models, discriminated unions the
// UI builds, and shapes for routes the contract does not yet describe.
//
// A few aliases REFINE the contract with `Omit<...> & { ... }`. Those are places the published
// schema is genuinely less precise than the server's behaviour: a Rust `String` that only ever holds
// four values arrives as `string`, and a `Vec<Vec<f64>>` that is always pairs arrives as
// `number[][]`. The shape still comes from the contract — only the named field is narrowed — so a
// field added or removed by the server still surfaces here. Tightening the schema itself is the real
// fix and is filed as #156; each refinement disappears when it lands.

// TypeScript mirror of the Heldar HTTP API contract (serde JSON).
// Field names match the Rust structs in crates/heldar-kernel/src/{models.rs,routes/*} (kernel)
// and crates/heldar-entry/src/{models.rs,routes.rs} (access-control app).

export type CameraStatusState =
  | "disabled"
  | "connecting"
  | "recording"
  | "offline"
  | "error"
  | "unknown";

export type RecordStream = "main" | "sub";

/** When the recorder runs for a camera: `continuous` (always), `scheduled` (time-of-day window),
 * `event` (records only during a trigger window: a zone/breach event or a manual record-trigger
 * extends it to now + post_roll_seconds), or `scheduled_event` (windows AND triggers). */
export type RecordMode = Contract.RecordMode;

/** Known vendors with auto-built RTSP URLs, plus the catch-all. */
export type Vendor = "hikvision" | "dahua" | "generic" | (string & {});

export type Severity = "info" | "warning" | "critical";

export type CameraView = Contract.CameraView;

export interface CameraCreate {
  id?: string;
  name: string;
  /** The camera's site, which carries the timezone its recording schedule is read in (#125).
   *
   *  On CREATE: omit or `null` for no site. On UPDATE (`CameraUpdate` is a `Partial` of this),
   *  absent LEAVES the camera where it is and explicit `null` DETACHES it — the two are different
   *  requests, and the server distinguishes them. */
  site_id?: string | null;
  vendor?: string;
  model?: string;
  address?: string;
  rtsp_port?: number;
  username?: string;
  password?: string;
  main_stream_url?: string;
  sub_stream_url?: string;
  record_stream?: RecordStream;
  capabilities?: Record<string, unknown>;
  record_enabled?: boolean;
  segment_seconds?: number;
  retention_hours?: number;
  storage_quota_bytes?: number | null;
  record_audio?: boolean;
  record_mode?: RecordMode;
  pre_roll_seconds?: number;
  post_roll_seconds?: number;
  mirror_enabled?: boolean;
  anr_enabled?: boolean;
  anr_replay_url_template?: string | null;
  native_anpr_enabled?: boolean;
  native_events_enabled?: boolean;
  enabled?: boolean;
  live_warm?: boolean;
}

export type CameraUpdate = Partial<Omit<CameraCreate, "id">>;

/* ---- Camera device control (capability-driven Device panel) ---- */

export type IoOutput = Contract.IoOutput;

/** Normalized per-camera device-control capability map (persisted by the kernel probe). */
export interface DeviceControlCapabilities {
  vendor?: string;
  day_night?: boolean;
  image?: boolean;
  io_outputs?: IoOutput[];
  native_anpr?: boolean;
  ptz?: boolean;
  /** Supplement-light modes the device supports (empty = no supplement light). e.g.
   * eventIntelligence (smart: IR normally, white light on events), colorVuWhiteLight,
   * irLight, close. */
  supplement_light_modes?: string[];
  /** The camera's OWN smart-event detections (motion, line_crossing, intrusion, …) with their
   * arm state where readable — distinct from Heldar's server-side zone engine. */
  built_in_detections?: { kind: string; enabled?: boolean | null }[];
  probed_at?: string;
}

export type SmartLine = Omit<Contract.SmartLine, "points"> & {
  points: [number, number][];
};

export type LineCrossingConfig = Omit<Contract.LineCrossingConfig, "lines"> & {
  lines: SmartLine[];
};

export type SmartRegion = Omit<Contract.SmartRegion, "points"> & {
  points: [number, number][];
};

export type IntrusionConfig = Omit<Contract.IntrusionConfig, "regions"> & {
  regions: SmartRegion[];
};

export type MotionConfig = Contract.MotionConfig;

export type DayNightConfig = Contract.DayNightConfig;

export type GatePolicy = Contract.GatePolicy;

/** Global gate state: kill-switch + every configured lane policy. */
export interface GateState {
  kill_switch: boolean;
  policies: GatePolicy[];
}

export type ImageConfig = Contract.ImageConfig;

/** Result of POST /api/v1/cameras/{id}/record-trigger (manual event-recording trigger, manager+). */
export interface RecordTriggerResult {
  camera_id: string;
  triggered: boolean;
  /** When the post-roll recording window currently ends (server UTC time); repeated triggers extend it. */
  window_end: string;
  pre_roll_seconds: number;
  post_roll_seconds: number;
}

export interface CameraTestResult {
  reachable: boolean;
  codec?: string;
  width?: number;
  height?: number;
  url: string;
  error?: string;
}

export interface SegmentView {
  id: string;
  camera_id: string;
  path: string;
  start_time: string;
  end_time: string;
  duration_s: number;
  codec?: string | null;
  width?: number | null;
  height?: number | null;
  size_bytes: number;
  container: string;
  /** Transient export read-lock; cleared at startup. Not a durable hold. */
  locked: boolean;
  /** Durable evidence hold: when true the segment is never pruned by retention. */
  evidence_locked: boolean;
  incident_id?: string | null;
  created_at: string;
  /** Browser-playable URL under /media/recordings/... */
  url: string;
}

export type IncidentSummary = Contract.IncidentSummary;

export interface TimelineRange {
  start: string;
  end: string;
  seconds: number;
}

export interface Timeline {
  camera_id: string;
  from: string | null;
  to: string | null;
  ranges: TimelineRange[];
  recorded_seconds: number;
  segment_count: number;
}

export interface ClipResult {
  id: string;
  camera_id: string;
  filename: string;
  /** Browser-playable URL under /media/clips/... */
  url: string;
  from: string;
  to: string;
  requested_seconds: number;
  size_bytes: number;
  segment_count: number;
}

/** A segment-spanning HLS playback session over a recorded time range (POST
 * /api/v1/cameras/{id}/playback/sessions). Players seek natively within the VOD playlist; DELETE
 * /api/v1/playback/sessions/{id} tears it down. Sessions expire after HELDAR_PLAYBACK_SESSION_TTL_MINUTES. */
export interface PlaybackSession {
  id: string;
  camera_id: string;
  /** HLS VOD playlist under /media/playback/{id}/index.m3u8 — play with hls.js. */
  playlist_url: string;
  from: string;
  to: string;
  /** Requested window length in seconds (the playlist may be shorter where footage has gaps). */
  duration_s: number;
  segment_count: number;
}

export interface LiveUrls {
  name: string;
  /** HLS .m3u8 playlist — play with hls.js. Already carries `?token=` when auth-gated. */
  hls_url: string;
  webrtc_url: string;
  rtsp_url: string;
  /** STUN/TURN ICE servers for the WebRTC path (P2 remote access). Absent/empty = LAN/host-only. */
  ice_servers?: RTCIceServer[];
  /** Short-lived MediaMTX read token. The player appends it after the WHEP suffix and onto every HLS
   *  request (hls.js drops the playlist query on relative segment URLs otherwise). */
  token?: string;
}

export interface CameraStatus {
  camera_id: string;
  state: CameraStatusState;
  last_segment_at?: string | null;
  last_started_at?: string | null;
  reconnect_count: number;
  segments_written: number;
  fps_observed?: number | null;
  bitrate_kbps?: number | null;
  last_error?: string | null;
  recorder_pid?: number | null;
  updated_at: string;
}

export interface VisionEvent {
  id: string;
  camera_id?: string | null;
  site_id?: string | null;
  event_type: string;
  severity: Severity;
  timestamp: string;
  payload: Record<string, unknown>;
  created_at: string;
}

export type DiscoverOptions = Contract.DiscoverOptions;

export interface DiscoveredDevice {
  address: string;
  rtsp_port: number;
  rtsp_open: boolean;
  http_open: boolean;
  vendor_guess: string;
  http_server?: string | null;
  verified: boolean;
  codec?: string | null;
  width?: number | null;
  height?: number | null;
  suggested_id: string;
  already_registered: boolean;
}

export interface DiscoverResponse {
  /** Echo of the requested targets spec. */
  scanned: string;
  found: number;
  verified: number;
  /** IDs of cameras registered during this scan (when auto_add was set). */
  added: string[];
  devices: DiscoveredDevice[];
}

/** Free/total space on the filesystem backing the recordings dir (statvfs). */
export interface DiskStats {
  total_bytes: number;
  free_bytes: number;
  used_bytes: number;
  used_percent: number;
}

/** Storage observability: disk space + recordings footprint + projected retention. */
export interface StorageReport {
  disk: DiskStats | null;
  recordings_bytes: number;
  segment_count: number;
  oldest_segment: string | null;
  newest_segment: string | null;
  /** Bytes/day written over the last 24h of indexed segments. */
  write_rate_bytes_per_day: number;
  /** Projected days of free space remaining at the recent write rate (null if idle/unknown). */
  projected_days_remaining: number | null;
}

/** Remote-access overlay (WireGuard/Tailscale/NetBird) reachability, reported by the kernel. */
export interface OverlayStatus {
  enabled: boolean;
  kind: string; // tailscale | netbird | wireguard | none
  iface?: string | null;
  present: boolean;
  operstate?: string | null;
  up: boolean;
  note: string;
}

export interface SystemInfo {
  name: string;
  version: string;
  /** The API CONTRACT version (#120) — not the build version. Pin generated clients to this. */
  api_version: string;
  started_at: string;
  uptime_seconds: number;
  recorder_enabled: boolean;
  cameras_total: number;
  cameras_recording: number;
  active_recorders: number;
  segments_total: number;
  recordings_bytes: number;
  recordings_gb: number;
  max_recordings_gb: number;
  storage: StorageReport;
  /** Remote-access overlay status (LAN-only when not enabled). */
  remote_access: OverlayStatus;
  /** WebRTC remote-dashboard relay health: configured=false → hide; healthy=false while configured →
   *  the box is up but the remote path is dead (surface it, don't mask it). */
  relay: { configured: boolean; healthy: boolean; last_ok_at: string | null };
  /** No recent disk_smart_warning/raid_degraded events (SMART/RAID health pass). */
  disk_health_ok: boolean;
  /** Timestamp of the most recent disk-health alert (any time), or null if none. */
  last_disk_alert_at?: string | null;
  /** Active live-preview transcode engine (software | vaapi | nvenc). */
  live_transcode_engine: string;
}

export type RetentionLimits = Contract.RetentionLimits;
export type RetentionUpdate = Contract.RetentionUpdate;

/** Live-preview transcode engine (effective value + detected hardware encoders). */
export type TimezoneSettings = Omit<Contract.TimezoneSettings, "configured"> & {
  /** Always sent by the server; `Option<String>` makes utoipa mark it optional. */
  configured: string | null;
};
export type TimezoneUpdate = Contract.TimezoneUpdate;

/** A site, and the timezone its cameras' schedules are read in. */
export interface Site {
  id: string;
  name: string;
  /** null means no zone chosen — NOT UTC. The box-wide default applies. */
  timezone: string | null;
  created_at: string;
}

export type TranscodeSettings = Contract.TranscodeSettings;
export type TranscodeUpdate = Contract.TranscodeUpdate;

export type DbStatus = Contract.DbStatus;
export type DbLimitUpdate = Contract.DbLimitUpdate;
export type DbConvertResult = Contract.DbConvertResult;

// ---- Webhook subscriptions (the generic event-delivery substrate; supersedes single-URL alerting) ----

/** A webhook subscription (GET /api/v1/webhooks). The signing `secret` is never returned; only
 * `has_secret` indicates whether one is configured. `event_types` of `["*"]` matches every type. */
export interface WebhookSubscription {
  id: string;
  name: string;
  url: string;
  /** Exact-membership set of event types; `["*"]` means all types. */
  event_types: string[];
  min_severity: Severity;
  /** Whether an HMAC-SHA256 signing secret is configured (the value itself is never returned). */
  has_secret: boolean;
  enabled: boolean;
  /** Delivery cursor (an events.created_at); null until the first delivery cycle. */
  cursor_at?: string | null;
  created_at: string;
  updated_at: string;
}

export type WebhookSubscriptionCreate = Contract.WebhookSubscriptionCreate;

export type WebhookSubscriptionUpdate = Contract.WebhookSubscriptionUpdate;

export type WebhookDeliveryStatus = "delivered" | "failed";

export type WebhookDelivery = Contract.WebhookDelivery;

export type WebhookTestResult = Contract.WebhookTestResult;

/** One known event type plus a one-line description (GET /api/v1/events/types). */
export interface EventTypeInfo {
  event_type: string;
  description: string;
}

// ---- Modules (the plugin platform: GET /api/v1/modules drives the nav rail + routes) ----

/** Provenance of a loaded module; drives store shelving + nav badging. Matches the kernel enum. */
export type ModuleKind = "core" | "proprietary" | "community" | "imported";

/** How the dashboard renders a module's content: a bundled page, a `runtime` UI bundle imported from
 *  `ui_url`, an iframe to /m/{id}/, or headless (no UI — a compute plugin like a sandboxed Wasm
 *  DetectionConsumer). */
export type ModuleMount = "bundled" | "runtime" | "iframe" | "headless";

/** A nav destination a module contributes. `icon` is a key the dashboard maps to a glyph. */
export interface ModuleNavEntry {
  path: string;
  label: string;
  icon: string;
}

/** One loaded module, as reported by the composing binary at GET /api/v1/modules. */
export interface ModuleManifest {
  id: string;
  name: string;
  version: string;
  publisher: string;
  kind: ModuleKind;
  description: string;
  nav: ModuleNavEntry[];
  mount: ModuleMount;
  /** For `mount: "runtime"`, the URL of the module's UI bundle the dashboard imports + mounts. */
  ui_url?: string;
  /** Reachability of a sidecar plugin (`unknown`/`healthy`/`unreachable`); absent for compiled. */
  health?: string;
}

export type ModuleRegisterRequest = Contract.ModuleRegisterRequest;

/** Admin detail for a registered sidecar. */
export interface ModuleDetail {
  id: string;
  name: string;
  version: string;
  publisher: string;
  description: string;
  base_url: string;
  nav: ModuleNavEntry[];
  subscribes: string[];
  role: string;
  api_key_id?: string | null;
  webhook_id?: string | null;
  health: string;
  health_checked_at?: string | null;
  created_at: string;
}

/** Register response: the detail plus the once-only credentials the sidecar must be configured with. */
export interface ModuleRegistered {
  module: ModuleDetail;
  api_key: string;
  webhook_secret: string;
}

// ---- Plugin store / registry (Phase C: GET /api/v1/registry) ----

/** Which store shelf an entry belongs on. */
export type Shelf = "core" | "proprietary" | "community" | "import";

/** Live state of a catalog entry, cross-referenced against loaded/installed modules. */
export type EntryState =
  | "available" // sidecar, installable now
  | "installed" // sidecar, registered
  | "included" // compiled module present in this build
  | "not_in_build" // compiled module advertised but not in this build (commercial add-on)
  | "unreachable" // installed sidecar whose health probe last failed
  | "loaded"; // headless plugin (e.g. Wasm) loaded from disk and running

/** How a catalog entry is installed (discriminated by `type`). */
export type InstallSpec =
  | { type: "builtin"; availability?: string | null; contact?: string | null }
  | {
      type: "sidecar";
      image?: string | null;
      default_base_url: string;
      subscribes?: string[];
      role?: string | null;
      nav?: ModuleNavEntry[];
      docs?: string | null;
    };

/** One catalog entry with its computed shelf/state/verification (flattened entry + extras). */
export interface RegistryEntry {
  id: string;
  name: string;
  publisher: string;
  kind: ModuleKind;
  summary: string;
  description?: string | null;
  version?: string | null;
  icon?: string | null;
  homepage?: string | null;
  categories?: string[];
  install: InstallSpec;
  shelf: Shelf;
  state: EntryState;
  verified: boolean;
  source: string; // "bundled" | "local" | url
  /** Set for entries derived from a loaded module (e.g. "headless" for a Wasm plugin). */
  mount?: ModuleMount;
}

/** A catalog source's signature status (for the registry indicator + diagnostics). */
export interface RegistrySource {
  source: string;
  name: string;
  verified: boolean;
  first_party: boolean;
  key_id?: string | null;
  error?: string | null;
  fetched_at?: string | null;
  entry_count: number;
}

/** The full GET /api/v1/registry response. */
export interface RegistryView {
  enabled: boolean;
  sources: RegistrySource[];
  entries: RegistryEntry[];
}

// ---- Fleet outbox + site identity (open-core seam, edge->cloud uplink foundation) ----

/** One durable outbox row: a committed detection batch (GET /api/v1/outbox, admin-only). */
export interface OutboxEntry {
  seq: number;
  topic: string;
  camera_id?: string | null;
  site_id?: string | null;
  frame_id?: string | null;
  task_type?: string | null;
  detection_count: number;
  created_at: string;
}

/** A page of outbox rows; pass `next_seq` as the next `since_seq` to continue draining. */
export interface OutboxPage {
  entries: OutboxEntry[];
  /** Highest `seq` in this page; null when caught up (empty page). */
  next_seq?: number | null;
  count: number;
}

/** This node's fleet identity (GET /api/v1/site, no auth). */
export interface SiteInfo {
  site_id?: string | null;
  name: string;
  version: string;
  started_at: string;
}

/** A hole in recording coverage (the span between two availability ranges). */
export interface GapSpan {
  start: string;
  end: string;
  seconds: number;
}

export interface Gaps {
  camera_id: string;
  from: string | null;
  to: string | null;
  gaps: GapSpan[];
  gap_count: number;
  total_gap_seconds: number;
}

/** ANR fill lifecycle for a persisted recording gap. */
export type GapFillState = "pending" | "filled" | "failed";

/** A persisted recording gap detected by the indexer (a hole > 3s between segments). ANR re-fills it
 * from the camera's onboard storage. Distinct from the computed coverage holes in `Gaps`:
 * GET /api/v1/cameras/{id}/recording-gaps, POST .../recording-gaps/{gap_id}/retry. */
export interface RecordingGap {
  id: string;
  camera_id: string;
  gap_start: string;
  gap_end: string;
  gap_seconds: number;
  fill_state: GapFillState;
  fill_attempts: number;
  last_attempt_at?: string | null;
  filled_at?: string | null;
  created_at: string;
}

// ---- Per-camera recording schedule (time-of-day windows) ----

/** A recurring per-camera recording window, applied when `record_mode` is `scheduled` or
 * `scheduled_event`. `days` are weekday ints 0=Mon..6=Sun; `time_start`/`time_end` are "HH:MM" 24h
 * read in the CAMERA'S SITE timezone (#125), falling back to the server's local zone when no zone is
 * configured anywhere (start > end means an overnight window). */
export interface RecordSchedule {
  id: string;
  camera_id: string;
  days: number[];
  time_start: string;
  time_end: string;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export type RecordScheduleCreate = Contract.RecordScheduleCreate;

export type RecordScheduleUpdate = Partial<RecordScheduleCreate>;

// ---- Scheduled interval snapshots ----

export type SnapshotSchedule = Contract.SnapshotSchedule;

export type SnapshotScheduleCreate = Contract.SnapshotScheduleCreate;

export type SnapshotScheduleUpdate = Partial<SnapshotScheduleCreate>;

/** A captured snapshot frame plus its browser-fetchable media URL (flattened PersistedSnapshot). */
export interface SnapshotView {
  id: string;
  camera_id: string;
  schedule_id?: string | null;
  path: string;
  taken_at: string;
  size_bytes: number;
  created_at: string;
  /** Browser-fetchable URL under /media/snapshots/... */
  url: string;
}

// ---- Stage 2: AI perception ----

/** Which encoded stream the sampler decodes for a task. */
export type StreamProfile = "sub" | "main";

/** Sampler runtime states (distinct from camera/recorder states). */
export type SamplerState =
  | "sampling"
  | "connecting"
  | "offline"
  | "error"
  | "stopped";

/** A perception task configured on a camera (consumed by AI workers). */
export interface AiTask {
  id: string;
  camera_id: string;
  /** Free-form: detection | anpr | tracking | … */
  task_type: string;
  enabled: boolean;
  stream_profile: string;
  /** Requested sample rate (the global budget may reduce the effective rate). */
  fps: number;
  /** Target sample width in px; height keeps aspect. */
  width: number;
  config: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export type AiTaskCreate = Contract.AiTaskCreate;

export type AiTaskUpdate = Partial<AiTaskCreate>;

/** Worker discovery view of an enabled task: includes the frame URL to pull. */
export interface WorkerTask {
  id: string;
  camera_id: string;
  task_type: string;
  stream_profile: string;
  fps: number;
  width: number;
  config: Record<string, unknown>;
  /** Path to the latest sampled JPEG (GET, image/jpeg). */
  frame_url: string;
}

/** Per-camera sampler status (state + effective fps after budgeting). */
export interface SamplerInfo {
  camera_id: string;
  state: SamplerState;
  fps: number;
}

/** A detection result posted by an AI worker. */
export interface Detection {
  id: string;
  camera_id: string;
  task_type: string;
  timestamp: string;
  label?: string | null;
  confidence?: number | null;
  /** Normalized [x, y, w, h] in 0..1, relative to the sampled frame. */
  bbox?: number[] | null;
  track_id?: string | null;
  attributes: Record<string, unknown>;
  created_at: string;
}

// ---- Stage 3: zones + zone events ----

/** A polygon vertex, normalized [x, y] in 0..1 over the sampled frame. */
export type ZonePoint = [number, number];

/** Zone geometry / behavior kind (free-form; common values below). */
export type ZoneKind = "region" | "line" | "dwell" | (string & {});

/** Zone event verbs raised by the tracking engine. */
export type ZoneEventType = "enter" | "exit" | "dwell";

/** A polygon region on a camera; tracked detections crossing it raise enter/exit/dwell events. */
export interface Zone {
  id: string;
  camera_id: string;
  name: string;
  kind: string;
  /** Array of [x, y] vertices, normalized 0..1 over the sampled frame. */
  polygon: ZonePoint[];
  dwell_seconds: number;
  /** Detection labels that count toward this zone (empty = all labels). */
  labels: string[];
  severity: Severity;
  config: Record<string, unknown>;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export type ZoneCreate = Omit<Contract.ZoneCreate, "polygon"> & {
  polygon: [number, number][];
};

export type ZoneUpdate = Partial<ZoneCreate>;

/** A zone enter/exit/dwell event raised by the tracking engine. */
export interface ZoneEvent {
  id: string;
  camera_id: string;
  zone_id: string;
  zone_name: string;
  track_id?: string | null;
  event_type: ZoneEventType;
  label?: string | null;
  timestamp: string;
  dwell_seconds?: number | null;
  /** Served URL of the captured evidence frame (under /media/...), if any. */
  evidence_path?: string | null;
  created_at: string;
}

// ---- Stage 4: Access control + RBAC ----

export type Role = "admin" | "manager" | "guard" | "viewer" | "integration";

export interface Principal {
  id: string;
  name: string;
  role: Role;
  kind: "user" | "api_key" | "system";
}

export type UserView = Contract.UserView;

export interface LoginResult {
  token: string;
  expires_at: string;
  user: UserView;
}

export type UserCreate = Contract.UserCreate;

export type UserUpdate = Partial<Omit<UserCreate, "username">>;

export interface ApiKeyView {
  id: string;
  name: string;
  key_prefix: string;
  role: Role;
  active: boolean;
  last_used_at?: string | null;
  created_at: string;
}

/** Response from creating an API key — `key` is shown exactly once. */
export interface ApiKeyCreated {
  id: string;
  name: string;
  role: Role;
  key: string;
}

export type OwnerType = "student" | "staff" | "resident" | "contractor" | "visitor";

export type Vehicle = Contract.Vehicle;

export type VehicleCreate = Contract.VehicleCreate;

export type VehicleUpdate = Partial<VehicleCreate>;

export type PassStatus = Contract.PassStatus;

export type VisitorPass = Contract.VisitorPass;

export type VisitorPassCreate = Contract.VisitorPassCreate;

export type VisitorPassUpdate = Partial<VisitorPassCreate> & { status?: PassStatus };

export type WatchKind = "block" | "vip" | "alert";

export interface WatchlistEntry {
  id: string;
  plate: string;
  plate_norm: string;
  kind: WatchKind;
  reason?: string | null;
  severity: Severity;
  active: boolean;
  created_by?: string | null;
  created_at: string;
  updated_at: string;
}

export type WatchlistCreate = Contract.WatchlistCreate;

export type WatchlistUpdate = Partial<Omit<WatchlistCreate, "plate">>;

export type AuthStatus = "matched" | "exception" | "unmatched" | "blocked";
export type WorkflowStatus = "pending" | "confirmed" | "rejected" | "auto";
export type EntryEventType =
  | "vehicle_entry"
  | "vehicle_exit"
  | "visitor_checkin"
  | "visitor_checkout";

/** Canonical entry/exit event. */
export interface EntryEvent {
  id: string;
  site_id?: string | null;
  camera_id?: string | null;
  event_type: EntryEventType;
  timestamp: string;
  direction: "inbound" | "outbound" | "unknown";
  plate?: string | null;
  plate_confidence?: number | null;
  subject: Record<string, unknown>;
  authorization: Record<string, unknown>;
  auth_status: AuthStatus;
  evidence: Record<string, unknown>;
  workflow_status: WorkflowStatus;
  workflow: Record<string, unknown>;
  audit: Record<string, unknown>;
  track_id?: string | null;
  created_at: string;
}

export interface AuditLogEntry {
  id: string;
  actor: string;
  actor_name?: string | null;
  role?: string | null;
  action: string;
  target_type?: string | null;
  target_id?: string | null;
  detail: Record<string, unknown>;
  created_at: string;
}

export interface EntryLogReport {
  from: string;
  to: string;
  total: number;
  by_auth_status: Record<string, number>;
  events: EntryEvent[];
}

export interface ExceptionReport {
  from: string;
  to: string;
  total: number;
  events: EntryEvent[];
}

// ---- Stage 6: Movement intelligence (ReID candidates, trails, breaches) ----

export interface CameraLink {
  id: string;
  from_camera: string;
  to_camera: string;
  transit_seconds: number;
  bidirectional: boolean;
  note?: string | null;
  created_at: string;
  updated_at: string;
}

export interface MovementCandidate {
  id: string;
  subject_type: string;
  anchor?: string | null;
  from_camera?: string | null;
  from_ref?: string | null;
  from_time?: string | null;
  to_camera?: string | null;
  to_ref?: string | null;
  to_time?: string | null;
  transit_seconds?: number | null;
  score: number;
  signals: Record<string, unknown>;
  status: "pending" | "confirmed" | "rejected";
  reviewed_by?: string | null;
  reviewed_at?: string | null;
  created_at: string;
}

export interface BreachAlert {
  id: string;
  camera_id?: string | null;
  zone_id?: string | null;
  zone_name?: string | null;
  zone_event_id?: string | null;
  rule: string;
  subject_type?: string | null;
  subject?: string | null;
  track_id?: string | null;
  severity: Severity;
  status: "open" | "acknowledged" | "resolved";
  detail: Record<string, unknown>;
  evidence_path?: string | null;
  created_at: string;
  resolved_by?: string | null;
  resolved_at?: string | null;
}

export interface PlateSearchResult {
  plate: string;
  appearances: Array<{
    event_id: string;
    camera_id?: string | null;
    timestamp: string;
    event_type: string;
    auth_status: string;
    direction: string;
  }>;
  candidates: MovementCandidate[];
  note: string;
}

// ---- Stage 7: Semantic search ----

export type QueryPlan = Contract.QueryPlan;

export interface SearchHit {
  source: string;
  id: string;
  timestamp: string;
  camera_id?: string | null;
  kind: string;
  plate?: string | null;
  subject: Record<string, unknown>;
  auth_status?: string | null;
  zone?: string | null;
  zone_kind?: string | null;
  evidence_path?: string | null;
  claim_level: string;
}

/** Which clock a search was answered on (#125). Hour filters and relative dates are read in this
 *  zone; every timestamp in the response is UTC. */
export interface SearchInterpretation {
  timezone: string;
  timezone_source: string;
  hour_filter_read_in: string;
  note: string;
}

export interface SearchResponse {
  query?: string | null;
  planner: string;
  plan: QueryPlan;
  /** Absent on responses from a server older than #125 — treat as UTC, unlabelled. */
  interpretation?: SearchInterpretation;
  count: number;
  hits: SearchHit[];
  proof: {
    claim_levels: Array<Record<string, unknown>>;
    note: string;
  };
}

export interface SearchPlanResponse {
  query: string;
  planner: string;
  plan: QueryPlan;
}

/** Body for POST /api/v1/search/semantic — exactly one of `text` | `image_b64` (issue #38). */
export interface SemanticSearchRequest {
  text?: string;
  /** Base64 image payload (data-URL prefix stripped), <= 10,000,000 b64 chars. */
  image_b64?: string;
  from?: string;
  to?: string;
  cameras?: string[];
  label?: string;
  /** Zone-id scope (#77): only crops whose bbox ground point is inside the zone's polygon rank.
   *  Zones are per-camera — the server pins the camera; a conflicting `cameras` list is a 400. */
  zone?: string;
  /** Top-k, clamped server-side to 1..=100 (default 24). */
  k?: number;
}

/** One similarity-ranked embedding match (a detection crop, NOT a verified fact). */
export interface SemanticHit {
  id: string;
  /** Cosine similarity — higher = closer. Relative rank, not a probability. */
  score: number;
  camera_id: string;
  /** == embeddings.ts (observation time) — feed to playback. */
  timestamp: string;
  label?: string | null;
  track_id?: string | null;
  bbox?: number[] | null;
  evidence_path?: string | null;
  detection?: {
    confidence?: number | null;
    attributes?: Record<string, unknown> | null;
  } | null;
}

export interface SemanticSearchResponse {
  /** Echo of the text query, or "[image]" for image queries. */
  query: string;
  mode: string;
  /** Embedding model id; null only in degenerate cases (e.g. legacy rows with no model echo). */
  model: string | null;
  count: number;
  /** Echo of the zone scope when one was applied (#77); null/absent otherwise. */
  zone?: { id: string; name: string } | null;
  /** True if the candidate scan hit its cap — narrow the window/filters. */
  truncated: boolean;
  hits: SemanticHit[];
  proof: {
    claim_levels: Array<Record<string, unknown>>;
    note: string;
  };
}

// ---- Backup subsystem: destinations, policies, jobs, archive export ----

/** Transport for a backup destination. `local` copies via fs (NAS mounts); the rest use rclone. */
export type BackupKind = Contract.BackupKind;

/** Lifecycle of a backup job. */
export type BackupJobStatus = Contract.BackupJobStatus;

export type BackupDestinationView = Contract.BackupDestinationView;

export type BackupDestinationCreate = Contract.BackupDestinationCreate;

/** Partial update; to keep an existing secret, send it back as the `***` placeholder (or omit it). */
export type BackupDestinationUpdate = Partial<BackupDestinationCreate>;

export type BackupTestResult = Contract.BackupTestResult;

export type BackupPolicy = Contract.BackupPolicy;

export type BackupPolicyCreate = Contract.BackupPolicyCreate;

export type BackupPolicyUpdate = Partial<BackupPolicyCreate>;

export type BackupJob = Contract.BackupJob;

export type ArchiveExportRequest = Contract.ArchiveExportRequest;

// ---- ONVIF (Profile S MVP): discovery, device profile, PTZ ----

/** A device found by WS-Discovery (POST /api/v1/onvif/discover). */
export interface DiscoveredOnvifDevice {
  /** The device's wsa:EndpointReference Address (a urn:uuid: URN), if present. */
  endpoint_reference?: string | null;
  /** First transport address (the ONVIF device service URL to probe). */
  device_url: string;
  /** All advertised transport addresses. */
  xaddrs: string[];
  /** Host extracted from device_url (matches a camera's address). */
  address?: string | null;
  /** Advertised device types (e.g. `dn:NetworkVideoTransmitter`). */
  types?: string | null;
  /** Advertised scope URIs (name/hardware/location hints). */
  scopes: string[];
}

/** Response of POST /api/v1/onvif/discover. */
export interface OnvifDiscoverResponse {
  found: number;
  devices: DiscoveredOnvifDevice[];
}

/** Per-camera ONVIF device profile (GET /api/v1/cameras/{id}/onvif, POST .../onvif/probe). */
export interface CameraOnvif {
  camera_id: string;
  /** ONVIF device service endpoint URL. */
  device_url: string;
  manufacturer?: string | null;
  model?: string | null;
  firmware_version?: string | null;
  serial_number?: string | null;
  hardware_id?: string | null;
  /** ONVIF scope URIs (from WS-Discovery; empty when probed directly). */
  scopes: string[];
  /** Media service endpoint URL. */
  media_url?: string | null;
  /** PTZ service endpoint URL. */
  ptz_url?: string | null;
  /** Media profile token used for streaming + PTZ. */
  profile_token?: string | null;
  /** PTZ node bound to the chosen profile's PTZConfiguration. */
  ptz_node_token?: string | null;
  /** True when the device exposes PTZ AND the chosen profile carries a PTZConfiguration. */
  ptz_enabled: boolean;
  probed_at: string;
}

/** Optional request body for POST /api/v1/cameras/{id}/onvif/probe. */
export interface OnvifProbeRequest {
  /** Explicit ONVIF device service URL. Omit to derive from a prior probe or the camera's address. */
  device_url?: string;
}

/** A PTZ preset fetched from a camera's ONVIF PTZ service. */
export interface PtzPreset {
  id: string;
  camera_id: string;
  /** The device's preset token. */
  token: string;
  name?: string | null;
  fetched_at: string;
}

/** Request body for POST /api/v1/cameras/{id}/ptz/continuous (normalized velocities, -1.0..1.0). */
export interface PtzContinuousMoveRequest {
  pan?: number;
  tilt?: number;
  zoom?: number;
}

/** Request body for POST /api/v1/cameras/{id}/ptz/goto_preset. */
export interface PtzGotoPresetRequest {
  /** The device preset token to move to. */
  token: string;
}

// ---- Camera configuration (HikVision ISAPI): device, video, clock/NTP, ONVIF, OSD, bulk ----

export type DeviceInfo = Contract.DeviceInfo;

export type VideoConfig = Contract.VideoConfig;

export type VideoConfigPatch = Contract.VideoConfigPatch;

export type TimeConfig = Contract.TimeConfig;

export type NtpConfig = Contract.NtpConfig;

export type OnvifSettings = Contract.OnvifSettings;

export type OsdConfig = Contract.OsdConfig;

/** ONVIF user role; the device's verbatim `userType` values. */
export type OnvifUserType = "administrator" | "operator" | "mediaUser";

export type EnsureOnvifUserRequest = Contract.EnsureOnvifUserRequest;

/** Per-camera HikVision ISAPI state cache row (GET reads refresh it). */
export interface CameraIsapi {
  camera_id: string;
  device_name?: string | null;
  model?: string | null;
  firmware_version?: string | null;
  serial_number?: string | null;
  onvif_enabled: boolean;
  onvif_user_created: boolean;
  time_mode?: string | null;
  ntp_server?: string | null;
  fetched_at: string;
}

export type RebootRequest = Contract.RebootRequest;

/** Result of POST /api/v1/cameras/{id}/config/onvif/ensure_user. */
export interface EnableOnvifResult {
  ok: boolean;
  /** True when the kernel created the user on this call (false if it had already provisioned it). */
  created: boolean;
}

/** A single configuration action applied across one or more cameras (discriminated on `type`). */
export type BulkAction =
  | {
      type: "enable_onvif";
      /** Defaults server-side to `heldar_onvif` when omitted. */
      onvif_username?: string;
      onvif_password: string;
    }
  | { type: "sync_time"; ntp_server?: string | null }
  | { type: "set_ntp"; ntp_server: string }
  | {
      type: "set_video";
      /** null/omitted = the camera's main channel. */
      channel?: number | null;
      patch: VideoConfigPatch;
    };

export type BulkConfigRequest = Contract.BulkConfigRequest;

export type BulkCameraResult = Contract.BulkCameraResult;

export type BulkConfigResponse = Contract.BulkConfigResponse;
