---
id: architecture
title: 架构
sidebar_label: 架构
sidebar_position: 1
---

# 架构

Heldar 是一个轻量级 HTTP 控制面（Axum 路由），运行于一组长驻后台服务之上，所有服务共享同一个 SQLite 存储和同一套配置。内核（`heldar-kernel`）**与领域无关**：它管理摄像头、摄取并录制 RTSP、对帧进行采样以供 AI 使用、接受来自工作进程的检测结果、评估空间区域，并提供认证、保留策略和可观测性。它对门禁控制、移动智能或检索一无所知；这些属于应用层。

整个系统由两条规则决定：

- **只有内核与摄像头通信。** 全天候录像机将压缩码流直接复制到磁盘，无需解码。一个受预算限制的采样器是唯一执行解码的组件，它为每台摄像头写入一张当前 JPEG。速度缓慢或缺席的 AI 工作进程永远不会阻塞摄取或录制。
- **内核不依赖任何应用。** 应用依赖内核，并由组合二进制文件链接进来。添加一个应用只需在少数几个组合点推入，永远不需要修改内核。

有关各阶段的完整设计（录像机监督器、索引器、保留清扫器、区域引擎、指标等），请参阅代码仓库中的
[ARCHITECTURE.md](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)。

## 四个公共接缝

应用只通过这些接缝接入内核。它们共同使新应用能够添加表、路由、感知逻辑和授权，而内核无需知晓其名称。

### 1. `DetectionConsumer` 特征

一批工作进程检测结果持久化后，内核摄取路径会将其分发给消费者注册表。消费者自行声明其关心哪些 `task_type`，因此内核永远不需要增加 `if task_type == "..."` 分支。

```rust
pub struct DetectionBatch<'a> {
    pub camera_id: &'a str,
    pub site_id: Option<&'a str>,
    pub task_type: &'a str,
    pub detections: &'a [DetectionIngest],
    pub timestamp: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait DetectionConsumer: Send + Sync {
    fn name(&self) -> &'static str;
    fn interested_in(&self, task_type: &str) -> bool;
    async fn consume(&self, batch: &DetectionBatch<'_>);
}
```

开放的区域引擎（一种空间原语）和开放的门禁控制引擎（车牌授权）都是消费者。区域引擎对任何包含被追踪检测结果的任务类型返回 `true`；门禁控制引擎仅对 `anpr` 返回 `true`。

### 2. `Router<AppState>` 合并

每个应用以绝对路径 `/api/v1/...` 暴露其自身的 Axum `Router<AppState>`。组合服务器将这些路由与内核路由合并；内核路由对它们一无所知。应用处理器通过 `AppState` 访问共享的 SQLite 连接池、内核配置以及录像机/采样器/HTTP 客户端。

### 3. 自安装 Schema

每个应用拥有自己的表，并在启动时以版本化、只追加的迁移形式将其自行安装到共享连接池（`db::run_app_migrations`，按组件记录在 `_heldar_app_migrations` 中）。内核不定义领域表。其模式是运行一个 `schema::init(pool)`，该函数执行应用的 `MIGRATIONS` 数组中的 `migrations/NNNN_*.sql` 文件——演进 schema 时追加新迁移，绝不修改已发布的迁移。

### 4. 认证原语

内核提供一个 `Principal` 提取器，以及 RBAC 能力检查（`can_view`、`can_manage_registry`、`can_operate_gate` 等）和一个审计辅助工具。应用复用这些来进行授权和审计，而不是自行实现，因此单个 `HELDAR_AUTH_ENABLED` 开关管控整个组合界面。

## 部署如何组合

组合服务器（`heldar-server`）是内核与所选应用集合汇聚之处。对于每个应用，它会：应用应用模式、构建应用的检测消费者并将其添加至 `AppState` 中的消费者向量、合并应用路由，并在一个遇到 panic 会自动重启的监督器下派生任何后台循环。开放参考构建仅组合内核加上 Apache-2.0 通用应用；不同的部署在此处链接不同的 crate 集合。

并非每个应用都是 `DetectionConsumer`。某些应用是周期性后台循环，或是对已存储内核事实的只读查询层；它们使用相同的模式 + 路由 + 循环组合，而不位于摄取热路径上。

请参阅[构建模块](../develop/build-a-module.md)以获取编写模块的分步指导，以及[开放核心](./open-core.md)了解开放与专有的边界。
