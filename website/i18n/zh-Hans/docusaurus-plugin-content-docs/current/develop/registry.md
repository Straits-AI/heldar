---
id: registry
title: 插件注册表
sidebar_label: 插件注册表
sidebar_position: 3
---

# 插件注册表

**插件商店**浏览可用插件的*目录*，并将其与已加载的插件进行交叉对比。目录来自两类来源：

- **内置**的第一方目录，编译进二进制文件中——始终可用，即使离线也能使用，且由构建过程保证可信；
- 可选的**远程**注册表（由管理员配置的 URL 上的已签名 JSON 文档）——这是专有版和社区版插件货架的填充方式，无需将任何内容烧入二进制文件。

安装 sidecar 条目须经过 [sidecar 注册流程](./sidecar-plugins.md)；目录仅负责发现。进程内模块显示为 *Included* / *Contact*——它们在构建时链接到内核，无法在运行时安装。

## 目录格式（`heldar-catalog/v1`）

```json
{
  "format": "heldar-catalog/v1",
  "name": "Acme Registry",
  "issued_at": "2026-06-16T00:00:00Z",
  "expires_at": "2026-12-16T00:00:00Z",
  "entries": [
    {
      "id": "weather-overlay",
      "name": "Weather Overlay",
      "publisher": "Acme Plugins",
      "kind": "community",
      "summary": "Overlay local weather on the wall.",
      "description": "Longer copy shown in the detail drawer.",
      "version": "1.0.0",
      "icon": "module",
      "homepage": "https://example.com/weather-overlay",
      "categories": ["overlay"],
      "install": {
        "type": "sidecar",
        "image": "ghcr.io/acme/weather-overlay:1.0.0",
        "default_base_url": "http://127.0.0.1:9300",
        "subscribes": ["*"],
        "role": "viewer"
      }
    }
  ]
}
```

| 字段 | 含义 |
| --- | --- |
| `kind` | `core` / `proprietary` / `community`——决定插件所在货架及徽章样式。 |
| `install.type` | `sidecar`（运行时可安装，预填注册表单）或 `builtin`（已编译；仅提供行动按钮）。 |
| `install.default_base_url` | 仅限 sidecar：安装表单预填的 URL（运营商可编辑）。 |
| `install.subscribes` / `role` | 仅限 sidecar：要接收的事件类型 + 生成密钥的角色。 |
| `install.image` | 仅限 sidecar：信息性的部署提示——内核不会拉取或运行它。 |
| `install.availability` / `contact` | 仅限 builtin：`open` / `commercial` + 行动按钮的联系方式。 |

仪表板将每个条目与实时状态交叉对比，并呈现以下之一：**Available**、**Installed**、**Included**、**Not in build**、**Unreachable**。

## 信任模型

只有当远程目录的 **Ed25519 分离签名**能通过**固定公钥**验证时，该目录才被信任。签名覆盖*精确*的目录字节（无 JSON 规范化），与 webhook 签名者的机制一致。

- `<catalog-url>.sig` 工件位于目录旁边：`{ "alg": "ed25519", "key_id": "...", "signature": "<base64 raw 64-byte sig>" }`。
- 验证在**服务端**进行，对照编译时固定的密钥以及 `HELDAR_REGISTRY_TRUSTED_KEYS` 中的任何运营商密钥。浏览器既不见密钥，也不做验证——因此伪造的目录永远无法显示虚假的 **Verified** 徽章。
- 系统采用**失败关闭**原则：未经验证的远程来源贡献**零**条目（设置 `HELDAR_REGISTRY_ALLOW_UNVERIFIED=true` 可对受信任的内部注册表放宽此限制）。
- 内置目录由构建过程保证可信（它*本身就是*二进制文件），因此其条目始终经过验证——即使离线，徽章也是真实的。

**Verified** 徽章意味着*该列表已由固定的发布者密钥签名*——并不代表插件代码是安全的。sidecar 仍以最小权限密钥在进程外运行。

## 签名与发布

```bash
openssl genpkey -algorithm ed25519 -out registry.pem            # once; keep the private key secret
openssl pkey -in registry.pem -pubout -outform DER | tail -c 32 | base64   # the pinnable public key
./scripts/sign-catalog.sh catalog.json registry.pem my-key      # -> catalog.json.sig
```

通过 HTTPS 托管 `catalog.json` + `catalog.json.sig`，固定公钥（`HELDAR_REGISTRY_TRUSTED_KEYS=my-key:<base64>`），并将 `HELDAR_REGISTRY_URLS` 设置为目录 URL。可运行的端到端示例位于
[`examples/registry`](https://github.com/Straits-AI/heldar/tree/main/examples/registry)。

## 配置

| 环境变量 | 默认值 | 用途 |
| --- | --- | --- |
| `HELDAR_REGISTRY_ENABLED` | `true` | 远程注册表拉取的主开关（内置目录始终加载）。 |
| `HELDAR_REGISTRY_URLS` | *(空)* | 以逗号分隔的目录 URL。为空表示不进行任何外部访问。 |
| `HELDAR_REGISTRY_REFRESH_S` | `900` | 后台刷新周期（秒）。 |
| `HELDAR_REGISTRY_FETCH_TIMEOUT_S` | `10` | 单次拉取超时（秒）。 |
| `HELDAR_REGISTRY_TRUSTED_KEYS` | *(空)* | 额外固定的密钥，格式为 `key_id:base64pubkey,...`。 |
| `HELDAR_REGISTRY_ALLOW_UNVERIFIED` | `false` | 显示未经验证的远程条目（标注为未验证）。 |
| `HELDAR_REGISTRY_ALLOW_PRIVATE` | `false` | 允许使用 http 或私有/回环地址的注册表 URL（SSRF 防护）。 |

远程拉取使用专用客户端，禁用了重定向，默认仅允许 HTTPS，请求体上限为 2 MiB，并对私有/回环 IP 进行字面拒绝。主机名到私有 IP 的重绑定攻击不在 v1 的防护范围内（URL 由管理员配置，且重定向已关闭）。
