---
id: remote-access
title: 远程访问
sidebar_label: 远程访问
sidebar_position: 3
---

# 远程访问

从局域网外部查看 Heldar 部署——包括在 **CGNAT** 后方（常见的家庭/小型站点情况：共享公网 IP，无法进行入站端口转发）。远程查看是**基于浏览器的 WebRTC 方案**，是一项**开放内核能力**：每个部署都可获得私有远程访问，无需客户端应用，也无需开放入站端口。

## 工作原理

设备**主动拨出**至汇聚点；查看者只需在浏览器中打开控制面板。CGNAT 始终允许出站连接，因此在端口转发和 DDNS 无法使用的情况下，此方案同样有效。

- **实时视频**通过 **MediaMTX / WHEP** 传输，并采用**端对端加密（DTLS-SRTP）**。汇聚点仅负责 SDP/ICE 握手和中继控制信令——从不经手视频字节。当直接对等路径无法穿透对称型 CGNAT 时，**TURN** 以无法读取的方式中继数据包。
- **完整控制面板**（实时查看、录像回放、配置、事件）通过同一中介路径运行，并采用**双重门禁**认证模型，确保内核是唯一的 RBAC 权威：
  - **外层门禁** — 短期有效、按用户、按站点范围颁发的能力令牌，用以证明浏览器可*访问*本设备。
  - **内层门禁** — 您的**真实内核会话**被原样转发并在设备自身的 `127.0.0.1` 内核上重放，内核运行其正常的认证 + RBAC。中继是一条受限、仅限白名单的管道，绝不是认证旁路；会话令牌存储在 HttpOnly cookie 中，浏览器 JS 永远无法获取。
- **故障保护：** 中继**拒绝在认证未启用或不存在真实用户的情况下运行**——未开启认证的公开 API 永远不会被远程暴露。

> **为何不使用端口转发 / DDNS / 公共反向代理？** 运营商 CGNAT 屏蔽所有入站流量，且通常是*对称型* NAT，这使普通 STUN 打洞失效。唯一可靠的方式是设备主动拨出——这正是 WebRTC 路径（以及下文介绍的叠加网络替代方案）的做法。

## 开启方法（设备端）

远程访问需要手动开启。设备需要启用认证并配置拨出汇聚点：

```bash
HELDAR_AUTH_ENABLED=true              # required — the relay refuses to run without it
HELDAR_AUTH_COOKIE_SECURE=true        # the rendezvous terminates TLS
HELDAR_REMOTE_RENDEZVOUS_URL=https://<your-rendezvous>   # the box dials OUT to this
HELDAR_CP_TOKEN=<dial-out bearer>     # authenticates the box to the rendezvous (== the Worker's BOX_TOKEN)
HELDAR_SITE_ID=<stable-id-for-this-box>
```

配置完成后，内核将主动拨出，将浏览器 WHEP 请求桥接至自身的 MediaMTX，并为 MediaMTX 配置 ICE 服务器，以便设备为对称型 NAT 穿透收集中继候选地址。查看者通过汇聚点为您的站点提供的控制面板进行访问。

**TURN — 使用托管汇聚点，或自行部署：**

- **托管方式：** 将 `HELDAR_REMOTE_RENDEZVOUS_URL` 指向托管汇聚点；内核会从中获取短期 TURN 凭证并自动刷新。
- **自托管方式：** 将 `HELDAR_WEBRTC_ICE_SERVERS` 设置为 MediaMTX `webrtcICEServers2` JSON 数组，指向您自行运行的任意 STUN/TURN（coturn、Cloudflare Realtime 等）。内核会将其配置到 MediaMTX 中。
- **两者均不配置** → MediaMTX 仅使用 STUN 基线：仅限局域网 / 非对称型 NAT。

录像回放采用 **HEVC/H.265 直通**——设备原样传输已录制的码流，由客户端硬件解码（在上行带宽受限时是最高效的路径）；不支持 HEVC 的浏览器会收到明确提示，而不是黑色画面。

:::warning 暴露前请先完成加固
中继会强制执行认证，但面向互联网的设备还需完成完整检查清单——包括 `Secure` cookie、较短的会话 TTL 与空闲超时、摄像头凭证加密（`HELDAR_SECRET_KEY`），以及汇聚点密钥（可选配 Cloudflare Turnstile 登录挑战）。请先阅读[生产加固指南](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)——内核在汇聚点后方关闭认证时也会**大声报错**并拒绝启动。
:::

## 替代方案：自托管网络叠加层

如果您希望获得对整台主机的完整 L3 访问，而不想使用托管汇聚点，可将 WireGuard 叠加层作为外部守护进程运行，并将内核指向其接口以获取状态：

```bash
HELDAR_OVERLAY_ENABLED=true
HELDAR_OVERLAY_KIND=tailscale         # or netbird
HELDAR_OVERLAY_IFACE=tailscale0       # wt0 for netbird
```

- **个人 / 开发用途 → Tailscale**（免费，运维工作量接近零；仅限非商业用途）。
- **量产产品 → NetBird 自托管**（每个部署一个容器，无按席位收费，无第三方元数据）。

内核仅*观察*叠加层（从不管理 WireGuard），并在 `GET /api/v1/system → remote_access` 处暴露其健康状态。将叠加层的 ACL 限制在媒体/控制端口（`8889` / `8888` / `8000`），而非整台主机。

## 进一步了解

- **完整参考 + 叠加层配置示例：**
  [`docs/REMOTE-ACCESS.md`](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md) — CGNAT 原理说明、P2P 优先隐私模型，以及完整的 Tailscale / NetBird 配置示例。
- **加固：**
  [生产加固](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)。
- **运维中心：** [运维](../operate/index.md)。
