// Typed fetch client for the VisionOps Core API.
//
// All paths are relative so they flow through the Vite dev proxy (-> :8000)
// in development and the same origin in production.

import type {
  CameraCreate,
  CameraStatus,
  CameraTestResult,
  CameraUpdate,
  CameraView,
  ClipResult,
  DiscoverOptions,
  DiscoverResponse,
  LiveUrls,
  SegmentView,
  SystemInfo,
  Timeline,
  VisionEvent,
} from "./types";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

function qs(params: object = {}): string {
  const sp = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") {
      sp.set(key, String(value));
    }
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (init?.body) headers["Content-Type"] = "application/json";

  const res = await fetch(path, {
    ...init,
    headers: { ...headers, ...(init?.headers as Record<string, string> | undefined) },
  });

  if (!res.ok) {
    let message = `HTTP ${res.status} ${res.statusText}`;
    try {
      const data = (await res.json()) as { error?: string; message?: string };
      message = data.error ?? data.message ?? message;
    } catch {
      /* non-JSON error body — keep the status line */
    }
    throw new ApiError(res.status, message);
  }

  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

const enc = encodeURIComponent;

export interface SegmentQuery {
  from?: string;
  to?: string;
  limit?: number;
}

export interface TimelineQuery {
  from?: string;
  to?: string;
}

export interface EventQuery {
  camera_id?: string;
  event_type?: string;
  severity?: string;
  limit?: number;
}

export const api = {
  // ---- Cameras ----
  listCameras: () => request<CameraView[]>("/api/v1/cameras"),
  getCamera: (id: string) => request<CameraView>(`/api/v1/cameras/${enc(id)}`),
  createCamera: (body: CameraCreate) =>
    request<CameraView>("/api/v1/cameras", { method: "POST", body: JSON.stringify(body) }),
  updateCamera: (id: string, body: CameraUpdate) =>
    request<CameraView>(`/api/v1/cameras/${enc(id)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteCamera: (id: string) =>
    request<void>(`/api/v1/cameras/${enc(id)}`, { method: "DELETE" }),
  testCamera: (id: string) =>
    request<CameraTestResult>(`/api/v1/cameras/${enc(id)}/test`, { method: "POST" }),

  // ---- Recordings ----
  listSegments: (id: string, q: SegmentQuery = {}) =>
    request<SegmentView[]>(`/api/v1/cameras/${enc(id)}/segments${qs(q)}`),
  getTimeline: (id: string, q: TimelineQuery = {}) =>
    request<Timeline>(`/api/v1/cameras/${enc(id)}/timeline${qs(q)}`),

  // ---- Playback ----
  exportClip: (id: string, from: string, to: string) =>
    request<ClipResult>(`/api/v1/cameras/${enc(id)}/clip`, {
      method: "POST",
      body: JSON.stringify({ from, to }),
    }),
  /** URL for a JPEG snapshot (live if `at` omitted). Use directly as an <img> src. */
  snapshotUrl: (id: string, at?: string) =>
    `/api/v1/cameras/${enc(id)}/snapshot${at ? qs({ at }) : ""}`,

  // ---- Live view ----
  liveview: (id: string) =>
    request<LiveUrls>(`/api/v1/cameras/${enc(id)}/liveview`, { method: "POST" }),

  // ---- Discovery ----
  discover: (opts: DiscoverOptions) =>
    request<DiscoverResponse>("/api/v1/discover", {
      method: "POST",
      body: JSON.stringify(opts),
    }),

  // ---- Health / system / events ----
  listHealth: () => request<CameraStatus[]>("/api/v1/health/cameras"),
  cameraHealth: (id: string) => request<CameraStatus>(`/api/v1/cameras/${enc(id)}/health`),
  listEvents: (q: EventQuery = {}) => request<VisionEvent[]>(`/api/v1/events${qs(q)}`),
  system: () => request<SystemInfo>("/api/v1/system"),
};
