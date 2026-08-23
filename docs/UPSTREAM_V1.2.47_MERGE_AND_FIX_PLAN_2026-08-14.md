# Codex++ 上游 v1.2.47 合并与修复方案

更新时间：2026-08-14（Asia/Shanghai）
适用仓库：`D:/Codex/Codex++`

## 1. 目标与基本原则

本轮目标不是把当前分支中的所有历史修复机械地覆盖到上游，而是：

1. 以最新 `upstream/main` 作为代码与行为基准。
2. 上游已经修复的问题直接采用上游实现，删除本 fork 中对应的兼容补丁、旧分支和重复测试。
3. 只向上游基线重新叠加本 fork 的独有功能。
4. 每项新增功能必须保持 opt-in，不改变上游默认行为。
5. 所有行为修改必须在正确 seam 增加或迁移回归测试。
6. 不覆盖旧发布目录，不自动执行安装器，不读取或输出凭据。

最终目标分支应表现为“上游 v1.2.47 + 清晰隔离的新增功能”，而不是“旧 fork + 大量冲突解决”。

## 2. 已确认的 Git 状态

| 项目 | 当前值 |
| --- | --- |
| 当前分支 | `upgrade-Rulio` |
| 当前 HEAD | `08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2` |
| 当前远端 | `origin/upgrade-Rulio`，与本地 HEAD 一致 |
| 上游最新分支 | `upstream/main` |
| 上游最新 HEAD | `1f431ae49b57b3055e0e6845ba6156c6b4232b4d` |
| 上游最新版本 | `1.2.47` |
| 上游标签 | `v1.2.46`、`v1.2.47` |
| 双方共同祖先 | `0ceb2d02a993188b386b85d15a573e27e1656785` |
| 本地独有提交 | 154 |
| 上游独有提交 | 83 |
| 双方改动文件交集 | 56 |
| merge-tree 文本冲突 | 38 个文件 |

注意：交接文档记录的 `0553026` 是曾经同步的 v1.2.45 内容基线，但该提交不是当前分支 HEAD 的 Git 祖先。因此不能按普通线性 rebase 理解当前历史，直接 rebase 会重复处理大量旧提交。

## 3. 推荐集成方式

### 3.1 推荐：从上游创建独立集成工作树

不在当前 `upgrade-Rulio` 工作树上直接合并。以 `upstream/main` 新建工作树和分支，再按功能域移植本 fork 的新增能力：

```powershell
git fetch upstream --prune
git branch codex/pre-v1.2.47-port-20260814 08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2
git worktree add `
  -b codex/upstream-v1.2.47-feature-port `
  'D:/Codex/Codex++-v1.2.47-port' `
  1f431ae49b57b3055e0e6845ba6156c6b4232b4d
```

优点：

- 上游天然成为基线，不需要在 38 个冲突中判断哪些旧修复应被删除。
- 当前可运行分支和发布物保持不动，可随时对照行为。
- 每个新增功能可以形成独立提交、独立测试和独立回滚点。
- 避免把 `299a2d4`、`a15c1d7` 这类“同步上游 + 本地增强”混合提交整体 cherry-pick 回去。

### 3.2 不推荐：直接 merge 或全量 rebase

以下方式只适合作为冲突审计，不作为最终集成方式：

```powershell
git merge --no-commit --no-ff upstream/main
git rebase upstream/main
```

原因：

- 当前历史与上游在 v1.2.45 前已经形成非线性分叉。
- 大量本地提交属于对旧上游版本的修复，继续保留会覆盖上游的新实现。
- `App.tsx`、`renderer-inject.js`、`relay_config.rs` 等文件同时包含上游同步和本地功能，按 ours/theirs 整文件选择都会丢行为。

## 4. 上游修复优先清单

下列问题上游已有正式实现。移植时应保留上游代码结构和测试，本地同类补丁只用于核对需求，不直接恢复旧实现。

| 领域 | 上游提交 | 处理决定 |
| --- | --- | --- |
| projectless 原生新会话 | `771cc82` | 采用上游原生流程，删除旧拦截状态机 |
| 临时新会话 ID 删除 | `2fcb70e` | 采用上游 ID 解析与删除流程 |
| Remote Control 会话恢复 | `a366941`、`12d54a5`、`1f431ae` | 采用上游规范化和恢复实现 |
| VLM 工具图片处理 | `d49408f` | 采用上游消息遍历基础，只补上游缺少的模型窗口语义 |
| LLM bridge proxy | `2afff3a` | 采用上游路由与错误处理 |
| provider key 保留 | `9c70a71`、`e2a98f9` | 采用上游配置回填规则 |
| cc-switch catalog 接管 | `79e9237`、`b960878` | 采用上游 catalog 所有权判定 |
| service tier 继承标签 | `43b1279`、`46bc7c5` | 采用上游读取和展示逻辑 |
| 当前 Codex 顶栏菜单 | `635bc92`、`5de141d` | 采用上游 DOM 观察和菜单锚点 |
| 会话删除撤销刷新 | `c22bdbe`、`ed55339` | 采用上游刷新行为 |
| DreamSkin 社区主题 | `a30b550` | 采用上游主题模型和资产规则，再补本地独有能力 |
| 嵌入浏览器注入隔离 | `ce1ed22`、`2f6cd30` | 采用上游 guard |
| 供应商内逐模型路由 | `ab8dce9` | 完整保留上游路由模型和 UI |
| 逐模型路由竞态和反向校验 | `7081e84` | 完整保留上游修复 |
| 逐模型路由输入焦点 | `951fccd` | 完整保留上游 UI 修复 |
| 系统证书信任 | `405b848`、`119bade` | 上游 HTTP 客户端为基准 |
| 上游分支菜单 CPU 占用 | `464819b`、`f3a0f8e` | 采用上游 observer 范围控制 |

判断规则：即使本地实现测试更多，只要解决的是同一缺陷，也先保留上游生产实现，再把仍有价值的测试改写为上游行为测试。

## 5. 必须保留的本 fork 新增功能

### 5.1 P0：按模型上下文窗口

这是本 fork 的主功能，必须完整移植：

- 模型列表后缀：例如 `MODEL[1M]`。
- `model_windows` 或等价的结构化逐模型配置。
- 使用 Codex 原生 `model_catalog_json`。
- 只更新 Codex++ 自己管理的 catalog。
- 用户提供的外部 catalog 保持不变。
- 没有逐模型配置时维持上游和旧 profile 的单值行为。
- 不跨供应商混合模型窗口和模型列表。

与上游“供应商内逐模型路由”的共存约束：

1. 路由映射决定“模型请求发往哪个 relay”。
2. 上下文窗口决定“该模型写入 catalog 的 context window/压缩阈值能力”。
3. 两者使用不同字段，不互相推导、不互相覆盖。
4. 切换供应商时同时重建当前供应商的托管 catalog 和路由状态。
5. 无逐模型窗口但已有托管 catalog 时，应按当前供应商模型列表刷新，避免残留上一供应商模型。

主要移植文件：

```text
crates/codex-plus-core/src/settings.rs
crates/codex-plus-core/src/model_suffix.rs
crates/codex-plus-core/src/model_catalog.rs
crates/codex-plus-core/src/relay_config.rs
apps/codex-plus-manager/src/App.tsx
apps/codex-plus-manager/src/model-windows.test.ts
crates/codex-plus-core/tests/model_catalog.rs
crates/codex-plus-core/tests/relay_config.rs
```

### 5.2 P0：管理员模式

管理员模式属于新增能力，不应被上游普通 launcher/exec 修复替代：

- Exec/App Server 高完整性路由。
- Terminal/PowerShell shim。
- Computer Use 认证 broker。
- 可信运行时复制、ACL、身份校验、lease 和恢复流程。
- 未知 helper/transport 版本、路径、哈希或签名继续 fail closed。
- 普通模式必须保持纯上游启动路径。

优先移植本地新增文件，再对上游 wiring 点做最小修改：

```text
apps/codex-plus-admin-shim/
apps/codex-plus-terminal-shim/
crates/codex-plus-core/src/admin_app_server.rs
crates/codex-plus-core/src/admin_mode/
crates/codex-plus-core/src/admin_secure_io.rs
apps/codex-plus-manager/src/administrator-mode.ts
apps/codex-plus-manager/src/administrator-mode.test.ts
scripts/installer/windows/secure-recovery-*.ps1
scripts/installer/windows/secure-recovery-acl.nsh
```

需要人工融合的 wiring 点：

- workspace members 与依赖。
- launcher 启动分支。
- manager commands 和状态 payload。
- installer staging、恢复脚本和 Windows manifest。
- UI 设置项和能力状态展示。

### 5.3 P0：官方账号保险库

保留以下新增行为：

- AES-GCM 加密保存账号材料。
- UI/command payload 只返回摘要。
- 当前账号根据实时官方登录结构判断。
- 供应商切换不使用旧 provider-selected marker 隐藏有效登录。
- API Key-only 状态不显示为 ChatGPT 官方账号。
- 切换失败不覆盖 live auth。
- 不记录 token、nonce、ciphertext 或真实 auth 内容。

主要文件：

```text
crates/codex-plus-core/src/official_accounts.rs
crates/codex-plus-core/tests/official_accounts.rs
apps/codex-plus-manager/src/official-accounts.ts
apps/codex-plus-manager/src/official-accounts.test.ts
apps/codex-plus-manager/src-tauri/src/commands.rs
apps/codex-plus-manager/src/App.tsx
```

### 5.4 P1：会话导入、导出与 rollout

这部分属于新增功能，应在上游最新会话结构上重新接入：

- 导出和 `.codexbackup` 包含真实 rollout。
- 支持 rollout 位于 `CODEX_HOME` 外部。
- 导入时重建缺失的项目会话索引。
- 缺少 rollout 正文时不伪造记录。
- projectless 会话保持 projectless。
- 多数据库写入失败时完整回滚。

主要文件：

```text
crates/codex-plus-data/src/session_transfer.rs
crates/codex-plus-data/tests/session_transfer.rs
apps/codex-plus-manager/src-tauri/src/commands.rs
apps/codex-plus-manager/src/App.tsx
```

### 5.5 P1：Relay 独有能力

以 v1.2.47 HTTP 客户端为基础，只迁移上游尚未具备的部分：

- 可选官方 Codex 请求指纹。
- aggregate relay 请求内故障转移。
- Responses、Chat Completions、Audio、Models 的统一代理扩展。
- custom/namespace tool 与 `apply_patch` 历史转换。
- relay latency 与模型测试复用同一认证和 transport 语义。
- 模型级上下文窗口传递到 VLM 处理。

必须先逐项做“上游是否已有等价实现”的语义比较；已有部分不再移植本地实现。

### 5.6 P1：Manager/Renderer 独有增强

只保留能明确证明上游 v1.2.47 尚未包含的能力：

- 管理员模式和官方账号 UI。
- 会话导入、导出、移动。
- 本 fork 的插件市场扩展。
- 底部 Terminal persisted atom 修复（若上游仍未覆盖）。
- Dream Skin 的本地扩展、图像伴随和失败回滚（建立在上游社区主题结构上）。
- model/service tier patch 中上游仍缺失的部分。
- runtime smoke checker。

`renderer-inject.js` 禁止整文件采用本地版本。必须从上游文件开始，按功能安装函数逐块移植，并保留上游新增的 Remote Control、临时会话 ID、菜单 observer 和嵌入浏览器 guard。

### 5.7 P2：产品策略和外观改动

以下内容单独决策，不与核心合并绑定：

- 去广告和手动更新策略。
- 自定义图标。
- 中文静态映射。
- 额外的 UI 样式和本地主题。

它们应位于独立提交，便于主功能 PR 排除非必要改动。

## 6. 38 个冲突文件的处理矩阵

### 6.1 直接以上游为基准

| 文件 | 处理方式 |
| --- | --- |
| `.gitattributes` | 采用上游最新资产 EOL 规则；仅在确有本地新增资产时追加规则 |
| `Cargo.toml` | 采用 v1.2.47 版本和依赖，再添加 admin shim workspace member/确需依赖 |
| `Cargo.lock` | 不手工拼接；完成 `Cargo.toml` 后由 Cargo 统一生成 |
| `apps/codex-plus-manager/package.json` | 采用 v1.2.47；仅补本地确需脚本或依赖 |
| `apps/codex-plus-manager/package-lock.json` | 不手工解冲突；根据最终 package.json 生成 |
| `apps/codex-plus-manager/src-tauri/Cargo.toml` | 采用上游，再补管理员/账号所需依赖 |
| `apps/codex-plus-manager/src-tauri/tauri.conf.json` | 采用上游版本号和构建配置，再补确需打包资源 |
| `tools/i18n-keys.json` | 以上游 key 集为基准，最后运行项目检查生成/校验 |

### 6.2 以上游结构为基准后移植功能

```text
apps/codex-plus-launcher/src/main.rs
apps/codex-plus-manager/src/App.tsx
apps/codex-plus-manager/src/i18n-en.ts
apps/codex-plus-manager/src/model-windows.test.ts
apps/codex-plus-manager/src/styles.css
assets/inject/renderer-inject.js
crates/codex-plus-core/src/assets.rs
crates/codex-plus-core/src/ccs_import.rs
crates/codex-plus-core/src/cdp.rs
crates/codex-plus-core/src/codex_app_state.rs
crates/codex-plus-core/src/dream_skin_library.rs
crates/codex-plus-core/src/launcher.rs
crates/codex-plus-core/src/model_suffix.rs
crates/codex-plus-core/src/paths.rs
crates/codex-plus-core/src/protocol_proxy.rs
crates/codex-plus-core/src/provider_import.rs
crates/codex-plus-core/src/relay_config.rs
crates/codex-plus-core/src/routes.rs
crates/codex-plus-core/src/settings.rs
crates/codex-plus-core/src/vision.rs
crates/codex-plus-data/src/provider_sync.rs
```

这些文件禁止使用整文件 ours。流程应是：

1. 保留上游文件。
2. 找出本地新增的类型、字段、函数、路由和 UI section。
3. 按最小块移植。
4. 调整为上游当前数据流和命名。
5. 每移植一块立即运行对应定向测试。

### 6.3 测试冲突文件

```text
crates/codex-plus-core/tests/bridge_routes.rs
crates/codex-plus-core/tests/cdp_bridge.rs
crates/codex-plus-core/tests/codex_app_state.rs
crates/codex-plus-core/tests/dream_skin_runtime.rs
crates/codex-plus-core/tests/launcher.rs
crates/codex-plus-core/tests/protocol_proxy.rs
crates/codex-plus-core/tests/relay_config.rs
crates/codex-plus-core/tests/upstream_theme_assets.rs
crates/codex-plus-data/tests/provider_sync.rs
```

测试处理原则：

- 先保留全部上游测试。
- 本地测试按“新增能力断言”迁移，不恢复旧实现字符串和旧 DOM 结构断言。
- 同一 bug 的重复测试合并为上游测试的补充 case。
- 对 renderer 的测试优先验证公开行为、bridge 消息和持久化状态，减少对压缩产物变量名的绑定。

### 6.4 特殊文件

`crates/codex-plus-core/tests/dream_skin.rs` 在 merge-tree 中出现合并标记但结果可自动形成，需要人工复核自动结果，确认本地可见 composer guard 与上游社区主题行为同时存在。

## 7. 分阶段实施步骤

### 阶段 0：冻结基线与证据

1. 保留 `upgrade-Rulio` 在 `08f6ef6`。
2. 创建只读保护分支 `codex/pre-v1.2.47-port-20260814`。
3. 记录以下命令输出：

```powershell
git rev-parse HEAD
git rev-parse upstream/main
git status --short
git diff --check
```

4. 不提交 `.codex/`、`output/`、`target-console/`。
5. 当前两份 2026-08-14 交接文档为未跟踪文件，需在正式集成前明确是否纳入文档提交。

### 阶段 1：建立纯上游可运行基线

在新工作树中执行上游原始验证，不做功能修改：

```powershell
cargo fmt --all -- --check
npm test -- --run
npm run check
npm run vite:build
cargo test --workspace -- --test-threads=1
```

若纯上游基线已有失败，单独记录，不与移植问题混合。

### 阶段 2：移植按模型上下文窗口

建议提交：

```text
feat: restore provider-scoped per-model context catalogs
```

实施顺序：

1. `settings.rs`：添加数据字段、serde 默认值和兼容读取。
2. `model_suffix.rs`：恢复后缀解析、单位转换、错误提示。
3. `model_catalog.rs`：在上游 catalog 模型上添加窗口覆盖，不复制旧模板结构。
4. `relay_config.rs`：接入上游 v1.2.47 的 provider model routes，分离路由和窗口逻辑。
5. `App.tsx`：在上游逐模型路由 UI 附近增加窗口输入，不重建整个模型列表组件。
6. 迁移 core/manager 测试。

定向验证：

```powershell
cargo test -p codex-plus-core --test model_catalog -- --test-threads=1
cargo test -p codex-plus-core --test relay_config -- --test-threads=1
npm test -- --run src/model-windows.test.ts
```

必须覆盖：

- `MODEL[1M]`、`MODEL[128K]` 等合法后缀。
- 非法单位、零值、重复模型。
- profile 单值 fallback。
- 外部 catalog 不被覆盖。
- 托管 catalog 随供应商切换刷新。
- 逐模型路由与逐模型窗口同时存在。
- 逐模型路由首次启动竞态修复仍通过。

### 阶段 3：移植官方账号保险库

建议提交：

```text
feat: restore encrypted official account vault
```

顺序：core 模块与测试 → Tauri commands → TS model/tests → App UI。

验证：

```powershell
cargo test -p codex-plus-core --test official_accounts -- --test-threads=1
npm test -- --run src/official-accounts.test.ts
```

额外检查：任何失败日志不得包含凭据；实时 auth 识别逻辑必须独立于供应商选择状态。

### 阶段 4：移植管理员模式

建议拆成多个提交：

```text
feat: restore administrator execution primitives
feat: restore administrator terminal and app-server shims
feat: restore administrator computer-use broker
feat: expose administrator capability state in manager
build: package administrator runtime components
```

每个提交保持普通模式可运行。先移植纯新增文件，再修改 launcher、commands、UI 和 installer wiring。

定向验证：

```powershell
cargo test -p codex-plus-core --test admin_environment -- --test-threads=1
cargo test -p codex-plus-core --test admin_feature -- --test-threads=1
cargo test -p codex-plus-core --test admin_mode -- --test-threads=1
npm test -- --run src/administrator-mode.test.ts
```

生产烟雾验证放在构建后进行，且不自动停止当前 Codex/Manager 进程。

### 阶段 5：移植会话 transfer 与缺失的 Relay 功能

建议提交：

```text
feat: restore transactional session transfer
feat: restore relay transport extensions
```

Relay 迁移前逐函数比较上游 `http_client.rs`、`protocol_proxy.rs`、`routes.rs` 和 `vision.rs`。系统证书、上游代理基础、VLM 工具图片等已有能力直接沿用上游。

验证：

```powershell
cargo test -p codex-plus-data --test session_transfer -- --test-threads=1
cargo test -p codex-plus-core --test protocol_proxy -- --test-threads=1
cargo test -p codex-plus-core --test bridge_routes -- --test-threads=1
```

### 阶段 6：Manager、Renderer 与 Dream Skin

建议按功能拆分，不用一个大提交覆盖：

```text
feat: restore manager session and account extensions
feat: restore renderer plugin marketplace extensions
feat: restore bottom terminal compatibility guard
feat: extend upstream DreamSkin runtime behavior
test: restore Codex runtime smoke checker
```

`renderer-inject.js` 的移植顺序：

1. 复制上游文件作为基线。
2. 管理员/账号不应直接依赖 renderer 时避免注入。
3. 逐个恢复插件市场、service tier、模型 patch、底部 panel 和 Dream Skin 安装函数。
4. 每加入一个 install 函数，运行 Node 语法检查及对应 Rust/JS 测试。
5. 最后运行真实 CDP 烟雾检查。

验证：

```powershell
node --check tools/codex-runtime-smoke-check.mjs
cargo test -p codex-plus-core --test cdp_bridge -- --test-threads=1
cargo test -p codex-plus-core --test dream_skin_runtime -- --test-threads=1
npm test -- --run
```

### 阶段 7：产品策略与资源

去广告、手动更新、自定义图标、中文静态映射分别形成独立提交。若目标是向上游提交“按模型上下文窗口”PR，这些提交不进入该 PR。

### 阶段 8：完整回归与发布候选

必须完整通过：

```powershell
cargo fmt --all -- --check
git diff --check
npm test -- --run
npm run check
npm run vite:build
cargo test --workspace -- --test-threads=1
node --check tools/codex-runtime-smoke-check.mjs
```

Windows 构建和打包要求：

- 使用全新的 `target`、app-build、packaging 和 release 目录。
- 不覆盖 `dist/windows/release-2026-08-13-v1/`。
- 安装器只构建和验证，不自动运行。
- portable ZIP 与 staging 做逐文件数量和 SHA-256 比较。
- 验证记录包含命令、退出码、文件大小、SHA-256、缺失文件数和哈希不一致数。

## 8. 每个冲突的判定流程

对每个冲突文件使用同一语义流程，而不是简单选择 ours/theirs：

1. 确定上游修改解决的问题和新数据流。
2. 确定本地修改属于：新增功能、旧 bug 修复、兼容补丁、测试或格式/版本变化。
3. 如果是同一 bug：采用上游实现。
4. 如果是本地新增功能：以最小接口接入上游数据流。
5. 如果上游重构了 seam：迁移功能到新 seam，不恢复旧 seam。
6. 删除重复 helper、重复状态字段、重复 DOM observer 和重复后台路由。
7. 同时运行上游原测试和本地新增功能测试。

冲突解决完成标准：

- 文件中没有 merge marker。
- 没有整段恢复旧上游代码。
- 上游新增测试仍然存在。
- 本地新增功能有明确入口、默认关闭或默认兼容。
- `git diff` 能解释每一块本地新增代码的功能归属。

## 9. 建议提交序列

新分支从上游 `1f431ae` 开始，建议保持以下顺序：

```text
feat: restore provider-scoped per-model context catalogs
feat: restore encrypted official account vault
feat: restore administrator execution primitives
feat: restore administrator terminal and app-server shims
feat: restore administrator computer-use broker
feat: expose administrator capability state in manager
build: package administrator runtime components
feat: restore transactional session transfer
feat: restore relay transport extensions
feat: restore manager session and account extensions
feat: restore renderer plugin marketplace extensions
feat: restore bottom terminal compatibility guard
feat: extend upstream DreamSkin runtime behavior
test: restore Codex runtime smoke checker
chore: restore optional product branding and policy changes
docs: record v1.2.47 feature-port verification
```

每个提交要求：

- 单一功能域。
- 对应测试在同一提交中。
- 不夹带版本号或无关格式化。
- 可单独 revert。
- 提交说明注明“采用的上游 seam”和“保留的本地新增行为”。

## 10. 回滚方案

在集成完成前，旧分支和旧发布始终保持可用：

```text
保护分支：codex/pre-v1.2.47-port-20260814
保护提交：08f6ef691ccdf2bef9acb4dc9790ce16a52c5ad2
旧发布：D:/Codex/Codex++/dist/windows/release-2026-08-13-v1/
```

单功能回滚优先使用对该功能提交的 `git revert <commit>`，不使用 `reset`。集成分支发生整体失败时保留工作树和日志，切回原 `upgrade-Rulio` 继续使用，不改写已推送历史。

## 11. 最终验收标准

### 上游一致性

- 版本和依赖基于 v1.2.47。
- 上游新增的 per-model relay routing、Remote Control 恢复、临时会话 ID、系统证书、菜单 observer 等测试全部通过。
- 本地旧 bug 补丁没有覆盖上游新实现。

### 新增功能

- 不同供应商、不同模型可以配置独立上下文窗口。
- 逐模型窗口和逐模型 relay route 可以同时工作。
- 管理员 Exec、Terminal、Computer Use 分别可检测和降级。
- 官方账号保险库只暴露摘要，实时账号识别正确。
- 会话 transfer 保留 rollout 和事务回滚。
- Manager/Renderer 独有增强在当前 Codex 运行时通过 smoke check。

### 工程质量

- 全量 Rust、前端、Vite 和 Node 检查通过。
- 工作树不存在冲突标记或意外生成文件。
- 没有提交 `.codex/`、`output/`、`target-console/` 或凭据文件。
- 新发布目录与旧发布隔离，安装器未自动执行。
- 最终交接文档记录准确 HEAD、测试数量、发布路径和哈希。

## 12. 首个实际执行批次

建议第一批只完成以下内容，不同时进入管理员模式或 renderer 大文件：

1. 从 `upstream/main@1f431ae` 创建独立工作树。
2. 验证纯上游测试基线。
3. 移植按模型上下文窗口。
4. 验证它与上游逐模型 relay routing 共存。
5. 形成一个可审查、可回滚的独立提交。

这一批完成后再进入官方账号、管理员模式和 Renderer，从而把最高价值主功能与高风险运行时改动隔离。
