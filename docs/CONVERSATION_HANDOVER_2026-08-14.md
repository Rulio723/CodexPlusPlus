# Codex++ 对话交接文档

更新时间：2026-08-14（Asia/Shanghai）

## 1. 当前权威状态

- 工作区：`D:/Codex/Codex++`
- 分支：`upgrade-Rulio`
- 本地 HEAD：`08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2`
- 远端：`origin = https://github.com/Rulio723/CodexPlusPlus.git`
- 远端 `origin/upgrade-Rulio` 与本地 HEAD 完全一致。
- Workspace 版本：`1.2.45`
- 上游同步基线：`055302664af09e6e3bb76a00b36d318132094ee3`，tag `v1.2.45`
- 当前兼容的官方 Codex 包：`OpenAI.Codex 26.803.10989.0`

已跟踪文件没有未提交修改。以下目录仍为本地未跟踪运行/构建产物，未提交到 Git：

```text
.codex/
output/
target-console/
```

不得批量删除、清理或提交这些目录。

## 2. 2026-08-14 已完成的 Git 提交与推送

以下五个提交已推送到 `origin/upgrade-Rulio`：

```text
299a2d4 feat: sync relay transport and session compatibility
a15c1d7 feat: sync manager and renderer runtime integrations
1995af3 fix: support Codex 26.803 admin and live official login
a5b7140 test: add Codex runtime smoke checker
08f6ef6 docs: record v1.2.45 migration and release handover
```

推送后核验：

```text
local  = 08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2
remote = 08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2
```

## 3. 当前唯一推荐发布物

发布目录：

```text
D:/Codex/Codex++/dist/windows/release-2026-08-13-v1/
```

安装包：

```text
CodexPlusPlus-1.2.45-official-account-detection-fix-windows-x64-setup.exe
大小：106733546 bytes
SHA-256：CBB3D91A9A53B5B012FB4C5E845357918007F05C156263F806E0ACBBF437ABC2
```

便携包：

```text
CodexPlusPlus-1.2.45-official-account-detection-fix-windows-x64-portable.zip
大小：145053632 bytes
SHA-256：71F4519C02C96FCDD44EB8AC3FA33B6A48084AA71CDF8221160EB8A79347193B
```

验证记录：

```text
D:/Codex/Codex++/output/packaging-2026-08-13-v1/verification-record.txt
```

ZIP 与 staging 均为 837 个文件，缺失 0、哈希不一致 0。安装器只完成构建和验证，没有自动执行。

## 4. 当前关键功能状态

### 官方账号识别

- 当前官方登录只根据实时 `auth.json` 的非敏感结构判断。
- 供应商切换不再创建或消费旧的 `official-accounts-provider-selected.marker`。
- 有效 ChatGPT 登录在供应商切换后仍显示为当前账号。
- 只有 API Key、没有 ChatGPT token 的状态不会显示为官方账号。
- 不读取、输出或提交真实 token、API Key 或 `auth.json` 内容。

### 管理员模式

- 已支持官方 Codex `26.803.10989.0` 中的 `@oai/sky 0.6.6`。
- helper、transport、路径、签名和启动模板继续采用精确契约并 fail closed。
- 管理员 Exec、Terminal、Computer Use 的生产运行时测试已通过。

### 中继、模型与会话

- 按模型上下文窗口继续使用原生 `model_catalog_json`。
- relay transport、模型目录获取、协议代理和 VLM 图片处理已同步。
- 会话导出包含真实 rollout；导入会修复缺失的项目会话索引。
- projectless 新会话采用上游原生流程，不恢复旧拦截状态机。

### 管理器和 Renderer

- 底部 Terminal launcher persisted atom 兼容逻辑保留。
- Dream Skin、插件市场、service tier、模型 patch、会话移动/导出等本地功能保留。
- 新增运行时烟雾检查脚本：
  `D:/Codex/Codex++/tools/codex-runtime-smoke-check.mjs`

## 5. 2026-08-14 最新验证

以下命令已重新执行并通过：

```text
cargo fmt --all -- --check
git diff --check
npm test -- --run
npm run check
npm run vite:build
cargo test --workspace -- --test-threads=1
node --check tools/codex-runtime-smoke-check.mjs
```

主要结果：

- 前端测试：62/62 通过。
- `codex-plus-core`：397 passed，1 ignored。
- `official_accounts`：14 passed。
- manager commands：42 passed。
- launcher：93 passed。
- cdp_bridge：91 passed。
- relay_config：109 passed。
- protocol_proxy：50 passed。
- session_transfer：12 passed。
- 仅有普通 MSVC linker message、Vite chunk size 提示和 LF/CRLF 提示。

## 6. 操作约束

- 中文沟通。
- 不执行 `reset`、`checkout`、`clean` 或批量删除。
- 不读取或输出 `.env`、真实 `auth.json`、token、API Key 或真实 `config.toml` 凭据。
- 不自动运行安装器，不随意停止当前 Codex/Manager 进程。
- 新发布使用新的 release、app-build 和 packaging 目录，不覆盖旧发布。
- 行为变更必须添加正确 seam 的回归测试。
- 除非任务必需，不修改 `Cargo.toml`、`package.json` 或 `.gitignore`。

## 7. 下一任务续接提示

```text
继续处理 D:/Codex/Codex++。当前分支 upgrade-Rulio，本地与 origin/upgrade-Rulio 均位于 08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2。已跟踪文件干净；.codex、output、target-console 是本地未跟踪运行/构建产物，不得批量清理或提交。最新推荐发布物仍是 dist/windows/release-2026-08-13-v1。不要读取或输出凭据，不要自动执行安装器或停止当前 Codex/Manager 进程。
```