// GENERATED FROM the served OpenAPI document BY scripts/gen_clients.py — DO NOT EDIT.
//
// The dashboard aliases these in `types.ts` rather than re-declaring them, so a field the
// server adds cannot go unnoticed here. Regenerate with:
//
//   cargo test -p heldar-server --test openapi_contract write_the_served_document
//   python3 scripts/gen_clients.py target/openapi.json clients
//
// Contract version: 0.1.0

export interface AiIngest {
  camera_id: string;
  detections?: DetectionIngest[];
  event?: IngestEvent | null;
  /** Optional per-camera monotonic frame id. When present, ingest is idempotent on (camera_id, frame_id): a duplicate redelivery is a no-op (no double-insert, no re-fire of consumer side effects). Omit it (e.g. the dependency-light client) to accept every batch. */
  frame_id?: string | null;
  /** Server-issued frame ticket from `x-frame-ticket` on the frame this batch describes.  When present and valid, `camera_id`, `task_type` and `frame_id` are all DERIVED from it and the body's own values are only cross-checked (409 on disagreement) — a worker can only speak about frames it was actually handed. Required under `HELDAR_INGEST_PROVENANCE=enforce`. */
  frame_ticket?: string | null;
  task_type: string;
  timestamp?: string | null;
}

export interface AiTaskCreate {
  config?: unknown;
  enabled?: boolean | null;
  fps?: number | null;
  stream_profile?: string | null;
  task_type: string;
  width?: number | null;
}

export interface AiTaskUpdate {
  config?: unknown;
  enabled?: boolean | null;
  fps?: number | null;
  stream_profile?: string | null;
  task_type?: string | null;
  width?: number | null;
}

export interface ApiKeyCreate {
  /** Explicit capability grant. Omitted = fall back to role expansion (what the dashboard and `validate_rbac.sh` do today), reported back as `legacy_role_expansion: true`. */
  capabilities?: string[] | null;
  /** Required to be `true` when the grant includes admin / registry:manage / gate:operate. */
  confirm_privileged?: boolean;
  expires_at?: string | null;
  name: string;
  role?: string | null;
  scope_cameras?: string[] | null;
  /** `all` (default) | `cameras`. */
  scope_kind?: string | null;
}

export interface ApiKeyUpdate {
  active?: boolean | null;
  capabilities?: string[] | null;
  confirm_privileged?: boolean;
  expires_at?: string | null;
  revoked_at?: string | null;
  scope_cameras?: string[] | null;
  scope_kind?: string | null;
}

export interface ArchiveExportRequest {
  /** Camera ids to include; empty/omitted means all cameras. */
  camera_ids?: string[];
  from?: string | null;
  incident_lock_only?: boolean | null;
  to?: string | null;
  /** Trim each segment to the [from, to] window (re-mux with -c copy); requires both bounds. */
  trim?: boolean | null;
}

export interface BackupDestinationCreate {
  config?: unknown;
  enabled?: boolean | null;
  /** `local` | `sftp` | `ftp` | `s3`. */
  kind: string;
  name: string;
}

export interface BackupDestinationUpdate {
  config?: unknown;
  enabled?: boolean | null;
  kind?: string | null;
  name?: string | null;
}

export interface BackupDestinationView {
  /** The config blob with any secret values masked to `***`. */
  config: Record<string, string>;
  created_at: string;
  enabled: boolean;
  /** Whether at least one secret credential is configured (so the UI can show "set" without the value). */
  has_credentials: boolean;
  id: string;
  kind: BackupKind;
  name: string;
  updated_at: string;
}

export interface BackupJob {
  bytes_copied: number;
  /** JSON array of camera ids the run covers; empty array means every camera on the box. */
  camera_ids: string[];
  created_at: string;
  /** Principal id that ORDERED this job — an api key id, a user id, or `system` when auth is off. NULL for the background scheduler (it holds no principal) and for rows predating migration 0015. A detached transfer re-checks this credential while it runs; see [`crate::services::backup::creator_standing`]. */
  created_by?: string | null;
  /** `api_key` | `user` | `system`, deciding HOW `created_by` is re-checked. NULL alongside a NULL `created_by`. */
  created_by_kind?: string | null;
  destination_id?: string | null;
  error?: string | null;
  files_copied: number;
  files_total: number;
  finished_at?: string | null;
  from_time?: string | null;
  id: string;
  incident_lock_only: boolean;
  /** `policy` | `on_demand_archive`. */
  kind: BackupKind;
  /** Filesystem path of the produced artifact (archive .zip), if any. */
  output_path?: string | null;
  /** Browser-fetchable URL of the produced artifact (under /media/archives/...), if any. */
  output_url?: string | null;
  policy_id?: string | null;
  started_at?: string | null;
  /** `pending` | `running` | `completed` | `error`. */
  status: BackupJobStatus;
  to_time?: string | null;
}

export type BackupJobStatus = "pending" | "running" | "completed" | "error";

export type BackupKind = "local" | "sftp" | "ftp" | "s3";

export interface BackupPolicy {
  /** JSON array of camera ids; empty array means all cameras. */
  camera_ids: string[];
  created_at: string;
  destination_id: string;
  enabled: boolean;
  id: string;
  incident_lock_only: boolean;
  last_job_id?: string | null;
  last_run_at?: string | null;
  lookback_hours: number;
  name: string;
  schedule_interval_s: number;
  updated_at: string;
}

export interface BackupPolicyCreate {
  camera_ids?: unknown;
  destination_id: string;
  enabled?: boolean | null;
  incident_lock_only?: boolean | null;
  lookback_hours?: number | null;
  name: string;
  schedule_interval_s?: number | null;
}

export interface BackupPolicyUpdate {
  camera_ids?: unknown;
  destination_id?: string | null;
  enabled?: boolean | null;
  incident_lock_only?: boolean | null;
  lookback_hours?: number | null;
  name?: string | null;
  schedule_interval_s?: number | null;
}

export interface BackupTestResult {
  error?: string | null;
  latency_ms: number;
  ok: boolean;
}

export interface BulkAction {
}

export interface BulkCameraResult {
  camera_id: string;
  error?: string | null;
  ok: boolean;
}

export interface BulkConfigRequest {
  action: BulkAction;
  camera_ids?: string[] | null;
}

export interface BulkConfigResponse {
  failed: number;
  results: BulkCameraResult[];
  succeeded: number;
}

export interface CameraLinkCreate {
  bidirectional?: boolean | null;
  from_camera: string;
  note?: string | null;
  to_camera: string;
  transit_seconds?: number | null;
}

export interface CameraView {
  address?: string | null;
  anr_enabled: boolean;
  anr_replay_url_template?: string | null;
  capabilities: unknown;
  codec?: string | null;
  created_at: string;
  enabled: boolean;
  fps_main?: number | null;
  fps_sub?: number | null;
  has_password: boolean;
  id: string;
  live_warm: boolean;
  mirror_enabled: boolean;
  model?: string | null;
  name: string;
  native_anpr_enabled: boolean;
  native_events_enabled: boolean;
  post_roll_seconds: number;
  pre_roll_seconds: number;
  priority: number;
  record_audio: boolean;
  record_enabled: boolean;
  record_mode: RecordMode;
  record_stream: string;
  /** Effective RTSP URL for the recorded stream, with credentials masked. */
  record_url_masked?: string | null;
  resolution_main?: string | null;
  resolution_sub?: string | null;
  retention_hours: number;
  rtsp_port: number;
  segment_seconds: number;
  site_id?: string | null;
  storage_quota_bytes?: number | null;
  updated_at: string;
  username?: string | null;
  vendor: string;
}

export interface ClipRequest {
  from: string;
  to: string;
}

export interface ContinuousMoveRequest {
  /** Pan velocity, clamped to -1.0..=1.0. */
  pan?: number;
  /** Tilt velocity, clamped to -1.0..=1.0. */
  tilt?: number;
  /** Zoom velocity, clamped to -1.0..=1.0. */
  zoom?: number;
}

/** An [x, y] pair, normalized 0..1 against the frame's width and height. */
export type Coordinate = [number, number];

export interface CreateSessionRequest {
  from: string;
  to: string;
}

export interface Credential {
  password: string;
  username: string;
}

export interface DayNightConfig {
  /** `auto` | `day` | `night` | `schedule` (verbatim ISAPI `IrcutFilterType`). */
  mode: string;
  /** Auto-switch sensitivity where the device exposes one (typically 0–7). */
  sensitivity?: number | null;
}

export interface DayNightPatch {
  mode?: string | null;
  sensitivity?: number | null;
}

export interface DbConvertResult {
  /** "already-incremental" (no-op) or "started" (a background conversion was kicked off). */
  status: string;
}

export interface DbLimitUpdate {
  /** New metadata-DB size cap in GB (> 0). Omit to leave unchanged. */
  max_db_gb?: number | null;
}

export interface DbStatus {
  db_bytes: number;
  incremental: boolean;
  max_db_bytes: number;
  max_db_gb: number;
  max_overridden: boolean;
}

export interface DetectionIngest {
  attributes?: unknown;
  bbox?: unknown;
  confidence?: number | null;
  label?: string | null;
  track_id?: string | null;
}

export interface DetectionUpdate {
  enabled: boolean;
}

export interface DeviceInfo {
  device_name?: string | null;
  firmware_version?: string | null;
  model?: string | null;
  serial_number?: string | null;
}

export interface DiscoverOptions {
  /** Register verified, not-yet-known devices as cameras (recording disabled by default). */
  auto_add?: boolean;
  connect_timeout_ms?: number | null;
  /** Additional credential sets to try, in order. */
  credentials?: Credential[] | null;
  password?: string | null;
  rtsp_port?: number | null;
  /** CIDR ("192.168.0.0/24"), range ("192.168.0.2-192.168.0.12"), single IP, or comma list. */
  targets: string;
  /** Also try a built-in default-credentials list (non-HikVision hosts only). */
  try_default_creds?: boolean;
  /** Single credential (convenience). Combined with `credentials` if both are given. */
  username?: string | null;
  /** Probe RTSP paths + credentials with ffprobe to confirm a working stream. */
  verify?: boolean;
}

export interface EmbeddingIngest {
  camera_id: string;
  dim: number;
  /** Batch idempotency key (same convention as detection ingest: `"{task_id}:{captured_at}"`).  Ignored when a valid `frame_ticket` is present — the kernel derives it, so a client can no longer name a frame it never held a ticket for. */
  frame_id?: string | null;
  /** Server-issued frame ticket (see [`crate::services::frame_ticket`]). Required under `HELDAR_INGEST_PROVENANCE=enforce`; when present, `camera_id` and `frame_id` come from it. */
  frame_ticket?: string | null;
  items: EmbeddingItem[];
  model: string;
}

export interface EmbeddingItem {
  /** Normalized `[x, y, w, h]`, like `detections.bbox`. */
  bbox?: unknown;
  detection_id?: string | null;
  label?: string | null;
  /** Optional base64 JPEG crop thumbnail, persisted to the snapshots dir as search evidence. */
  thumb_b64?: string | null;
  /** RFC3339 observation time; defaults to now. */
  timestamp?: string | null;
  track_id?: string | null;
  vec: number[];
}

export interface EnsureOnvifUserRequest {
  password: string;
  user_type?: OnvifUserType | null;
  username?: string;
}

export interface ErrorBody {
  /** Stable machine-readable identifier. Branch on this, not on `error`.  Exactly one of: `bad_request`, `unauthorized`, `forbidden`, `not_found`, `conflict`, `payload_too_large`, `rate_limited`, `unavailable`, `internal`.  This list is held to `AppError::ALL_CODES` by `codes_documented_match_codes_returned`, in both directions — a code the server can emit must appear here, and a code named here must be reachable. It previously listed one the server has never returned and omitted two it returns routinely, which is why the test exists rather than a correction. (The test also refuses the obsolete identifier by name, so this text cannot quietly reintroduce it while explaining it.) */
  code: string;
  /** Human-readable message. Not a stable identifier — do not match on it. */
  error: string;
  /** Whether retrying the SAME request could plausibly succeed. True only for transient saturation; a `404` or a validation failure will fail identically forever. Retryable responses also carry `Retry-After`. */
  retryable: boolean;
}

export interface EvidenceLockBody {
  incident_id?: string | null;
}

export interface ExportRequest {
  camera_id?: string | null;
  /** Default TRUE. An export writes footage off the box under a signature; making the destructive direction the one you have to ask for is the right way round. */
  dry_run?: boolean;
  from: string;
  /** Derive the camera from an incident's locked segments instead of naming it. */
  incident_id?: string | null;
  to: string;
}

export interface GatePolicy {
  camera_id: string;
  /** Auto-open on `matched` entry events (manual guard-open works whenever a policy row exists). */
  enabled: boolean;
  output_port: number;
  pulse_ms: number;
  updated_at: string;
}

export interface GatePolicyUpdate {
  enabled?: boolean | null;
  output_port?: number | null;
  pulse_ms?: number | null;
}

export interface GateSettingsUpdate {
  kill_switch: boolean;
}

export interface GotoPresetRequest {
  /** The preset's DEVICE token, as reported by `/ptz/presets` — not the stored row id. */
  token: string;
}

export interface ImageConfig {
  /** Backlight compensation enabled (`BLC`). */
  blc_enabled?: boolean | null;
  /** 0–100 (`color`). */
  brightness?: number | null;
  /** 0–100 (`color`). */
  contrast?: number | null;
  /** IR-light brightness 0–100. */
  ir_light_brightness?: number | null;
  /** 0–100 (`color`). */
  saturation?: number | null;
  /** Supplement-light brightness regulation: `auto` | `manual` (brightness sliders apply in `manual`; in `auto` the camera manages them). */
  supplement_brightness_mode?: string | null;
  /** Supplement-light mode where exposed (`supplementLight/supplementLightMode`). Verified live values: `irLight` (infrared B/W), `colorVuWhiteLight` (white light, full-color night), `eventIntelligence` ("smart" — IR normally, white light on detected events), `close`. The camera's actual option list is in the capability map (`supplement_light_modes`). */
  supplement_light_mode?: string | null;
  /** WDR strength 0–100 (`WDR`). */
  wdr_level?: number | null;
  /** Wide dynamic range: `open` | `close` | `auto` (`WDR`). */
  wdr_mode?: string | null;
  /** White-light brightness 0–100 (white-light-capable models only). */
  white_light_brightness?: number | null;
}

export interface IncidentSummary {
  incident_id: string;
  newest_end: string;
  oldest_start: string;
  segment_count: number;
  total_bytes: number;
}

export interface IncidentTagBody {
  incident_id?: string | null;
}

export interface IngestEvent {
  event_type: string;
  payload?: unknown;
  severity?: string | null;
}

export interface IntrusionConfig {
  enabled: boolean;
  regions: SmartRegion[];
}

export interface IoOutput {
  /** Idle/default state as the device reports it (e.g. `low`). */
  default_state?: string | null;
  id: number;
  name?: string | null;
}

export interface LeaseRequest {
  /** Cap on how many tasks to take in one call (default: all eligible). */
  max_tasks?: number | null;
  /** Restrict the lease to these task types (default: any). */
  task_types?: string[] | null;
  /** Requested lease lifetime; clamped to 15..=300 s. */
  ttl_secs?: number | null;
  worker_id: string;
}

export interface LineCrossingConfig {
  enabled: boolean;
  lines: SmartLine[];
}

export interface LoginRequest {
  password: string;
  username: string;
}

export interface ModuleRegisterRequest {
  /** The sidecar's origin the kernel reverse-proxies to (http/https), e.g. `http://127.0.0.1:9123`. */
  base_url: string;
  description?: string;
  /** Stable id (slug): the `/m/{id}/` mount + nav key. Must not collide with a compiled module. */
  id: string;
  name: string;
  /** Nav entries to surface (defaults to one entry at `/{id}` if omitted). */
  nav?: NavEntry[];
  publisher?: string;
  /** Role of the minted API key. Restricted to least-privilege (`viewer` | `integration`). */
  role?: string | null;
  /** Event types to deliver to the sidecar's webhook (`["*"]` = all). Defaults to all. */
  subscribes?: string[] | null;
  version?: string;
}

export interface MotionConfig {
  enabled: boolean;
  /** 0–100 where exposed (`MotionDetectionLayout/sensitivityLevel`). */
  sensitivity?: number | null;
}

export interface NavEntry {
  /** Icon key the dashboard maps to a glyph. */
  icon: string;
  /** Human label shown in the nav rail. */
  label: string;
  /** Client route path, e.g. `/entry`. */
  path: string;
}

export interface NlBody {
  /** The question, in plain language. Must be non-empty. */
  query: string;
}

export interface NtpConfig {
  /** `hostname` or `ipaddress`. */
  addressing_format: string;
  host_name: string;
  port: number;
}

export interface OnvifSettings {
  isapi_enabled: boolean;
  onvif_enabled: boolean;
}

export type OnvifUserType = "administrator" | "operator" | "mediaUser";

export interface OsdConfig {
  channel_name_enabled: boolean;
  date_style?: string | null;
  datetime_enabled: boolean;
  display_week?: boolean | null;
  time_style?: string | null;
}

export type PassStatus = "active" | "checked_in" | "checked_out" | "expired" | "revoked";

export interface ProbeRequest {
  /** Optional explicit ONVIF device service URL (e.g. `http://host/onvif/device_service`). When omitted, the URL is taken from a prior probe or derived from the camera's address. */
  device_url?: string | null;
}

export interface PulseRequest {
  /** Pulse width in milliseconds (0/absent = the service default of 1000; clamped to 30000). */
  pulse_ms?: number;
}

export interface QueryPlan {
  auth_status?: string[];
  cameras?: string[];
  color?: string | null;
  event_type?: string | null;
  from?: string | null;
  hour_max?: number | null;
  /** Time-of-day filter, e.g. "after 6pm" ⇒ hour_min=18.  Read in [`QueryPlan::tz`], NOT in UTC — see that field. "after 6pm" meaning 18:00 UTC is how a search at a Malaysian site quietly answers about 2am. */
  hour_min?: number | null;
  limit?: number | null;
  plate?: string | null;
  /** Which fact sources to search: any of entry | zone | breach (empty ⇒ all). */
  sources?: string[];
  /** vehicle | person */
  subject_type?: string | null;
  /** Free-text substring matched across plate / zone / kind. */
  text?: string | null;
  to?: string | null;
  /** The IANA zone `hour_min`/`hour_max` and relative dates are read in (#125).  ALWAYS SERIALIZED, even when absent, because this plan is written to `search_log` as the accountability record for identity-bearing searches. A logged plan with no `tz` field is one written before this existed and means "UTC, unlabelled"; a plan with `tz: null` means the zone was resolved at execution time and is echoed in the response. Without the field, rows from before and after this change look identical and mean different things. */
  tz?: string | null;
  vehicle_type?: string | null;
  /** Zone-id scope — SEMANTIC route only (issue #77): recorded here so the search_log snapshot captures it. The structured executor ignores it and `planner::sanitize` clears it, so a structured/NL caller can never set it expecting filtering that doesn't happen. */
  zone?: string | null;
  zone_kind?: string | null;
}

export interface QueryResult {
  dim?: number | null;
  error?: string | null;
  model?: string | null;
  vec?: number[] | null;
}

export interface RebootRequest {
  confirm: boolean;
}

export type RecordMode = "continuous" | "scheduled" | "event" | "scheduled_event";

export interface RecordScheduleCreate {
  /** JSON array of weekday ints (0=Mon..6=Sun). */
  days: unknown;
  enabled?: boolean | null;
  /** "HH:MM" 24h, server local time (start > end means an overnight window). */
  time_end: string;
  /** "HH:MM" 24h, server local time. */
  time_start: string;
}

export interface RecordScheduleUpdate {
  days?: unknown;
  enabled?: boolean | null;
  time_end?: string | null;
  time_start?: string | null;
}

export interface ResolveBody {
  note?: string | null;
}

export interface RetentionLimits {
  max_overridden: boolean;
  max_recordings_bytes: number;
  max_recordings_gb: number;
  min_free_disk_bytes: number;
  min_free_disk_gb: number;
  min_free_overridden: boolean;
}

export interface RetentionUpdate {
  /** Compute the effect and change nothing (#121). */
  dry_run?: boolean;
  /** New global recordings cap in GB (> 0). Omit to leave unchanged. */
  max_recordings_gb?: number | null;
  /** New free-disk floor in GB (>= 0; 0 disables the floor). Omit to leave unchanged. */
  min_free_disk_gb?: number | null;
  /** The `plan_hash` from a dry run. Supplying it makes the commit refuse if anything the plan depended on has moved since. Omit to commit without planning. */
  plan_hash?: string | null;
}

export interface SemanticBody {
  cameras?: string[];
  from?: string | null;
  /** Base64 image (JPEG/PNG). A `data:` URL prefix is tolerated and stripped. */
  image_b64?: string | null;
  k?: number | null;
  /** Exact detection label filter (e.g. "car"). */
  label?: string | null;
  text?: string | null;
  to?: string | null;
  /** Zone scope (issue #77): a zone id — only crops whose bbox ground point falls inside the zone's polygon are ranked. Zones are per-camera, so this pins the camera implicitly. */
  zone?: string | null;
}

export interface SiteCreate {
  id: string;
  name: string;
  timezone?: string | null;
}

export interface SiteRow {
  /** Typed, not a raw column string: sqlx hands back `+00:00` while every other model in the API re-serializes as `Z`, and one box speaking two timestamp dialects is a trap a generated client will not catch — OpenAPI types both as plain `string`. */
  created_at: string;
  id: string;
  name: string;
  /** IANA identifier, or `null` when the site has not chosen one.  Null is a real state, not a placeholder: migration 0019 removed the `NOT NULL DEFAULT 'UTC'` precisely so that "nobody has chosen" is distinguishable from "chose UTC". A site with no zone falls through to the box-wide default. */
  timezone?: string | null;
}

export interface SiteUpdate {
  name?: string | null;
  /** An IANA identifier, or explicit `null` to clear it back to the box default. */
  timezone?: string | null;
}

export interface SmartLine {
  /** `any` | `left-right` | `right-left` (verbatim device tokens). */
  direction: string;
  enabled: boolean;
  id: number;
  /** Exactly two endpoints, normalized 0..1. */
  points: Coordinate[];
  /** 1–100. */
  sensitivity: number;
}

export interface SmartRegion {
  enabled: boolean;
  id: number;
  /** Polygon vertices, normalized 0..1 (empty = slot unconfigured). */
  points: Coordinate[];
  /** 1–100. */
  sensitivity: number;
  /** Seconds a target must stay inside before the alarm fires (device `timeThreshold`). */
  time_threshold: number;
}

export interface SnapshotSchedule {
  camera_id: string;
  created_at: string;
  enabled: boolean;
  id: string;
  interval_seconds: number;
  last_fired_at?: string | null;
  updated_at: string;
}

export interface SnapshotScheduleCreate {
  enabled?: boolean | null;
  interval_seconds?: number | null;
}

export interface SnapshotScheduleUpdate {
  enabled?: boolean | null;
  interval_seconds?: number | null;
}

export interface TimeConfig {
  /** ISO8601 local time with tz offset. */
  local_time: string;
  /** `manual` or `NTP`. */
  time_mode: string;
  /** e.g. `CST-8:00:00`. */
  time_zone: string;
}

export interface TimezoneSettings {
  /** The IANA identifier configured box-wide, or `null` when none is.  Always present in the response. */
  configured: string | null;
  /** The server's own local offset (`%:z`), for spotting a container whose `TZ` disagrees. */
  server_local_offset: string;
  source: TzSource;
  /** What an unconfigured box does, stated rather than left to be discovered: schedules follow the SERVER's local zone and search follows UTC. Setting a zone makes both follow it. */
  unconfigured_behaviour: string;
}

export interface TimezoneUpdate {
  /** An IANA identifier, e.g. `Asia/Kuala_Lumpur`. Empty clears it. */
  timezone: string;
}

export interface TranscodeSettings {
  /** The engine new live publishers use: `software` | `vaapi` | `nvenc`. */
  engine: string;
  /** The `HELDAR_LIVE_TRANSCODE_ENGINE` env default this falls back to. */
  env_default: string;
  /** `/dev/nvidia*` present (NVIDIA NVENC). */
  nvenc_available: boolean;
  /** True when the engine is an operator override (settings table) vs the env default. */
  overridden: boolean;
  /** `/dev/dri/renderD*` present (Intel/AMD VAAPI render node). */
  vaapi_available: boolean;
}

export interface TranscodeUpdate {
  /** New engine (`software` | `vaapi` | `nvenc`). */
  engine: string;
}

export type TzSource = "site" | "default" | "unset";

export interface UserCreate {
  active?: boolean | null;
  display_name?: string | null;
  password: string;
  role?: string | null;
  username: string;
}

export interface UserUpdate {
  active?: boolean | null;
  display_name?: string | null;
  password?: string | null;
  role?: string | null;
}

export interface UserView {
  active: boolean;
  created_at: string;
  display_name?: string | null;
  id: string;
  role: string;
  updated_at: string;
  username: string;
}

export interface Vehicle {
  active: boolean;
  color?: string | null;
  created_at: string;
  id: string;
  make?: string | null;
  model?: string | null;
  notes?: string | null;
  owner_name?: string | null;
  owner_ref?: string | null;
  owner_type: string;
  plate: string;
  plate_norm: string;
  site_id?: string | null;
  updated_at: string;
  valid_from?: string | null;
  valid_until?: string | null;
  vehicle_type?: string | null;
}

export interface VehicleCreate {
  active?: boolean | null;
  color?: string | null;
  make?: string | null;
  model?: string | null;
  notes?: string | null;
  owner_name?: string | null;
  owner_ref?: string | null;
  owner_type?: string | null;
  plate: string;
  site_id?: string | null;
  valid_from?: string | null;
  valid_until?: string | null;
  vehicle_type?: string | null;
}

export interface VehicleUpdate {
  active?: boolean | null;
  color?: string | null;
  make?: string | null;
  model?: string | null;
  notes?: string | null;
  owner_name?: string | null;
  owner_ref?: string | null;
  owner_type?: string | null;
  plate?: string | null;
  site_id?: string | null;
  valid_from?: string | null;
  valid_until?: string | null;
  vehicle_type?: string | null;
}

export interface VideoConfig {
  bitrate: number;
  channel_id: number;
  channel_name?: string | null;
  codec: string;
  fps: number;
  gop: number;
  height: number;
  quality_control: string;
  vbr_upper_cap: number;
  width: number;
}

export interface VideoConfigPatch {
  bitrate?: number | null;
  codec?: string | null;
  fps?: number | null;
  gop?: number | null;
  height?: number | null;
  quality_control?: string | null;
  vbr_upper_cap?: number | null;
  width?: number | null;
}

export interface VisitorPass {
  checked_in_at?: string | null;
  checked_out_at?: string | null;
  code: string;
  company?: string | null;
  created_at: string;
  created_by?: string | null;
  host?: string | null;
  id: string;
  phone?: string | null;
  plate?: string | null;
  plate_norm?: string | null;
  purpose?: string | null;
  site_id?: string | null;
  status: PassStatus;
  updated_at: string;
  valid_from: string;
  valid_until: string;
  vehicle_desc?: string | null;
  visitor_name: string;
}

export interface VisitorPassCreate {
  company?: string | null;
  host?: string | null;
  phone?: string | null;
  plate?: string | null;
  purpose?: string | null;
  site_id?: string | null;
  valid_from?: string | null;
  valid_until?: string | null;
  vehicle_desc?: string | null;
  visitor_name: string;
}

export interface VisitorPassUpdate {
  company?: string | null;
  host?: string | null;
  phone?: string | null;
  plate?: string | null;
  purpose?: string | null;
  status?: string | null;
  valid_from?: string | null;
  valid_until?: string | null;
  vehicle_desc?: string | null;
  visitor_name?: string | null;
}

export interface Watchlist {
  active: boolean;
  created_at: string;
  created_by?: string | null;
  id: string;
  kind: string;
  plate: string;
  plate_norm: string;
  reason?: string | null;
  severity: string;
  updated_at: string;
}

export interface WatchlistCreate {
  active?: boolean | null;
  kind?: string | null;
  plate: string;
  reason?: string | null;
  severity?: string | null;
}

export interface WatchlistUpdate {
  active?: boolean | null;
  kind?: string | null;
  reason?: string | null;
  severity?: string | null;
}

export interface WebhookDelivery {
  attempts: number;
  created_at: string;
  delivered_at?: string | null;
  error?: string | null;
  event_id?: string | null;
  event_type?: string | null;
  id: string;
  response_code?: number | null;
  status: string;
  subscription_id: string;
}

export interface WebhookSubscriptionCreate {
  enabled?: boolean | null;
  /** Omitted/empty = all types (`["*"]`). */
  event_types?: string[] | null;
  /** `info` | `warning` | `critical` (default `info`). */
  min_severity?: string | null;
  name: string;
  /** Optional HMAC-SHA256 signing secret. */
  secret?: string | null;
  url: string;
}

export interface WebhookSubscriptionUpdate {
  enabled?: boolean | null;
  event_types?: string[] | null;
  min_severity?: string | null;
  name?: string | null;
  /** Three-state on the wire: omitted = unchanged, `null` = clear, a string = set. OpenAPI has no way to say "omitted differs from null", so it types as a nullable string. */
  secret?: string | null;
  url?: string | null;
}

export interface WebhookSubscriptionView {
  created_at: string;
  cursor_at?: string | null;
  enabled: boolean;
  event_types: string[];
  /** Whether an HMAC signing secret is configured (the value itself is never returned). */
  has_secret: boolean;
  id: string;
  min_severity: string;
  name: string;
  updated_at: string;
  url: string;
}

export interface WebhookTestResult {
  /** Why the delivery failed, absent on success. */
  error?: string | null;
  /** Whether the target accepted the delivery. */
  ok: boolean;
  /** The target's HTTP status, absent when the request never got a response. */
  status?: number | null;
}

export interface ZoneCreate {
  config?: unknown;
  dwell_seconds?: number | null;
  enabled?: boolean | null;
  kind?: string | null;
  labels?: unknown;
  name: string;
  /** Vertices, normalized 0..1. At least 3 for a polygon, exactly 2 for a `line` zone. */
  polygon: Coordinate[];
  severity?: string | null;
}

export interface ZoneUpdate {
  config?: unknown;
  dwell_seconds?: number | null;
  enabled?: boolean | null;
  kind?: string | null;
  labels?: unknown;
  name?: string | null;
  /** Vertices, normalized 0..1. Omit to leave the geometry unchanged. */
  polygon?: Coordinate[] | null;
  severity?: string | null;
}
