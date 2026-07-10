# Codex++

<p align="center">
  <img src="docs/images/codex-plus-plus.png" alt="Codex++ 图标" width="160">
</p>

<p align="center">
  中文 | <a href="README_EN.md">English</a>
</p>

<p align="center">
  <img alt="Release" src="https://img.shields.io/github/v/release/BigPizzaV3/CodexPlusPlus">
  <img alt="Stars" src="https://img.shields.io/github/stars/BigPizzaV3/CodexPlusPlus">
  <img alt="License" src="https://img.shields.io/github/license/BigPizzaV3/CodexPlusPlus">
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange">
  <img alt="Tauri" src="https://img.shields.io/badge/tauri-2.x-24C8DB">
</p>

Codex++ 是面向 OpenAI Codex / ChatGPT 桌面应用的外部启动器与管理工具。它通过 Chromium DevTools Protocol 和本地辅助服务提供供应商切换、协议转换、会话管理与界面增强，不修改官方应用的 `app.asar`，也不向安装目录写入补丁文件。

## 快速使用

从 [GitHub Releases](https://github.com/BigPizzaV3/CodexPlusPlus/releases) 下载最新版安装包：

- Windows：`CodexPlusPlus-*-windows-x64-setup.exe`
- macOS Intel：`CodexPlusPlus-*-macos-x64.dmg`
- macOS Apple Silicon：`CodexPlusPlus-*-macos-arm64.dmg`

安装后会有两个入口：

- `Codex++`：静默启动官方桌面应用，并加载已保存的供应商配置与增强功能。
- `Codex++ 管理工具`：管理供应商、模型、工具插件、会话、增强功能、脚本、更新和诊断。

首次使用建议先打开管理工具，确认应用路径和运行状态，再配置供应商与增强功能，最后从 `Codex++` 入口启动。Windows 安装包会创建桌面和开始菜单快捷方式；macOS DMG 会安装 `/Applications/Codex++.app` 和 `/Applications/Codex++ 管理工具.app`。

## 交流与支持

欢迎加入 Codex++ 交流 3 群（QQ群：619480492），反馈问题、交流使用体验或提出新功能建议。<a href="https://qm.qq.com/q/Erf1F1zwqs">点击链接加入群聊</a>。

<img src="docs/images/discussion-group-qr.jpg" alt="Codex++ 微信群二维码" width="260">

Telegram 频道：<https://t.me/CodexPlusPlus>

## 主要功能

所有界面增强都可以单独关闭。关闭“Codex 增强”总开关后，Codex++ 仍可作为供应商和启动管理工具使用。

## 供应商模式

Codex++ 将官方登录、混入 API 和纯 API 分开保存和切换：

| 模式 | 用途 | 认证边界 |
| --- | --- | --- |
| 官方登录 | 只使用 ChatGPT / Codex 官方账号 | 清理自定义 provider 和 API Key，保留官方登录状态 |
| 官方登录 + API | 保留官方账号与插件入口，模型请求走兼容 API | API Key 写入 provider bearer token，不写入纯 API 的 `auth.json` |
| 纯 API | 不依赖官方账号，完全使用自定义 Base URL / Key | 独立保存 `config.toml` 与 API Key，不混入官方认证 |
| 聚合供应商 | 在多个普通 API 供应商之间路由 | 支持故障转移、按会话轮转、按请求轮转和权重轮转 |

每个供应商可配置 Responses 或 Chat Completions 协议、模型列表、测试模型、User-Agent、上下文窗口、自动压缩阈值，以及该供应商启用的 MCP Server、Skill 和 Plugin。Chat Completions 可通过本地代理转换为 Codex 使用的 Responses 协议。

每模型窗口支持 `1M`、`200K` 或纯数字。Codex++ 会生成独立 `model_catalog_json`，让 Codex 按当前模型使用对应窗口。

切换供应商时会先保存当前配置，再写入目标配置。真实 API Key 只保存在本机，请勿放入日志、截图或 issue。

## Codex 界面增强

- 会话删除、批量删除、Markdown 导出和项目移动。
- 插件市场解锁、插件自动展开和模型白名单处理。
- 富文本粘贴转纯文本、强制中文、启动加速和原生菜单本地化。
- 会话宽度、滚动位置恢复、线程 ID、服务层级切换和 Goals。
- Stepwise 下一步建议，可单独配置 API、模型、建议数量与超时。
- Upstream worktree、Zed Remote、自定义图片覆盖层和用户脚本。

![Codex++ 后端状态指示灯](docs/images/backend-status-indicator.png)
![Codex++ 设置面板](docs/images/settings-panel.png)

## 中转注入

中转注入适合已经在 Codex/ChatGPT 中完成官方账号登录，同时希望把模型请求转到自定义兼容 API 的场景。

这种混合模式的边界是：

- 官方 ChatGPT/Codex 登录态继续负责 Codex App 的账号能力和插件入口。
- 中转配置只接管模型请求使用的 Base URL、Key 和模型名称。
- 兼容 API 供应商不需要固定为某一家；只要上游协议和 Codex 配置匹配即可。
- 清除 API 模式后应能回到官方登录态，继续使用官方账号和插件。

应用中转注入前建议先做一次最小检查：

1. 先确认 Codex 已检测到 ChatGPT 登录状态，插件入口可用。
2. 确认自定义 Base URL 可访问，并且支持所选上游协议（例如 Responses 兼容接口）。
3. 用目标 Key 做一次最小认证测试，例如模型列表或很短的消息请求。
4. 只记录 Key 是否存在和认证结果，不要把真实 Key 写入日志、截图或 issue。
5. 确认 `~/.codex/config.toml` 已有备份，便于清除 API 模式后回滚。

在管理工具的“中转注入”页面：

1. 确认已经检测到 ChatGPT 登录状态。
2. 添加一个或多个中转配置，填写 Base URL 和 Key。
3. 选择当前配置并应用中转注入。
4. 启动 `Codex++`。

Codex++ 会在 `~/.codex/config.toml` 中写入类似配置：

```toml
model_provider = "CodexPlusPlus"

[model_providers.CodexPlusPlus]
name = "CodexPlusPlus"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://example.com/v1"
experimental_bearer_token = "sk-..."
```

如果需要回到官方登录态，在“中转注入”页面点击清除 API 模式即可移除 `OPENAI_API_KEY` 相关配置并切回官方 ChatGPT 登录模式。

## 增强功能

增强功能在管理工具中统一开关。默认开启增强注入；关闭后不会注入 Codex++ 菜单和脚本。

如果启用中转注入模式，插件市场解锁不再需要，界面会提示“中转注入模式下无需开启”。会话删除、导出、移动、粘贴修复和用户脚本等增强仍可继续使用。

## 自动更新与安装包

Codex++ 通过 GitHub Release 发布安装包。Windows 会生成 NSIS 安装程序，macOS 会生成 Intel x64 和 Apple Silicon arm64 两个 DMG。

Codex++ 不会在启动时自动检查更新。需要更新时，可在管理工具的“关于”页手动检查并启动安装。

## 数据位置

- Codex 配置：`~/.codex/config.toml`
- Codex 登录状态：`~/.codex/auth.json`
- Codex 本地数据库：优先读取 `~/.codex/sqlite/*.db`，旧版回退到 `~/.codex/state_5.sqlite`
- Codex++ 状态与日志：`~/.codex-session-delete/`
- Provider 同步备份：`~/.codex/backups_state/provider-sync`

## 常见问题

### Codex++ 菜单没出现

确认从 `Codex++` 入口启动，而不是直接打开官方应用。然后在管理工具的“安装维护”和“关于”页面检查应用路径、启动状态与诊断日志。

### 切换供应商后请求失败

先在供应商详情中运行模型测试或 Provider Doctor，并确认协议、Base URL、Key 和测试模型匹配。纯 API 与官方混入模式使用不同的认证位置，不要手工复制两种模式的 `auth.json`。

### Upstream worktree 和 Codex 原生创建有什么区别

Codex++ 的 Upstream worktree 功能等价于先更新远端分支，再执行：

```bash
git worktree add -b <new-branch> <worktree-path> upstream/<base-branch>
```

这样新 worktree 从最新的远端跟踪分支开始，而不是从当前会话所在的本地 HEAD 开始。如果 Codex++ 无法安全识别当前 Codex 版本的原生 worktree 创建表单，请从 Codex++ 菜单中手动填写仓库路径、分支名、worktree 路径、remote 和 base branch。

### macOS 提示无法打开或已损坏

当前安装包未签名/未公证时，macOS Gatekeeper 可能拦截，出现“已损坏，无法打开”的提示：

![macOS 提示 Codex++ 管理工具已损坏](docs/images/macos-damaged-warning.png)

如果遇到该提示，可以在终端执行下面两条命令，解除苹果系统的安全隔离限制：

```bash
sudo xattr -rd com.apple.quarantine /Applications/Codex++\ 管理工具.app
sudo xattr -rd com.apple.quarantine /Applications/Codex++.app
```

执行后重新打开 `Codex++` 或 `Codex++ 管理工具` 即可。

### macOS Intel 能用吗

可以。Release 会分别提供 `macos-x64.dmg` 和 `macos-arm64.dmg`。Intel Mac 下载 x64 包，Apple Silicon 下载 arm64 包。

## 开发

```bash
# 前端检查
cd apps/codex-plus-manager
npm ci
npm run check
npm run vite:build

# Rust 检查
cd ../..
cargo fmt --all -- --check
cargo test
cargo build --release
```

主要结构：

```text
apps/
  codex-plus-launcher/          静默启动入口
  codex-plus-manager/           Tauri 管理工具
assets/inject/
  renderer-inject.js            注入到 Codex 渲染端的增强脚本
crates/
  codex-plus-core/              启动、注入、配置、更新、安装、桥接等核心逻辑
  codex-plus-data/              会话数据、导出、Provider 同步
scripts/installer/
  windows/CodexPlusPlus.nsi     Windows NSIS 安装包
  macos/package-dmg.sh          macOS DMG 打包
```

## 开源协议

Copyright (C) 2026 BigPizzaV3

CodexPlusPlus 采用 [GNU Affero General Public License v3.0](LICENSE)，SPDX 标识为 `AGPL-3.0-only`。修改并分发本项目，或通过网络提供修改后的版本时，需要按 AGPLv3 提供对应源代码。

许可证只覆盖 CodexPlusPlus 自身代码，不授予 OpenAI、ChatGPT、Codex 的商标、应用资源或其他第三方内容的权利。

## 兼容性说明

Codex++ 依赖官方桌面应用的页面结构、CDP 和本地数据格式。官方应用更新后，部分注入功能可能需要跟随适配；修改供应商配置或本地会话数据前应保留备份。
