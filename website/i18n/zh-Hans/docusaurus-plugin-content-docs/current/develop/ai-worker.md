---
id: ai-worker
title: 构建 AI Worker
sidebar_label: 构建 AI Worker
sidebar_position: 2
---

# 构建 AI Worker

Heldar AI Worker 是任何能够响应四个 HTTP 端点的进程。内核负责采样帧并存储结果；你的 Worker 是一个纯粹的 HTTP 客户端，用于发现任务、拉取帧、运行模型，并将检测结果回传。由于协议基于 HTTP，崩溃或响应缓慢的 Worker 绝不会阻塞数据摄取或录制流程。

参考实现为
[`apps/ai/worker.py`](https://github.com/Straits-AI/heldar/blob/main/apps/ai/worker.py)
（一个轻量级、依赖精简的 Python Worker）。完整的集成指南（包括区域分析器和 ANPR 分析器）请参阅
[docs/AI-WORKERS.md](https://github.com/Straits-AI/heldar/blob/main/docs/AI-WORKERS.md)。

## 接口约定

所有端点均位于 `/api/v1` 下，返回 JSON 格式数据；帧数据除外，其类型为
`image/jpeg`。

### 1. 发现任务 - `GET /api/v1/ai/tasks`

返回所有已启用摄像头上的所有已启用任务，每个任务携带用于拉取帧的 `frame_url`。
这是你的完整任务列表。每隔数秒重新轮询一次，以获取新启用或已禁用的任务。

```json
[
  {
    "id": "ai_3f2a9c1b...",
    "camera_id": "gate_a",
    "task_type": "detection",
    "stream_profile": "sub",
    "fps": 5.0,
    "width": 1280,
    "config": { "classes": ["person", "car"], "min_confidence": 0.4 },
    "frame_url": "/api/v1/cameras/gate_a/frame"
  }
]
```

`task_type` 是你自定义的自由格式字符串；内核仅使用 `fps` / `width` /
`enabled` 来驱动采样器，`config` 是由你的 Worker 解释的不透明数据块。这里的 `fps` 是请求速率；经过预算调度后的实际采样速率由 `GET /api/v1/ai/samplers` 报告。

### 2. 拉取帧 - `GET /api/v1/cameras/{id}/frame`

为指定摄像头提供最新采样的 JPEG 帧。按照你自己的节奏拉取，通常等于或略低于任务的 `fps`。

```
200 OK
Content-Type: image/jpeg
Cache-Control: no-store
x-frame-age-ms: 142
x-frame-captured-at: 2026-06-13T08:15:31.120+00:00

<JPEG bytes>
```

- `x-frame-age-ms` - 自帧写入以来经过的毫秒数。当采样器离线时，可用此值跳过过期帧。
- `x-frame-captured-at` - 写入时间戳。以此进行去重，避免重复分析未变化的帧；也可选择将其作为检测结果的 `timestamp` 回传，使检测结果与采集时间对齐。

返回 `404` 表示该帧尚不存在（摄像头没有已启用的 AI 任务，或采样器尚未产生第一帧）。将其视为跳过本次周期，而非错误。

### 3. 提交结果 - `POST /api/v1/ai/events`

为一个摄像头和任务提交一批检测结果，可在同一请求中附带一个派生事件。

```json
{
  "camera_id": "gate_a",
  "task_type": "detection",
  "timestamp": "2026-06-13T08:15:31.120Z",
  "detections": [
    {
      "label": "person",
      "confidence": 0.92,
      "bbox": [0.41, 0.30, 0.08, 0.22],
      "track_id": "t-17",
      "attributes": { "zone": "entry_lane_a" }
    }
  ],
  "event": {
    "event_type": "person_in_red_zone",
    "severity": "warning",
    "payload": { "zone": "red_a", "track_id": "t-17" }
  }
}
```

字段规则：

- `camera_id` 为必填项，且必须存在，否则返回 `404`。
- `task_type` 为必填项，并存储于每条检测记录行中。
- `timestamp` 为可选的 RFC3339 格式，应用于整个批次；若缺失或无法解析，则回退为服务器的 `now()`。
- `detections` 为可选项（默认为 `[]`）；发送 `[]` 可仅提交事件。检测对象内的每个字段均为可选。
- `event` 为可选项。`event_type` 在存在时为必填；`severity` 默认为 `info`（使用 `warning` 或 `critical` 可触发告警 webhook）；`payload` 默认为空对象。

响应格式为 `{ "detections_ingested": N }`。

### 4. 采样器状态 - `GET /api/v1/ai/samplers`

每个摄像头的采样器状态（`connecting` / `sampling` / `offline` / `error` /
`stopped`）及经过预算调度后的实际帧率。适用于仪表盘展示以及确认内核是否正常产生帧。

## bbox 约定

`bbox` 格式为 `[x, y, w, h]`，**归一化至 0..1**，以左上角为原点。归一化处理使检测结果与分辨率无关，因此在采样 `width` 发生变更时仍然有效，并能直接映射至归一化区域多边形。内核将边界框以原始 JSON 形式存储，不验证其形状，因此你的 Worker 需自行保证正确性。

同时包含 `track_id` 和 `bbox` 的检测结果会驱动内核区域引擎（其地面点为边界框的底部中心）；不含这两项的检测结果仍会被存储，但无法触发区域事件。

## Worker 循环

```text
tasks = GET /api/v1/ai/tasks                 # refresh every few seconds
for each task (own thread / async task):
    loop at ~task.fps:
        resp = GET task.frame_url
        if resp is 404:        sleep, continue          # no frame yet
        if x-frame-captured-at == last_seen: continue   # unchanged frame; skip
        dets, event = analyze(task, resp.body)
        if dets or event:
            POST /api/v1/ai/events { camera_id, task_type, timestamp,
                                     detections: dets, event: event }
```

由于提供的帧为最新值，以快于采样器写入的速度拉取会返回相同的字节；请通过 `x-frame-captured-at` 进行去重。以较慢的速度拉取仅会丢弃中间帧，在当前帧率下用于检测和跟踪时这是可以接受的。

## 接入模型

参考 Worker 定义了一个 `Analyzer` 基类，并为每个任务线程创建一个实例，因此每个摄像头的状态（前一帧、跟踪器）存放于 `self` 上。分析器按 `task_type` 注册；未知类型将回退为一个占位分析器，该分析器会执行帧路径，但不会伪造检测结果。一个可工作的、无需模型的 `MotionAnalyzer` 已注册为 `task_type: "motion"`，因此你可以在没有模型和 GPU 的情况下验证从采样器到 Worker 再到事件的完整链路。

真实检测器只需作为一个子类并调用一次 `register(...)` 即可接入，无需更改内核或 HTTP 约定：

```python
from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

class YoloAnalyzer(Analyzer):
    name = "yolo"
    def __init__(self, config, log):
        super().__init__(config, log)
        from ultralytics import YOLO
        self.model = YOLO(config.get("weights", "yolov8n.pt"))
        self.conf = float(config.get("threshold", 0.25))

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image(); w, h = img.size
        dets = []
        for r in self.model(img, conf=self.conf, verbose=False):
            for b in r.boxes:
                x1, y1, x2, y2 = b.xyxy[0].tolist()
                dets.append(Detection(
                    label=self.model.names[int(b.cls)],
                    confidence=float(b.conf),
                    bbox=[x1/w, y1/h, (x2-x1)/w, (y2-y1)/h]))  # normalized 0..1
        return AnalysisResult(detections=dets)

register("detection", YoloAnalyzer)   # replaces the placeholder for "detection"
```

内核从不接触你的模型。它仅按 `task_type` 将结果路由至相应消费者：带有 track id 的 `detection` 结果驱动区域引擎，`anpr` 结果输送至访问控制引擎，以此类推。若要添加新的处理流水线，只需选择一个新的 `task_type`，提交其结果，并为其编写一个消费者（请参阅
[构建模块](./build-a-module.md)）。

## 运行参考 Worker

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
# or: python worker.py --api http://localhost:8000 --log-format json
```

Worker 端配置（CLI 参数或环境变量）涵盖 API 基础 URL
（`--api` / `HELDAR_API`）、任务轮询间隔
（`--poll-interval` / `HELDAR_AI_POLL_INTERVAL`），以及 HTTP/退避/日志相关参数。
完整参数列表请参阅
[apps/ai/README.md](https://github.com/Straits-AI/heldar/blob/main/apps/ai/README.md)。
