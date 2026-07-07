---
id: deploy
title: 部署
sidebar_label: 部署
sidebar_position: 2
---

# 部署

Heldar 被设计为**单一二进制、单一 URL** 运行。组合服务器
（`heldar-server`，即 `heldar-core` 二进制文件）通过单个进程提供 JSON API、录制媒体、指标/健康端点以及内置仪表盘。

有三种运行方式，按复杂程度排列：

- **Docker（拉取并运行）：** `docker compose -f deploy/compose.yml up -d` — 或使用
  [快速入门一键命令](quickstart#fastest-docker-one-liner)。预构建的开放镜像，无需工具链。
- **原生二进制** — 从源码构建并运行（本页）。
- **刷入设备** — 用于专用 DVR 设备的原生 systemd 操作系统镜像（设备上不使用 Docker；参见仓库中的 `infra/systemd/`）。

## 单一二进制，单一 URL

构建仪表盘，然后通过 `HELDAR_WEB_DIR` 将服务器指向它：

```bash
cd apps/web && npm install && npm run build      # writes apps/web/dist
# in .env:
HELDAR_WEB_DIR=./apps/web/dist
```

当 `HELDAR_WEB_DIR` 已设置且目录存在时，服务器将把 SPA 作为回退提供服务。显式路由优先，SPA 仅作为其他所有请求的回退：

- `/api/*` - JSON API。
- `/media/recordings`、`/media/clips`、`/media/snapshots`、`/media/playback`、
  `/media/archives` - 从数据目录提供的静态媒体。
- `/healthz`（存活检测）、`/readyz`（就绪检测，运行 `SELECT 1`）、`/metrics`
  （Prometheus 指标暴露）。
- 其他所有路径 - 仪表盘，未知的客户端路由路径回退到
  `index.html`，使深层链接返回 `200`。

如果未设置 `HELDAR_WEB_DIR`，默认为相对于二进制文件工作目录的 `apps/web/dist`。当两者均不存在时，服务器以仅 API 模式运行，并记录日志说明仪表盘未提供服务。

## 端口

| 端口 | 服务 |
| --- | --- |
| 8000 | Heldar Core HTTP API + 仪表盘（`HELDAR_API_HOST` / `HELDAR_API_PORT`） |
| 5173 | Vite 开发服务器（仅开发环境；单二进制部署中不使用） |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | MediaMTX 控制 API（本地回环） |

实时预览通过 MediaMTX 代理：摄像头凭据仅保存在网关的路径配置中，永远不会传递到浏览器，浏览器只会看到不含凭据的 HLS/WebRTC/RTSP URL。

## 认证

认证和 RBAC 通过 `HELDAR_AUTH_ENABLED` **按需启用**（默认值 `false`）。

- **`false`** - 开放 API，适用于单租户局域网设备。管理界面无需令牌即可访问，并以管理员身份运行。
- **`true`** - 每个请求都需要会话（登录）或 `X-API-Key`。五个角色在各项能力上强制执行（`admin` / `manager` / `guard` / `viewer` / `integration`），每次变更都会写入不可篡改的审计日志。首次运行且无用户时，将从引导环境变量中生成一个管理员账户。

会话使用 HttpOnly、SameSite=Strict Cookie。在 TLS 后端设置
`HELDAR_AUTH_COOKIE_SECURE=true`（明文 HTTP 局域网或覆盖网络访问时保持 `false`）。通过 `HELDAR_SESSION_TTL_HOURS` 调整会话有效期
（默认 12），并可通过
`HELDAR_SESSION_IDLE_TIMEOUT_MIN` 使空闲会话过期（默认 `0`，即无空闲超时）。

暴力破解锁定默认启用：账户在连续
`HELDAR_LOGIN_MAX_FAILURES`（5）次登录失败后锁定
`HELDAR_LOGIN_LOCKOUT_MIN`（15）分钟 — 即使密码正确也会被拒绝，时间窗口过后自动解锁（管理员可通过
`POST /api/v1/users/{id}/unlock` 提前解除锁定）。

多用户或网络化部署请设置 `HELDAR_AUTH_ENABLED=true`。

:::tip 暴露于互联网？请先加固。
对于可从公网访问的部署，内核在不安全配置下**会大声报错** — 它拒绝在认证关闭的情况下通过中继节点启动，并在非 `Secure` Cookie、无空闲超时或明文摄像头凭据时发出警告（或在 `HELDAR_STRICT_PROD=true` 下拒绝启动）。在上线前请完成
[生产加固清单](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)
— 认证、TLS、摄像头凭据加密（`HELDAR_SECRET_KEY`），以及可选的 Cloudflare Turnstile 登录验证。
:::

## 存储与数据目录

Heldar 仅使用 SQLite（WAL 日志，内置迁移）。默认 URL 为
`sqlite://./data/heldar.db`。

| 变量 | 默认值 | 含义 |
| --- | --- | --- |
| `HELDAR_DATABASE_URL` | `sqlite://./data/heldar.db` | 仅支持 SQLite；非 `sqlite` URL 在启动时会被拒绝 |
| `HELDAR_DATA_DIR` | `./data` | 数据库和媒体子目录的根目录 |
| `HELDAR_RECORDINGS_DIR` / `CLIPS_DIR` / `SNAPSHOTS_DIR` / `FRAMES_DIR` | 在 `./data` 下 | 媒体根目录（启动时创建） |
| `HELDAR_MAX_RECORDINGS_GB` | `20` | 软性存储上限；超过后将修剪最旧的未锁定片段 |
| `HELDAR_MIN_FREE_DISK_GB` | `5` | 硬性主机保护下限；在可用空间低于此值时修剪未锁定片段 |

录制内容保留在本地磁盘并从本地提供服务；默认不会推送到云端。证据锁定的片段永远不会被保留策略删除。两个限制均可**在运行时无需重启地调整** — 通过仪表盘系统页面（"录制限制"面板）或 `GET`/`PUT /api/v1/system/retention` 接口（PUT 仅限管理员）；已存储的覆盖值优先于环境变量值，环境变量值仍作为默认值。

摄像头凭据保存在此 SQLite 数据库中。设置 `HELDAR_SECRET_KEY`（32 字节的 base64 编码，例如 `openssl rand -base64 32`）以使用 AES-256-GCM 对其进行静态加密；不设置则对可信局域网设备保持明文存储。现有明文凭据将在下次启动时被加密封存 — 参见
[生产加固指南](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)。

## CORS

`HELDAR_CORS_ORIGINS` 控制跨域访问。空值或 `*` 允许所有来源；否则限制为已配置的列表（默认允许 Vite 开发服务器）。在单二进制部署中，仪表盘与 API 同源，因此 CORS 主要在独立前端或集成服务调用 API 时才相关。

## 部署运维

有关规模调整、调试、可观测性和远程访问 — 包括通过 `HELDAR_WEBRTC_ICE_SERVERS` 接入自定义 STUN/TURN — 请参见
[运维](../operate/index.md) 中心。
