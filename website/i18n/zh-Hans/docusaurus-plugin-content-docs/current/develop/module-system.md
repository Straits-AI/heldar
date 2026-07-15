---
id: module-system
title: 模块系统
sidebar_label: 模块系统
sidebar_position: 1
---

# 模块系统

Heldar 采用组合式架构，而非单体设计：内核不依赖具体业务领域，所有产品功能面——门禁控制、移动侦测、搜索以及你自己的应用——都以**模块**的形式通过一组精简的接口插入。仪表盘的导航和路由在**运行时**根据当前二进制文件所暴露的模块动态构建，因此新增模块永远不会分叉核心代码。

本页是整体导览；各类型的详细指南——[构建模块](./build-a-module.md)、[Sidecar 插件](./sidecar-plugins.md)、[Wasm 插件](./wasm-plugins.md)、[注册表](./registry.md)——请查阅对应文档。

## 三种类型

| 类型 | 进程 | UI（挂载方式） | 无需……即可添加 | 适用场景 |
|---|---|---|---|---|
| **进程内** | 内核链接的 Rust crate | crate 在 `/api/v1/modules/{id}/ui` 提供的 React ES 包，由仪表盘**在运行时**导入（`mount: runtime`） | 重新构建仪表盘 | 需要热路径 + 共享数据库的一方应用（entry、movement、search、verticals） |
| **Sidecar** | 进程外服务（任意语言） | 通过 `/m/{id}/*` 反向代理的沙箱 iframe（`mount: iframe`） | 重新编译内核或仪表盘 | 第三方 / 独立部署的应用 |
| **Wasm** | 进程内，沙箱化（wasmi） | 无界面——无页面（`mount: headless`） | 重新编译内核 | 在检测流上执行不受信任的计算 |

- **进程内**模块通过内核接口注册——包括 `DetectionConsumer`、`Router<AppState>` 合并，以及版本化的自安装 schema（`schema::init`，基于 `db::run_app_migrations`）——并暴露一个携带 `mount: runtime` 与 `ui_url` 的 `manifest()`。其 UI **不会**编译进仪表盘（详见下文）。参见[构建模块](./build-a-module.md)。
- **Sidecar** 插件在运行时通过 `POST /api/v1/modules` 注册：内核为插件生成最小权限 API 密钥和 webhook 订阅，并在 `/m/{id}/*` 下反向代理其 UI 与 API。参见 [Sidecar 插件](./sidecar-plugins.md)。
- **Wasm** 插件从目录加载（需启用默认关闭的 `wasm` feature），作为沙箱化的 `DetectionConsumer` 运行。参见 [Wasm 插件](./wasm-plugins.md)。

## 运行时加载的模块 UI

进程内模块的仪表盘页面**不会**打包进 SPA。每个 crate 将其页面构建为独立的 Vite **library** 包（ES 模块）并通过 `include_str!` 嵌入；内核通过 `GET /api/v1/modules/{id}/ui/index.js` 提供服务（需查看者权限）。仪表盘的 `ModuleHost` 组件读取 manifest 中的 `ui_url`，动态 `import()` 该包，并挂载其默认导出的 React 组件。

该包**不**携带自己的 React 或 UI 组件库。它将 `react` 和 Shell SDK（**`@heldar/shell`**——API 客户端、身份验证/会话、设计系统和格式化工具）作为 *externals* 引入；仪表盘中的 import map 在运行时将它们解析为 Shell 的单一实例。因此模块共享 Shell 的 React 和设计系统，而非各自复制——构建产物体积小（约 10–50 KB），且始终与宿主版本一致。

这样做的意义在于：由于没有任何模块 UI 被编译进仪表盘，SPA 在**开源版与完整版构建中完全相同**。两者共用同一个 `heldar-web` 镜像，开源仓库生成器只需删除专有垂直应用的一个独立目录即可移除其 UI，无需逐文件修改源码。不带页面的模块（如无界面计算插件）直接省略 `ui_url` 即可。

## 单一 manifest，启动与运行时组合

仪表盘从单一端点——**`GET /api/v1/modules`**——渲染模块导航，该端点将三种类型合并为一个列表：

- **启动时**，组合服务器收集进程内模块的 manifest（各自携带 `ui_url`）——在私有构建中，还通过一个在开源版中为空操作的接口收集专有垂直应用——以及所有 Wasm 模块，并存储在应用状态中。
- **运行时**，列表处理器将上述内容与数据库中的 **sidecar** 注册信息合并（每条记录投影为 manifest，附带实时健康状态字段）。

仪表盘**每 30 秒轮询一次 `GET /api/v1/modules`**，因此安装或移除 sidecar 后，导航栏无需刷新或重启即可更新。未知模块图标将回退为通用图形——缺少图标不会导致错误。

## 组合接口

添加一个进程内应用只需在组合服务器中**一处推入**，无需修改内核：

```rust
// crates/heldar-server/src/main.rs (sketch)
let mut modules = vec![
    heldar_entry::manifest(),
    heldar_movement::manifest(),
    heldar_search::manifest(),
];
modules.extend(verticals::manifests());          // proprietary verticals — a no-op stub in the open build
let (wasm_consumers, wasm_modules) = wasm_plugins::load(/* … */);  // no-op when the `wasm` feature is off
```

`verticals` 和 `wasm_plugins` 接口是*可选*代码无需内核引用即可组合的方式：在开源构建中两者均为返回空值的桩函数；私有构建（或 `--features wasm`）则替换为真实实现。`main.rs` 在开源与私有仓库中完全相同——参见[开源内核](../concepts/open-core.md)。

## 健康状态

Sidecar 在 `GET /heldar/health` 上报告健康状态，内核每 30 秒探测一次；应用商店将每个 sidecar 显示为 `healthy` / `unreachable` / `unknown`。[注册表](./registry.md)通过对比进程内（内核链接的）集合与实时注册信息，计算每个目录条目的**货架分类**（core / proprietary / community / compute）和**状态**（`included` / `available` / `installed` / `unreachable` / `not-in-build`），从而准确反映当前二进制文件实际链接的内容以及当前已安装的内容。

## 远程访问下的模块

[远程仪表盘](../getting-started/remote-access.md)通过中继运行完整的 SPA，因此模块可远程使用——各类型有一点细微差别：

- **进程内**模块在 `/api/v1/modules/{id}/ui` 提供 UI 包，并在 `/api/v1/*` 下发起 API 调用——两者均经由中继传输，因此仪表盘无需额外配置即可远程加载和运行它们（这也是**专有**模块在不进入开源镜像的情况下触达远程运维人员的方式）。✅
- **Sidecar** iframe 在 `/m/{id}/*` 处反向代理，中继将其转发至设备（内核随后使用自己生成的密钥访问 sidecar，而非用户的密钥）。✅
- **Wasm** 模块无界面且在进程内运行——无需中继。✅

中继是一条受白名单限制的管道（`/api/v1/*`、`/media/*`、`/m/*`；路径穿越和 Worker 内部路径均被拒绝），设备对每个转发请求都运行其**自身**的 RBAC，因此远程访问不会扩大任何角色的权限范围。
