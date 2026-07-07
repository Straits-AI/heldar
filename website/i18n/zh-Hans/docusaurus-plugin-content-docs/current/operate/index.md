---
id: operate
title: 运营
sidebar_label: 运营
sidebar_position: 1
slug: /operate
---

# 运营

运行、保护和维护 Heldar 部署。若要首次搭建部署，请从[部署](../getting-started/deploy.md)开始。

详细的运营人员和集成人员指南目前存放在代码仓库中。
以下每个链接均会在 GitHub 上打开对应的仓库内指南：

- [访问控制](https://github.com/Straits-AI/heldar/blob/main/docs/ACCESS-CONTROL.md)
  - 车牌授权入场引擎、车辆/访客/黑名单注册表、
  保安确认/拒绝工作流、RBAC 及报告。
- [移动轨迹](https://github.com/Straits-AI/heldar/blob/main/docs/MOVEMENT.md)
  - 多信号跨摄像头 ReID 候选结果（需人工审核）及禁区
  入侵事件。
- [搜索](https://github.com/Straits-AI/heldar/blob/main/docs/SEARCH.md)
  - 对已存储事件事实的确定性查询，以自然语言规划作为
  唯一可能出错的步骤，并对每个答案提供证明层。
- [可观测性](https://github.com/Straits-AI/heldar/blob/main/docs/OBSERVABILITY.md)
  - 健康/指标/事件 API、Prometheus 指标暴露、告警 Webhook、
  存储监控及录像间隙报告。
- [远程访问](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md)
  - 基于浏览器的 WebRTC 远程查看，由信令 + TURN 中继处理 NAT 穿透，
  媒体流进行端到端加密，并支持可选的自托管网络覆盖层以访问
  位于 CGNAT 后的站点。
- [生产加固](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)
  - 面向公网部署的安全检查清单：必需的认证 + TLS
  Cookie、按账户登录锁定、摄像头凭据静态加密、
  启动失败大声报错的防护机制，以及汇聚 Worker 密钥（包括
  可选的 Cloudflare Turnstile 登录挑战）。
- [容量规划](https://github.com/Straits-AI/heldar/blob/main/docs/sizing.md)
  - 摄像头、存储及 AI 帧预算的容量规划。
- [现场调试](https://github.com/Straits-AI/heldar/blob/main/docs/commissioning-checklist.md)
  - 新站点上线的操作检查清单。

如需了解这些内容背后的架构，请参阅
[ARCHITECTURE.md](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)
及[架构](../concepts/architecture.md)概述。
