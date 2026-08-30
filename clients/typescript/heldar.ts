// GENERATED FROM openapi.json BY scripts/gen_clients.py — DO NOT EDIT.
//
// Regenerate with:  cargo test -p heldar-server --test openapi_contract write_the_served_document
//                   python3 scripts/gen_clients.py target/openapi.json clients
//
// Contract version: 0.1.0


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
  record_mode: string;
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

export interface CreateSessionRequest {
  from: string;
  to: string;
}

export interface ErrorBody {
  code: string;
  error: string;
  retryable: boolean;
}

export interface ExportRequest {
  camera_id?: string | null;
  dry_run?: boolean;
  from: string;
  incident_id?: string | null;
  to: string;
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

export interface TimezoneSettings {
  configured?: string | null;
  server_local_offset: string;
  source: TzSource;
  unconfigured_behaviour: string;
}

export interface TimezoneUpdate {
  timezone: string;
}

export type TzSource = "site" | "default" | "unset";

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

  /** Requires capability `camera:read`, scope-filtered. */
  listCameras(): Promise<CameraView[]> {
    return this.call<CameraView[]>("GET", `/api/v1/cameras`);
  }

  /** Requires capability `admin`, camera-keyed. */
  deleteCamera(id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/cameras/${encodeURIComponent(id)}`);
  }

  /** Requires capability `camera:read`, camera-keyed. */
  getCamera(id: string): Promise<CameraView> {
    return this.call<CameraView>("GET", `/api/v1/cameras/${encodeURIComponent(id)}`);
  }

  /** Requires capability `video:export`, camera-keyed. */
  exportClip(id: string, body: ClipRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/clip`, body);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  listGaps(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/gaps`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  createPlaybackSession(id: string, body: CreateSessionRequest): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/playback/sessions`, body);
  }

  /** Requires capability `registry:manage`, camera-keyed. */
  triggerRecording(id: string): Promise<unknown> {
    return this.call<unknown>("POST", `/api/v1/cameras/${encodeURIComponent(id)}/record-trigger`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  listSegments(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/segments`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  getSnapshot(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/snapshot`);
  }

  /** Requires capability `video:playback`, camera-keyed. */
  getTimeline(id: string): Promise<unknown> {
    return this.call<unknown>("GET", `/api/v1/cameras/${encodeURIComponent(id)}/timeline`);
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

  /** Requires capability `video:playback`, camera-keyed. */
  deletePlaybackSession(session_id: string): Promise<unknown> {
    return this.call<unknown>("DELETE", `/api/v1/playback/sessions/${encodeURIComponent(session_id)}`);
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

  /** Requires capability `system:read`, scope-neutral. */
  getTimezone(): Promise<TimezoneSettings> {
    return this.call<TimezoneSettings>("GET", `/api/v1/system/timezone`);
  }

  /** Requires admin, fleet-only. */
  setTimezone(body: TimezoneUpdate): Promise<TimezoneSettings> {
    return this.call<TimezoneSettings>("PUT", `/api/v1/system/timezone`, body);
  }

}
