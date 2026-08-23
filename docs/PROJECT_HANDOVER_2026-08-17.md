# Codex++ 项目交接文档

更新时间：2026-08-17（Asia/Shanghai）

## 1. 当前权威基线

| 项目 | 当前值 |
| --- | --- |
| 工作区 | `D:/Codex/Codex++` |
| 仓库 | `https://github.com/Rulio723/CodexPlusPlus` |
| 分支 | `upgrade-Rulio` |
| 本地 HEAD | `642f0c1e089ba209128a8dfd52249e3dcc6461cc` |
| 远端 HEAD | `origin/upgrade-Rulio = 642f0c1e089ba209128a8dfd52249e3dcc6461cc` |
| Workspace 版本 | `1.2.47` |
| 上游 | `https://github.com/BigPizzaV3/CodexPlusPlus.git` |
| 本地 upstream/main 引用 | `1f431ae49b57b3055e0e6845ba6156c6b4232b4d`（`v1.2.47-7-g1f431ae`） |
| Dream Skin renderer revision | `24-modern-home-composer-contract` |

当前已跟踪源码没有未提交修改。`.codex/`、`output/` 以及交接/计划文档是本地未跟踪资料，不应与功能提交混在一起。

## 2. 项目目标与已完成范围

本 fork 以 BigPizzaV3/CodexPlusPlus 官方上游源码为基础，已经完成用户原项目主要增强功能的迁移与 Windows 正式打包：

- 管理员统一启动与管理员能力检测。
- 中继延迟测试、官方请求指纹与模型测试。
- 模拟/官方指纹相关配置逻辑。
- 会话导入、导出、删除、撤销删除与项目移动。
- 官方账号登录、账号保险库与官方混合登录。
- 工具、插件市场与插件自动展开兼容。
- Dream Skin / Snow Skin 等皮肤、主题图片及诊断。
- Codex 运行检测、安装前结束进程、恢复程序和 NSIS 安装流程。
- Codex++ 自定义软件图标。
- 去除上游/原仓库中的广告与推广模块。
- Windows x64 安装包，沿用原项目 NSIS 管理员安装流程。
- 原有按模型上下文窗口与 `model_catalog_json` 功能继续保留。

## 3. 关键目录

```text
crates/codex-plus-core/              核心设置、配置、账号、CDP、launcher、皮肤与协议逻辑
crates/codex-plus-data/              会话和持久化数据
apps/codex-plus-manager/             React + TypeScript + Tauri 管理器
apps/codex-plus-launcher/            Codex++ 启动器
apps/codex-plus-admin-shim/          管理员 Exec/App Server/Computer Use shim
apps/codex-plus-terminal-shim/       管理员 PowerShell shim
assets/inject/                       Renderer、皮肤、插件及会话增强注入
scripts/installer/windows/           NSIS、恢复程序与 PowerShell 运行时脚本
tools/                               运行时诊断和辅助工具
docs/                                设计、计划和交接文档
dist/windows/                        当前唯一正式安装包
output/                              本地验证、补丁、回滚和历史构建记录
```

## 4. 官方登录与官方混合模式

### 模式语义

- 纯官方模式使用实时 ChatGPT/Codex 官方登录结构。
- 纯 API 模式使用供应商 API 配置。
- 官方混合模式保留官方 ChatGPT 登录状态，同时为模型请求注入混合 API bearer。
- 供应商切换不应清除有效的官方登录 token，也不应把 API Key 保存到官方账号保险库。
- 账号 UI 和后端命令只返回脱敏摘要，不返回 token、cookie、nonce 或密文正文。

### 关键文件

```text
crates/codex-plus-core/src/official_accounts.rs
crates/codex-plus-core/src/relay_config.rs
apps/codex-plus-manager/src/App.tsx
apps/codex-plus-manager/src-tauri/src/commands.rs
```

### 最新兼容修复

官方新版首页已将项目选择器合并进 composer 底栏。真实 DOM 验证输入为：

```text
homeRoute=true
homePresent=true
hero.visible=true
composer.visible=true
legacySuggestionsPresent=false
visibleCardCount=0
projectButton=null
```

p23 曾错误地把 `projectButton` 作为新版首页硬条件。p24 改为：

- 旧式首页：保留建议卡 `1..=6` 的严格检查。
- 新式首页：要求首页、横幅和 composer 可见，不再要求独立项目按钮。
- 普通会话页不执行首页内容检查。

对应提交：

```text
80dfabb fix: accept modern official mixed home diagnostics
642f0c1 fix: verify current official mixed home DOM
```

## 5. 管理员模式

管理员模式覆盖：

1. Codex/APP Server 高完整性启动。
2. 管理员 Terminal/PowerShell shim。
3. Computer Use 管理员 broker。
4. 安装器检测运行中的 Codex/Codex++ 并安全结束相关进程。
5. 安装失败/管理员恢复使用 `codex-plus-recovery.exe`。

最新运行状态抽样（2026-08-17）：

```text
status=running
debug_port=9229
helper_port=57321
administrator_mode.requested=true
administrator_mode.state=active
```

端口是运行时动态值，后续测试必须从 `latest-status.json` 读取，不要硬编码 helper 端口。

## 6. 会话增强

当前保留并兼容：

- Markdown 和 `.codexbackup` 导出。
- rollout 正文、项目关系、资源和索引迁移。
- 会话导入后重建缺失索引。
- 会话删除、撤销删除及目录/数据库同步。
- 会话移动到项目或移出项目。
- 新建临时会话不抢先执行删除/移动增强。
- ChatGPT/Codex 更新后侧栏去重及 stale catalog row 过滤。

关键位置：

```text
crates/codex-plus-data/
crates/codex-plus-core/src/session_*.rs
assets/inject/renderer-inject.js
apps/codex-plus-manager/src-tauri/src/commands.rs
```

## 7. 工具与插件

- 中继模式与兼容模式会按设置决定是否启用插件补丁。
- 插件市场保持远端目录和本地目录语义，不强制解锁禁用的安装按钮。
- ChatGPT 远端插件目录认证失败时提供受控回退。
- 插件自动展开只点击明确的“显示更多/Load more”按钮。
- p24 同时修复空候选签名未去重的问题；空闲首页不再每秒产生 `plugin_auto_expand_finished` 日志循环。

回归测试：

```text
injection_script_deduplicates_empty_plugin_auto_expand_scans
```

## 8. Dream Skin / Snow Skin

- 当前 renderer revision：`24-modern-home-composer-contract`。
- Snow Skin 使用新版首页的 soft layout，不强制套用旧建议卡 structured layout。
- 皮肤诊断检查官方应用身份、皮肤标记、注入版本、样式、装饰层、侧栏、输入框、首页和溢出。
- 实机回归测试：

```text
live_apply_keeps_the_running_renderer_available
live_official_mixed_home_passes_skin_verification
```

关键文件：

```text
crates/codex-plus-core/src/dream_skin_runtime.rs
crates/codex-plus-core/src/assets.rs
crates/codex-plus-core/tests/dream_skin_runtime.rs
assets/inject/upstream/snow-skin/
apps/codex-plus-manager/src/dream-skin.test.ts
```

## 9. Relay、指纹与模型

- 延迟测试与模型测试复用实际供应商认证和 transport 语义。
- 支持 Responses、Chat Completions、Audio 和 Models 路由。
- 支持官方请求指纹、HTTP/TLS/HTTP2 transport 配置。
- aggregate relay 支持请求内故障转移。
- 自定义模型、VLM 图片和 tool/apply_patch 消息继续兼容。
- 按模型上下文窗口通过 Codex 原生 `model_catalog_json` 注入，不覆盖用户外部 catalog。

## 10. Windows 安装与发布

安装器保持原项目 NSIS 流程：

- `RequestExecutionLevel admin`。
- 安装前检测并结束 Codex/Codex++ 进程。
- 安装 launcher、manager、admin shim、terminal shim。
- 包含轻量管理员终端兼容 shim 和恢复程序；完整 PowerShell 7 runtime 不再随包分发，安装时在本机 PowerShell 7 与 Windows PowerShell 5.1 之间选择。
- 使用 Codex++ 自定义图标。
- 安装和卸载均包含安全恢复文件处理。

### 当前唯一正式构建

```text
D:/Codex/Codex++/dist/windows/CodexPlusPlus-1.2.47-windows-x64-setup.exe
大小：106799865 bytes
SHA-256：0B2E8DC38D08EC985B6DAC555FF00552845BC17C88F72BD2B17AFF6022746CCB
PE：MZ
```

2026-08-17 已将 `dist` 内 174 个旧安装包、旧 staging、诊断和测试产物逐项移入回收站。当前 `dist` 只保留上述安装包；清理记录：

```text
D:/Codex/Codex++/output/dist-cleanup-2026-08-17.txt
```

最新打包验证资料：

```text
D:/Codex/Codex++/output/packaging-2026-08-16-v2/verification-record.md
D:/Codex/Codex++/output/packaging-2026-08-16-v2/official-mixed-home-real-dom-fix.patch
D:/Codex/Codex++/output/packaging-2026-08-16-v2/rollback-official-mixed-home-real-dom-fix.ps1
```

## 11. 最新验证结果

最新正式构建前确认：

```text
npm test                         71 passed
npm run check                    exit 0
dream_skin_runtime               14 passed, 2 ignored
cdp_bridge                       102 passed
live_apply...                    1 passed
live_official_mixed_home...      1 passed
cargo fmt --all -- --check       exit 0
git diff --check                 exit 0
npm run vite:build               exit 0
cargo build --release --workspace exit 0
NSIS                             exit 0
```

Vite 仅有 chunk size 提示；Rust 仅有既存 unused variable/linker message；未发现阻断发布的问题。

## 12. 关键提交

```text
642f0c1 fix: verify current official mixed home DOM
80dfabb fix: accept modern official mixed home diagnostics
0392bd2 release: finalize CodexPlusPlus Windows feature port
2bc7a1a feat: complete official CodexPlusPlus feature port
aba83a5 feat: port installer recovery and process shutdown
04b55b9 build: apply custom Codex++ icon
0ba933a refactor: remove promotional manager content
d613a4b refactor: remove advertising subsystem
4921e67 feat: port relay latency and official fingerprint
dfb9bda build: package administrator mode runtime
94f32eb fix: make administrator lifecycle cancellation-safe
5511533 feat: launch Codex with unified administrator mode
```

## 13. 后续维护建议

1. ChatGPT/Codex 更新后，优先运行真实 CDP 首页验证，不要只依赖静态 fixture。
2. 首页检测应依赖稳定语义：可见首页、横幅、composer；避免依赖 hash class 或独立项目按钮。
3. 管理员、会话、工具/插件和皮肤需要分别建立运行态检查，不把单个诊断红项解释为整体启动失败。
4. 新构建先保存旧安装包哈希和回滚，再生成新包；不要自动运行会关闭当前对话的安装器。
5. `dist` 继续只保留一份最新正式安装包；验证与回滚资料放在 `output/`。
6. 推送前运行对应回归测试、格式检查和 `git diff --check`。

## 14. 操作与安全约束

- 中文沟通；代码和测试名称可用英文。
- 不输出 `.env`、token、API Key、cookie、auth 内容或真实凭据。
- 不把 `.codex/`、`output/` 和运行状态文件加入功能提交。
- 不执行 `git reset`、`git clean` 或未经确认的批量删除。
- 删除前验证绝对路径；旧构建优先移入回收站。
- 不自动运行安装器或无故停止承载当前任务的 Codex/Manager。
- 不回退已经迁移完成的管理员、会话、插件、皮肤、登录和安装功能。

## 15. 2026-08-22 Windows 安装器最新记忆（覆盖本文旧发布信息）

Windows 安装器已改为由 High Integrity recovery 循环强制结束占用进程：原生 `TerminateProcess` 失败后使用绝对路径 `taskkill.exe /PID <PID> /F`，捕获重启后的新 PID，并在连续两轮无目标后才覆盖安装文件。

当前已验证安装包：

```text
D:/Codex/Codex++/dist/windows/release-2026-08-22-force-stop-loop-tested/CodexPlusPlus-1.2.50-windows-x64-setup-force-stop-loop-tested.exe
SHA-256: FB273709C4DD3AED257E6D51A03FC368C4E235F7F109A37B68CD21CE74421B44
```

完整实现、测试门槛和后续合并规则以下列文件为准：

```text
docs/PROJECT_MEMORY_2026-08-22.md
```
