---
id: wasm-plugins
title: Wasm 插件
sidebar_label: Wasm 插件
sidebar_position: 4
---

# Wasm 插件

**Wasm 插件**是一种沙箱化的无界面 [`DetectionConsumer`](./build-a-module.md)：内核将检测批次持久化后，会将其（以 JSON 形式）传递给插件，插件在 WebAssembly 沙箱中运行，**零环境权限**——无文件系统、无网络、无时钟、无随机数——并将派生事件回传。发出的事件具有命名空间、摄像头作用域、数量上限，并通过内核的正常事件路径持久化，因此可免费流转至 webhooks 和 sidecars。

这是用于摄取热路径上**轻量、强沙箱、进程内规则/转换逻辑**的专用工具。凡涉及 UI、多语言或繁重/有状态工作的场景，请改用 [sidecar 插件](./sidecar-plugins.md)——sidecar 拥有网络和作用域密钥，而 Wasm 客户机两者皆无。

## 适用场景

| | Sidecar（阶段 B） | Wasm 插件（阶段 D） |
| --- | --- | --- |
| 进程 | 独立进程（任意语言） | 进程内（沙箱） |
| UI | 有（iframe 位于 `/m/{id}/`） | 无（无界面） |
| 能力 | 作用域 API 密钥 + 网络 | **无**——纯计算 |
| 最适合 | 应用、UI、集成 | 规则、过滤、派生事件 |

运行时（[wasmi](https://github.com/wasmi-labs/wasmi)，纯 Rust 解释器）隐藏在**默认关闭的 `wasm` cargo feature** 后面——默认设备二进制文件从不链接它。使用 `--features wasm` 构建服务器以启用插件加载。

## 插件结构

客户机是一个 `wasm32-unknown-unknown` 核心模块。它导出一小套 ABI，并且只能导入两个宿主函数（`heldar.log`、`heldar.emit_event`）——导入其他任何内容（例如 WASI）均会导致加载失败，因此沙箱在构造上就是封闭的。完整的可直接复制的模板位于 [`examples/wasm-plugin`](https://github.com/Straits-AI/heldar/tree/main/examples/wasm-plugin)；唯一需要修改的部分是 `rule()` 函数：

```rust
fn rule(input: &Input) {
    let threshold = input.config.get("threshold").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
    let persons = input.detections.iter().filter(|d| d.label.as_deref() == Some("person")).count();
    if persons > threshold {
        emit(&Event {
            event_type: "occupancy.high".into(),
            severity: "warning".into(),
            payload: json!({ "persons": persons }),
        });
    }
}
```

宿主在加载时调用一次 `heldar_describe()` 以读取 `{ id, name, version, publisher, description, interested_in }`，然后对每批数据调用 `heldar_handle(ptr, len)`（JSON 输入写入客户机内存）。客户机通过 `emit_event` 传递的事件会被缓冲，并在调用结束后以 `wasm.{plugin_id}.{event_type}` 的形式持久化，**始终限定于该批次对应的摄像头**（客户机无法伪造其他摄像头的事件），严重级别被限制为 `info`/`warning`/`critical`。

## 构建与加载

```bash
# 1. build the guest to wasm32 (the example)
cd examples/wasm-plugin
cargo build --release --target wasm32-unknown-unknown

# 2. drop it into the plugins directory
cp target/wasm32-unknown-unknown/release/heldar_occupancy_plugin.wasm \
   <data>/wasm-plugins/occupancy.wasm

# 3. run the server with the wasm feature
cargo run -p heldar-server --features wasm
```

已加载的插件会出现在 `GET /api/v1/modules` 中（挂载类型为 `headless`，无导航路由），并在**插件**商店中以*沙箱计算*方式展示。v1 在启动时加载；更换插件需要重启服务器。

## 沙箱与限制

每个客户机以硬性约束运行，通过环境变量配置（由插件宿主读取）：

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `HELDAR_WASM_ENABLED` | `true` | 主开关（在启用 `wasm` feature 的前提下） |
| `HELDAR_WASM_PLUGINS_DIR` | `<data>/wasm-plugins` | `*.wasm` 的加载目录 |
| `HELDAR_WASM_FUEL` | `50000000` | 每次调用的指令预算（CPU DoS 防护——无限循环会触发陷阱） |
| `HELDAR_WASM_MAX_MEMORY_MB` | `64` | 每次调用的线性内存上限 |
| `HELDAR_WASM_MAX_TABLE_ELEMENTS` | `100000` | 每次调用的表元素上限（表使用宿主 RAM，不计入内存上限） |
| `HELDAR_WASM_MAX_EVENTS` | `64` | 客户机每次调用可发出的事件数上限 |
| `HELDAR_WASM_MAX_EVENT_BYTES` | `16384` | 每个事件的字节上限 |
| `HELDAR_WASM_MAX_LOG_CALLS` | `256` | 每批次的 `heldar.log` 调用次数上限（防止日志洪泛） |
| `HELDAR_WASM_MAX_FAILURES` | `5` | 连续失败次数超过此值后插件将被自动禁用 |

客户机陷阱、燃料耗尽、内存不足或 panic 均被隔离——将被记录日志，绝不会导致内核崩溃；反复失败的插件会被熔断（禁用并生成 `wasm_plugin_disabled` 事件）。客户机在 `spawn_blocking` 上运行，因此 Wasm CPU 计算永远不会阻塞异步反应器。

## 信任与作用域

v1 从**本地、由运维人员控制的目录**加载插件（运维人员信任级别）。内核永远不会下载或执行远程 `.wasm`，v1 也尚无逐制品签名机制——[组件模型](https://component-model.bytecodealliance.org/)、WASI、宿主提供的状态以及多语言 SDK，均是 v1 有意不支持的目标。若日后需要运行不受信任的第三方 Wasm，升级路径是在相同接缝后使用 [wasmtime](https://wasmtime.dev/) 运行时（时代中断 + 更强化的沙箱）。
