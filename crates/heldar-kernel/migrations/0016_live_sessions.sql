-- Which credential opened each live MediaMTX session, so a read can be WITHDRAWN mid-stream.
--
-- `/internal/mediamtx-auth` re-resolves a token's subject on every read, which bounds a transport to
-- the rate at which it RE-PRESENTS the token. HLS re-presents per segment, so revocation bites in
-- seconds. WebRTC does not: it authorizes once at WHEP negotiation and then media flows over the
-- established peer connection, so a revoked credential kept streaming until it renegotiated. RTSP
-- readers have the same property.
--
-- MediaMTX sends a session UUID and the protocol on every auth callback (verified against a live box:
-- `probe_action=read probe_protocol=hls probe_id=e8c05c98-… probe_path=cam_cam2`) and exposes
-- `POST /v3/{webrtcsessions,rtspsessions}/kick/{id}`. Recording the pair here is what lets the sweeper
-- close the loop: list the sessions MediaMTX still holds, re-ask whether each one's credential still
-- stands, and kick the ones that do not.
--
-- In the DATABASE rather than in memory on purpose: a kernel restart is exactly when the mapping is
-- most needed, because an established WebRTC session survives it and would otherwise become
-- permanently unattributable — the token signing key is per boot, so nothing would ever re-check it.
CREATE TABLE IF NOT EXISTS live_sessions (
    -- MediaMTX's own session id; the same value its list/kick endpoints use.
    id            TEXT PRIMARY KEY,
    -- `webrtc` | `rtsp` | `hls` | … — decides WHICH kick endpoint applies.
    protocol      TEXT NOT NULL,
    -- The MediaMTX path (`cam_<camera_id>`), so a re-scope can be judged per camera.
    path          TEXT NOT NULL,
    -- Mirrors `live_token::Subject`: 'api_key' | 'user' | 'site'. 'site' is never withdrawn.
    subject_kind  TEXT NOT NULL,
    subject_id    TEXT,
    created_at    TEXT NOT NULL,
    -- Refreshed whenever MediaMTX re-authorizes (HLS does so continuously); lets the sweeper drop
    -- records for sessions that ended without us seeing the teardown.
    last_seen_at  TEXT NOT NULL
);

-- The sweeper's access pattern: everything still live, oldest first.
CREATE INDEX IF NOT EXISTS idx_live_sessions_seen ON live_sessions(last_seen_at);
