---
id: hosting-the-docs
title: 托管文档站
sidebar_label: 托管文档站
sidebar_position: 2
---

# 托管文档站

文档站是一个静态 [Docusaurus](https://docusaurus.io/) 构建产物
（位于 `website/` 目录），托管在 **Cloudflare Workers** 上，使用
[Static Assets](https://developers.cloudflare.com/workers/static-assets/)。
Cloudflare [推荐新项目使用 Workers 而非 Pages](https://developers.cloudflare.com/workers/static-assets/migration-guides/migrate-from-pages/)：
"你应该从 Workers 开始……今后，我们所有的投入、优化和功能开发都将专注于改进 Workers。"

这是一个**纯资产 Worker**（无服务端代码）：Cloudflare 直接提供预构建的输出。配置位于 `website/wrangler.jsonc`：

```jsonc
{
  "name": "heldar-docs",
  "compatibility_date": "2026-06-15",
  "assets": {
    "directory": "./build",
    "not_found_handling": "404-page"
  }
}
```

`not_found_handling: "404-page"` 会在访问不存在的页面时返回 Docusaurus 生成的 `404.html`
（仅在 SPA 风格的客户端路由场景下使用 `"single-page-application"`）。
站点从 Worker 域的根路径提供服务，因此 `baseUrl` 保持 `/` 不变。

## 从本机部署（已安装 wrangler）

```bash
cd website
npm ci
npm run build          # -> website/build
wrangler deploy        # uploads ./build as an assets-only Worker
```

部署前的本地预览：

```bash
cd website
npm run build && wrangler dev   # serves the built site on http://localhost:8787
```

首次执行 `wrangler deploy` 会创建 `heldar-docs` Worker，并将其发布到
`https://heldar-docs.<your-subdomain>.workers.dev`。

## 从 CI 部署（可选）

仓库中包含 `.github/workflows/cloudflare-workers.yml`：每次推送到 `main` 分支时，
它会构建站点并通过
[`cloudflare/wrangler-action`](https://github.com/cloudflare/wrangler-action) 执行 `wrangler deploy`。
无论是否存在密钥，构建始终会运行（从而验证构建是否有效），仅在令牌存在时才执行部署。启用方式：

1. 创建一个拥有 **Workers Scripts: Edit** 权限的作用域 **API 令牌**
   （以及令牌 UI 提示的 **Workers R2 / Account** 读取权限）。
2. 在 GitHub 仓库设置中添加两个**仓库密钥**：
   - `CLOUDFLARE_API_TOKEN` — 上述作用域令牌。
   - `CLOUDFLARE_ACCOUNT_ID` — 你的账户 ID（可在 Workers & Pages 概览页或控制台 URL 中找到）。

## 自定义域名

在 Cloudflare 控制台打开 `heldar-docs` Worker，进入 **Settings ->
Domains & Routes -> Add -> Custom Domain**，添加你的域名（例如
`docs.heldar.ai`）。Cloudflare 会自动申请证书。由于 Worker 从域名根路径提供服务，无需修改 `baseUrl`，保持 `/` 即可。
（将 `website/docusaurus.config.ts` 中的 `url` 设置为该域名，以生成正确的规范链接和站点地图 URL。）

## 国际化（i18n）

本站点为多语言站点：**英文为源语言**（`website/docs/`），完整译文位于 `website/i18n/<locale>/` 下——目前包括中文（`zh-Hans`）和西班牙文（`es`）。导航栏的语言切换器可让读者切换语言。`npm run build` 会将所有语言输出到同一目录树（`build/`、`build/zh-Hans/`、`build/es/`），由同一个 Worker 提供服务，因此译文**无需**额外配置即可部署。任何未翻译的页面都会自动回退到英文。

**添加语言：** 在仓库根目录运行 `scripts/i18n-scaffold-locale.sh <locale>`（例如 `fr`、`ja`、`pt-BR`），然后按其打印的步骤操作——在 `docusaurus.config.ts` 中将该语言加入 `i18n.locales` 与 `localeConfigs`，添加 `README.<locale>.md`，并翻译生成的 `i18n/<locale>/**` 文件。译者指引（保真规则 + 目录结构）位于 `website/i18n/TRANSLATING.md`。
