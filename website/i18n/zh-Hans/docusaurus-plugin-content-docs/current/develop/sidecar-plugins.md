---
id: sidecar-plugins
title: Sidecar 插件
sidebar_label: Sidecar 插件
sidebar_position: 2
---

# Sidecar 插件

**Sidecar 插件**无需编译进二进制文件即可扩展 Heldar。它是一个进程外的 HTTP 服务——可使用任意语言，以进程或容器形式运行——由 Heldar **在运行时安装**：无需重新构建，进程/容器隔离，最小权限访问。这是第三方及自制模块的推荐路径。（若需要与内核数据库和采集热路径深度集成的第一方 Rust 应用，请改用[编译内置的应用 crate](./build-a-module.md)。）

完整可运行的参考示例位于
[`examples/hello-module`](https://github.com/Straits-AI/heldar/tree/main/examples/hello-module) ——
这是一个零依赖的 Python sidecar，几分钟内即可注册并观察其接收事件。

## 整体架构

安装 sidecar 时，Heldar 会执行三项可逆操作：

1. **生成一个作用域 API 密钥**，供 sidecar 回调内核 API。该密钥遵循最小权限原则：
   `viewer`（只读）或 `integration`（读取 + 采集）。插件永远不会被授予 `admin`/`manager` 权限。
2. **创建 webhook 订阅**，对订阅的事件进行签名并投递。
3. **反向代理 `/m/{id}/*`** 至你的服务，使你的 UI 和 API 与控制台同源（以微前端方式挂载——你的 UI 不会打包进 Heldar 的 bundle）。

卸载时，以上三项操作全部回滚：密钥被撤销，订阅被删除，路由被移除。

![Heldar Core 与一个 sidecar 插件](/img/diagrams/sidecar.svg)

## 四个端点

你的 sidecar 需要提供以下端点。仅前两个是必须实现的。

| 端点 | 调用方 | 约定 |
| --- | --- | --- |
| `GET /heldar/health` | 内核（每 30 秒） | 返回任意 `2xx` 即标记为**健康** |
| `POST /heldar/events` | 内核 | 事件投递；验证 `X-Heldar-Signature`（见下文） |
| `GET /` 及静态资源 | 控制台 iframe | 你的插件 UI，挂载于 `/m/{id}/` |
| `GET /api/...` | 你的 UI | 你的 UI 数据 API，通过 `/m/{id}/api/...` 访问 |

由于 UI 挂载在 `/m/{id}/` 下，请使用**相对路径**发起资源和 API 请求
（`fetch("api/events")`，而非 `fetch("/api/events")`），以确保请求正确通过代理解析。

## Manifest 清单

注册时需要提交一个 manifest。其结构与进程内模块相同（进程内模块通过代码返回该结构）；sidecar 则将其发送至 `POST /api/v1/modules`：

```json
{
  "id": "visitor-portal",
  "name": "Visitor Portal",
  "version": "1.0.0",
  "publisher": "ACME Corp",
  "description": "Self-service visitor pre-registration",
  "base_url": "http://127.0.0.1:9123",
  "nav": [{ "path": "/visitor-portal", "label": "Visitors", "icon": "module" }],
  "subscribes": ["entry_matched", "entry_unmatched"],
  "role": "integration"
}
```

| 字段 | 含义 |
| --- | --- |
| `id` | 稳定的 slug；用于 `/m/{id}/` 挂载和导航键，不得与内置模块冲突。 |
| `base_url` | Heldar 反向代理的目标源（http/https）。 |
| `nav` | 要显示的导航条目。省略时，默认在 `/{id}` 生成单条目。`icon` 会回退为通用图标。 |
| `subscribes` | 要接收的事件类型（`["*"]` 表示全部）。参见[事件分类](./webhooks.md)。 |
| `role` | 生成密钥的角色：`viewer` 或 `integration`。 |

## 注册

在控制台中：**插件 → 安装 sidecar 插件**。或通过 API（需管理员权限）：

```bash
curl -sX POST http://localhost:8000/api/v1/modules \
  -H 'authorization: Bearer <ADMIN_API_KEY>' \
  -H 'content-type: application/json' \
  -d @manifest.json
```

响应会**一次性**返回用于配置 sidecar 的凭据：

```json
{
  "module": { "id": "visitor-portal", "base_url": "http://127.0.0.1:9123", ... },
  "api_key": "vok_…",          // -> your HELDAR_API_KEY (calls to the kernel API)
  "webhook_secret": "whsec_…"  // -> your HELDAR_WEBHOOK_SECRET (verify deliveries)
}
```

请立即保存这两项；它们不会再次显示。卸载时使用
`DELETE /api/v1/modules/{id}`（或点击**卸载**按钮）。

## 接收事件

内核会将每个订阅事件以 `POST` 请求的形式投递至 `{base_url}/heldar/events`，并附带以下请求头：

- `X-Heldar-Event` — 事件类型
- `X-Heldar-Delivery` — 唯一投递 ID
- `X-Heldar-Timestamp` — Unix 时间戳（秒）
- `X-Heldar-Signature` — `sha256=<hex HMAC-SHA256(webhook_secret, raw_body)>`

**请务必对原始请求字节验证签名**：

```python
import hashlib, hmac
def verify(raw: bytes, header: str, secret: str) -> bool:
    expected = "sha256=" + hmac.new(secret.encode(), raw, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

返回 `2xx` 表示确认收到。非 2xx 响应（或超时）会由至少一次投递引擎重试，因此请确保你的处理器对 `X-Heldar-Delivery` 保持幂等性。

## 回调内核

使用生成的密钥作为 Bearer token，调用你的角色所允许的任意内核 API：

```bash
curl http://localhost:8000/api/v1/events \
  -H "authorization: Bearer $HELDAR_API_KEY"
```

`integration` 密钥还可通过 POST 将检测结果写入采集管道；`viewer` 密钥为只读。

## 安全模型

- 插件由**管理员安装**，**进程外运行**——请像对待任何服务一样对其进行隔离（容器、网络策略，仅在信任链路时才使用非回环地址的 `base_url`）。
- 控制台**不会将你的 session cookie 转发给 sidecar**；sidecar 仅使用自身生成的密钥向内核进行身份验证。
- 插件 UI iframe 已沙箱化，且**不含 `allow-same-origin`**（`allow-scripts allow-forms`），因此运行在不透明源中：无法访问控制台的 DOM 或存储，其请求也不携带会话 Cookie。插件须通过宿主桥接调用自身后端，该桥接会将每个请求限制在该插件自己的 `/m/{id}/` 根路径内。
- 卸载会完全撤销密钥和订阅，因此已移除的插件不再保留任何持久访问权限。
