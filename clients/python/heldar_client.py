# GENERATED FROM openapi.json BY scripts/gen_clients.py — DO NOT EDIT.
#
# Regenerate with:  cargo test -p heldar-server --test openapi_contract write_the_served_document
#                   python3 scripts/gen_clients.py target/openapi.json clients
#
# Contract version: 0.1.0

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

# The dataclasses below DESCRIBE the wire shapes; the client returns parsed JSON, not
# instances of them. Saying so is more useful than implying a deserialization step that
# does not happen — a caller can construct one to type a payload, or ignore them entirely.


class HeldarError(Exception):
    """Every endpoint returns the same shape, so a caller writes one error path."""

    def __init__(self, message: str, code: str, retryable: bool, status: int) -> None:
        super().__init__(message)
        self.code = code
        self.retryable = retryable
        self.status = status


@dataclass
class AiIngest:
    camera_id: str
    task_type: str
    detections: list[DetectionIngest] = None
    event: IngestEvent | None = None
    frame_id: str | None = None
    frame_ticket: str | None = None
    timestamp: str | None = None


@dataclass
class AiTaskCreate:
    task_type: str
    config: object = None
    enabled: bool | None = None
    fps: float | None = None
    stream_profile: str | None = None
    width: int | None = None


@dataclass
class AiTaskUpdate:
    config: object = None
    enabled: bool | None = None
    fps: float | None = None
    stream_profile: str | None = None
    task_type: str | None = None
    width: int | None = None


@dataclass
class ApiKeyCreate:
    name: str
    capabilities: list[str] | None = None
    confirm_privileged: bool = None
    expires_at: str | None = None
    role: str | None = None
    scope_cameras: list[str] | None = None
    scope_kind: str | None = None


@dataclass
class ApiKeyUpdate:
    active: bool | None = None
    capabilities: list[str] | None = None
    confirm_privileged: bool = None
    expires_at: str | None = None
    revoked_at: str | None = None
    scope_cameras: list[str] | None = None
    scope_kind: str | None = None


@dataclass
class ArchiveExportRequest:
    camera_ids: list[str] = None
    from_: str | None = None  # wire name: "from"
    incident_lock_only: bool | None = None
    to: str | None = None
    trim: bool | None = None


@dataclass
class BackupDestinationCreate:
    kind: str
    name: str
    config: object = None
    enabled: bool | None = None


@dataclass
class BackupDestinationUpdate:
    config: object = None
    enabled: bool | None = None
    kind: str | None = None
    name: str | None = None


@dataclass
class BackupDestinationView:
    config: object
    created_at: str
    enabled: bool
    has_credentials: bool
    id: str
    kind: str
    name: str
    updated_at: str


@dataclass
class BackupJob:
    bytes_copied: int
    camera_ids: list[str]
    created_at: str
    files_copied: int
    files_total: int
    id: str
    incident_lock_only: bool
    kind: str
    status: str
    created_by: str | None = None
    created_by_kind: str | None = None
    destination_id: str | None = None
    error: str | None = None
    finished_at: str | None = None
    from_time: str | None = None
    output_path: str | None = None
    output_url: str | None = None
    policy_id: str | None = None
    started_at: str | None = None
    to_time: str | None = None


@dataclass
class BackupPolicy:
    camera_ids: list[str]
    created_at: str
    destination_id: str
    enabled: bool
    id: str
    incident_lock_only: bool
    lookback_hours: int
    name: str
    schedule_interval_s: int
    updated_at: str
    last_job_id: str | None = None
    last_run_at: str | None = None


@dataclass
class BackupPolicyCreate:
    destination_id: str
    name: str
    camera_ids: object = None
    enabled: bool | None = None
    incident_lock_only: bool | None = None
    lookback_hours: int | None = None
    schedule_interval_s: int | None = None


@dataclass
class BackupPolicyUpdate:
    camera_ids: object = None
    destination_id: str | None = None
    enabled: bool | None = None
    incident_lock_only: bool | None = None
    lookback_hours: int | None = None
    name: str | None = None
    schedule_interval_s: int | None = None


@dataclass
class BackupTestResult:
    latency_ms: int
    ok: bool
    error: str | None = None


@dataclass
class BulkAction:
    pass


@dataclass
class BulkCameraResult:
    camera_id: str
    ok: bool
    error: str | None = None


@dataclass
class BulkConfigRequest:
    action: BulkAction
    camera_ids: list[str] | None = None


@dataclass
class BulkConfigResponse:
    failed: int
    results: list[BulkCameraResult]
    succeeded: int


@dataclass
class CameraLinkCreate:
    from_camera: str
    to_camera: str
    bidirectional: bool | None = None
    note: str | None = None
    transit_seconds: int | None = None


@dataclass
class CameraView:
    anr_enabled: bool
    capabilities: object
    created_at: str
    enabled: bool
    has_password: bool
    id: str
    live_warm: bool
    mirror_enabled: bool
    name: str
    native_anpr_enabled: bool
    native_events_enabled: bool
    post_roll_seconds: int
    pre_roll_seconds: int
    priority: int
    record_audio: bool
    record_enabled: bool
    record_mode: str
    record_stream: str
    retention_hours: int
    rtsp_port: int
    segment_seconds: int
    updated_at: str
    vendor: str
    address: str | None = None
    anr_replay_url_template: str | None = None
    codec: str | None = None
    fps_main: int | None = None
    fps_sub: int | None = None
    model: str | None = None
    record_url_masked: str | None = None
    resolution_main: str | None = None
    resolution_sub: str | None = None
    site_id: str | None = None
    storage_quota_bytes: int | None = None
    username: str | None = None


@dataclass
class ClipRequest:
    from_: str  # wire name: "from"
    to: str


@dataclass
class ContinuousMoveRequest:
    pan: float = None
    tilt: float = None
    zoom: float = None


@dataclass
class CreateSessionRequest:
    from_: str  # wire name: "from"
    to: str


@dataclass
class Credential:
    password: str
    username: str


@dataclass
class DayNightConfig:
    mode: str
    sensitivity: int | None = None


@dataclass
class DayNightPatch:
    mode: str | None = None
    sensitivity: int | None = None


@dataclass
class DbConvertResult:
    status: str


@dataclass
class DbLimitUpdate:
    max_db_gb: float | None = None


@dataclass
class DbStatus:
    db_bytes: int
    incremental: bool
    max_db_bytes: int
    max_db_gb: float
    max_overridden: bool


@dataclass
class DetectionIngest:
    attributes: object = None
    bbox: object = None
    confidence: float | None = None
    label: str | None = None
    track_id: str | None = None


@dataclass
class DetectionUpdate:
    enabled: bool


@dataclass
class DeviceInfo:
    device_name: str | None = None
    firmware_version: str | None = None
    model: str | None = None
    serial_number: str | None = None


@dataclass
class DiscoverOptions:
    targets: str
    auto_add: bool = None
    connect_timeout_ms: int | None = None
    credentials: list[Credential] | None = None
    password: str | None = None
    rtsp_port: int | None = None
    try_default_creds: bool = None
    username: str | None = None
    verify: bool = None


@dataclass
class EmbeddingIngest:
    camera_id: str
    dim: int
    items: list[EmbeddingItem]
    model: str
    frame_id: str | None = None
    frame_ticket: str | None = None


@dataclass
class EmbeddingItem:
    vec: list[float]
    bbox: object = None
    detection_id: str | None = None
    label: str | None = None
    thumb_b64: str | None = None
    timestamp: str | None = None
    track_id: str | None = None


@dataclass
class EnsureOnvifUserRequest:
    password: str
    user_type: OnvifUserType | None = None
    username: str = None


@dataclass
class ErrorBody:
    code: str
    error: str
    retryable: bool


@dataclass
class EvidenceLockBody:
    incident_id: str | None = None


@dataclass
class ExportRequest:
    from_: str  # wire name: "from"
    to: str
    camera_id: str | None = None
    dry_run: bool = None
    incident_id: str | None = None


@dataclass
class GatePolicy:
    camera_id: str
    enabled: bool
    output_port: int
    pulse_ms: int
    updated_at: str


@dataclass
class GatePolicyUpdate:
    enabled: bool | None = None
    output_port: int | None = None
    pulse_ms: int | None = None


@dataclass
class GateSettingsUpdate:
    kill_switch: bool


@dataclass
class GotoPresetRequest:
    token: str


@dataclass
class ImageConfig:
    blc_enabled: bool | None = None
    brightness: int | None = None
    contrast: int | None = None
    ir_light_brightness: int | None = None
    saturation: int | None = None
    supplement_brightness_mode: str | None = None
    supplement_light_mode: str | None = None
    wdr_level: int | None = None
    wdr_mode: str | None = None
    white_light_brightness: int | None = None


@dataclass
class IncidentSummary:
    incident_id: str
    newest_end: str
    oldest_start: str
    segment_count: int
    total_bytes: int


@dataclass
class IncidentTagBody:
    incident_id: str | None = None


@dataclass
class IngestEvent:
    event_type: str
    payload: object = None
    severity: str | None = None


@dataclass
class IntrusionConfig:
    enabled: bool
    regions: list[SmartRegion]


@dataclass
class IoOutput:
    id: int
    default_state: str | None = None
    name: str | None = None


@dataclass
class LeaseRequest:
    worker_id: str
    max_tasks: int | None = None
    task_types: list[str] | None = None
    ttl_secs: int | None = None


@dataclass
class LineCrossingConfig:
    enabled: bool
    lines: list[SmartLine]


@dataclass
class LoginRequest:
    password: str
    username: str


@dataclass
class ModuleRegisterRequest:
    base_url: str
    id: str
    name: str
    description: str = None
    nav: list[NavEntry] = None
    publisher: str = None
    role: str | None = None
    subscribes: list[str] | None = None
    version: str = None


@dataclass
class MotionConfig:
    enabled: bool
    sensitivity: int | None = None


@dataclass
class NavEntry:
    icon: str
    label: str
    path: str


@dataclass
class NlBody:
    query: str


@dataclass
class NtpConfig:
    addressing_format: str
    host_name: str
    port: int


@dataclass
class OnvifSettings:
    isapi_enabled: bool
    onvif_enabled: bool


OnvifUserType = str  # one of: administrator, operator, mediaUser

@dataclass
class OsdConfig:
    channel_name_enabled: bool
    datetime_enabled: bool
    date_style: str | None = None
    display_week: bool | None = None
    time_style: str | None = None


@dataclass
class ProbeRequest:
    device_url: str | None = None


@dataclass
class PulseRequest:
    pulse_ms: int = None


@dataclass
class QueryPlan:
    auth_status: list[str] = None
    cameras: list[str] = None
    color: str | None = None
    event_type: str | None = None
    from_: str | None = None  # wire name: "from"
    hour_max: int | None = None
    hour_min: int | None = None
    limit: int | None = None
    plate: str | None = None
    sources: list[str] = None
    subject_type: str | None = None
    text: str | None = None
    to: str | None = None
    tz: str | None = None
    vehicle_type: str | None = None
    zone: str | None = None
    zone_kind: str | None = None


@dataclass
class QueryResult:
    dim: int | None = None
    error: str | None = None
    model: str | None = None
    vec: list[float] | None = None


@dataclass
class RebootRequest:
    confirm: bool


@dataclass
class RecordScheduleCreate:
    days: object
    time_end: str
    time_start: str
    enabled: bool | None = None


@dataclass
class RecordScheduleUpdate:
    days: object = None
    enabled: bool | None = None
    time_end: str | None = None
    time_start: str | None = None


@dataclass
class ResolveBody:
    note: str | None = None


@dataclass
class RetentionLimits:
    max_overridden: bool
    max_recordings_bytes: int
    max_recordings_gb: float
    min_free_disk_bytes: int
    min_free_disk_gb: float
    min_free_overridden: bool


@dataclass
class RetentionUpdate:
    max_recordings_gb: float | None = None
    min_free_disk_gb: float | None = None


@dataclass
class SemanticBody:
    cameras: list[str] = None
    from_: str | None = None  # wire name: "from"
    image_b64: str | None = None
    k: int | None = None
    label: str | None = None
    text: str | None = None
    to: str | None = None
    zone: str | None = None


@dataclass
class SiteCreate:
    id: str
    name: str
    timezone: str | None = None


@dataclass
class SiteRow:
    created_at: str
    id: str
    name: str
    timezone: str | None = None


@dataclass
class SiteUpdate:
    name: str | None = None
    timezone: str | None = None


@dataclass
class SmartLine:
    direction: str
    enabled: bool
    id: int
    points: list[list[float]]
    sensitivity: int


@dataclass
class SmartRegion:
    enabled: bool
    id: int
    points: list[list[float]]
    sensitivity: int
    time_threshold: int


@dataclass
class SnapshotSchedule:
    camera_id: str
    created_at: str
    enabled: bool
    id: str
    interval_seconds: int
    updated_at: str
    last_fired_at: str | None = None


@dataclass
class SnapshotScheduleCreate:
    enabled: bool | None = None
    interval_seconds: int | None = None


@dataclass
class SnapshotScheduleUpdate:
    enabled: bool | None = None
    interval_seconds: int | None = None


@dataclass
class TimeConfig:
    local_time: str
    time_mode: str
    time_zone: str


@dataclass
class TimezoneSettings:
    server_local_offset: str
    source: TzSource
    unconfigured_behaviour: str
    configured: str | None = None


@dataclass
class TimezoneUpdate:
    timezone: str


@dataclass
class TranscodeSettings:
    engine: str
    env_default: str
    nvenc_available: bool
    overridden: bool
    vaapi_available: bool


@dataclass
class TranscodeUpdate:
    engine: str


TzSource = str  # one of: site, default, unset

@dataclass
class UserCreate:
    password: str
    username: str
    active: bool | None = None
    display_name: str | None = None
    role: str | None = None


@dataclass
class UserUpdate:
    active: bool | None = None
    display_name: str | None = None
    password: str | None = None
    role: str | None = None


@dataclass
class UserView:
    active: bool
    created_at: str
    id: str
    role: str
    updated_at: str
    username: str
    display_name: str | None = None


@dataclass
class Vehicle:
    active: bool
    created_at: str
    id: str
    owner_type: str
    plate: str
    plate_norm: str
    updated_at: str
    color: str | None = None
    make: str | None = None
    model: str | None = None
    notes: str | None = None
    owner_name: str | None = None
    owner_ref: str | None = None
    site_id: str | None = None
    valid_from: str | None = None
    valid_until: str | None = None
    vehicle_type: str | None = None


@dataclass
class VehicleCreate:
    plate: str
    active: bool | None = None
    color: str | None = None
    make: str | None = None
    model: str | None = None
    notes: str | None = None
    owner_name: str | None = None
    owner_ref: str | None = None
    owner_type: str | None = None
    site_id: str | None = None
    valid_from: str | None = None
    valid_until: str | None = None
    vehicle_type: str | None = None


@dataclass
class VehicleUpdate:
    active: bool | None = None
    color: str | None = None
    make: str | None = None
    model: str | None = None
    notes: str | None = None
    owner_name: str | None = None
    owner_ref: str | None = None
    owner_type: str | None = None
    plate: str | None = None
    site_id: str | None = None
    valid_from: str | None = None
    valid_until: str | None = None
    vehicle_type: str | None = None


@dataclass
class VideoConfig:
    bitrate: int
    channel_id: int
    codec: str
    fps: int
    gop: int
    height: int
    quality_control: str
    vbr_upper_cap: int
    width: int
    channel_name: str | None = None


@dataclass
class VideoConfigPatch:
    bitrate: int | None = None
    codec: str | None = None
    fps: int | None = None
    gop: int | None = None
    height: int | None = None
    quality_control: str | None = None
    vbr_upper_cap: int | None = None
    width: int | None = None


@dataclass
class VisitorPass:
    code: str
    created_at: str
    id: str
    status: str
    updated_at: str
    valid_from: str
    valid_until: str
    visitor_name: str
    checked_in_at: str | None = None
    checked_out_at: str | None = None
    company: str | None = None
    created_by: str | None = None
    host: str | None = None
    phone: str | None = None
    plate: str | None = None
    plate_norm: str | None = None
    purpose: str | None = None
    site_id: str | None = None
    vehicle_desc: str | None = None


@dataclass
class VisitorPassCreate:
    visitor_name: str
    company: str | None = None
    host: str | None = None
    phone: str | None = None
    plate: str | None = None
    purpose: str | None = None
    site_id: str | None = None
    valid_from: str | None = None
    valid_until: str | None = None
    vehicle_desc: str | None = None


@dataclass
class VisitorPassUpdate:
    company: str | None = None
    host: str | None = None
    phone: str | None = None
    plate: str | None = None
    purpose: str | None = None
    status: str | None = None
    valid_from: str | None = None
    valid_until: str | None = None
    vehicle_desc: str | None = None
    visitor_name: str | None = None


@dataclass
class Watchlist:
    active: bool
    created_at: str
    id: str
    kind: str
    plate: str
    plate_norm: str
    severity: str
    updated_at: str
    created_by: str | None = None
    reason: str | None = None


@dataclass
class WatchlistCreate:
    plate: str
    active: bool | None = None
    kind: str | None = None
    reason: str | None = None
    severity: str | None = None


@dataclass
class WatchlistUpdate:
    active: bool | None = None
    kind: str | None = None
    reason: str | None = None
    severity: str | None = None


@dataclass
class WebhookDelivery:
    attempts: int
    created_at: str
    id: str
    status: str
    subscription_id: str
    delivered_at: str | None = None
    error: str | None = None
    event_id: str | None = None
    event_type: str | None = None
    response_code: int | None = None


@dataclass
class WebhookSubscriptionCreate:
    name: str
    url: str
    enabled: bool | None = None
    event_types: list[str] | None = None
    min_severity: str | None = None
    secret: str | None = None


@dataclass
class WebhookSubscriptionUpdate:
    enabled: bool | None = None
    event_types: list[str] | None = None
    min_severity: str | None = None
    name: str | None = None
    secret: str | None = None
    url: str | None = None


@dataclass
class WebhookSubscriptionView:
    created_at: str
    enabled: bool
    event_types: list[str]
    has_secret: bool
    id: str
    min_severity: str
    name: str
    updated_at: str
    url: str
    cursor_at: str | None = None


@dataclass
class WebhookTestResult:
    ok: bool
    error: str | None = None
    status: int | None = None


@dataclass
class ZoneCreate:
    name: str
    polygon: object
    config: object = None
    dwell_seconds: float | None = None
    enabled: bool | None = None
    kind: str | None = None
    labels: object = None
    severity: str | None = None


@dataclass
class ZoneUpdate:
    config: object = None
    dwell_seconds: float | None = None
    enabled: bool | None = None
    kind: str | None = None
    labels: object = None
    name: str | None = None
    polygon: object = None
    severity: str | None = None


class HeldarClient:
    def __init__(self, base_url: str = '', token: str | None = None) -> None:
        self.base_url = base_url.rstrip('/')
        self.token = token

    def _call(self, method: str, path: str, body: Any = None) -> Any:
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base_url + path, data=data, method=method)
        req.add_header('content-type', 'application/json')
        if self.token:
            req.add_header('Authorization', f'Bearer {self.token}')
        try:
            with urllib.request.urlopen(req) as r:
                return json.loads(r.read() or b'null')
        except urllib.error.HTTPError as e:
            try:
                err = json.loads(e.read() or b'{}')
            except Exception:
                err = {}
            raise HeldarError(
                err.get('error', str(e)),
                err.get('code', 'internal'),
                bool(err.get('retryable', False)),
                e.code,
            ) from None

    def delete_ai_task(self, task_id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/ai-tasks/{task_id}')

    def update_ai_task(self, task_id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PATCH', f'/api/v1/ai-tasks/{task_id}', body)

    def claim_embed_queries(self) -> Any:
        """Requires capability `ai:embedwork`, scope-neutral."""
        return self._call('GET', f'/api/v1/ai/embed-queries')

    def submit_embed_query_result(self, id: str, body: Any) -> Any:
        """Requires capability `ai:embedwork`, scope-neutral."""
        return self._call('POST', f'/api/v1/ai/embed-queries/{id}/result', body)

    def ingest_ai_embeddings(self, body: Any) -> Any:
        """Requires capability `ai:ingest`, camera-keyed."""
        return self._call('POST', f'/api/v1/ai/embeddings', body)

    def ingest_ai_events(self, body: Any) -> Any:
        """Requires capability `ai:ingest`, camera-keyed."""
        return self._call('POST', f'/api/v1/ai/events', body)

    def acquire_ai_lease(self, body: Any) -> Any:
        """Requires capability `ai:tasks`, scope-filtered."""
        return self._call('POST', f'/api/v1/ai/leases', body)

    def release_ai_lease(self, lease_id: str) -> Any:
        """Requires capability `ai:tasks`, scope-neutral."""
        return self._call('DELETE', f'/api/v1/ai/leases/{lease_id}')

    def list_ai_samplers(self) -> Any:
        """Requires capability `ai:tasks`, scope-filtered."""
        return self._call('GET', f'/api/v1/ai/samplers')

    def discover_ai_tasks(self) -> Any:
        """Requires capability `ai:tasks`, scope-filtered."""
        return self._call('GET', f'/api/v1/ai/tasks')

    def list_api_keys(self) -> Any:
        """Requires admin, fleet-only."""
        return self._call('GET', f'/api/v1/api-keys')

    def create_api_key(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/api-keys', body)

    def delete_api_key(self, id: str) -> Any:
        """Requires admin, fleet-only."""
        return self._call('DELETE', f'/api/v1/api-keys/{id}')

    def update_api_key(self, id: str, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PATCH', f'/api/v1/api-keys/{id}', body)

    def create_archive_export(self, body: Any) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('POST', f'/api/v1/archive/export', body)

    def list_archive_exports(self) -> Any:
        """Requires capability `system:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/archive/exports')

    def list_audit_log(self) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('GET', f'/api/v1/audit')

    def login(self, body: Any) -> Any:
        """Requires scope-neutral."""
        return self._call('POST', f'/api/v1/auth/login', body)

    def logout(self) -> Any:
        """Requires scope-neutral."""
        return self._call('POST', f'/api/v1/auth/logout')

    def get_current_principal(self) -> Any:
        """Requires scope-neutral."""
        return self._call('GET', f'/api/v1/auth/me')

    def list_backup_destinations(self) -> Any:
        """Requires capability `system:read`, fleet-only."""
        return self._call('GET', f'/api/v1/backup/destinations')

    def create_backup_destination(self, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/backup/destinations', body)

    def delete_backup_destination(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('DELETE', f'/api/v1/backup/destinations/{id}')

    def update_backup_destination(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('PATCH', f'/api/v1/backup/destinations/{id}', body)

    def test_backup_destination(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/backup/destinations/{id}/test')

    def list_backup_jobs(self) -> Any:
        """Requires capability `system:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/backup/jobs')

    def delete_backup_job(self, id: str) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('DELETE', f'/api/v1/backup/jobs/{id}')

    def get_backup_job(self, id: str) -> Any:
        """Requires capability `system:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/backup/jobs/{id}')

    def list_backup_policies(self) -> Any:
        """Requires capability `system:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/backup/policies')

    def create_backup_policy(self, body: Any) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('POST', f'/api/v1/backup/policies', body)

    def delete_backup_policy(self, id: str) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('DELETE', f'/api/v1/backup/policies/{id}')

    def update_backup_policy(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('PATCH', f'/api/v1/backup/policies/{id}', body)

    def trigger_backup_policy(self, id: str) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('POST', f'/api/v1/backup/policies/{id}/trigger')

    def list_cameras(self) -> Any:
        """Requires capability `camera:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/cameras')

    def bulk_camera_config(self, body: Any) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('POST', f'/api/v1/cameras/config/bulk', body)

    def delete_camera(self, id: str) -> Any:
        """Requires capability `admin`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/cameras/{id}')

    def get_camera(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}')

    def list_camera_ai_tasks(self, id: str) -> Any:
        """Requires capability `ai:tasks`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/ai-tasks')

    def create_ai_task(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/ai-tasks', body)

    def export_clip(self, id: str, body: Any) -> Any:
        """Requires capability `video:export`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/clip', body)

    def get_camera_device_info(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/device_info')

    def get_camera_onvif_settings(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/onvif')

    def put_camera_onvif_settings(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/config/onvif', body)

    def ensure_camera_onvif_user(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/config/onvif/ensure_user', body)

    def get_camera_osd(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/osd')

    def put_camera_osd(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/config/osd', body)

    def reboot_camera(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/config/reboot', body)

    def get_camera_time(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/time')

    def put_camera_time(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/config/time', body)

    def get_camera_ntp(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/time/ntp')

    def put_camera_ntp(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/config/time/ntp', body)

    def sync_camera_time_now(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/config/time/sync_now')

    def list_camera_video_configs(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/video')

    def get_camera_video_config(self, id: str, channel: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/config/video/{channel}')

    def put_camera_video_config(self, id: str, channel: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/config/video/{channel}', body)

    def get_camera_control_capabilities(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/capabilities')

    def get_camera_day_night(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/day_night')

    def set_camera_day_night(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/control/day_night', body)

    def set_camera_builtin_detection(self, id: str, kind: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/control/detections/{kind}', body)

    def get_camera_image(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/image')

    def set_camera_image(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/control/image', body)

    def get_camera_intrusion(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/intrusion')

    def set_camera_intrusion(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/control/intrusion', body)

    def list_camera_io_outputs(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/io/outputs')

    def pulse_camera_io_output(self, id: str, port: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/control/io/outputs/{port}/pulse', body)

    def get_camera_line_crossing(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/line_crossing')

    def set_camera_line_crossing(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/control/line_crossing', body)

    def get_camera_motion(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/control/motion')

    def set_camera_motion(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/cameras/{id}/control/motion', body)

    def probe_camera_control(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/control/probe')

    def list_detections(self, id: str) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/detections')

    def get_latest_frame(self, id: str) -> Any:
        """Requires capability `ai:frames`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/frame')

    def list_gaps(self, id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/gaps')

    def get_camera_health(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/health')

    def get_live_view(self, id: str) -> Any:
        """Requires capability `video:live`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/liveview')

    def get_camera_onvif(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/onvif')

    def probe_camera_onvif(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/onvif/probe', body)

    def create_playback_session(self, id: str, body: Any) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/playback/sessions', body)

    def ptz_continuous_move(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/ptz/continuous', body)

    def ptz_goto_preset(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/ptz/goto_preset', body)

    def list_ptz_presets(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/ptz/presets')

    def refresh_ptz_presets(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/ptz/presets/refresh')

    def ptz_stop(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/ptz/stop')

    def trigger_recording(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/record-trigger')

    def list_recording_gaps(self, id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/recording-gaps')

    def retry_recording_gap(self, id: str, gap_id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/recording-gaps/{gap_id}/retry')

    def list_recording_schedules(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/schedules')

    def create_recording_schedule(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/schedules', body)

    def list_segments(self, id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/segments')

    def get_snapshot(self, id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/snapshot')

    def list_snapshot_schedules(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/snapshot-schedules')

    def create_snapshot_schedule(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/snapshot-schedules', body)

    def list_camera_snapshots(self, id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/snapshots')

    def test_camera(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/test')

    def get_timeline(self, id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/timeline')

    def list_zone_events(self, id: str) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/zone-events')

    def get_zone_event_aggregates(self, id: str) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/zone-events/aggregates')

    def list_zones(self, id: str) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/zones')

    def create_zone(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/cameras/{id}/zones', body)

    def get_zone_occupancy(self, id: str) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}/zones/occupancy')

    def discover_cameras(self, body: Any) -> Any:
        """Requires capability `net:scan`, fleet-only."""
        return self._call('POST', f'/api/v1/discover', body)

    def list_entry_events(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/entry-events')

    def get_entry_event(self, id: str) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/entry-events/{id}')

    def confirm_entry_event(self, id: str, body: Any) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/entry-events/{id}/confirm', body)

    def reject_entry_event(self, id: str, body: Any) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/entry-events/{id}/reject', body)

    def get_gate_state(self) -> Any:
        """Requires capability `identity:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/entry/gate')

    def open_gate(self, camera_id: str) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/entry/gate/open/{camera_id}')

    def delete_gate_policy(self, camera_id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/entry/gate/policies/{camera_id}')

    def put_gate_policy(self, camera_id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PUT', f'/api/v1/entry/gate/policies/{camera_id}', body)

    def update_gate_settings(self, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('PUT', f'/api/v1/entry/gate/settings', body)

    def list_events(self) -> Any:
        """Requires capability `events:read`, fleet-only."""
        return self._call('GET', f'/api/v1/events')

    def list_event_types(self) -> Any:
        """Requires capability `events:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/events/types')

    def list_evidence_exports(self) -> Any:
        """Requires capability `video:export`, scope-filtered."""
        return self._call('GET', f'/api/v1/evidence/exports')

    def create_evidence_export(self, body: Any) -> Any:
        """Requires capability `video:export`, camera-keyed."""
        return self._call('POST', f'/api/v1/evidence/exports', body)

    def get_evidence_export(self, id: str) -> Any:
        """Requires capability `video:export`, camera-keyed."""
        return self._call('GET', f'/api/v1/evidence/exports/{id}')

    def get_evidence_signing_key(self) -> Any:
        """Requires capability `camera:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/evidence/signing-key')

    def list_camera_health(self) -> Any:
        """Requires capability `camera:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/health/cameras')

    def list_incidents(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/incidents')

    def list_incident_segments(self, incident_id: str) -> Any:
        """Requires capability `video:playback`, scope-filtered."""
        return self._call('GET', f'/api/v1/incidents/{incident_id}/segments')

    def list_modules(self) -> Any:
        """Requires capability `system:read`, fleet-only."""
        return self._call('GET', f'/api/v1/modules')

    def register_module(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/modules', body)

    def get_entry_module_ui(self) -> Any:
        """Requires capability `events:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/modules/entry/ui/index.js')

    def get_movement_module_ui(self) -> Any:
        """Requires capability `events:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/modules/movement/ui/index.js')

    def get_search_module_ui(self) -> Any:
        """Requires capability `events:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/modules/search/ui/index.js')

    def unregister_module(self, id: str) -> Any:
        """Requires admin, camera-keyed."""
        return self._call('DELETE', f'/api/v1/modules/{id}')

    def get_module(self, id: str) -> Any:
        """Requires admin, camera-keyed."""
        return self._call('GET', f'/api/v1/modules/{id}')

    def list_movement_breaches(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/movement/breaches')

    def ack_movement_breach(self, id: str) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/movement/breaches/{id}/ack')

    def resolve_movement_breach(self, id: str) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/movement/breaches/{id}/resolve')

    def list_movement_candidates(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/movement/candidates')

    def confirm_movement_candidate(self, id: str) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/movement/candidates/{id}/confirm')

    def reject_movement_candidate(self, id: str) -> Any:
        """Requires capability `gate:operate`, camera-keyed."""
        return self._call('POST', f'/api/v1/movement/candidates/{id}/reject')

    def list_movement_links(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/movement/links')

    def create_movement_link(self, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/movement/links', body)

    def delete_movement_link(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/movement/links/{id}')

    def run_movement_engines(self) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/movement/run')

    def search_movement_by_person_track(self) -> Any:
        """Requires capability `events:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/movement/search/person')

    def search_movement_by_plate(self, plate: str) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/movement/search/plate/{plate}')

    def discover_onvif_devices(self) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/onvif/discover')

    def get_open_api_document(self) -> Any:
        """Requires scope-neutral."""
        return self._call('GET', f'/api/v1/openapi.json')

    def list_outbox(self) -> Any:
        """Requires admin, fleet-only."""
        return self._call('GET', f'/api/v1/outbox')

    def list_visitor_passes(self) -> Any:
        """Requires capability `identity:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/passes')

    def create_visitor_pass(self, body: Any) -> Any:
        """Requires capability `gate:operate`, fleet-only."""
        return self._call('POST', f'/api/v1/passes', body)

    def delete_visitor_pass(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('DELETE', f'/api/v1/passes/{id}')

    def get_visitor_pass(self, id: str) -> Any:
        """Requires capability `identity:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/passes/{id}')

    def update_visitor_pass(self, id: str, body: Any) -> Any:
        """Requires capability `gate:operate`, fleet-only."""
        return self._call('PATCH', f'/api/v1/passes/{id}', body)

    def check_in_visitor_pass(self, id: str) -> Any:
        """Requires capability `gate:operate`, fleet-only."""
        return self._call('POST', f'/api/v1/passes/{id}/checkin')

    def check_out_visitor_pass(self, id: str) -> Any:
        """Requires capability `gate:operate`, fleet-only."""
        return self._call('POST', f'/api/v1/passes/{id}/checkout')

    def delete_playback_session(self, session_id: str) -> Any:
        """Requires capability `video:playback`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/playback/sessions/{session_id}')

    def list_registry(self) -> Any:
        """Requires capability `system:read`, fleet-only."""
        return self._call('GET', f'/api/v1/registry')

    def refresh_registry(self) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/registry/refresh')

    def get_entry_log_report(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/reports/entry-log')

    def get_entry_exceptions_report(self) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/reports/exceptions')

    def delete_recording_schedule(self, schedule_id: str) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('DELETE', f'/api/v1/schedules/{schedule_id}')

    def update_recording_schedule(self, schedule_id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, scope-filtered."""
        return self._call('PATCH', f'/api/v1/schedules/{schedule_id}', body)

    def search_events(self, body: Any) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('POST', f'/api/v1/search/events', body)

    def search_natural_language(self, body: Any) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('POST', f'/api/v1/search/nl', body)

    def plan_search(self, body: Any) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('POST', f'/api/v1/search/plan', body)

    def search_semantic(self, body: Any) -> Any:
        """Requires capability `events:read`, scope-filtered."""
        return self._call('POST', f'/api/v1/search/semantic', body)

    def unlock_segment_evidence(self, id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/segments/{id}/evidence-lock')

    def lock_segment_evidence(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('POST', f'/api/v1/segments/{id}/evidence-lock', body)

    def tag_segment_incident(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PATCH', f'/api/v1/segments/{id}/incident', body)

    def get_site_info(self) -> Any:
        """Requires capability `system:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/site')

    def list_sites(self) -> Any:
        """Requires capability `camera:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/sites')

    def create_site(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/sites', body)

    def delete_site(self, id: str) -> Any:
        """Requires admin, fleet-only."""
        return self._call('DELETE', f'/api/v1/sites/{id}')

    def get_site(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/sites/{id}')

    def update_site(self, id: str, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PATCH', f'/api/v1/sites/{id}', body)

    def delete_snapshot_schedule(self, schedule_id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('DELETE', f'/api/v1/snapshot-schedules/{schedule_id}')

    def update_snapshot_schedule(self, schedule_id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('PATCH', f'/api/v1/snapshot-schedules/{schedule_id}', body)

    def get_system_info(self) -> Any:
        """Requires capability `system:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/system')

    def get_db_status(self) -> Any:
        """Requires capability `system:read`, fleet-only."""
        return self._call('GET', f'/api/v1/system/db')

    def set_db_limit(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PUT', f'/api/v1/system/db', body)

    def convert_db_auto_vacuum(self) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/system/db/convert')

    def get_retention_limits(self) -> Any:
        """Requires capability `system:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/system/retention')

    def set_retention_limits(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PUT', f'/api/v1/system/retention', body)

    def get_timezone(self) -> Any:
        """Requires capability `system:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/system/timezone')

    def set_timezone(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PUT', f'/api/v1/system/timezone', body)

    def get_transcode_settings(self) -> Any:
        """Requires capability `system:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/system/transcode')

    def set_transcode_engine(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PUT', f'/api/v1/system/transcode', body)

    def list_users(self) -> Any:
        """Requires admin, fleet-only."""
        return self._call('GET', f'/api/v1/users')

    def create_user(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/users', body)

    def delete_user(self, id: str) -> Any:
        """Requires admin, fleet-only."""
        return self._call('DELETE', f'/api/v1/users/{id}')

    def update_user(self, id: str, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PATCH', f'/api/v1/users/{id}', body)

    def unlock_user(self, id: str) -> Any:
        """Requires admin, fleet-only."""
        return self._call('POST', f'/api/v1/users/{id}/unlock')

    def list_vehicles(self) -> Any:
        """Requires capability `identity:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/vehicles')

    def create_vehicle(self, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/vehicles', body)

    def delete_vehicle(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('DELETE', f'/api/v1/vehicles/{id}')

    def get_vehicle(self, id: str) -> Any:
        """Requires capability `identity:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/vehicles/{id}')

    def update_vehicle(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('PATCH', f'/api/v1/vehicles/{id}', body)

    def list_watchlist(self) -> Any:
        """Requires capability `identity:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/watchlist')

    def create_watchlist_entry(self, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/watchlist', body)

    def delete_watchlist_entry(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('DELETE', f'/api/v1/watchlist/{id}')

    def update_watchlist_entry(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('PATCH', f'/api/v1/watchlist/{id}', body)

    def list_webhook_subscriptions(self) -> Any:
        """Requires capability `events:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/webhooks')

    def create_webhook_subscription(self, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/webhooks', body)

    def delete_webhook_subscription(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('DELETE', f'/api/v1/webhooks/{id}')

    def update_webhook_subscription(self, id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('PATCH', f'/api/v1/webhooks/{id}', body)

    def list_webhook_deliveries(self, id: str) -> Any:
        """Requires capability `events:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/webhooks/{id}/deliveries')

    def test_webhook_subscription(self, id: str) -> Any:
        """Requires capability `registry:manage`, fleet-only."""
        return self._call('POST', f'/api/v1/webhooks/{id}/test')

    def delete_zone(self, zone_id: str) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/zones/{zone_id}')

    def update_zone(self, zone_id: str, body: Any) -> Any:
        """Requires capability `registry:manage`, camera-keyed."""
        return self._call('PATCH', f'/api/v1/zones/{zone_id}', body)
