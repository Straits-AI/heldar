---
id: build-a-module
title: 构建模块
sidebar_label: 构建模块
sidebar_position: 1
---

# 构建模块

模块通过自有的数据表、路由、界面和感知逻辑来扩展 Heldar。**构建模块有两种方式**，选择哪种取决于集成深度和所用编程语言：

- **编译内嵌应用 crate（本指南）。** 一个依赖 `heldar-kernel` 的 Rust crate，由组合二进制文件链接。它共享内核的 SQLite 连接池，并接入 `DetectionConsumer` 摄入接缝——集成最紧密，适用于第一方应用。开源的 `heldar-entry`/`movement`/`search` crate 均采用此方式构建。
- **进程外 sidecar 插件**（[另见独立指南](./sidecar-plugins.md)）。任意语言实现的 HTTP 服务，由 Heldar 反向代理并推送事件，**在运行时安装，无需重新编译**。进程/容器隔离，最小权限。适用于第三方及自制插件，也是**插件**页面所安装的方式。

本页其余内容介绍进程内路径。应用添加数据表、路由和感知逻辑，内核无需感知其存在。你依赖 `heldar-kernel`；组合二进制文件将你链接进来。（其仪表板页面仍以运行时加载的 bundle 单独分发——详见步骤 6。）

整个过程以开源访问控制应用
[`heldar-entry`](https://github.com/Straits-AI/heldar/tree/main/crates/heldar-entry)
作为参考实现。这是以下每个步骤真实可编译的示例。进程内应用还需声明 `manifest()`，以便在仪表板导航中显示（参见 sidecar 指南的"Manifest"章节——结构相同；进程内模块只需从代码中返回，而非在运行时注册）。

## 设计理念

内核**不依赖你的 crate**。依赖关系单向：你的 crate 依赖 `heldar-kernel`，组合服务器
（[`heldar-server`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-server/src/main.rs)）
同时依赖两者并将其串联。你通过四个公开接缝接入：
一个 `DetectionConsumer`、一个 `Router<AppState>`、自安装的 schema，以及
认证原语。添加你的应用只需在服务器的几个组合点做少量推入，
无需修改内核的摄入处理器或路由器。

## 1. 新建一个依赖内核的 crate

创建一个库 crate 并依赖 `heldar-kernel`：

```toml
# crates/heldar-dwell/Cargo.toml
[package]
name = "heldar-dwell"
version = "0.1.0"
edition = "2021"

[dependencies]
heldar-kernel = "0.1"          # or a path/git dep during local development
axum = "0.8"
sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio"] }
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
```

你的 `lib.rs` 暴露服务器将调用的模块：一个 consumer、一个 schema 初始化器、一个路由器，以及可选的配置。参照
[`heldar-entry/src/lib.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/lib.rs)。

## 2. 实现 `DetectionConsumer`

一批 worker 检测结果持久化后，内核会将其扇出到每个
`interested_in(task_type)` 返回 `true` 的已注册 consumer。以下是来自 `heldar_kernel::services::consumer` 的 trait：

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

一个按摄像头统计检测数量的最简 consumer：

```rust
use std::sync::Arc;
use sqlx::SqlitePool;
use heldar_kernel::services::consumer::{DetectionBatch, DetectionConsumer};

pub struct DwellCounter {
    pool: SqlitePool,
}

impl DwellCounter {
    pub fn new(pool: SqlitePool) -> Arc<Self> {
        Arc::new(Self { pool })
    }
}

#[async_trait::async_trait]
impl DetectionConsumer for DwellCounter {
    fn name(&self) -> &'static str {
        "dwell_counter"
    }

    // Self-select on task_type. Return true for all types only if you are
    // genuinely task-agnostic. heldar-entry returns true only for "anpr".
    fn interested_in(&self, task_type: &str) -> bool {
        task_type.eq_ignore_ascii_case("detection")
    }

    async fn consume(&self, batch: &DetectionBatch<'_>) {
        // batch.camera_id / .site_id / .task_type / .timestamp are the context;
        // each det carries label, confidence, bbox ([x,y,w,h] 0..1), track_id,
        // and a free-form attributes blob. Write to YOUR tables on self.pool.
        for det in batch.detections {
            let _ = sqlx::query(
                "INSERT INTO dwell_counts (camera_id, label, ts)
                 VALUES (?, ?, ?)",
            )
            .bind(batch.camera_id)
            .bind(det.label.as_deref())
            .bind(batch.timestamp)
            .execute(&self.pool)
            .await;
        }
    }
}
```

`consume` 不得 panic；错误由你自行记录或忽略。内核在批次提交后同步调用它，
因此请保持操作轻量，或将工作推入队列或你自己的后台循环。完整的 consumer 实现示例请参见
[`heldar-entry/src/anpr.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/anpr.rs)
中的 ANPR 引擎（`impl DetectionConsumer for AnprEngine`）。

## 3. 幂等地自安装 schema

你的应用拥有自己的数据表。在启动时将其应用到共享连接池，与
[`heldar-entry/src/schema.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/schema.rs)
完全一致：

```rust
// src/schema.rs
use sqlx::SqlitePool;

pub async fn init(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("schema.sql")).execute(pool).await?;
    Ok(())
}
```

```sql
-- src/schema.sql
CREATE TABLE IF NOT EXISTS dwell_counts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    camera_id  TEXT NOT NULL,
    label      TEXT,
    ts         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dwell_cam_ts ON dwell_counts (camera_id, ts);
```

使用 `CREATE TABLE IF NOT EXISTS` 使 `init` 具备幂等性（每次部署单租户）。内核不会定义你的业务表；你共享其 `SqlitePool`，但拥有自己的 schema。

## 4. 暴露 `Router<AppState>` 并合并

你的处理器运行于内核的 `AppState` 之上，可访问共享连接池（`st.pool`）、内核配置（`st.cfg`）以及录制器/采样器/HTTP 客户端。使用绝对路径 `/api/v1/...`；服务器将你的路由器挂载在根路径。复用认证原语进行鉴权和审计，参照
[`heldar-entry/src/routes.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/routes.rs)：

```rust
// src/routes.rs
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use heldar_kernel::auth::Principal;
use heldar_kernel::error::AppResult;
use heldar_kernel::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/dwell/summary", get(summary))
}

async fn summary(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    // Capability check + audit come from the kernel auth primitive. When
    // HELDAR_AUTH_ENABLED is false this is a no-op (open appliance).
    principal.require(principal.can_view(), "view dwell summary")?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dwell_counts")
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(json!({ "total": total })))
}
```

## 5. 在服务器中注册 consumer 并启动循环

所有内容在组合服务器中汇聚。在
[`heldar-server/src/main.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-server/src/main.rs)
中做四处小改动：

```rust
// (a) apply your schema, after the kernel migrations have run
heldar_dwell::schema::init(&pool).await.context("dwell schema init")?;

// (b) add your consumer to the consumer vector that goes into AppState
let consumers: Arc<Vec<Arc<dyn DetectionConsumer>>> = Arc::new(vec![
    services::zones::ZoneEngine::new(pool.clone(), cfg.clone(), recorder.clone()),
    heldar_entry::anpr::AnprEngine::new(pool.clone(), cfg.clone(), entry_cfg.clone()),
    heldar_dwell::DwellCounter::new(pool.clone()),   // <- your consumer
]);

// (c) merge your router next to the kernel and the other apps
let app = Router::new()
    .merge(routes::api_router())
    .merge(heldar_entry::routes::router())
    .merge(heldar_dwell::routes::router());          // <- your router

// (d) if you have a background loop, supervise it (respawns on panic)
let p = pool.clone();
spawn_supervised("dwell_rollup", move || heldar_dwell::rollup::run(p.clone()));
```

集成至此完成。内核摄入路径在不指名任何 consumer 的情况下将批次扇出；路由器合并对内核路由器不可见；schema 归你所有；`spawn_supervised` 在循环 panic 后 5 秒自动重启。

## 6. 发布仪表板界面（运行时加载）

你的模块仪表板页面**不**编译进 SPA——它是一个运行时加载的 ES bundle，由你的 crate 提供服务，使仪表板镜像保持模块无关（开源版和完整版共用同一个 `heldar-web`）。原因详见
[模块系统 → 运行时加载的模块界面](./module-system.md#runtime-loaded-module-uis)。
步骤与 `heldar-entry`/`search` 一致：

1. **编写页面**，置于 `apps/web/src/modules/{id}/` 下（`page.tsx` 加一个默认导出它的 `entry.tsx`）。正常导入 React，并从 **`@heldar/shell`** 导入 shell 接口——API 客户端、认证、UI 套件、格式化工具（不得从 `../lib/*` 导入）；运行时它会解析到 shell 的共享实例。拥有后端路由的模块使用 SDK 中的 `request` 和 `qs` 调用这些路由。
2. **在 manifest 中指向它：** 在你的 `manifest()` 构建器链中添加 `.with_runtime_ui("/api/v1/modules/{id}/ui/index.js")`。
3. **从你的 crate 路由器提供构建好的 bundle：** `const UI: &str = include_str!("../ui/{id}.js");`，并在 `GET /api/v1/modules/{id}/ui/index.js` 上设置一个带查看者权限的 `serve_ui` 处理器（从 `heldar-search/src/routes.rs` 复制，包括 `Cache-Control: no-cache` 头）。
4. **构建并嵌入**，使用 `make module-bundles`（Vite 将 `react` 和 `@heldar/shell` 作为外部依赖构建库 bundle，然后将其复制到你的 crate 的 `ui/` 目录下）。每次修改界面后重新运行。

无界面模块（无页面）可直接跳过此步骤——省略 `ui_url`，商店将以计算处理方式列出它们，且不提供打开入口。

## 内核不依赖你的 crate

这正是此设计的要点。依赖边为：`heldar-dwell` 依赖 `heldar-kernel`，`heldar-server` 依赖两者。内核不导入你的 crate，也不会为你的 `task_type` 增加分支。这使得开源内核和通用应用可以作为独立 crate 发布，也使专有垂直应用能够以同样的方式在私有构建中组合，而无需修改公开代码。

## 并非所有应用都是 consumer

`heldar-movement` 和 `heldar-search` 基于相同的接缝构建，但不在摄入热路径上：movement 是两个受监督的后台循环（ReID 候选提议器和违规规则引擎），search 是对已存储内核事实的只读查询层。两者仍拥有自己的 schema、暴露路由器，movement 还会启动循环，在服务器中的组合方式与上述完全一致，只是省去了 consumer 注册。根据需要选择合适的模式：使用 consumer 进行逐批解读，使用循环做周期性汇聚，或使用纯路由器进行只读查询。
