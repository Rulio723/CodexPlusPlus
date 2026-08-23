# CodexPlusPlus 项目持久记忆（2026-08-22）

本文件记录已确认的 Windows 安装器行为、实现位置、测试门槛和最新安装包。后续上游合并、重构或发布时必须保留这些行为。

## 1. 用户确认的安装行为

Windows 安装包在覆盖 Codex++ 文件前，必须自动获得 High Integrity 权限，并强制结束所有可能占用安装文件的目标进程。用户不应需要打开任务管理器手动结束进程。

目标映像名：

```text
codex-plus-plus.exe
codex-plus-plus-manager.exe
codex-plus-recovery.exe
codex-plus-admin-shim.exe
ChatGPT.exe
```

必须保留的算法：

1. NSIS 通过 PowerShell `Start-Process -Verb RunAs -Wait` 启动 `--recover-admin-mode` recovery。
2. recovery 先恢复 stale administrator state，再结束更新阻塞进程。
3. 每轮都重新枚举目标映像，以捕获被重新拉起的新 PID。
4. 结束顺序为 launcher/watchdog、manager、其他 recovery、admin shim、ChatGPT。
5. 排除当前 recovery PID。
6. 每个 PID 先调用原生 `TerminateProcess`。
7. 原生调用失败后，立即调用绝对路径 `%SystemRoot%\System32\taskkill.exe /PID <PID> /F`。
8. 禁止使用 `/T`；不得依赖 `PATH`；不得用 Medium token 的 taskkill 作为成功判据。
9. 单次 `TerminateProcess` 或 `taskkill` 失败只能触发下一轮重试，不得立即放弃安装。
10. 连续两轮枚举均无目标进程后，才能开始覆盖文件。
11. 完整 recovery 有 30 秒硬超时，且必须小于 NSIS 的 120 秒等待上限。

## 2. 关键源码

```text
crates/codex-plus-core/src/watcher.rs
  force_terminate_process
  admin_recovery_process_ids_from_snapshot
  stop_processes_with_hooks
  stop_admin_recovery_processes_and_wait
  stop_windows_process_id_and_wait

apps/codex-plus-launcher/src/main.rs
  launcher_main 的 recover_only 分支

scripts/installer/windows/CodexPlusPlus.nsi
  InvokeElevatedRecovery
  TryRecoverAdminMode
  RecoverAdminMode
  StopRunningCodexPlus

crates/codex-plus-core/tests/watcher.rs
apps/codex-plus-manager/src-tauri/tests/windows_subsystem.rs
```

`recover_only` 分支的顺序必须是：

```text
recover_stale_admin_mode_for_shutdown(...)
stop_admin_recovery_processes_and_wait()
return Ok(())
```

## 3. 不得回归的历史问题

- Medium Integrity NSIS 结束不了 High Integrity manager/launcher/recovery，导致 `codex-plus-plus-manager.exe` 仍被锁定。
- 只移除 `taskkill /T` 不能解决权限边界。
- 只处理首次快照会遗漏被 watchdog 重启后的新 PID。
- 单次 terminate 失败立即退出会将瞬时竞态错误当成最终失败。
- recovery 的 PowerShell EncodedCommand 不得超过 NSIS 命令长度限制；当前已使用压缩后的 RunAs payload。

## 4. 回归测试门槛

发布 Windows 安装包前至少运行：

```text
cargo fmt --all -- --check
cargo test -p codex-plus-core --test watcher -- --test-threads=1 --nocapture
cargo test -p codex-plus-core --lib -- --test-threads=1
cargo test -p codex-plus-core --test launcher -- --test-threads=1
cargo test -p codex-plus-manager --test windows_subsystem
npm run check --prefix apps/codex-plus-manager
npm test --prefix apps/codex-plus-manager
npm run vite:build --prefix apps/codex-plus-manager
cargo build --release --workspace
```

watcher 测试必须覆盖：

- 精确映像名和当前 PID 排除。
- 结束顺序。
- 首次结束失败后自动重试。
- PID 自然消失竞态。
- 目标以新 PID 重启后继续结束。
- 连续两轮空集合确认。
- 持续存活目标的短超时测试。
- 原生结束失败后的绝对路径 `/PID ... /F` fallback，且没有 `/T`。
- 运行中的临时 `codex-plus-plus-manager.exe` 锁定自身，强制结束后同路径可成功覆盖。

2026-08-22 已确认的结果：

```text
watcher tests                 30 passed
core lib                      429 passed, 1 ignored
launcher tests                99 passed
Windows subsystem             29 passed
frontend tests                84 passed
TypeScript check              exit 0
release workspace build       exit 0
NSIS build                    exit 0
7-Zip archive test            Everything is Ok, 842 files
High RunAs lock/overwrite     outer 0, inner 0, S-1-16-12288
```

## 5. 当前测试安装包

```text
D:/Codex/Codex++/dist/windows/release-2026-08-22-force-stop-loop-tested/CodexPlusPlus-1.2.50-windows-x64-setup-force-stop-loop-tested.exe
大小：107827469 bytes
SHA-256：FB273709C4DD3AED257E6D51A03FC368C4E235F7F109A37B68CD21CE74421B44
```

该包已通过 NSIS 解包完整性检查；包内 launcher、manager 和 admin shim 与当次 release 编译产物哈希一致。

证据与回滚：

```text
D:/Codex/Codex++/output/upstream-sync-2026-08-22/installer-force-stop-loop/source-changes.patch
D:/Codex/Codex++/output/upstream-sync-2026-08-22/installer-force-stop-loop/verification-record.md
D:/Codex/Codex++/output/upstream-sync-2026-08-22/installer-force-stop-loop/rollback.ps1
```

## 6. 后续合并规则

1. 上游修复同类问题时，以上游实现为基准合并，但必须保留本文的可观测行为和回归测试。
2. 不得在上游合并时删除循环重枚举、新 PID 捕获、双路强制结束或连续空轮确认。
3. 重新打包时使用新的唯一文件名和 SHA-256，不覆盖已验证包。
4. 完整安装器会结束正在运行的 Codex++/ChatGPT；保持当前对话时，使用 High RunAs 临时锁文件 fixture 验证，不直接启动安装器。

## 7. 管理员终端 PowerShell 选择（2026-08-23）

Windows 安装包不得再内置完整 PowerShell 7 portable runtime。`admin-terminal/pwsh.exe` 是 Codex++ 自身的轻量安全 shim，不是 PowerShell 7 本体。

安装器必须：

1. 检测本机 PowerShell 7。
2. 检测到时提示用户选择 PowerShell 7 或 Windows PowerShell 5.1；静默安装默认选择 PowerShell 7。
3. 未检测到时提示并选择 Windows 10/11 自带的 Windows PowerShell 5.1。
4. 安装所选 shim 为 `admin-terminal/pwsh.exe`，并写入 `admin-terminal/shell-mode.txt`。
5. 升级时移除旧版本残留的 `runtime/powershell7` 目录。
6. broker 必须严格遵循 `shell-mode.txt`；旧安装没有该文件时，才允许按 PowerShell 7 → Windows PowerShell 5.1 自动回退。

发布 staging 保留两个明确命名的 shim 输入：

```text
admin-terminal/pwsh-powershell7.exe
admin-terminal/pwsh-windows-powershell.exe
```

两者都是同一安全传输 shim 的选择变体，不包含 PowerShell runtime；真正的 shell 来自本机安装。

## 8. Microsoft Store PowerShell 7 检测（2026-08-23）

PowerShell 7 的 Microsoft Store 安装可能同时创建
`%LOCALAPPDATA%\Microsoft\WindowsApps\pwsh.exe` App Execution Alias。该文件可能为
0 字节 reparse alias，且没有 FileVersion；它不能作为 PowerShell 7 检测结果或执行路径。

安装器和 broker 必须通过固定路径的 Windows PowerShell 5.1 查询
`Appx\Get-AppxPackage -Name Microsoft.PowerShell`，并严格验证：

- 包名为 `Microsoft.PowerShell`；
- 版本主号为 7；
- 包全名符合 `Microsoft.PowerShell_<version>_<arch>__8wekyb3d8bbwe`；
- 安装目录是绝对路径并位于 `WindowsApps`；
- 目录内存在真实 `pwsh.exe`，且不是 reparse alias。

查询通过后，broker 使用真实 Store 包路径，不直接执行 PATH 中的同名 alias；MSI、ZIP、每用户安装与 PATH 检测仍保留为后续候选来源。
