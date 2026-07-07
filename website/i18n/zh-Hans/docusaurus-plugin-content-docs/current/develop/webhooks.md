---
id: webhooks
title: Webhooks 与集成
sidebar_label: Webhooks 与集成
sidebar_position: 3
---

# Webhooks 与集成基础设施

Webhooks 是外部或**父**应用程序近实时接收 Heldar 事件的机制。*Webhook 订阅*注册一个 URL、事件类型过滤器、最低严重级别和可选签名密钥；内核随后将每个匹配的事件以签名 JSON 的形式 POST 到该 URL，并提供至少一次投递及重试保证。

这是存在于开放内核中的**通用**集成机制。各垂直应用基于同一基础设施构建——它们声明自己的领域事件类型并暴露自己的 REST 端点——而内核无需感知其存在（参见[垂直应用使用同一基础设施](#verticals-on-the-same-substrate)）。

以下所有路径均位于 `/api/v1` 下。管理订阅需要 `manager` 角色（或 `admin`）；读取操作需要任意已认证的主体。当 `HELDAR_AUTH_ENABLED=false`（默认的单租户 LAN 设备模式）时，每个调用者均为宽松主体，端点对外开放。已认证部署通过 `Authorization: Bearer <key>` 或 `X-API-Key: <key>` 传递密钥。

## 注册 Webhook

使用 `POST /api/v1/webhooks` 创建订阅：

```bash
curl -sS -X POST http://localhost:8000/api/v1/webhooks \
  -H 'Authorization: Bearer <api-key>' \
  -H 'Content-Type: application/json' \
  -d '{
        "name": "Ops Slack bridge",
        "url": "https://example.com/heldar/webhook",
        "event_types": ["zone_enter", "disk_pressure"],
        "min_severity": "warning",
        "secret": "whsec_a5f3…"
      }'
```

| 字段           | 默认值   | 含义                                                                                          |
| -------------- | ------- | --------------------------------------------------------------------------------------------- |
| `name`         | —       | 可读标签（必填）。                                                                             |
| `url`          | —       | `http(s)` POST 目标地址（必填）。                                                              |
| `event_types`  | `["*"]` | 需要投递的事件类型精确集合。`["*"]`（或省略）匹配**所有**类型。                                  |
| `min_severity` | `info`  | `info`（全部）、`warning`（warning + critical）或 `critical`（仅 critical）。                   |
| `secret`       | none    | HMAC-SHA256 签名密钥。设置后，每次投递均携带 `X-Heldar-Signature` 头。                          |
| `enabled`      | `true`  | 暂停投递而不删除订阅。                                                                         |

密钥为**只写**：不会被返回。读取操作仅暴露 `has_secret` 布尔值。更新时（`PATCH /api/v1/webhooks/{id}`），`secret` 字段为三态——省略以保留当前密钥，发送 `null`/`""` 以清除，或发送新值以替换。

其他端点：

- `GET /api/v1/webhooks` — 列出订阅。
- `PATCH /api/v1/webhooks/{id}` — 部分更新（缺失字段保持不变）。
- `DELETE /api/v1/webhooks/{id}` — 删除订阅。
- `POST /api/v1/webhooks/{id}/test` — 向该 URL 投递一个合成签名事件并返回 `{ ok, status, error }`。
- `GET /api/v1/webhooks/{id}/deliveries?limit=` — 最近的投递尝试记录（状态、响应码、时间戳）。

运营人员可以不通过 API，直接在控制台操作以上所有功能：
**系统 → Webhooks**。

## 投递的载荷

每次投递是一个 JSON 对象——事件信封——以如下头部 POST：

| 头部                 | 值                                                                   |
| -------------------- | -------------------------------------------------------------------- |
| `Content-Type`       | `application/json`                                                   |
| `X-Heldar-Event`     | 事件类型（例如 `zone_enter`）。                                       |
| `X-Heldar-Delivery`  | 本次投递尝试的唯一 ID（用于去重）。                                    |
| `X-Heldar-Timestamp` | 请求发送时的 Unix 秒数。                                              |
| `X-Heldar-Signature` | `sha256=<hex>` 对**原始请求体**的 HMAC-SHA256——仅在设置了密钥时存在。  |

请求体：

```json
{
  "id": "evt_9c1f…",
  "camera_id": "gate_a",
  "site_id": "hq",
  "event_type": "zone_enter",
  "severity": "warning",
  "timestamp": "2026-01-12T09:14:33.102Z",
  "payload": { "zone_id": "zone_7", "zone_name": "Loading bay", "track_id": "t-42", "label": "person" }
}
```

对于系统级事件，`camera_id` 和 `site_id` 可能为 `null`。`payload` 是特定事件类型的对象——其结构由事件发射者定义（内核、应用或 AI worker）。

## 验证签名

配置了密钥后，在信任请求前请验证 `X-Heldar-Signature`。对**确切的原始请求字节**计算 HMAC-SHA256——不要重新序列化已解析的 JSON，因为键顺序和空白字符会不同，导致签名不匹配。始终使用常数时间比较。

```python
import hashlib
import hmac

def verify(secret: str, raw_body: bytes, signature_header: str | None) -> bool:
    if not signature_header:
        return False
    expected = "sha256=" + hmac.new(secret.encode(), raw_body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature_header)
```

```js
// Node.js
import { createHmac, timingSafeEqual } from "node:crypto";

function verify(secret, rawBody, signatureHeader) {
  if (!signatureHeader) return false;
  const expected = "sha256=" + createHmac("sha256", secret).update(rawBody).digest("hex");
  const a = Buffer.from(expected);
  const b = Buffer.from(signatureHeader);
  return a.length === b.length && timingSafeEqual(a, b);
}
```

## 投递语义

- **至少一次。** 每个订阅维护自己的投递游标（事件时间戳）。请为重复投递做好准备：通过对事件 `id`（或 `X-Heldar-Delivery`）去重，使处理器具备幂等性。
- **无历史回放。** 新订阅从"当前"开始，因此添加订阅不会用历史事件淹没你。
- **以 2xx 确认。** 任何 `2xx` 响应均视为投递成功。非 2xx 响应、超时或连接错误视为失败，并在下一个周期重试（轮询间隔，最短 5 秒）。
- **有界重试。** 每个事件最多重试 5 次。之后内核放弃该事件并将游标前进，因此单个异常端点不会阻塞整个队列。每次尝试——无论成功或失败——均记录在投递日志中（`GET /api/v1/webhooks/{id}/deliveries`）。
- **快速响应。** 请尽快返回（先确认，再异步处理）。慢速处理器会消耗每请求超时时间，并被视为失败。

## 事件类型分类

`GET /api/v1/events/types` 返回内置事件类型及其单行描述——与控制台事件类型选择器中显示的列表相同。可用于驱动 UI 或验证过滤器。内置内核及参考应用类型包括：

| `event_type`         | 描述                                                                  |
| -------------------- | -------------------------------------------------------------------- |
| `camera_offline`     | 摄像头录制器丢失了 RTSP 连接。                                         |
| `recorder_error`     | 录制器进程报错或其分段已过期。                                          |
| `recording_gap`      | 检测到连续录制分段之间存在空洞。                                        |
| `sampler_offline`    | 某摄像头的 AI 帧采样器已离线。                                          |
| `retention_delete`   | 保留策略清理器已删除旧分段。                                            |
| `disk_pressure`      | 录制存储承压（配额、大小上限或可用空间下限）。                           |
| `disk_smart_warning` | SMART 自检报告了磁盘健康警告。                                          |
| `raid_degraded`      | Linux md/RAID 阵列报告了降级或故障成员。                                |
| `zone_enter`         | 被追踪目标进入了已配置的区域。                                          |
| `zone_exit`          | 被追踪目标离开了已配置的区域。                                          |
| `zone_dwell`         | 被追踪目标在区域内停留时间超过阈值。                                     |
| `entry_matched`      | 门禁控制：入口匹配注册表且已授权。                                       |
| `entry_exception`    | 门禁控制：入口需要运营人员审查。                                         |
| `entry_unmatched`    | 门禁控制：入口未匹配任何注册表记录。                                     |
| `entry_blocked`      | 门禁控制：入口匹配了观察名单/黑名单并被拒绝。                            |

此列表为**描述性，非穷尽性**。应用和 AI worker 在同一事件日志上发送自定义 `event_type` 字符串，配置了 `event_types: ["*"]` 的 Webhook 也会投递这些事件。

## 垂直应用使用同一基础设施

垂直应用（基于内核构建的领域应用）复用此机制而非重复发明。它声明自己的领域 `event_type` 字符串——通过内核写入规范事件日志——并暴露自己的 REST 端点；通用认证、事件日志、事务性发件箱和 Webhook 投递均从内核继承。参见[构建模块](./build-a-module.md)了解接口。

以校园访客门户作为实践模式示例。它在内核之上以两个方向进行集成：

- **入站** — 门户调用垂直应用自己的 REST 端点（例如预注册访客），通过限定在 `integration` 角色的 Heldar **API 密钥**认证。端点属于垂直应用；API 密钥、RBAC 和审计日志属于内核。
- **出站** — 父应用订阅垂直应用领域事件的 Webhook（例如垂直应用发出的 `campus.*` 事件），按事件类型和严重级别过滤，并使用同一 `X-Heldar-Signature` HMAC 验证。无需垂直应用特定的投递代码——使用的是上文记录的同一引擎。

因此，垂直应用的集成方式仅为：*声明领域事件类型 + 暴露领域端点*，通用 API 密钥认证（入站）和 Webhook 订阅（出站）随之免费获得。
