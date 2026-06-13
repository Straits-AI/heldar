// TypeScript mirror of the VisionOps Core HTTP API contract (serde JSON).
// Field names match the Rust structs in apps/core/src/{models.rs,routes/*}.

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
}
