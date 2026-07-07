[English](README.md) · **简体中文** · [Español](README.es.md)

# Heldar Core

Heldar 是一个面向物理空间的视觉事件智能操作系统。它将摄像头流转化为结构化事件，将事件转化为工作流，再将工作流转化为运营智能。Heldar 既不封装现有的 DVR/NVR，也不从 AI 功能出发，而是首先构建自己的**媒体内核**（摄像头注册、RTSP 采集、录制、回放、实时预览），然后将感知层、事件引擎和应用以*消费者*的形式叠加其上。掌控内核意味着掌控元数据模型、事件引擎和产品逻辑，同时无需重新实现编解码器（低级媒体工作由 FFmpeg 和 MediaMTX 完成）。

本平台采用**开源核心**模式：Apache-2.0 授权的内核及通用参考应用，垂直行业与客户产品以独立的专有 crate 形式存在。详见 [LICENSING.md](./LICENSING.md)。

## 文档

完整文档位于 **https://heldar.swmengappdev.workers.dev/docs/**，涵盖快速入门、部署、架构及其公开接口、开源核心边界，以及针对内核构建自定义应用或 AI 工作进程的指南。

仓库内参考文档：[ARCHITECTURE.md](./ARCHITECTURE.md)（内核接口及各阶段设计）、[ROADMAP.md](./ROADMAP.md)（阶段状态）、[LICENSING.md](./LICENSING.md)（开源核心边界），以及 [`docs/`](./docs) 中的运维/集成指南。

## 快速入门

**最快方式 —— Docker（拉取并运行）：**

```bash
curl -fsSL https://heldar.swmengappdev.workers.dev/install.sh | sh
# 已有仓库？直接执行：  docker compose -f deploy/compose.yml up -d
```

拉取预构建的 **OPEN** 镜像（内核 + 通用应用），启动 MediaMTX + 核心服务 + Web —— 之后可在 `http://localhost:8080` 访问仪表盘。使用 `--profile ai` 添加参考 AI 工作进程；使用 `docker compose pull` 更新。生产环境（私有完整镜像、认证、密钥、TLS）使用叠加配置 `docker compose -f deploy/compose.yml -f deploy/compose.prod.yml up -d` —— 详见 [`docs/PRODUCTION.md`](docs/PRODUCTION.md)。对于刷机的 DVR/设备，请改用 native-systemd 镜像（`make appliance-image`，[`infra/systemd/`](infra/systemd/)）。

**从源码构建：**

**前置条件：** Rust（通过 `rustup`）、`PATH` 中的 FFmpeg + ffprobe、`curl`。仪表盘需要 Node.js；AI 工作进程需要 Python 3。

```bash
rustup update                        # 本项目跟随最新稳定版
cargo build --workspace
cp .env.example .env                 # 默认配置开箱即用；切勿提交 .env
scripts/setup_mediamtx.sh            # 获取 MediaMTX 实时预览网关
scripts/run_stack.sh                 # MediaMTX + 核心服务 (http://localhost:8000) + Web (Vite on :5173)
```

当 `HELDAR_WEB_DIR` 指向 `apps/web/dist` 时，核心服务在 `http://localhost:8000` 提供已构建的仪表盘（一个二进制文件，一个 URL）。`scripts/run_stack.sh` 还会在 `http://localhost:5173` 运行 Vite 开发服务器，用于前端开发。

**远程访问**（在任意网络下，无需应用程序，即使在 CGNAT 之后）：设备主动向外拨出 WebRTC 会合连接，完整仪表盘在浏览器中运行 —— 包括实时多路摄像头、录像回放和配置管理 —— 采用双重认证门控模型，内核始终是唯一的 RBAC 权威。选用说明与设计详见 [`docs/REMOTE-ACCESS.md`](docs/REMOTE-ACCESS.md)；面向公网的加固措施（认证、TLS、密钥、封锁、凭证加密、Turnstile）详见 [`docs/PRODUCTION.md`](docs/PRODUCTION.md)。

接入摄像头（提供地址和凭证；RTSP URL 由厂商模板自动构建）：

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # 运行时间、摄像头/片段计数
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # 录像时间范围
curl http://localhost:8000/api/v1/system/retention           # 录像容量上限 + 磁盘最低余量
```

> 请勿暴力破解摄像头凭证。HikVision 设备在多次失败尝试后会触发封锁。

> **录像空间限制。** 保留策略清扫器会防止录像填满磁盘：容量上限（`HELDAR_MAX_RECORDINGS_GB`，默认 20）和磁盘最低余量（`HELDAR_MIN_FREE_DISK_GB`，默认 5），按最旧优先的顺序删除（已锁定的证据片段不会被删除）。两项参数均可在运行时通过 `GET`/`PUT /api/v1/system/retention`（PUT 仅限管理员）以及仪表盘系统页面动态调整，无需重启。

针对已启用 AI 的摄像头运行参考 AI 工作进程：

```bash
cd apps/ai && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
HELDAR_API=http://localhost:8000 .venv/bin/python worker.py
```

启用检测任务、绘制区域和配置告警，请参阅[快速入门](https://heldar.swmengappdev.workers.dev/docs/getting-started/quickstart)。

### 默认端口

| 端口 | 服务 |
| --- | --- |
| 8000 | Heldar Core HTTP API + 仪表盘 |
| 5173 | Web 仪表盘（Vite 开发服务器） |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | MediaMTX 控制 API（本地回环） |
