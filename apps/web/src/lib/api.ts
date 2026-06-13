// Typed fetch client for the VisionOps Core API.
//
// All paths are relative so they flow through the Vite dev proxy (-> :8000)
// in development and the same origin in production.

import type {
  AiTask,
  AiTaskCreate,
  AiTaskUpdate,
  ApiKeyCreated,
  ApiKeyView,
  AuditLogEntry,
  CameraCreate,
  CameraStatus,
  CameraTestResult,
  CameraUpdate,
  CameraView,
  ClipResult,
  Detection,
  DiscoverOptions,
  DiscoverResponse,
  EntryEvent,
  EntryLogReport,
  ExceptionReport,
  Gaps,
  LiveUrls,
  LoginResult,
  Principal,
  SamplerInfo,
  SegmentView,
  StreamProfile,
  SystemInfo,
  Timeline,
  UserCreate,
  UserUpdate,
  UserView,
  Vehicle,
  VehicleCreate,
  VehicleUpdate,
  VisionEvent,
  VisitorPass,
  VisitorPassCreate,
  VisitorPassUpdate,
  WatchlistCreate,
  WatchlistEntry,
  WatchlistUpdate,
  WorkerTask,
  Zone,
  ZoneCreate,
  ZoneEvent,
  ZoneUpdate,
} from "./types";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

// ---- Bearer token (RBAC) -------------------------------------------------
// When auth is enabled, the login token is held here and persisted so a reload stays signed in.
const TOKEN_KEY = "visionops.token";
let authToken: string | null =
  typeof localStorage !== "undefined" ? localStorage.getItem(TOKEN_KEY) : null;

export function setAuthToken(token: string | null): void {
  authToken = token;
  if (typeof localStorage === "undefined") return;
  if (token) localStorage.setItem(TOKEN_KEY, token);
  else localStorage.removeItem(TOKEN_KEY);
}

export function getAuthToken(): string | null {
  return authToken;
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
  if (authToken) headers["Authorization"] = `Bearer ${authToken}`;

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

export interface DetectionQuery {
  from?: string;
  to?: string;
  label?: string;
  limit?: number;
}

export interface ZoneEventQuery {
  from?: string;
  to?: string;
  zone_id?: string;
  event_type?: string;
  limit?: number;
}

export interface EntryEventQuery {
  from?: string;
  to?: string;
  plate?: string;
  auth_status?: string;
  workflow_status?: string;
  event_type?: string;
  limit?: number;
}

export interface ReportQuery {
  date?: string;
  from?: string;
  to?: string;
  limit?: number;
}

export interface AuditQuery {
  from?: string;
  to?: string;
  actor?: string;
  action?: string;
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
  /** Holes in recording coverage over an optional [from,to] window. */
  cameraGaps: (id: string, from?: string, to?: string) =>
    request<Gaps>(`/api/v1/cameras/${enc(id)}/gaps${qs({ from, to })}`),

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

  // ---- AI (Stage 2) ----
  /** AI tasks configured on one camera. */
  listAiTasks: (cameraId: string) =>
    request<AiTask[]>(`/api/v1/cameras/${enc(cameraId)}/ai-tasks`),
  createAiTask: (cameraId: string, body: AiTaskCreate) =>
    request<AiTask>(`/api/v1/cameras/${enc(cameraId)}/ai-tasks`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  updateAiTask: (taskId: string, body: AiTaskUpdate) =>
    request<AiTask>(`/api/v1/ai-tasks/${enc(taskId)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteAiTask: (taskId: string) =>
    request<void>(`/api/v1/ai-tasks/${enc(taskId)}`, { method: "DELETE" }),
  /** Every enabled task across enabled cameras (worker discovery view). */
  aiTasks: () => request<WorkerTask[]>("/api/v1/ai/tasks"),
  /** Per-camera sampler status (state + effective fps). */
  samplers: () => request<SamplerInfo[]>("/api/v1/ai/samplers"),
  /** Detections for one camera, newest first. */
  cameraDetections: (id: string, opts: DetectionQuery = {}) =>
    request<Detection[]>(`/api/v1/cameras/${enc(id)}/detections${qs(opts)}`),
  /** URL for the latest AI-sampled JPEG frame. Use directly as an <img> src. */
  frameUrl: (id: string, profile?: StreamProfile) =>
    `/api/v1/cameras/${enc(id)}/frame${profile ? qs({ profile }) : ""}`,

  // ---- Zones (Stage 3) ----
  /** Zones configured on one camera, oldest first. */
  listZones: (cameraId: string) =>
    request<Zone[]>(`/api/v1/cameras/${enc(cameraId)}/zones`),
  createZone: (cameraId: string, body: ZoneCreate) =>
    request<Zone>(`/api/v1/cameras/${enc(cameraId)}/zones`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  updateZone: (zoneId: string, body: ZoneUpdate) =>
    request<Zone>(`/api/v1/zones/${enc(zoneId)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteZone: (zoneId: string) =>
    request<void>(`/api/v1/zones/${enc(zoneId)}`, { method: "DELETE" }),
  /** Zone enter/exit/dwell events for one camera, newest first. */
  cameraZoneEvents: (id: string, q: ZoneEventQuery = {}) =>
    request<ZoneEvent[]>(`/api/v1/cameras/${enc(id)}/zone-events${qs(q)}`),

  // ---- Auth + RBAC (Stage 4) ----
  login: (username: string, password: string) =>
    request<LoginResult>("/api/v1/auth/login", {
      method: "POST",
      body: JSON.stringify({ username, password }),
    }),
  logout: () => request<void>("/api/v1/auth/logout", { method: "POST" }),
  me: () => request<Principal>("/api/v1/auth/me"),
  listUsers: () => request<UserView[]>("/api/v1/users"),
  createUser: (body: UserCreate) =>
    request<UserView>("/api/v1/users", { method: "POST", body: JSON.stringify(body) }),
  updateUser: (id: string, body: UserUpdate) =>
    request<UserView>(`/api/v1/users/${enc(id)}`, { method: "PATCH", body: JSON.stringify(body) }),
  deleteUser: (id: string) => request<void>(`/api/v1/users/${enc(id)}`, { method: "DELETE" }),
  listApiKeys: () => request<ApiKeyView[]>("/api/v1/api-keys"),
  createApiKey: (name: string, role?: string) =>
    request<ApiKeyCreated>("/api/v1/api-keys", {
      method: "POST",
      body: JSON.stringify({ name, role }),
    }),
  deleteApiKey: (id: string) => request<void>(`/api/v1/api-keys/${enc(id)}`, { method: "DELETE" }),

  // ---- Campus Entry: registry (Stage 4) ----
  listVehicles: (q: { plate?: string; owner_type?: string; q?: string; limit?: number } = {}) =>
    request<Vehicle[]>(`/api/v1/vehicles${qs(q)}`),
  getVehicle: (id: string) => request<Vehicle>(`/api/v1/vehicles/${enc(id)}`),
  createVehicle: (body: VehicleCreate) =>
    request<Vehicle>("/api/v1/vehicles", { method: "POST", body: JSON.stringify(body) }),
  updateVehicle: (id: string, body: VehicleUpdate) =>
    request<Vehicle>(`/api/v1/vehicles/${enc(id)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteVehicle: (id: string) => request<void>(`/api/v1/vehicles/${enc(id)}`, { method: "DELETE" }),

  listPasses: (q: { status?: string; q?: string; limit?: number } = {}) =>
    request<VisitorPass[]>(`/api/v1/passes${qs(q)}`),
  getPass: (id: string) => request<VisitorPass>(`/api/v1/passes/${enc(id)}`),
  createPass: (body: VisitorPassCreate) =>
    request<VisitorPass>("/api/v1/passes", { method: "POST", body: JSON.stringify(body) }),
  updatePass: (id: string, body: VisitorPassUpdate) =>
    request<VisitorPass>(`/api/v1/passes/${enc(id)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deletePass: (id: string) => request<void>(`/api/v1/passes/${enc(id)}`, { method: "DELETE" }),
  checkinPass: (id: string) =>
    request<VisitorPass>(`/api/v1/passes/${enc(id)}/checkin`, { method: "POST" }),
  checkoutPass: (id: string) =>
    request<VisitorPass>(`/api/v1/passes/${enc(id)}/checkout`, { method: "POST" }),

  listWatchlist: () => request<WatchlistEntry[]>("/api/v1/watchlist"),
  createWatch: (body: WatchlistCreate) =>
    request<WatchlistEntry>("/api/v1/watchlist", { method: "POST", body: JSON.stringify(body) }),
  updateWatch: (id: string, body: WatchlistUpdate) =>
    request<WatchlistEntry>(`/api/v1/watchlist/${enc(id)}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteWatch: (id: string) => request<void>(`/api/v1/watchlist/${enc(id)}`, { method: "DELETE" }),

  // ---- Campus Entry: events + workflow + reports (Stage 4) ----
  listEntryEvents: (q: EntryEventQuery = {}) =>
    request<EntryEvent[]>(`/api/v1/entry-events${qs(q)}`),
  getEntryEvent: (id: string) => request<EntryEvent>(`/api/v1/entry-events/${enc(id)}`),
  confirmEntryEvent: (id: string, note?: string) =>
    request<EntryEvent>(`/api/v1/entry-events/${enc(id)}/confirm`, {
      method: "POST",
      body: JSON.stringify({ note }),
    }),
  rejectEntryEvent: (id: string, note?: string) =>
    request<EntryEvent>(`/api/v1/entry-events/${enc(id)}/reject`, {
      method: "POST",
      body: JSON.stringify({ note }),
    }),
  reportEntryLog: (q: ReportQuery = {}) =>
    request<EntryLogReport>(`/api/v1/reports/entry-log${qs(q)}`),
  reportExceptions: (q: ReportQuery = {}) =>
    request<ExceptionReport>(`/api/v1/reports/exceptions${qs(q)}`),
  listAudit: (q: AuditQuery = {}) => request<AuditLogEntry[]>(`/api/v1/audit${qs(q)}`),
};
