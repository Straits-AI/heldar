// Typed fetch client for the Heldar Core API.
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
  BreachAlert,
  CameraCreate,
  CameraLink,
  MovementCandidate,
  PlateSearchResult,
  QueryPlan,
  SearchPlanResponse,
  SearchResponse,
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

// ---- Session auth (RBAC) -------------------------------------------------
// The login session lives in an HttpOnly `heldar_session` cookie set by the server (see auth.rs), sent
// automatically on every request via `credentials: "include"` — including the media plane
// (<img>/<video>/HLS), since the SPA is same-origin with the API. The cookie is NOT readable by JS,
// so it cannot be exfiltrated by XSS, and it survives reloads / new tabs. We deliberately do NOT
// persist the token in localStorage. We keep it in memory only for the current tab as an
// Authorization-header fallback; bootstrap/reload relies on the cookie + GET /auth/me.
let authToken: string | null = null;

export function setAuthToken(token: string | null): void {
  authToken = token;
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

const REQUEST_TIMEOUT_MS = 30000;

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (init?.body) headers["Content-Type"] = "application/json";
  if (authToken) headers["Authorization"] = `Bearer ${authToken}`;

  // Always bound a request with a timeout so a slow/hung Core can't leave the UI spinning forever;
  // merge in the caller's signal (if any) so a component can also cancel on unmount / re-nav.
  const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS);
  const signal = init?.signal ? AbortSignal.any([init.signal, timeout]) : timeout;

  let res: Response;
  try {
    res = await fetch(path, {
      ...init,
      signal,
      credentials: "include", // send the HttpOnly session cookie (auth-enabled deployments)
      headers: { ...headers, ...(init?.headers as Record<string, string> | undefined) },
    });
  } catch (e) {
    // Caller-initiated cancellation (unmount/re-nav): re-throw so the caller's cleanup ignores it.
    if (init?.signal?.aborted) throw e;
    // Timeout or network failure → a clean ApiError the UI can surface instead of hanging.
    throw new ApiError(0, "Network error or request timed out");
  }

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

  // ---- Movement intelligence (Stage 6) ----
  movementLinks: () => request<CameraLink[]>("/api/v1/movement/links"),
  createMovementLink: (body: {
    from_camera: string;
    to_camera: string;
    transit_seconds?: number;
    bidirectional?: boolean;
    note?: string;
  }) => request<CameraLink>("/api/v1/movement/links", { method: "POST", body: JSON.stringify(body) }),
  deleteMovementLink: (id: string) =>
    request<void>(`/api/v1/movement/links/${enc(id)}`, { method: "DELETE" }),
  movementCandidates: (q: { status?: string; anchor?: string; limit?: number } = {}) =>
    request<MovementCandidate[]>(`/api/v1/movement/candidates${qs(q)}`),
  confirmMovementCandidate: (id: string) =>
    request<MovementCandidate>(`/api/v1/movement/candidates/${enc(id)}/confirm`, { method: "POST" }),
  rejectMovementCandidate: (id: string) =>
    request<MovementCandidate>(`/api/v1/movement/candidates/${enc(id)}/reject`, { method: "POST" }),
  movementBreaches: (q: { status?: string; limit?: number } = {}) =>
    request<BreachAlert[]>(`/api/v1/movement/breaches${qs(q)}`),
  ackBreach: (id: string) =>
    request<BreachAlert>(`/api/v1/movement/breaches/${enc(id)}/ack`, { method: "POST" }),
  resolveBreach: (id: string) =>
    request<BreachAlert>(`/api/v1/movement/breaches/${enc(id)}/resolve`, { method: "POST" }),
  searchPlate: (plate: string) =>
    request<PlateSearchResult>(`/api/v1/movement/search/plate/${enc(plate)}`),
  triggerMovement: () => request<{ ok: boolean }>("/api/v1/movement/run", { method: "POST" }),

  // ---- Semantic search (Stage 7) ----
  searchEvents: (plan: QueryPlan) =>
    request<SearchResponse>("/api/v1/search/events", { method: "POST", body: JSON.stringify(plan) }),
  searchNl: (query: string) =>
    request<SearchResponse>("/api/v1/search/nl", { method: "POST", body: JSON.stringify({ query }) }),
  searchPlan: (query: string) =>
    request<SearchPlanResponse>("/api/v1/search/plan", {
      method: "POST",
      body: JSON.stringify({ query }),
    }),
};
