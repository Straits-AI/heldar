// GENERATED FROM openapi.json BY scripts/gen_clients.py — DO NOT EDIT.
//
// Regenerate with:  cargo test -p heldar-server --test openapi_contract write_the_served_document
//                   python3 scripts/gen_clients.py target/openapi.json clients
//
// Contract version: 0.1.0


export interface AiIngest {
  camera_id: string;
  detections?: DetectionIngest[];
  event?: IngestEvent | null;
  frame_id?: string | null;
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
  capabilities?: string[] | null;
  confirm_privileged?: boolean;
  expires_at?: string | null;
  name: string;
  role?: string | null;
  scope_cameras?: string[] | null;
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
  camera_ids?: string[];
  from?: string | null;
  incident_lock_only?: boolean | null;
  to?: string | null;
  trim?: boolean | null;
}

export interface BackupDestinationCreate {
  config?: unknown;
  enabled?: boolean | null;
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
  config: Record<string, string>;
  created_at: string;
  enabled: boolean;
  has_credentials: boolean;
  id: string;
  kind: BackupKind;
  name: string;
  updated_at: string;
}

export interface BackupJob {
  bytes_copied: number;
  camera_ids: string[];
  created_at: string;
  created_by?: string | null;
  created_by_kind?: string | null;
  destination_id?: string | null;
  error?: string | null;
  files_copied: number;
  files_total: number;
  finished_at?: string | null;
  from_time?: string | null;
  id: string;
  incident_lock_only: boolean;
  kind: BackupKind;
  output_path?: string | null;
  output_url?: string | null;
  policy_id?: string | null;
  started_at?: string | null;
  status: BackupJobStatus;
  to_time?: string | null;
}

export type BackupJobStatus = "pending" | "running" | "completed" | "error";

export type BackupKind = "local" | "sftp" | "ftp" | "s3";

export interface BackupPolicy {
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
  pan?: number;
  tilt?: number;
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
  mode: string;
  sensitivity?: number | null;
}

export interface DayNightPatch {
  mode?: string | null;
  sensitivity?: number | null;
}

export interface DbConvertResult {
  status: string;
}

export interface DbLimitUpdate {
  dry_run?: boolean;
  max_db_gb?: number | null;
  plan_hash?: string | null;
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
  auto_add?: boolean;
  connect_timeout_ms?: number | null;
  credentials?: Credential[] | null;
  password?: string | null;
  rtsp_port?: number | null;
  targets: string;
  try_default_creds?: boolean;
  username?: string | null;
  verify?: boolean;
}

export interface EmbeddingIngest {
  camera_id: string;
  dim: number;
  frame_id?: string | null;
  frame_ticket?: string | null;
  items: EmbeddingItem[];
  model: string;
}

export interface EmbeddingItem {
  bbox?: unknown;
  detection_id?: string | null;
  label?: string | null;
  thumb_b64?: string | null;
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
  code: string;
  error: string;
  retryable: boolean;
}

export interface EvidenceLockBody {
  incident_id?: string | null;
}

export interface ExportRequest {
  camera_id?: string | null;
  dry_run?: boolean;
  from: string;
  incident_id?: string | null;
  to: string;
}

export interface GatePolicy {
  camera_id: string;
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
  token: string;
}

export interface ImageConfig {
  blc_enabled?: boolean | null;
  brightness?: number | null;
  contrast?: number | null;
  ir_light_brightness?: number | null;
  saturation?: number | null;
  supplement_brightness_mode?: string | null;
  supplement_light_mode?: string | null;
  wdr_level?: number | null;
  wdr_mode?: string | null;
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
  default_state?: string | null;
  id: number;
  name?: string | null;
}

export interface LeaseRequest {
  max_tasks?: number | null;
  task_types?: string[] | null;
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
  base_url: string;
  description?: string;
  id: string;
  name: string;
  nav?: NavEntry[];
  publisher?: string;
  role?: string | null;
  subscribes?: string[] | null;
  version?: string;
}

export interface MotionConfig {
  enabled: boolean;
  sensitivity?: number | null;
}

export interface NavEntry {
  icon: string;
  label: string;
  path: string;
}

export interface NlBody {
  query: string;
}

export interface NtpConfig {
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
  device_url?: string | null;
}

export interface PulseRequest {
  pulse_ms?: number;
}

export interface QueryPlan {
  auth_status?: string[];
  cameras?: string[];
  color?: string | null;
  event_type?: string | null;
  from?: string | null;
  hour_max?: number | null;
  hour_min?: number | null;
  limit?: number | null;
  plate?: string | null;
  sources?: string[];
  subject_type?: string | null;
  text?: string | null;
  to?: string | null;
  tz?: string | null;
  vehicle_type?: string | null;
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
  days: unknown;
  enabled?: boolean | null;
  time_end: string;
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
  dry_run?: boolean;
  max_recordings_gb?: number | null;
  min_free_disk_gb?: number | null;
  plan_hash?: string | null;
}

export interface SemanticBody {
  cameras?: string[];
  from?: string | null;
  image_b64?: string | null;
  k?: number | null;
  label?: string | null;
  text?: string | null;
  to?: string | null;
  zone?: string | null;
}

export interface SiteCreate {
  id: string;
  name: string;
  timezone?: string | null;
}

export interface SiteRow {
  created_at: string;
  id: string;
  name: string;
  timezone?: string | null;
}

export interface SiteUpdate {
  name?: string | null;
  timezone?: string | null;
}

export interface SmartLine {
  direction: string;
  enabled: boolean;
  id: number;
  points: Coordinate[];
  sensitivity: number;
}

export interface SmartRegion {
  enabled: boolean;
  id: number;
  points: Coordinate[];
  sensitivity: number;
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
  local_time: string;
  time_mode: string;
  time_zone: string;
}

export interface TimezoneSettings {
  configured: string | null;
  server_local_offset: string;
  source: TzSource;
  unconfigured_behaviour: string;
}

export interface TimezoneUpdate {
  timezone: string;
}

export interface TranscodeSettings {
  engine: string;
  env_default: string;
  nvenc_available: boolean;
  overridden: boolean;
  vaapi_available: boolean;
}

export interface TranscodeUpdate {
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
  event_types?: string[] | null;
  min_severity?: string | null;
  name: string;
  secret?: string | null;
  url: string;
}

export interface WebhookSubscriptionUpdate {
  enabled?: boolean | null;
  event_types?: string[] | null;
  min_severity?: string | null;
  name?: string | null;
  secret?: string | null;
  url?: string | null;
}

export interface WebhookSubscriptionView {
  created_at: string;
  cursor_at?: string | null;
  enabled: boolean;
  event_types: string[];
  has_secret: boolean;
  id: string;
  min_severity: string;
  name: string;
  updated_at: string;
  url: string;
}

export interface WebhookTestResult {
  error?: string | null;
  ok: boolean;
  status?: number | null;
}

export interface ZoneCreate {
  config?: unknown;
  dwell_seconds?: number | null;
  enabled?: boolean | null;
  kind?: string | null;
  labels?: unknown;
  name: string;
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
  polygon?: Coordinate[] | null;
  severity?: string | null;
}

export interface RequestOptions { baseUrl?: string; token?: string; }

export class HeldarClient {
  constructor(private opts: RequestOptions = {}) {}

  private async call<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (this.opts.token) headers["Authorization"] = `Bearer ${this.opts.token}`;
    const res = await fetch(`${this.opts.baseUrl ?? ""}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) {
      // Every endpoint returns the same error shape, so a caller writes one error path.
      const err = (await res.json().catch(() => ({}))) as Partial<ErrorBody>;
      throw Object.assign(new Error(err.error ?? res.statusText), {
        code: err.code ?? "internal",
        retryable: err.retryable ?? false,
        status: res.status,
      });
    }
    return (await res.json()) as T;
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  deleteAiTask(task_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/ai-tasks/${encodeURIComponent(task_id)}`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  updateAiTask(task_id: string, body: AiTaskUpdate): Promise<unknown> {
    return this.call<unknown>("PATCH", `/api/v1/ai-tasks/${encodeURIComponent(task_id)}`, body);
  }

  /** Requires capability `ai:embedwork`, scope-neutral. */
  claimEmbedQueries(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/ai/embed-queries`);
  }

  /** Requires capability `ai:embedwork`, scope-neutral. */
  submitEmbedQueryResult(id: string, body: QueryResult): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/ai/embed-queries/${encodeURIComponent(id)}/result`, body);
  }

  /** Requires capability `ai:ingest`, camera-keyed. */
  ingestAiEmbeddings(body: EmbeddingIngest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/ai/embeddings`, body);
  }

  /** Requires capability `ai:ingest`, camera-keyed. */
  ingestAiEvents(body: AiIngest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/ai/events`, body);
  }

  /** Requires capability `ai:tasks`, scope-filtered. */
  acquireAiLease(body: LeaseRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/ai/leases`, body);
  }

  /** Requires capability `ai:tasks`, scope-neutral. */
  releaseAiLease(lease_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/ai/leases/${encodeURIComponent(lease_id)}`);
  }

  /** Requires capability `ai:tasks`, scope-filtered. */
  listAiSamplers(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/ai/samplers`);
  }

  /** Requires capability `ai:tasks`, scope-filtered. */
  discoverAiTasks(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/ai/tasks`);
  }

  /** Requires admin, fleet-only. */
  listApiKeys(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/api-keys`);
  }

  /** Requires admin, fleet-only. */
  createApiKey(body: ApiKeyCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/api-keys`, body);
  }

  /** Requires admin, fleet-only. */
  deleteApiKey(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/api-keys/${encodeURIComponent(id)}`);
  }

  /** Requires admin, fleet-only. */
  updateApiKey(id: string, body: ApiKeyUpdate): Promise<unknown> {
    return this.call<unknown>("PATCH", `/api/v1/api-keys/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  createArchiveExport(body: ArchiveExportRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/archive/export`, body);
  }

  /** Requires capability `system:read`, scope-filtered. */
  listArchiveExports(): Promise<BackupJob[]> {
    return this.call<BackupJob[]>("GET", `/api/v1/archive/exports`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  listAuditLog(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/audit`);
  }

  /** Requires scope-neutral. */
  login(body: LoginRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/auth/login`, body);
  }

  /** Requires scope-neutral. */
  logout(): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/auth/logout`);
  }

  /** Requires scope-neutral. */
  getCurrentPrincipal(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/auth/me`);
  }

  /** Requires capability `system:read`, fleet-only. */
  listBackupDestinations(): Promise<BackupDestinationView[]> {
    return this.call<BackupDestinationView[]>("GET", `/api/v1/backup/destinations`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  createBackupDestination(body: BackupDestinationCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/backup/destinations`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  deleteBackupDestination(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/backup/destinations/${encodeURIComponent(id)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  updateBackupDestination(id: string, body: BackupDestinationUpdate): Promise<BackupDestinationView> {
    return this.call<BackupDestinationView>("PATCH", `/api/v1/backup/destinations/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  testBackupDestination(id: string): Promise<BackupTestResult> {
    return this.call<BackupTestResult>("POST", `/api/v1/backup/destinations/${encodeURIComponent(id)}/test`);
  }

  /** Requires capability `system:read`, scope-filtered. */
  listBackupJobs(): Promise<BackupJob[]> {
    return this.call<BackupJob[]>("GET", `/api/v1/backup/jobs`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  deleteBackupJob(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/backup/jobs/${encodeURIComponent(id)}`);
  }

  /** Requires capability `system:read`, scope-filtered. */
  getBackupJob(id: string): Promise<BackupJob> {
    return this.call<BackupJob>("GET", `/api/v1/backup/jobs/${encodeURIComponent(id)}`);
  }

  /** Requires capability `system:read`, scope-filtered. */
  listBackupPolicies(): Promise<BackupPolicy[]> {
    return this.call<BackupPolicy[]>("GET", `/api/v1/backup/policies`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  createBackupPolicy(body: BackupPolicyCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/backup/policies`, body);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  deleteBackupPolicy(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/backup/policies/${encodeURIComponent(id)}`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  updateBackupPolicy(id: string, body: BackupPolicyUpdate): Promise<BackupPolicy> {
    return this.call<BackupPolicy>("PATCH", `/api/v1/backup/policies/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  triggerBackupPolicy(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/backup/policies/${encodeURIComponent(id)}/trigger`);
  }

  /** Requires capability `camera:read`, scope-filtered. */
  listCameras(): Promise<CameraView[]> {
    return this.call<CameraView[]>("GET", `/api/v1/cameras`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  bulkCameraConfig(body: BulkConfigRequest): Promise<BulkConfigResponse> {
    return this.call<BulkConfigResponse>("POST", `/api/v1/cameras/config/bulk`, body);
  }

  /** Requires capability `admin`, camera-keyed. */
  deleteCamera(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/cameras/${encodeURIComponent(id)}`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCamera(id: string): Promise<CameraView> {
    return this.call<CameraView>("GET", `/api/v1/cameras/${encodeURIComponent(id)}`);
  }

  /** Requires capability `ai:tasks`, camera-keyed. */
  listCameraAiTasks(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/ai-tasks`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  createAiTask(id: string, body: AiTaskCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/ai-tasks`, body);
  }

  /** Requires capability `video:export`, camera-keyed. */
  exportClip(id: string, body: ClipRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/clip`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraDeviceInfo(id: string): Promise<DeviceInfo> {
    return this.call<DeviceInfo>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/device_info`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraOnvifSettings(id: string): Promise<OnvifSettings> {
    return this.call<OnvifSettings>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/onvif`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  putCameraOnvifSettings(id: string, body: OnvifSettings): Promise<OnvifSettings> {
    return this.call<OnvifSettings>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/config/onvif`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  ensureCameraOnvifUser(id: string, body: EnsureOnvifUserRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/config/onvif/ensure_user`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraOsd(id: string): Promise<OsdConfig> {
    return this.call<OsdConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/osd`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  putCameraOsd(id: string, body: OsdConfig): Promise<OsdConfig> {
    return this.call<OsdConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/config/osd`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  rebootCamera(id: string, body: RebootRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/config/reboot`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraTime(id: string): Promise<TimeConfig> {
    return this.call<TimeConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/time`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  putCameraTime(id: string, body: TimeConfig): Promise<TimeConfig> {
    return this.call<TimeConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/config/time`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraNtp(id: string): Promise<NtpConfig> {
    return this.call<NtpConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/time/ntp`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  putCameraNtp(id: string, body: NtpConfig): Promise<NtpConfig> {
    return this.call<NtpConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/config/time/ntp`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  syncCameraTimeNow(id: string): Promise<TimeConfig> {
    return this.call<TimeConfig>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/config/time/sync_now`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  listCameraVideoConfigs(id: string): Promise<VideoConfig[]> {
    return this.call<VideoConfig[]>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/video`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraVideoConfig(id: string, channel: string): Promise<VideoConfig> {
    return this.call<VideoConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/config/video/${encodeURIComponent(channel)}`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  putCameraVideoConfig(id: string, channel: string, body: VideoConfigPatch): Promise<VideoConfig> {
    return this.call<VideoConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/config/video/${encodeURIComponent(channel)}`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraControlCapabilities(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/capabilities`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraDayNight(id: string): Promise<DayNightConfig> {
    return this.call<DayNightConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/day_night`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  setCameraDayNight(id: string, body: DayNightPatch): Promise<DayNightConfig> {
    return this.call<DayNightConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/control/day_night`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  setCameraBuiltinDetection(id: string, kind: string, body: DetectionUpdate): Promise<unknown> {
    return this.call<unknown>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/control/detections/${encodeURIComponent(kind)}`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraImage(id: string): Promise<ImageConfig> {
    return this.call<ImageConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/image`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  setCameraImage(id: string, body: ImageConfig): Promise<ImageConfig> {
    return this.call<ImageConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/control/image`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraIntrusion(id: string): Promise<IntrusionConfig> {
    return this.call<IntrusionConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/intrusion`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  setCameraIntrusion(id: string, body: IntrusionConfig): Promise<IntrusionConfig> {
    return this.call<IntrusionConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/control/intrusion`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  listCameraIoOutputs(id: string): Promise<IoOutput[]> {
    return this.call<IoOutput[]>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/io/outputs`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  pulseCameraIoOutput(id: string, port: string, body: PulseRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/control/io/outputs/${encodeURIComponent(port)}/pulse`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraLineCrossing(id: string): Promise<LineCrossingConfig> {
    return this.call<LineCrossingConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/line_crossing`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  setCameraLineCrossing(id: string, body: LineCrossingConfig): Promise<LineCrossingConfig> {
    return this.call<LineCrossingConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/control/line_crossing`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraMotion(id: string): Promise<MotionConfig> {
    return this.call<MotionConfig>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/control/motion`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  setCameraMotion(id: string, body: MotionConfig): Promise<MotionConfig> {
    return this.call<MotionConfig>("PUT", `/api/v1/cameras/${encodeURIComponent(id)}/control/motion`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  probeCameraControl(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/control/probe`);
  }

  /** Requires capability `events:read`, camera-keyed. */
  listDetections(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/detections`);
  }

  /** Requires capability `ai:frames`, camera-keyed. */
  getLatestFrame(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/frame`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  listGaps(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/gaps`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraHealth(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/health`);
  }

  /** Requires capability `video:live`, camera-keyed. */
  getLiveView(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/liveview`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCameraOnvif(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/onvif`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  probeCameraOnvif(id: string, body: ProbeRequest | null): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/onvif/probe`, body);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  createPlaybackSession(id: string, body: CreateSessionRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/playback/sessions`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  ptzContinuousMove(id: string, body: ContinuousMoveRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/ptz/continuous`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  ptzGotoPreset(id: string, body: GotoPresetRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/ptz/goto_preset`, body);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  listPtzPresets(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/ptz/presets`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  refreshPtzPresets(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/ptz/presets/refresh`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  ptzStop(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/ptz/stop`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  triggerRecording(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/record-trigger`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  listRecordingGaps(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/recording-gaps`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  retryRecordingGap(id: string, gap_id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/recording-gaps/${encodeURIComponent(gap_id)}/retry`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  listRecordingSchedules(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/schedules`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  createRecordingSchedule(id: string, body: RecordScheduleCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/schedules`, body);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  listSegments(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/segments`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  getSnapshot(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/snapshot`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  listSnapshotSchedules(id: string): Promise<SnapshotSchedule[]> {
    return this.call<SnapshotSchedule[]>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/snapshot-schedules`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  createSnapshotSchedule(id: string, body: SnapshotScheduleCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/snapshot-schedules`, body);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  listCameraSnapshots(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/snapshots`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  testCamera(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/test`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  getTimeline(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/timeline`);
  }

  /** Requires capability `events:read`, camera-keyed. */
  listZoneEvents(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/zone-events`);
  }

  /** Requires capability `events:read`, camera-keyed. */
  getZoneEventAggregates(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/zone-events/aggregates`);
  }

  /** Requires capability `events:read`, camera-keyed. */
  listZones(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/zones`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  createZone(id: string, body: ZoneCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/zones`, body);
  }

  /** Requires capability `events:read`, camera-keyed. */
  getZoneOccupancy(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/zones/occupancy`);
  }

  /** Requires capability `net:scan`, fleet-only. */
  discoverCameras(body: DiscoverOptions): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/discover`, body);
  }

  /** Requires capability `events:read`, scope-filtered. */
  listEntryEvents(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/entry-events`);
  }

  /** Requires capability `events:read`, camera-keyed. */
  getEntryEvent(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/entry-events/${encodeURIComponent(id)}`);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  confirmEntryEvent(id: string, body: ResolveBody): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/entry-events/${encodeURIComponent(id)}/confirm`, body);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  rejectEntryEvent(id: string, body: ResolveBody): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/entry-events/${encodeURIComponent(id)}/reject`, body);
  }

  /** Requires capability `identity:read`, scope-filtered. */
  getGateState(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/entry/gate`);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  openGate(camera_id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/entry/gate/open/${encodeURIComponent(camera_id)}`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  deleteGatePolicy(camera_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/entry/gate/policies/${encodeURIComponent(camera_id)}`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  putGatePolicy(camera_id: string, body: GatePolicyUpdate): Promise<GatePolicy> {
    return this.call<GatePolicy>("PUT", `/api/v1/entry/gate/policies/${encodeURIComponent(camera_id)}`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  updateGateSettings(body: GateSettingsUpdate): Promise<unknown> {
    return this.call<unknown>("PUT", `/api/v1/entry/gate/settings`, body);
  }

  /** Requires capability `events:read`, fleet-only. */
  listEvents(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/events`);
  }

  /** Requires capability `events:read`, scope-neutral. */
  listEventTypes(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/events/types`);
  }

  /** Requires capability `video:export`, scope-filtered. */
  listEvidenceExports(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/evidence/exports`);
  }

  /** Requires capability `video:export`, camera-keyed. */
  createEvidenceExport(body: ExportRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/evidence/exports`, body);
  }

  /** Requires capability `video:export`, camera-keyed. */
  getEvidenceExport(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/evidence/exports/${encodeURIComponent(id)}`);
  }

  /** Requires capability `camera:read`, scope-neutral. */
  getEvidenceSigningKey(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/evidence/signing-key`);
  }

  /** Requires capability `camera:read`, scope-filtered. */
  listCameraHealth(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/health/cameras`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  listIncidents(): Promise<IncidentSummary[]> {
    return this.call<IncidentSummary[]>("GET", `/api/v1/incidents`);
  }

  /** Requires capability `video:playback`, scope-filtered. */
  listIncidentSegments(incident_id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/incidents/${encodeURIComponent(incident_id)}/segments`);
  }

  /** Requires capability `system:read`, fleet-only. */
  listModules(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/modules`);
  }

  /** Requires admin, fleet-only. */
  registerModule(body: ModuleRegisterRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/modules`, body);
  }

  /** Requires capability `events:read`, scope-neutral. */
  getEntryModuleUi(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/modules/entry/ui/index.js`);
  }

  /** Requires capability `events:read`, scope-neutral. */
  getMovementModuleUi(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/modules/movement/ui/index.js`);
  }

  /** Requires capability `events:read`, scope-neutral. */
  getSearchModuleUi(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/modules/search/ui/index.js`);
  }

  /** Requires admin, camera-keyed. */
  unregisterModule(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/modules/${encodeURIComponent(id)}`);
  }

  /** Requires admin, camera-keyed. */
  getModule(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/modules/${encodeURIComponent(id)}`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  listMovementBreaches(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/movement/breaches`);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  ackMovementBreach(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/movement/breaches/${encodeURIComponent(id)}/ack`);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  resolveMovementBreach(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/movement/breaches/${encodeURIComponent(id)}/resolve`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  listMovementCandidates(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/movement/candidates`);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  confirmMovementCandidate(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/movement/candidates/${encodeURIComponent(id)}/confirm`);
  }

  /** Requires capability `gate:operate`, camera-keyed. */
  rejectMovementCandidate(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/movement/candidates/${encodeURIComponent(id)}/reject`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  listMovementLinks(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/movement/links`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  createMovementLink(body: CameraLinkCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/movement/links`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  deleteMovementLink(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/movement/links/${encodeURIComponent(id)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  runMovementEngines(): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/movement/run`);
  }

  /** Requires capability `events:read`, camera-keyed. */
  searchMovementByPersonTrack(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/movement/search/person`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  searchMovementByPlate(plate: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/movement/search/plate/${encodeURIComponent(plate)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  discoverOnvifDevices(): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/onvif/discover`);
  }

  /** Requires scope-neutral. */
  getOpenApiDocument(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/openapi.json`);
  }

  /** Requires admin, fleet-only. */
  listOutbox(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/outbox`);
  }

  /** Requires capability `identity:read`, scope-neutral. */
  listVisitorPasses(): Promise<VisitorPass[]> {
    return this.call<VisitorPass[]>("GET", `/api/v1/passes`);
  }

  /** Requires capability `gate:operate`, fleet-only. */
  createVisitorPass(body: VisitorPassCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/passes`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  deleteVisitorPass(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/passes/${encodeURIComponent(id)}`);
  }

  /** Requires capability `identity:read`, scope-neutral. */
  getVisitorPass(id: string): Promise<VisitorPass> {
    return this.call<VisitorPass>("GET", `/api/v1/passes/${encodeURIComponent(id)}`);
  }

  /** Requires capability `gate:operate`, fleet-only. */
  updateVisitorPass(id: string, body: VisitorPassUpdate): Promise<VisitorPass> {
    return this.call<VisitorPass>("PATCH", `/api/v1/passes/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `gate:operate`, fleet-only. */
  checkInVisitorPass(id: string): Promise<VisitorPass> {
    return this.call<VisitorPass>("POST", `/api/v1/passes/${encodeURIComponent(id)}/checkin`);
  }

  /** Requires capability `gate:operate`, fleet-only. */
  checkOutVisitorPass(id: string): Promise<VisitorPass> {
    return this.call<VisitorPass>("POST", `/api/v1/passes/${encodeURIComponent(id)}/checkout`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  deletePlaybackSession(session_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/playback/sessions/${encodeURIComponent(session_id)}`);
  }

  /** Requires capability `system:read`, fleet-only. */
  listRegistry(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/registry`);
  }

  /** Requires admin, fleet-only. */
  refreshRegistry(): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/registry/refresh`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  getEntryLogReport(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/reports/entry-log`);
  }

  /** Requires capability `events:read`, scope-filtered. */
  getEntryExceptionsReport(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/reports/exceptions`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  deleteRecordingSchedule(schedule_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/schedules/${encodeURIComponent(schedule_id)}`);
  }

  /** Requires capability `registry:manage`, scope-filtered. */
  updateRecordingSchedule(schedule_id: string, body: RecordScheduleUpdate): Promise<unknown> {
    return this.call<unknown>("PATCH", `/api/v1/schedules/${encodeURIComponent(schedule_id)}`, body);
  }

  /** Requires capability `events:read`, scope-filtered. */
  searchEvents(body: QueryPlan): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/search/events`, body);
  }

  /** Requires capability `events:read`, scope-filtered. */
  searchNaturalLanguage(body: NlBody): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/search/nl`, body);
  }

  /** Requires capability `events:read`, scope-filtered. */
  planSearch(body: NlBody): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/search/plan`, body);
  }

  /** Requires capability `events:read`, scope-filtered. */
  searchSemantic(body: SemanticBody): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/search/semantic`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  unlockSegmentEvidence(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/segments/${encodeURIComponent(id)}/evidence-lock`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  lockSegmentEvidence(id: string, body: EvidenceLockBody): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/segments/${encodeURIComponent(id)}/evidence-lock`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  tagSegmentIncident(id: string, body: IncidentTagBody): Promise<unknown> {
    return this.call<unknown>("PATCH", `/api/v1/segments/${encodeURIComponent(id)}/incident`, body);
  }

  /** Requires capability `system:read`, scope-filtered. */
  getSiteInfo(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/site`);
  }

  /** Requires capability `camera:read`, scope-filtered. */
  listSites(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/sites`);
  }

  /** Requires admin, fleet-only. */
  createSite(body: SiteCreate): Promise<SiteRow> {
    return this.call<SiteRow>("POST", `/api/v1/sites`, body);
  }

  /** Requires admin, fleet-only. */
  deleteSite(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/sites/${encodeURIComponent(id)}`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getSite(id: string): Promise<SiteRow> {
    return this.call<SiteRow>("GET", `/api/v1/sites/${encodeURIComponent(id)}`);
  }

  /** Requires admin, fleet-only. */
  updateSite(id: string, body: SiteUpdate): Promise<unknown> {
    return this.call<unknown>("PATCH", `/api/v1/sites/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  deleteSnapshotSchedule(schedule_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/snapshot-schedules/${encodeURIComponent(schedule_id)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  updateSnapshotSchedule(schedule_id: string, body: SnapshotScheduleUpdate): Promise<SnapshotSchedule> {
    return this.call<SnapshotSchedule>("PATCH", `/api/v1/snapshot-schedules/${encodeURIComponent(schedule_id)}`, body);
  }

  /** Requires capability `system:read`, scope-filtered. */
  getSystemInfo(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/system`);
  }

  /** Requires capability `system:read`, fleet-only. */
  getDbStatus(): Promise<DbStatus> {
    return this.call<DbStatus>("GET", `/api/v1/system/db`);
  }

  /** Requires admin, fleet-only. */
  setDbLimit(body: DbLimitUpdate): Promise<unknown> {
    return this.call<unknown>("PUT", `/api/v1/system/db`, body);
  }

  /** Requires admin, fleet-only. */
  convertDbAutoVacuum(): Promise<DbConvertResult> {
    return this.call<DbConvertResult>("POST", `/api/v1/system/db/convert`);
  }

  /** Requires admin, fleet-only. */
  getSecurityPosture(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/system/posture`);
  }

  /** Requires admin, fleet-only. */
  getProvenanceReadiness(): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/system/provenance-readiness`);
  }

  /** Requires capability `system:read`, scope-neutral. */
  getRetentionLimits(): Promise<RetentionLimits> {
    return this.call<RetentionLimits>("GET", `/api/v1/system/retention`);
  }

  /** Requires admin, fleet-only. */
  setRetentionLimits(body: RetentionUpdate): Promise<unknown> {
    return this.call<unknown>("PUT", `/api/v1/system/retention`, body);
  }

  /** Requires capability `system:read`, scope-neutral. */
  getTimezone(): Promise<TimezoneSettings> {
    return this.call<TimezoneSettings>("GET", `/api/v1/system/timezone`);
  }

  /** Requires admin, fleet-only. */
  setTimezone(body: TimezoneUpdate): Promise<TimezoneSettings> {
    return this.call<TimezoneSettings>("PUT", `/api/v1/system/timezone`, body);
  }

  /** Requires capability `system:read`, scope-neutral. */
  getTranscodeSettings(): Promise<TranscodeSettings> {
    return this.call<TranscodeSettings>("GET", `/api/v1/system/transcode`);
  }

  /** Requires admin, fleet-only. */
  setTranscodeEngine(body: TranscodeUpdate): Promise<TranscodeSettings> {
    return this.call<TranscodeSettings>("PUT", `/api/v1/system/transcode`, body);
  }

  /** Requires admin, fleet-only. */
  listUsers(): Promise<UserView[]> {
    return this.call<UserView[]>("GET", `/api/v1/users`);
  }

  /** Requires admin, fleet-only. */
  createUser(body: UserCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/users`, body);
  }

  /** Requires admin, fleet-only. */
  deleteUser(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/users/${encodeURIComponent(id)}`);
  }

  /** Requires admin, fleet-only. */
  updateUser(id: string, body: UserUpdate): Promise<UserView> {
    return this.call<UserView>("PATCH", `/api/v1/users/${encodeURIComponent(id)}`, body);
  }

  /** Requires admin, fleet-only. */
  unlockUser(id: string): Promise<UserView> {
    return this.call<UserView>("POST", `/api/v1/users/${encodeURIComponent(id)}/unlock`);
  }

  /** Requires capability `identity:read`, scope-neutral. */
  listVehicles(): Promise<Vehicle[]> {
    return this.call<Vehicle[]>("GET", `/api/v1/vehicles`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  createVehicle(body: VehicleCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/vehicles`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  deleteVehicle(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/vehicles/${encodeURIComponent(id)}`);
  }

  /** Requires capability `identity:read`, scope-neutral. */
  getVehicle(id: string): Promise<Vehicle> {
    return this.call<Vehicle>("GET", `/api/v1/vehicles/${encodeURIComponent(id)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  updateVehicle(id: string, body: VehicleUpdate): Promise<Vehicle> {
    return this.call<Vehicle>("PATCH", `/api/v1/vehicles/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `identity:read`, scope-neutral. */
  listWatchlist(): Promise<Watchlist[]> {
    return this.call<Watchlist[]>("GET", `/api/v1/watchlist`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  createWatchlistEntry(body: WatchlistCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/watchlist`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  deleteWatchlistEntry(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/watchlist/${encodeURIComponent(id)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  updateWatchlistEntry(id: string, body: WatchlistUpdate): Promise<Watchlist> {
    return this.call<Watchlist>("PATCH", `/api/v1/watchlist/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `events:read`, scope-neutral. */
  listWebhookSubscriptions(): Promise<WebhookSubscriptionView[]> {
    return this.call<WebhookSubscriptionView[]>("GET", `/api/v1/webhooks`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  createWebhookSubscription(body: WebhookSubscriptionCreate): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/webhooks`, body);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  deleteWebhookSubscription(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/webhooks/${encodeURIComponent(id)}`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  updateWebhookSubscription(id: string, body: WebhookSubscriptionUpdate): Promise<WebhookSubscriptionView> {
    return this.call<WebhookSubscriptionView>("PATCH", `/api/v1/webhooks/${encodeURIComponent(id)}`, body);
  }

  /** Requires capability `events:read`, scope-neutral. */
  listWebhookDeliveries(id: string): Promise<WebhookDelivery[]> {
    return this.call<WebhookDelivery[]>("GET", `/api/v1/webhooks/${encodeURIComponent(id)}/deliveries`);
  }

  /** Requires capability `registry:manage`, fleet-only. */
  testWebhookSubscription(id: string): Promise<WebhookTestResult> {
    return this.call<WebhookTestResult>("POST", `/api/v1/webhooks/${encodeURIComponent(id)}/test`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  deleteZone(zone_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/zones/${encodeURIComponent(zone_id)}`);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  updateZone(zone_id: string, body: ZoneUpdate): Promise<unknown> {
    return this.call<unknown>("PATCH", `/api/v1/zones/${encodeURIComponent(zone_id)}`, body);
  }

}
