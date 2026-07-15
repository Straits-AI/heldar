---
id: quickstart
title: 快速入门
sidebar_label: 快速入门
sidebar_position: 1
---

# 快速入门

在本地启动 Heldar，接入真实摄像头，并运行参考 AI 工作进程。

## 最快方式：Docker 一行命令

如果已安装 Docker，可跳过编译步骤——直接拉取预构建的开放镜像并启动整个服务栈：

```bash
curl -fsSL https://heldar.swmengappdev.workers.dev/install.sh | sh
```

该脚本会写入 `~/heldar/{compose.yml,mediamtx.yml,.env}` 并启动 MediaMTX、核心服务和 Web 界面；控制台随即可通过 `http://localhost:8080` 访问。（已克隆代码库？运行 `docker compose -f deploy/compose.yml up -d` 效果相同。）使用 `docker compose --profile ai up -d` 添加参考 AI 工作进程，使用 `docker compose pull` 更新镜像。如需从源码构建，请继续阅读以下内容。

## 前置条件

- **Rust**（通过 `rustup` 安装）——项目跟踪最新稳定版本。
- **FFmpeg + ffprobe** 已在 `PATH` 中——录制、剪辑、快照和帧采样均依赖它们。服务器启动时会对媒体二进制文件进行预检，若缺失则快速失败。
- **curl**——用于以下 API 调用。
- **Node.js**——用于 React 控制台（`apps/web`）。
- **Python 3**——用于 AI 工作进程（`apps/ai`）。

## 构建与运行

```bash
rustup update
cargo build --workspace
cp .env.example .env                 # defaults work out of the box; never commit .env
scripts/setup_mediamtx.sh            # fetch the MediaMTX live-view gateway
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + Vite dashboard
```

`scripts/run_stack.sh` 会启动三个进程：MediaMTX 实时预览网关、运行在 `http://localhost:8000` 的 Heldar Core 服务器，以及运行在 `http://localhost:5173` 的 Vite 开发服务器（用于控制台）。

### 两种访问控制台的方式

- **单一二进制（一个 URL）。** 构建控制台并将服务器指向它：

  ```bash
  cd apps/web && npm install && npm run build      # writes apps/web/dist
  ```

  在 `.env` 中设置 `HELDAR_WEB_DIR=./apps/web/dist`。核心服务随即会在 `http://localhost:8000` 同时提供 SPA 和 API。`/api/*`、`/media/*`、`/healthz`、`/readyz` 和 `/metrics` 路由优先响应；其余请求回退到 SPA，客户端路由的深层链接因此可正常工作。参见[部署](./deploy.md)。
- **Vite 开发服务器（热重载）。** `scripts/run_stack.sh` 运行 `npm run dev`，在 `http://localhost:5173` 提供控制台服务，并与 `:8000` 上的 API 通信。前端开发时使用此方式。

## 添加摄像头

RTSP URL 由厂商模板自动生成，因此只需提供地址和凭据：

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # uptime, camera/segment counts
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # recorded ranges
```

录像器为每个可录制的摄像头生成一个无需解码的 FFmpeg 进程并开始写入分段文件。索引器在几秒钟后将已关闭的分段文件转换为时间轴记录。

> 请勿暴力破解摄像头凭据。HikVision 设备在多次尝试失败后会锁定账户。

## 运行 AI 工作进程

感知推理运行在一个独立的工作进程中，通过 HTTP 拉取采样帧。首先在摄像头上启用检测任务（通过控制台或 API）：

```bash
curl -X POST http://localhost:8000/api/v1/cameras/gate_a/ai-tasks \
  -H 'content-type: application/json' \
  -d '{"task_type":"detection","fps":5,"width":1280,"enabled":true}'
```

启用第一个任务后，会为该摄像头启动帧采样器。然后运行参考工作进程：

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
```

工作进程会发现任务、拉取每个摄像头的最新帧、运行分析器，并将检测结果回传至 `POST /api/v1/ai/events`。完整协议说明请参见[构建 AI 工作进程](../develop/ai-worker.md)。若要在没有模型和 GPU 的情况下验证从采样器到工作进程再到事件的完整链路，请使用 `task_type: "motion"` 创建任务——参考工作进程内置了一个可用的帧差分析器。

## 在 UI 中配置检测、区域和告警

摄像头和 AI 任务运行后，可在控制台中进行以下操作：

- **检测** - 为每个摄像头创建或调整 AI 任务（`task_type`、请求的 `fps`、采样 `width`，以及工作进程读取的自由格式 `config` 字段）。
- **区域** - 在摄像头画面上绘制多边形区域。坐标已归一化至 0..1，与检测框对应。设置 `labels` 可过滤哪些检测结果计入区域，`dwell_seconds` 用于触发停留告警，`severity` 用于设置严重等级。被追踪的检测目标穿越区域时，会触发带有证据帧的 `enter` / `exit` / `dwell` 区域事件。
- **告警** - 在**系统 → Webhooks** 中创建一个 Webhook 订阅（或使用 `min_severity: "warning"` 调用 `POST /api/v1/webhooks`）。`warning` 和 `critical` 事件（包括区域事件和工作进程上报的事件）将被投递至该地址——参见 [Webhook 与集成](../develop/webhooks.md)。

## 下一步

- [部署](./deploy.md)——了解单二进制生产环境布局。
- [架构](../concepts/architecture.md)——了解内核与各应用如何协同工作。
