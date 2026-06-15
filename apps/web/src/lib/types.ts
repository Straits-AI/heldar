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
export type RecordMode = "continuous" | "scheduled" | "event" | "scheduled_event";

/** Known vendors with auto-built RTSP URLs, plus the catch-all. */
export type Vendor = "hikvision" | "dahua" | "generic" | (string & {});

export type Severity = "info" | "warning" | "critical";

export interface CameraView {
  id: string;
  site_id?: string | null;
  name: string;
  vendor: string;
  model?: string | null;
  address?: string | null;
  rtsp_port: number;
  username?: string | null;
  has_password: boolean;
  record_stream: RecordStream;
  /** Effective RTSP URL for the recorded stream, credentials masked. */
  record_url_masked?: string | null;
  codec?: string | null;
  resolution_main?: string | null;
  resolution_sub?: string | null;
  fps_main?: number | null;
  fps_sub?: number | null;
  capabilities: Record<string, unknown>;
  record_enabled: boolean;
  segment_seconds: number;
  retention_hours: number;
  /** Per-camera storage quota in bytes; null means no per-camera cap. */
  storage_quota_bytes?: number | null;
  /** Record the camera's audio stream (pass-through) instead of dropping it. */
  record_audio: boolean;
  /** When the recorder runs (continuous | scheduled | event | scheduled_event). */
  record_mode: RecordMode;
  /** Event recording: footage desired BEFORE a trigger (best-effort; honored only from recent
   * completed segments — no always-on ring buffer for idle event cameras). Clamped 0..300. */
  pre_roll_seconds: number;
  /** Event recording: how long the recorder keeps writing after a trigger (the window). 0..3600. */
  post_roll_seconds: number;
  /** Run a SECOND ffmpeg pipeline writing identical segments to HELDAR_MIRROR_RECORDINGS_DIR
   * (redundant DVR copy). No-op unless the mirror dir is configured server-side. */
  mirror_enabled: boolean;
  /** Let the ANR loop re-fetch missed footage from the camera's onboard storage to fill gaps. */
  anr_enabled: boolean;
  /** Replay URL template for ANR re-fill ({start}/{end} placeholders, Hikvision time format);
   * null = default Hikvision RTSP playback built from address+credentials. */
  anr_replay_url_template?: string | null;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface CameraCreate {
  id?: string;
  name: string;
  site_id?: string;
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
  enabled?: boolean;
}

export type CameraUpdate = Partial<Omit<CameraCreate, "id">>;

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

/** Roll-up of segments tagged to one incident (GET /api/v1/incidents). */
export interface IncidentSummary {
  incident_id: string;
  segment_count: number;
  total_bytes: number;
  oldest_start: string;
  newest_end: string;
}

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
  /** HLS .m3u8 playlist — play with hls.js. */
  hls_url: string;
  webrtc_url: string;
  rtsp_url: string;
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

export interface DiscoverOptions {
  /** CIDR ("192.168.0.0/24"), range ("192.168.0.2-192.168.0.12"), single IP, or comma list. */
  targets: string;
  username?: string;
  password?: string;
  /** Probe each candidate with ffprobe + credentials to confirm a working stream. */
  verify?: boolean;
  /** Register verified, not-yet-known devices as cameras (recording disabled by default). */
  auto_add?: boolean;
  rtsp_port?: number;
}

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

export interface SystemInfo {
  name: string;
  version: string;
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
  /** No recent disk_smart_warning/raid_degraded events (SMART/RAID health pass). */
  disk_health_ok: boolean;
  /** Timestamp of the most recent disk-health alert (any time), or null if none. */
  last_disk_alert_at?: string | null;
  /** Active live-preview transcode engine (software | vaapi | nvenc). */
  live_transcode_engine: string;
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
 * in the SERVER's local timezone (start > end means an overnight window). */
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

export interface RecordScheduleCreate {
  days: number[];
  time_start: string;
  time_end: string;
  enabled?: boolean;
}

export type RecordScheduleUpdate = Partial<RecordScheduleCreate>;

// ---- Scheduled interval snapshots ----

/** A per-camera schedule that captures a live JPEG every `interval_seconds`. */
export interface SnapshotSchedule {
  id: string;
  camera_id: string;
  interval_seconds: number;
  enabled: boolean;
  /** Last time the scheduler fired this schedule (null until it first fires). */
  last_fired_at?: string | null;
  created_at: string;
  updated_at: string;
}

export interface SnapshotScheduleCreate {
  interval_seconds?: number;
  enabled?: boolean;
}

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

export interface AiTaskCreate {
  task_type: string;
  fps?: number;
  width?: number;
  stream_profile?: StreamProfile;
  config?: Record<string, unknown>;
  enabled?: boolean;
}

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

export interface ZoneCreate {
  name: string;
  polygon: ZonePoint[];
  kind?: string;
  dwell_seconds?: number;
  labels?: string[];
  severity?: Severity;
  config?: Record<string, unknown>;
  enabled?: boolean;
}

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

export interface UserView {
  id: string;
  username: string;
  role: Role;
  display_name?: string | null;
  active: boolean;
  created_at: string;
  updated_at: string;
}

export interface LoginResult {
  token: string;
  expires_at: string;
  user: UserView;
}

export interface UserCreate {
  username: string;
  password: string;
  role?: Role;
  display_name?: string;
  active?: boolean;
}

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

export interface Vehicle {
  id: string;
  plate: string;
  plate_norm: string;
  owner_name?: string | null;
  owner_type: OwnerType;
  owner_ref?: string | null;
  site_id?: string | null;
  vehicle_type?: string | null;
  make?: string | null;
  model?: string | null;
  color?: string | null;
  notes?: string | null;
  active: boolean;
  valid_from?: string | null;
  valid_until?: string | null;
  created_at: string;
  updated_at: string;
}

export interface VehicleCreate {
  plate: string;
  owner_name?: string;
  owner_type?: OwnerType;
  owner_ref?: string;
  site_id?: string;
  vehicle_type?: string;
  make?: string;
  model?: string;
  color?: string;
  notes?: string;
  active?: boolean;
  valid_from?: string;
  valid_until?: string;
}

export type VehicleUpdate = Partial<VehicleCreate>;

export type PassStatus = "active" | "checked_in" | "checked_out" | "expired" | "revoked";

export interface VisitorPass {
  id: string;
  code: string;
  visitor_name: string;
  phone?: string | null;
  company?: string | null;
  host?: string | null;
  purpose?: string | null;
  plate?: string | null;
  plate_norm?: string | null;
  vehicle_desc?: string | null;
  site_id?: string | null;
  valid_from: string;
  valid_until: string;
  status: PassStatus;
  checked_in_at?: string | null;
  checked_out_at?: string | null;
  created_by?: string | null;
  created_at: string;
  updated_at: string;
}

export interface VisitorPassCreate {
  visitor_name: string;
  phone?: string;
  company?: string;
  host?: string;
  purpose?: string;
  plate?: string;
  vehicle_desc?: string;
  site_id?: string;
  valid_from?: string;
  valid_until?: string;
}

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

export interface WatchlistCreate {
  plate: string;
  kind?: WatchKind;
  reason?: string;
  severity?: Severity;
  active?: boolean;
}

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

export interface QueryPlan {
  from?: string | null;
  to?: string | null;
  hour_min?: number | null;
  hour_max?: number | null;
  cameras?: string[];
  sources?: string[];
  plate?: string | null;
  color?: string | null;
  vehicle_type?: string | null;
  subject_type?: string | null;
  auth_status?: string[];
  event_type?: string | null;
  zone_kind?: string | null;
  text?: string | null;
  limit?: number | null;
}

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

export interface SearchResponse {
  query?: string | null;
  planner: string;
  plan: QueryPlan;
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

// ---- Backup subsystem: destinations, policies, jobs, archive export ----

/** Transport for a backup destination. `local` copies via fs (NAS mounts); the rest use rclone. */
export type BackupKind = "local" | "sftp" | "ftp" | "s3";

/** Lifecycle of a backup job. */
export type BackupJobStatus = "pending" | "running" | "completed" | "error";

/** A backup destination as returned to clients — secret config values are masked to `***`. */
export interface BackupDestinationView {
  id: string;
  name: string;
  kind: BackupKind;
  /** Kind-specific config blob with secret values (pass/secret_key/…) replaced by `***`. */
  config: Record<string, unknown>;
  /** Whether at least one secret credential is configured (the masked value hides whether it is set). */
  has_credentials: boolean;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface BackupDestinationCreate {
  name: string;
  kind: BackupKind;
  /** local: {path}; sftp/ftp: {host,port,user,pass,path}; s3: {bucket,prefix,access_key,secret_key,endpoint,region}. */
  config?: Record<string, unknown>;
  enabled?: boolean;
}

/** Partial update; to keep an existing secret, send it back as the `***` placeholder (or omit it). */
export type BackupDestinationUpdate = Partial<BackupDestinationCreate>;

/** Result of POST /api/v1/backup/destinations/{id}/test. */
export interface BackupTestResult {
  ok: boolean;
  error?: string | null;
  latency_ms: number;
}

/** A scheduled backup policy: ship a camera selection's recent footage to a destination on an interval. */
export interface BackupPolicy {
  id: string;
  name: string;
  destination_id: string;
  /** Camera ids to include; empty array means all cameras. */
  camera_ids: string[];
  incident_lock_only: boolean;
  schedule_interval_s: number;
  /** How far back each run reaches (0 = everything up to now). */
  lookback_hours: number;
  last_run_at?: string | null;
  last_job_id?: string | null;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface BackupPolicyCreate {
  name: string;
  destination_id: string;
  camera_ids?: string[];
  incident_lock_only?: boolean;
  schedule_interval_s?: number;
  lookback_hours?: number;
  enabled?: boolean;
}

export type BackupPolicyUpdate = Partial<BackupPolicyCreate>;

/** A single backup run (scheduled, manually triggered, or an on-demand archive export). */
export interface BackupJob {
  id: string;
  policy_id?: string | null;
  destination_id?: string | null;
  /** `policy` | `on_demand_archive`. */
  kind: string;
  camera_ids: string[];
  from_time?: string | null;
  to_time?: string | null;
  incident_lock_only: boolean;
  status: BackupJobStatus;
  files_total: number;
  files_copied: number;
  bytes_copied: number;
  error?: string | null;
  output_path?: string | null;
  /** Browser-fetchable URL of the produced archive (under /media/archives/...), if any. */
  output_url?: string | null;
  started_at?: string | null;
  finished_at?: string | null;
  created_at: string;
}

/** Request body for POST /api/v1/archive/export. */
export interface ArchiveExportRequest {
  /** Camera ids to include; empty/omitted means all cameras. */
  camera_ids?: string[];
  from?: string;
  to?: string;
  incident_lock_only?: boolean;
  /** Trim each segment to the [from, to] window (re-mux with -c copy); requires both bounds. */
  trim?: boolean;
}

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
