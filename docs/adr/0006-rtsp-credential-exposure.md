# ADR 0006 — Camera credentials in ffmpeg's argv

*Status: accepted. Supersedes nothing. Implements the spike #126 asks for.*

## The exposure

The recorder, sampler, clip exporter and ANR replay all spawn ffmpeg with an RTSP URL, and for a
camera with a password that URL is `rtsp://user:pass@host/...`. It is a single argv entry, so on
Linux it lands in `/proc/<pid>/cmdline`.

Any local user who can read that file can read every camera password on the box. Encrypting
credentials at rest does not touch this: the whole point of storing them is to hand them to ffmpeg,
and by then they are plaintext in a process argument.

`HELDAR_SECRET_KEY_FILE` (#126's first half) does not help either — the variable is *inherited*, and
a same-uid child simply opens the path.

## What was evaluated

### 1. A local credential-holding RTSP proxy, so ffmpeg sees only a loopback URL

MediaMTX is already in the stack and can pull from a camera and republish. The recorder would then
read `rtsp://127.0.0.1:8554/cam_x`, with no credential anywhere in its argv.

**Rejected for now**, on two grounds that are about the product rather than the effort:

- It puts a second process on the **recording path**. Today a camera reconnect is ffmpeg
  reconnecting to a camera; with a proxy it is ffmpeg reconnecting to MediaMTX reconnecting to a
  camera, and a MediaMTX restart drops every recording at once rather than one. For a recorder,
  turning N independent failure domains into one shared one is a real cost.
- It moves the exposure rather than removing it. MediaMTX holds the credentials and takes them from
  a config file it re-reads — so the question becomes who can read *that*, plus its API, which is a
  smaller surface but not an absent one.

Worth revisiting if #128 (out-of-process decode) lands, because that already restructures the media
plane and the marginal cost drops.

### 2. A credential file or descriptor ffmpeg reads instead of the URL

ffmpeg has no general mechanism for this. RTSP auth is negotiated from the URL's userinfo; there is
no `--password-file`, and `-rtsp_transport` does not cover credentials. Faking it through a config
file would mean patching ffmpeg.

**Rejected: not available.**

### 3. A dedicated service user plus `/proc` `hidepid=2`

`hidepid=2` makes other users' `/proc/<pid>` invisible entirely, so the argv is unreadable by anyone
but root and the service account. Combined with running as a dedicated non-root user — which the
shipped compose files and systemd unit already do — this closes the exposure to every untrusted
local user.

**Accepted as the supported posture.** It requires no change to the media path, costs nothing at
runtime, and is a property of the deployment rather than of the code.

### 4. Container PID-namespace isolation

A container with its own PID namespace cannot see host processes, and the host cannot see into the
container's `/proc` without entering the namespace. This is the default for Docker unless
`pid: host` is set, so most deployments already have it.

**Accepted as a supporting control**, and included in the checked prerequisite below.

## The decision

Heldar does **not** claim to remove credentials from ffmpeg's argv. It claims the deployment can be
configured so no untrusted local user can read them, and it **checks that claim rather than
asserting it**:

- `GET /api/v1/system/posture` reports `process_visibility` from `/proc/self/mountinfo` — whether
  `hidepid` is actually in effect, not whether it was intended.
- `HELDAR_REQUIRE_CREDENTIAL_ISOLATION=true` makes the box **refuse to start** when it is not. This
  is the "checked prerequisite" #126 asks for: a deployment that declares itself high-assurance and
  is not gets an error at boot, not a warning nobody reads.

The default is off, because a sealed single-operator appliance does not need it and a box that
refuses to record is worse than one with a documented exposure.

## What this does not claim

- It does not protect against **root**. Root can read any process's argv, and nothing at this layer
  changes that.
- It does not protect against someone who can read the **database** — those credentials are
  encrypted at rest only if a master key is configured.
- `hidepid` is a **mount option on `/proc`**, so it is set by the host or the container runtime, not
  by Heldar. The box can only observe and refuse.
