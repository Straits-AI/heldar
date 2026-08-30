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
class ErrorBody:
    code: str
    error: str
    retryable: bool


@dataclass
class ExportRequest:
    from_: str  # wire name: "from"
    to: str
    camera_id: str | None = None
    dry_run: bool = None
    incident_id: str | None = None


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
class TimezoneSettings:
    server_local_offset: str
    source: TzSource
    unconfigured_behaviour: str
    configured: str | None = None


@dataclass
class TimezoneUpdate:
    timezone: str


TzSource = str  # one of: site, default, unset

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

    def list_cameras(self) -> Any:
        """Requires capability `camera:read`, scope-filtered."""
        return self._call('GET', f'/api/v1/cameras')

    def delete_camera(self, id: str) -> Any:
        """Requires capability `admin`, camera-keyed."""
        return self._call('DELETE', f'/api/v1/cameras/{id}')

    def get_camera(self, id: str) -> Any:
        """Requires capability `camera:read`, camera-keyed."""
        return self._call('GET', f'/api/v1/cameras/{id}')

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

    def get_timezone(self) -> Any:
        """Requires capability `system:read`, scope-neutral."""
        return self._call('GET', f'/api/v1/system/timezone')

    def set_timezone(self, body: Any) -> Any:
        """Requires admin, fleet-only."""
        return self._call('PUT', f'/api/v1/system/timezone', body)
