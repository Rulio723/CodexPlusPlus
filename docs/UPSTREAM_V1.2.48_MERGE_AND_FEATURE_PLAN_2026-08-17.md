# Codex++ 合并官方上游 v1.2.48 计划

更新时间：2026-08-17（Asia/Shanghai）

## 1. 基线与目标

- 当前分支：`upgrade-Rulio`
- 当前 HEAD：`642f0c1e089ba209128a8dfd52249e3dcc6461cc`
- 合并前上游基线：`1f431ae49b57b3055e0e6845ba6156c6b4232b4d`
- 最新官方上游：`fb3ebd9a82383aedd8d098aa9ae3aef9426d13a4`
- 官方版本：`1.2.48`
- 合并方式：在 `upgrade-Rulio` 上执行 `git merge --no-ff upstream/main`，保留双方历史。

目标：以官方上游修复和新增功能为基准升级到 v1.2.48，同时保留本 fork 已完成的管理员模式、官方账号与混合登录、会话增强、插件与皮肤、无广告、安装恢复、按模型上下文窗口等增强。

## 2. 上游增量

`1f431ae..fb3ebd9` 包含：

1. `8f8327e release: v1.2.48`
2. `14f7003 merge: sync latest main before v1.2.48 release`
3. `fb3ebd9 fix: align release build targets`

主要新增与修复：

- 微信连接与相应的 app-server/session-store 支持。
- 工作目录和会话搜索、Remote Control 与启动状态改进。
- DeepSeek 模型元数据和模型 catalog 兼容修复。
- launcher 构建目标与发布版本更新。
- 上游会话数据库/索引清理与相关测试。

## 3. 合并裁决原则

1. 双方修复同一问题时，采用官方上游实现与接口；仅补回本 fork 仍需要且上游未覆盖的行为。
2. 官方新增功能全部接入，包括后端模块、命令、设置、UI、样式、翻译、测试与依赖。
3. 本 fork 增强保持不变，除非与官方实现发生直接冲突；冲突时做组合实现，不整文件选边。
4. 不重新引入广告、赞助商推荐或推广模块。
5. 不削弱管理员模式、官方账号保险库/混合登录、会话导入导出删除移动、插件自动展开与空扫描去重、Dream/Snow Skin p24、安装恢复和自定义图标。
6. 按模型上下文窗口继续使用 `model_list` 旧后缀兼容 + `model_windows` 存储 + Codex 原生 `model_catalog_json`；保留用户外部 catalog，使用上游最新模型元数据能力。
7. 不触碰 `.codex/`、`output/`、交接文档、现有安装包或真实运行状态；不启动安装器，不停止当前 Codex/Manager。

## 4. 预演冲突与具体处理

预演发现以下 7 个文本冲突：

### `apps/codex-plus-launcher/src/main.rs`

- 接入上游启动状态、失败诊断、helper-only/发布目标修复。
- 保留本 fork 管理员统一启动、管理员 shim、动态端口、恢复流程和失败不降权语义。
- 将双方启动入口组合为单一生命周期，避免重复启动或重复写状态。

### `apps/codex-plus-manager/src/App.tsx`

- 接入上游微信连接、工作目录/会话搜索、启动状态与 Remote Control UI。
- 保留本 fork 管理员、官方账号、官方混合登录、会话移动、插件、皮肤、按模型窗口行编辑及相关设置。
- 不恢复广告/推广/赞助商推荐 UI。

### `apps/codex-plus-manager/src/i18n-en.ts`

- 合并双方功能性翻译键。
- 保留本 fork 功能翻译；加入 v1.2.48 新功能翻译。
- 不加入仅服务于广告或赞助商推荐的翻译键。

### `assets/inject/renderer-inject.js`

- 以官方 v1.2.48 DOM/运行时修复为基准。
- 保留本 fork project move、session delete/export、plugin auto-expand、空候选签名去重、Dream/Snow Skin revision 24、现代首页诊断及其他现有增强。
- 禁止用上游整文件覆盖本地注入器；逐段组合并保留回归测试 seam。

### `crates/codex-plus-core/src/settings.rs`

- 加入上游微信连接及 v1.2.48 新设置字段、默认值与合并逻辑。
- 保留管理员、官方账号/混合登录、project move/classic sidebar、`model_windows`、VLM、单模型路由等本 fork 字段。
- 维持 serde 向后兼容与默认值。

### `crates/codex-plus-data/src/lib.rs`

- 同时导出上游 storage API 与本 fork session-transfer API；不二选一。

### `tools/i18n-keys.json`

- 以最终代码真实引用为准生成/合并功能键集合。
- 保留双方功能键，排除已删除的广告/推广键。

## 5. 非文本冲突文件的语义检查

自动合并后仍逐项检查：

- `Cargo.toml`、`Cargo.lock`、core/manager package 版本统一为 `1.2.48`，保留本 fork workspace binaries 和依赖。
- `commands.rs`/`lib.rs` 注册 v1.2.48 命令，同时保留管理员、官方账号、会话迁移与增强命令。
- `model_suffix.rs`/`relay_config.rs` 采用上游最新 DeepSeek/catalog 修复，同时保持 per-model window 与外部 catalog 规则。
- `routes.rs`、`storage.rs`、bridge/cdp/relay tests 不丢本 fork 行为。
- `renderer-inject.js` 修改后必须确认 p24 revision 与插件空扫描去重仍存在。

## 6. 执行步骤

1. 建立 `codex/pre-v1.2.48-merge-20260817` 回退分支指向合并前 HEAD。
2. 执行 `git merge --no-ff upstream/main`。
3. 按第 4 节逐文件解决冲突，并审查所有自动合并文件。
4. 搜索冲突标记、版本号、广告/推广回归和关键本地功能标记。
5. 运行定向 Rust/前端测试，再运行完整格式、类型、测试和构建验证。
6. 保留合并提交，不推送远端；推送由用户后续决定。

## 7. 验证矩阵

最低验证：

```text
cargo test -p codex-plus-core --test model_suffix
cargo test -p codex-plus-core --test relay_config
cargo test -p codex-plus-core --test cdp_bridge
cargo test -p codex-plus-core --test launcher
cargo test -p codex-plus-data
npm test --prefix apps/codex-plus-manager
npm run check --prefix apps/codex-plus-manager
cargo fmt --all -- --check
git diff --check
```

构建验证：

```text
npm run vite:build --prefix apps/codex-plus-manager
cargo build --release --workspace
```

不自动执行 NSIS 安装器；若构建时间或环境阻塞完整 release build，必须返回已完成的精确测试证据与剩余项。

## 8. 回退

- 合并前分支：`codex/pre-v1.2.48-merge-20260817`
- 若合并尚未提交：`git merge --abort`
- 若合并已提交且需人工回退：从回退分支创建恢复分支或对合并提交执行显式 revert；不使用 `git reset`。

## 9. 2026-08-17 完成状态

- 合并冲突已全部解决，所有 merge 结果保持暂存，`git diff --name-only --diff-filter=U` 为空。
- Manager 的 `LaunchStatus` 初始化已补齐 `administrator_mode`，并以 `AdministratorModeStatus::default()` 保持启动请求阶段不虚报管理员能力。
- 隔离安装目录仅启动 `codex-plus-plus.exe --helper-only --helper-port 52506`：PID `18052`，`POST /backend/status` 返回 `version=1.2.48`；stdout/stderr 原始日志已保存，隔离 helper 已终止。
- 实机 live 回归继续使用既有 CDP `9229` 与原 helper `57321`：
  - `live_official_mixed_home_passes_skin_verification`：通过。
  - `live_apply_keeps_the_running_renderer_available`：通过。
- 前后 ChatGPT PID 集合一致；真实状态文件保持 `running / 9229 / 57321 / administrator_mode.active`；原 helper 前后均返回 `version=1.2.47`。
- 已通过 `cargo check -p codex-plus-manager`、`cargo build --release --workspace`、`cargo test -p codex-plus-manager`（53+28）、`npm run check --prefix apps/codex-plus-manager`、`cargo fmt --all -- --check` 与 staged diff 检查。
- 交付物记录于 `output/packaging-2026-08-17-v1.2.48/merge-verification/`；本地合并提交 subject 为 `merge: integrate upstream v1.2.48`，不推送远端。
