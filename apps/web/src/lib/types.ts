// TypeScript mirror of the Heldar HTTP API contract (serde JSON).
// Field names match the Rust structs in crates/heldar-kernel/src/{models.rs,routes/*} (kernel)
// and crates/heldar-entry/src/{models.rs,routes.rs} (Campus Entry app).

export type CameraStatusState =
  | "disabled"
  | "connecting"
  | "recording"
  | "offline"
  | "error"
  | "unknown";

export type RecordStream = "main" | "sub";

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
  enabled?: boolean;
}

export type CameraUpdate = Partial<Omit<CameraCreate, "id">>;

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
  locked: boolean;
  incident_id?: string | null;
  created_at: string;
  /** Browser-playable URL under /media/recordings/... */
  url: string;
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

// ---- Stage 4: Campus Entry + RBAC ----

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

/** Canonical entry/exit event (memo §8.1). */
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
