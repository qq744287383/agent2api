# AGENTS.md

> **涉及分支、发版、版本号的操作，一律按 Skill `release-flow` 执行：**
> **`.agents/skills/release-flow/SKILL.md`** —— 动手前先读它，细则一律以该文件为准，本文件只作入口指路。
>
> 覆盖范围：切分支 / 给 PR 选 base / 合并 PR / 回复 Issue、PR / 改版本号 / 写更新日志 /
> 打 tag / 挂 GitHub Release / 发 beta 预发布版 / 发版验收 / 排查发版失败。

## 两条红线（先问人，不得自行推进）

1. **发版**：改版本号、推 `main` / `beta`、打 tag、挂 Release 之前，先把版本号与「这次要发什么」讲清楚，等用户确认；**未经确认不得推任何分支或 tag** —— tag 一旦推出去就会触发构建、产出公开的安装包与镜像，收不回来。
2. **更新日志**：写好的日志（= 发布提交信息 = Release 正文）必须先给用户逐条过目；按反馈改完**再确认一次**才能提交。

其余细则（分支落脚点、署名与 AI 落款、五处版本号、正式版 / beta 完整流程、验收清单、已知坑与排查）见上述 Skill。

## 文档落点

安装 / 部署 / 升级 / 排错说明只在 `docs/install.md`（另有 5 份译文 `docs/install.{en,zh-Hant,ja,ko,pt-BR}.md`，六份一一对应）里维护；各语言 README 只留「快速开始」速查表 + 一个指向安装文档的链接，**不要再把安装细节写回 README**。产物名、端口、环境变量、安装脚本参数有改动时，六份一起改。

## 项目速览

Agent2API：把多家 AI 桌面客户端里已登录的账号接成一套本地 OpenAI 兼容接口（`/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1/models`），默认端口 `3065`（面板与网关同端口）。多账号轮转、凭证续期、出网代理、请求报表。

提供商按 **`ProviderKind` 变体**建模（`core/providers/mod.rs`，25 个）：同一家的国内 / 国际版、Cline 的 Free / Pass 各自是独立 provider（`provider_id` 是落盘契约，不能改名），需要哪家的行为就看 `core/providers/<名字>/`。新接一家 = 加变体 + 加目录 + 在 `core/providers/adapter.rs` 的 `adapter_for` 登记。

| 位置 | 是什么 |
|---|---|
| `desktop-tauri/src-tauri/server/` | **网关本体**，独立 crate `agent2api-server`。桌面端进程内链接它，Docker / 服务端用 headless 二进制 `agent2api-server` 链接同一个 lib；改网关逻辑看这里（`api/` 路由、`core/providers/` 各家适配、`db/` 存储） |
| `desktop-tauri/src-tauri/src/` | Tauri 2 桌面壳：托盘、单实例、自启、`bridge` / `gateway` / 命令层 |
| `desktop-tauri/ui/` | 管理面板（无框架原生 JS + `css/`），Tauri 的 `frontendDist` 指向它；`ui/islands/` 与 `ui/m/` 是 React 产物落点 |
| `desktop-tauri/{ui-kit,ui-islands,mobile}/` | React 源码包（Base UI + Tailwind v4），各自带 lock 文件 |

存储是单文件 SQLite `agent2api.db`（`PRAGMA user_version` 逐版本升级）；数据目录默认 `~/.agent2api/`，可用 `AGENT2API_PROXY_HOME` 覆盖；凭证明文落盘。

**前端产物入库，且 CI 会逐字节比对**：动了 `ui-islands/src/**`、`i18n/i18n-src/**`、`mobile/src/**` 后必须重新构建并提交产物（`npm run ui:islands`、`npm run mobile:build`、`npm --prefix desktop-tauri/i18n run build`），否则 `frontend-artifacts.yml` 直接红。

本机首次开工：`npm run tauri:install`，按需再 `npm --prefix desktop-tauri/{ui-kit,ui-islands,mobile} ci`。Rust ≥ 1.88、Node ≥ 18.17（当前 crate 名仍叫 `workbuddy-proxy-desktop`，是历史命名）。

## Agent skills

### Issue tracker

Issues and specs live as GitHub issues in `qq744287383/agent2api`, driven by the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default triage roles (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`) plus the `wayfinder:*` label set. See `docs/agents/triage-labels.md`.

### Domain docs

single-context. See `docs/agents/domain.md`.
