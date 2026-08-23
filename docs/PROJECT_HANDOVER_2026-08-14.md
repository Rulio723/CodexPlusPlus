# Codex++ 项目交接文档

更新时间：2026-08-14（Asia/Shanghai）

## 1. 项目基线

- 仓库：`D:/Codex/Codex++`
- GitHub：`https://github.com/Rulio723/CodexPlusPlus`
- 分支：`upgrade-Rulio`
- HEAD：`08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2`
- 远端分支：`origin/upgrade-Rulio`，与本地 HEAD 一致
- Workspace 版本：`1.2.45`
- 上游：`BigPizzaV3/CodexPlusPlus`
- 上游同步基线：`055302664af09e6e3bb76a00b36d318132094ee3`（`v1.2.45`）
- 官方 Codex 兼容基线：`OpenAI.Codex 26.803.10989.0`

已跟踪工作树干净。仅保留以下未跟踪本地产物：

```text
.codex/
output/
target-console/
```

这些目录未推送，不应加入提交或批量清理。

## 2. 2026-08-14 提交历史

本轮连续开发成果已按功能域拆分并推送：

| 提交 | 功能域 |
| --- | --- |
| `299a2d4` | relay transport、模型目录、协议代理、VLM 与会话迁移兼容 |
| `a15c1d7` | 管理器 UI、renderer 注入、Dream Skin、更新器与底部面板兼容 |
| `1995af3` | Codex 26.803 管理员 Computer Use 与官方账号实时识别 |
| `a5b7140` | Codex renderer 运行时烟雾检查工具 |
| `08f6ef6` | v1.2.45 迁移、维护说明和交接文档 |

## 3. 项目结构

- `crates/codex-plus-core/`：设置、配置生成、模型目录、协议代理、launcher、CDP、管理员模式和官方账号。
- `crates/codex-plus-data/`：会话存储、备份、导入和供应商同步。
- `apps/codex-plus-manager/`：React/TypeScript/Tauri 管理器。
- `apps/codex-plus-launcher/`：无界面启动器。
- `apps/codex-plus-admin-shim/`：管理员 Exec/App Server/Computer Use shim。
- `apps/codex-plus-terminal-shim/`：管理员 PowerShell shim。
- `assets/inject/`：Codex renderer 注入。
- `scripts/installer/windows/`：Windows 安装和恢复脚本。
- `tools/codex-runtime-smoke-check.mjs`：真实 renderer CDP 烟雾检查。
- `docs/`：设计、上游迁移计划和交接文档。

## 4. 按模型上下文窗口

原始核心功能保持不变：

- 通过模型列表后缀或 `model_windows` 声明每模型上下文窗口。
- 使用 Codex 原生 `model_catalog_json`。
- 只更新本 fork 管理的 catalog，不覆盖用户外部 catalog。
- 保持旧 profile 单值行为。
- 不跨供应商混合模型列表。

关键文件：

```text
crates/codex-plus-core/src/settings.rs
crates/codex-plus-core/src/model_suffix.rs
crates/codex-plus-core/src/model_catalog.rs
crates/codex-plus-core/src/relay_config.rs
apps/codex-plus-manager/src/App.tsx
```

## 5. Relay 与协议兼容

当前实现包含：

- 可配置 HTTP/TLS/HTTP2 transport 与官方 Codex 请求指纹。
- 标准和版本化 models endpoint 探测。
- Responses、Chat Completions、Audio 和 Models 代理。
- aggregate relay 请求内故障转移。
- Codex custom/namespace tool 与 apply_patch 历史转换。
- relay latency 和模型测试使用同一认证及 transport 语义。
- VLM 图片收集支持多轮 user/tool 消息和模型级上下文窗口。

主要提交：`299a2d4`。

## 6. 官方账号保险库

主要位置：

```text
crates/codex-plus-core/src/official_accounts.rs
crates/codex-plus-core/tests/official_accounts.rs
apps/codex-plus-manager/src-tauri/src/commands.rs
apps/codex-plus-manager/src/App.tsx
```

当前规则：

- 凭据以 AES-GCM 加密保存。
- UI 和命令 payload 只返回摘要。
- 当前账号始终从实时官方登录结构判断。
- 不再使用 provider-selected marker 强制隐藏账号。
- 切换账号保留合法混入 API Key，但不在保险库中保存该 Key。
- 失败切换不覆盖 live auth。
- 不输出 token、nonce、ciphertext 或真实 auth 内容。

## 7. 管理员模式

管理员能力包括：

1. Exec/App Server 高完整性路由。
2. 管理员 Terminal/PowerShell shim。
3. Computer Use 认证 broker。

Codex `26.803.10989.0` 的 `@oai/sky 0.6.6` 已加入精确契约：

- helper SHA-256：`BE488E66C38E12FA46850EE48C1F5E44ECDB0A3A64042E064E3A1A1DA286AC42`
- transport SHA-256：`7BC54C5BB7F49661FB1F501C6832F5490620501464D3F1593A361A85C7F66B39`
- Authenticode 签名者：OpenAI OpCo, LLC
- 启动模板：已审查的 `P()` 契约

未知版本、路径、哈希或模板继续 fail closed。

## 8. 会话与 rollout

- 导出和 `.codexbackup` 包含真实 rollout。
- 可处理 rollout 位于 `CODEX_HOME` 外的记录。
- 导入时为有 rollout、缺少索引的项目会话重建索引。
- 缺少 rollout 正文的 manifest 项不会伪造恢复。
- projectless 会话不会被强制绑定项目。
- 多数据库导入失败时会回滚已完成的 provider、项目状态、索引、rollout 和资源写入。

## 9. Manager、Renderer 与 UI

- 采用上游原生 projectless 新会话流程。
- 底部 Terminal launcher persisted atom 自动修复。
- Dream Skin 支持当前主题复制、图像伴随和失败回滚。
- 插件市场、service tier、模型 patch、会话删除/导出/移动功能保留。
- renderer runtime 可使用 `tools/codex-runtime-smoke-check.mjs` 检查 bridge、底部面板、Dream Skin、模型和插件补丁状态。

## 10. 最新验证矩阵

2026-08-14 已通过：

```text
cargo fmt --all -- --check
git diff --check
npm test -- --run
npm run check
npm run vite:build
cargo test --workspace -- --test-threads=1
node --check tools/codex-runtime-smoke-check.mjs
```

主要数量：

- 前端：62 passed。
- core library：397 passed，1 ignored。
- official_accounts：14 passed。
- manager commands：42 passed。
- launcher：93 passed。
- cdp_bridge：91 passed。
- relay_config：109 passed。
- protocol_proxy：50 passed。
- session_transfer：12 passed。

## 11. 最新发布基线

唯一推荐发布目录：

```text
D:/Codex/Codex++/dist/windows/release-2026-08-13-v1/
```

安装包：

```text
CodexPlusPlus-1.2.45-official-account-detection-fix-windows-x64-setup.exe
106733546 bytes
CBB3D91A9A53B5B012FB4C5E845357918007F05C156263F806E0ACBBF437ABC2
```

便携包：

```text
CodexPlusPlus-1.2.45-official-account-detection-fix-windows-x64-portable.zip
145053632 bytes
71F4519C02C96FCDD44EB8AC3FA33B6A48084AA71CDF8221160EB8A79347193B
```

验证记录：

```text
D:/Codex/Codex++/output/packaging-2026-08-13-v1/verification-record.txt
```

安装器未自动执行。

## 12. 后续建议

1. 用户手动安装 `release-2026-08-13-v1` 后检查官方账号显示、保存和切换。
2. 安装后重新实测管理员 Exec、Terminal 和 Computer Use。
3. 设计管理员三项能力的独立状态与分级降级。
4. 如继续开发，先从 `08f6ef6` 开始，不改写已推送历史。
5. 新发布必须使用新的日期/序号目录并完整执行 ZIP 逐文件哈希验证。

## 13. 安全与操作约束

- 不执行 `reset`、`checkout`、`clean` 或批量删除。
- 不读取或输出 `.env`、真实 `auth.json`、token、API Key 或真实配置凭据。
- 不自动执行安装器，不停止当前承载任务的 Codex/Manager。
- 单文件删除前验证绝对路径、普通文件属性、内容和归属。
- 不覆盖旧发布目录。
- 行为修改必须配套回归测试。