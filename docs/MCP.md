# heldar-mcp

*Issue [#123](https://github.com/Straits-AI/heldar/issues/123). Read-only.*

An MCP sidecar that lets an agent ask a Heldar box questions. It is a **separate binary**, not an
endpoint inside the recorder: a recorder's job is to keep recording while everything else is on fire,
and adding MCP libraries, model hosts and agent traffic to that process is the wrong trade.

The core remains the **sole authorization and camera-scope enforcement point**. This sidecar decides
what to expose; it never decides who may see it.

## Running it

```jsonc
// An MCP client's server config
{
  "command": "heldar-mcp",
  "args": ["stdio"],
  "env": {
    "HELDAR_URL": "http://127.0.0.1:8000",
    "HELDAR_TOKEN_FILE": "/run/secrets/heldar-mcp-key"
  }
}
```

`HELDAR_TOKEN_FILE` is preferred over `HELDAR_TOKEN` for the same reason the server prefers a file:
an environment variable is readable from `/proc` and inherited by children.

**Use a dedicated, capability-scoped key.** Never an administrator's, and never a browser session.
Mint one with only `camera:read`, `system:read` and `events:read`, scoped to the cameras the agent
should see. A key with more capability than the agent needs is a key the agent has.

## Read-only is structural

Every tool carries `method: "GET"`, and the dispatcher sends nothing else. There is **no code path**
from a tool call to a mutation — not a disabled branch, not a prompt instruction. A system prompt
saying "do not modify anything" is a request, and the first confused-deputy chain ignores it.

`every_tool_is_read_only` fails the test suite if a tool is ever added that is not a GET. Adding
mutations later means adding a capability group and a deliberate flag, which is what #123 asks for.

## Tools

| Tool | Answers |
|---|---|
| `get_system_health` | version, uptime, camera counts, recording state |
| `list_cameras` | cameras this credential can see |
| `get_camera_health` | recorder state per camera |
| `get_timeline` | what footage actually exists for a camera |
| `get_recording_gaps` | when a camera should have been recording and was not |
| `get_incident` | segments locked to an incident |
| `list_ai_workers` | which workers hold which cameras, and how fresh |
| `get_security_posture` | key source, process visibility, service user, plaintext credentials |
| `get_retention_limits` | the effective size cap and disk floor |
| `get_backup_status` | recent backup jobs |

## What never reaches model context

Passwords, tokens, session cookies, stream URLs and signed media URLs are stripped. A signed media
URL *is* a capability — in a transcript it has left the building.

**Device addresses are not stripped**, and that is a deliberate choice rather than an oversight: a
camera's address appears in `record_url_masked` regardless (the kernel masks the credential, not the
host), and an agent asking "which device is this?" needs it. So this sidecar does disclose your
camera addresses to whatever model is on the other end — scope its key accordingly, and do not point
it at a model host you would not tell your network topology to.

Usernames *are* stripped: half a credential with no diagnostic value.

## Errors

A tool failure comes back as a result with `isError: true`, not a protocol error — the model should
see it and adapt rather than have the connection fault. The box's own message is passed through
rather than reworded, because a sidecar that turns a 403 into something friendlier hides an
authorization boundary from the person debugging it. The correlation id is included so a finding can
be joined to the box's audit log.

## Not here

A listening transport, and every mutation. Both come after local operation has field experience —
and mutations additionally after #121's dry-run/plan semantics cover the endpoints in question.
Gate and relay actuation, PTZ, deletion, retention weakening, key creation, credential retrieval and
plugin installation are excluded outright, and a test asserts no tool's path reaches them.
