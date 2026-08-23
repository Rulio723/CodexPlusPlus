# Codex++ 对话交接文档

更新时间：2026-08-17（Asia/Shanghai）

## 1. 用户目标

用户要求以 BigPizzaV3/CodexPlusPlus 官方上游为基础，把原项目的完整增强迁移到新源码并作为正式版维护，主要包括：

- 管理员权限完整功能。
- 延迟测试和官方/模拟指纹。
- 会话导入、导出、删除和移动。
- 官方账号登录与官方混合登录。
- 工具、插件市场和插件调用逻辑。
- Dream Skin / Snow Skin 皮肤与软件图标。
- 安装器检测运行中的 Codex 并关闭。
- 沿用原项目 NSIS Windows x64 安装流程。
- 去除广告和推广内容。
- 整理源码并推送到 `Rulio723/CodexPlusPlus` 的 `upgrade-Rulio` 分支。

上述迁移已经完成，当前阶段主要是跟随 ChatGPT/Codex 官方更新修复 DOM 和运行时兼容问题。

## 2. 当前权威状态

```text
工作区：D:/Codex/Codex++
分支：upgrade-Rulio
HEAD：642f0c1e089ba209128a8dfd52249e3dcc6461cc
origin/upgrade-Rulio：642f0c1e089ba209128a8dfd52249e3dcc6461cc
版本：1.2.47
Dream Skin revision：24-modern-home-composer-contract
```

本地与远端分支一致。当前跟踪源码干净；`.codex/`、`output/` 和交接/计划文档是本地未跟踪资料。

## 3. 最近一次问题及最终根因

用户在官方混合登录模式下打开新版首页，皮肤管理的“首页内容”仍显示失败。

### p23 第一次修复

最初确认新版首页已经没有旧式建议卡，因此加入现代首页分支：

```text
80dfabb fix: accept modern official mixed home diagnostics
```

但 p23 仍要求独立 `projectButton` 可见，所以安装后仍然失败。

### 真实 DOM 复现

通过本机 CDP 点击“新对话”并抓到真实验证结果：

```text
homeRoute=true
homePresent=true
hero.visible=true
composer.visible=true
legacySuggestionsPresent=false
visibleCardCount=0
projectButton=null
```

官方更新已把项目选择合并到 composer 底栏，且未选择项目也是合法首页。`projectButton=null` 不代表登录或皮肤失败。

### p24 最终修复

```text
642f0c1 fix: verify current official mixed home DOM
```

新规则：

- 旧式首页仍要求 1..=6 张可见建议卡。
- 新式首页要求首页、横幅和 composer 可见。
- 不再把独立项目按钮作为硬条件。
- 同时修复插件自动展开空签名不去重造成的每秒日志/扫描循环。

真实首页回归已经从红灯变成绿灯。

## 4. 当前运行状态

2026-08-17 最近一次状态抽样：

```text
status=running
debug_port=9229
helper_port=57321
administrator_mode.requested=true
administrator_mode.state=active
```

这说明最近问题不是管理员启动失败，也不是官方登录配置整体丢失，而是皮肤首页诊断契约落后于官方 DOM。

端口是动态状态。继续测试时应读取：

```text
C:/Users/Rulio/.codex-session-delete/latest-status.json
```

不要复用历史 helper 端口。

## 5. 当前唯一正式安装包

```text
D:/Codex/Codex++/dist/windows/CodexPlusPlus-1.2.47-windows-x64-setup.exe
大小：106799865 bytes
SHA-256：0B2E8DC38D08EC985B6DAC555FF00552845BC17C88F72BD2B17AFF6022746CCB
```

这是包含 p24 首页修复和插件空扫描去重的最新 Windows x64 NSIS 安装包。

2026-08-17 已清理 `dist`：174 个旧版本、staging、诊断和测试产物已逐项移入回收站。现在 `dist` 只剩上述安装包。

清理记录：

```text
D:/Codex/Codex++/output/dist-cleanup-2026-08-17.txt
```

## 6. 最新验证证据

```text
verification_accepts_modern_official_mixed_home_without_legacy_cards  passed
live_official_mixed_home_passes_skin_verification                    passed
live_apply_keeps_the_running_renderer_available                      passed
dream_skin_runtime                                                   14 passed, 2 ignored
cdp_bridge                                                           102 passed
npm test                                                             71 passed
npm run check                                                        exit 0
cargo fmt --all -- --check                                           exit 0
git diff --check                                                     exit 0
npm run vite:build                                                   exit 0
cargo build --release --workspace                                    exit 0
NSIS                                                                 exit 0
```

打包和回滚记录：

```text
D:/Codex/Codex++/output/packaging-2026-08-16-v2/verification-record.md
D:/Codex/Codex++/output/packaging-2026-08-16-v2/official-mixed-home-real-dom-fix.patch
D:/Codex/Codex++/output/packaging-2026-08-16-v2/rollback-official-mixed-home-real-dom-fix.ps1
D:/Codex/Codex++/output/packaging-2026-08-16-v2/CodexPlusPlus-1.2.47-windows-x64-setup.baseline.exe
```

## 7. 已完成且不应重复的工作

- 已从官方上游源码完成正式迁移，不再需要重新 clone 或重新初始化仓库。
- 广告和推广模块已经移除。
- 软件图标、管理员启动、安装恢复、进程检测和 NSIS 流程已经迁移。
- 会话导入/导出/删除/移动、工具和插件逻辑已经迁移。
- 官方账号与官方混合模式配置已经迁移。
- 新对话皮肤翻页/布局混乱已经修复。
- 管理员启动失败相关生命周期和 shim 问题已经修复并提交。
- p23 首页误报已被 p24 真实 DOM 修复取代。
- `dist` 旧版本已经清理，只保留最新安装包。

不要重新扫描或恢复已清理的旧 `dist` 构建目录。

## 8. 后续如果再次出现“官方更新后失效”

优先按以下顺序处理：

1. 从 `latest-status.json` 获取当前 debug/helper 端口。
2. 通过主 `app://-/index.html` CDP target 复现，不要命中 avatar overlay 或 embedded webview。
3. 抓取脱敏后的 DOM 语义与几何字段，不读取 auth/token/cookie。
4. 先把真实输入写成红色回归测试。
5. 修复时优先依赖 `data-testid`、role、可见 composer 和语义结构；避免依赖 hash class。
6. 同时检查 renderer 日志是否出现短周期循环。
7. 运行 unit/integration/live 测试后再构建 release 和 NSIS。
8. 保存旧安装包哈希、补丁、验证记录和回滚脚本。
9. 不自动启动安装器，因为安装流程会结束当前 Codex 对话。

## 9. Git 提交脉络

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

## 10. 操作约束

- 中文沟通。
- 不读取或输出 token、API Key、cookie、`.env`、真实 auth/config 凭据。
- 不执行 `git reset`、`git clean` 或未经确认的批量删除。
- 不提交 `.codex/`、`output/` 或本地运行状态。
- 不回滚已经迁移的功能。
- 不自动运行安装器，不无故关闭当前 Codex/Manager。
- 修改行为必须配套真实 bug seam 的回归测试。
- 删除或移动文件前验证目标绝对路径位于预期工作区。

## 11. 下一对话可直接使用的续接提示

```text
继续处理 D:/Codex/Codex++。当前分支 upgrade-Rulio，本地和 origin/upgrade-Rulio 都在 642f0c1e089ba209128a8dfd52249e3dcc6461cc，Workspace 版本 1.2.47。官方混合首页真实 DOM 修复已完成，Dream Skin revision 为 24-modern-home-composer-contract；真实首页验证、管理员运行、cdp_bridge 和前端测试均已通过。dist 已清理，只保留 dist/windows/CodexPlusPlus-1.2.47-windows-x64-setup.exe，SHA-256 为 0B2E8DC38D08EC985B6DAC555FF00552845BC17C88F72BD2B17AFF6022746CCB。不要重新 clone、恢复旧 dist 或重复已完成迁移；遇到官方更新回归时先用当前 CDP 主页面建立真实红色反馈环，不读取凭据，也不要自动执行安装器。
```

## 12. 2026-08-22 最新覆盖状态

本文中的 1.2.47 安装包信息已过期。当前 Windows 发布基线是 1.2.50 强制结束循环包：

```text
D:/Codex/Codex++/dist/windows/release-2026-08-22-force-stop-loop-tested/CodexPlusPlus-1.2.50-windows-x64-setup-force-stop-loop-tested.exe
SHA-256: FB273709C4DD3AED257E6D51A03FC368C4E235F7F109A37B68CD21CE74421B44
```

安装时由提升权限的 recovery 循环重枚举并强制结束 launcher、manager、其他 recovery、admin shim 和 ChatGPT；单次结束失败会重试，且会捕获重启后的新 PID。

后续对话必须先读取：

```text
docs/PROJECT_MEMORY_2026-08-22.md
```
