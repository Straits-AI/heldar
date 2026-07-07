---
id: intro
title: 简介
sidebar_label: 简介
sidebar_position: 1
slug: /
---

# Heldar

Heldar 是面向物理空间的视觉事件智能操作系统。它将摄像头流转化为结构化事件，将事件转化为工作流，再将工作流转化为运营智能。Heldar 不依赖现有的 DVR/NVR 进行封装，也不以 AI 功能为起点，而是首先构建自己的**媒体内核**（摄像头注册表、RTSP 接入、录制、回放、实时预览），然后将感知层、区域引擎和应用程序作为*消费者*叠加其上。FFmpeg 和 MediaMTX 负责底层媒体处理；Heldar 拥有元数据模型、事件引擎和产品逻辑。

## 开放核心

Heldar 采用开放核心模式：

- Apache-2.0 授权的**内核**（`heldar-kernel`）以及**通用参考应用**
  （`heldar-entry`、`heldar-movement`、`heldar-search`）、参考组合服务器、参考 AI 工作器和 React 仪表板。这是公开的
  `heldar` 代码仓库。
- **垂直行业/客户产品**作为独立的专有 crate 存放于私有仓库，并依赖于开放的 crate。内核不引用它们。

应用程序仅通过一小组公共接缝接入内核，因此内核对任何应用程序**没有**依赖。一次部署由内核加上客户所需的应用程序*组合*而成（每次部署单租户）。边界详见[开放核心](./concepts/open-core.md)，接缝详见[架构](./concepts/architecture.md)。

## 架构概览

![Heldar open-core architecture](/img/diagrams/architecture.svg)

内核是**唯一**与摄像头通信的组件。全天候录制器保持压缩流免解码运行；一个有预算限制的采样器是唯一进行解码的组件，每个摄像头写入一帧当前画面。AI 工作器是纯 HTTP 客户端：它们拉取采样帧并将检测结果回传。应用程序将这些检测结果解释为领域事件。

## 下一步

- [快速入门](./getting-started/quickstart.md) — 构建、运行、添加摄像头并运行 AI 工作器。
- [部署](./getting-started/deploy.md) — 单一二进制文件，单一 URL（Docker 一行命令、原生二进制或烧录的一体机）。
- [远程访问](./getting-started/remote-access.md) — 通过浏览器、WebRTC 从任何地方查看站点，即使在 CGNAT 环境下也可使用。
- [使用仪表板](./getting-started/dashboard.md) — Web UI 导览：实时预览、回放、区域、事件和系统页面。
- [架构](./concepts/architecture.md) — 内核及其四个公共接缝。
- [开放核心](./concepts/open-core.md) — 哪些是开放的，哪些是专有的。
- [构建模块](./develop/build-a-module.md) — 基于开放内核构建自己的应用程序。
- [构建 AI 工作器](./develop/ai-worker.md) — 感知工作器 SDK 协议。
- [运维](./operate/index.md) — 仓库内的运维人员和集成人员指南。

源代码位于
[github.com/Straits-AI/heldar](https://github.com/Straits-AI/heldar)。
