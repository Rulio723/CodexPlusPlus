#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result};
use codex_plus_core::launcher::{
    BridgeReinjector, DefaultLaunchHooks, LaunchHooks, LaunchOptions, launch_and_inject_with_hooks,
};
use codex_plus_core::models::{DeleteResult, ExportResult, SessionRef};
use codex_plus_core::routes::{BridgeContext, BridgeDataService, BridgeRuntimeService};
use codex_plus_core::status::{AdministratorModeStatus, LaunchStatus, StatusStore};
use codex_plus_core::user_scripts::UserScriptManager;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
struct LauncherHooks {
    core: Arc<DefaultLaunchHooks>,
    data: Arc<LauncherDataService>,
    runtime: Arc<LauncherRuntimeService>,
    bridge_context: Arc<Mutex<Option<BridgeContext>>>,
}

impl Default for LauncherHooks {
    fn default() -> Self {
        Self {
            core: Arc::new(DefaultLaunchHooks::default()),
            data: Arc::new(LauncherDataService::default()),
            runtime: Arc::new(LauncherRuntimeService::new(
                9229,
                default_user_script_manager(),
            )),
            bridge_context: Arc::new(Mutex::new(None)),
        }
    }
}

impl LauncherHooks {
    fn watchdog_bridge_context(&self) -> anyhow::Result<BridgeContext> {
        self.bridge_context
            .lock()
            .map_err(|_| anyhow::anyhow!("bridge context lock poisoned"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("bridge context is not initialized"))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let helper_only = args.iter().any(|arg| arg == "--helper-only");
    let recover_only = args.iter().any(|arg| arg == "--recover-admin-mode");
    let options = parse_launch_options(args.iter());
    finalize_launcher_invocation(
        &options,
        helper_only,
        launcher_main(args, helper_only, recover_only, options.clone()).await,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LauncherStatusOwnership {
    Primary,
    SecondaryExistingInstance,
}

#[derive(Debug)]
struct LauncherRunFailure {
    error: anyhow::Error,
    status_ownership: LauncherStatusOwnership,
}

impl LauncherRunFailure {
    fn primary(error: anyhow::Error) -> Self {
        Self {
            error,
            status_ownership: LauncherStatusOwnership::Primary,
        }
    }

    fn secondary_existing_instance(error: anyhow::Error) -> Self {
        Self {
            error,
            status_ownership: LauncherStatusOwnership::SecondaryExistingInstance,
        }
    }
}

impl From<anyhow::Error> for LauncherRunFailure {
    fn from(error: anyhow::Error) -> Self {
        Self::primary(error)
    }
}

fn finalize_launcher_invocation(
    options: &LaunchOptions,
    helper_only: bool,
    invocation: std::result::Result<LauncherStatusOwnership, LauncherRunFailure>,
) -> Result<()> {
    match invocation {
        Ok(_) => Ok(()),
        Err(failure) => {
            let error = failure.error;
            let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                "launcher.run_failed",
                serde_json::json!({
                    "message": "startup failed",
                    "component": launcher_failure_component(&error),
                    "error_chain": codex_plus_core::diagnostic_log::sanitized_error_chain(&error)
                }),
            );
            eprintln!("Codex++ launcher startup failed: {error:#}");
            if !helper_only && failure.status_ownership == LauncherStatusOwnership::Primary {
                let _ = options.status_store.save_latest(&LaunchStatus {
                    status: "failed".to_string(),
                    message: error.to_string(),
                    started_at_ms: current_timestamp_ms(),
                    debug_port: Some(options.debug_port),
                    helper_port: Some(options.helper_port),
                    codex_app: options
                        .app_dir
                        .as_ref()
                        .map(|path| path.to_string_lossy().to_string()),
                    administrator_mode: administrator_mode_status_for_failure(
                        &options.status_store,
                        &error,
                    ),
                });
            }
            Err(error)
        }
    }
}

async fn launcher_main(
    _args: Vec<String>,
    helper_only: bool,
    recover_only: bool,
    options: LaunchOptions,
) -> std::result::Result<LauncherStatusOwnership, LauncherRunFailure> {
    if recover_only {
        codex_plus_core::admin_mode::recover_stale_admin_mode_for_shutdown(
            &codex_plus_core::codex_home::default_codex_home_dir(),
            &codex_plus_core::paths::default_app_state_dir(),
        )?;
        codex_plus_core::watcher::stop_admin_recovery_processes_and_wait()?;
        return Ok(LauncherStatusOwnership::Primary);
    }
    if helper_only {
        let hooks = LauncherHooks::default();
        hooks
            .start_helper(options.helper_port)
            .await
            .map_err(LauncherRunFailure::primary)?;
        std::future::pending::<()>().await;
        hooks.shutdown_helper(options.helper_port).await;
        return Ok(LauncherStatusOwnership::Primary);
    }
    prepare_administrator_mode_startup(&options).await?;
    let Some(_guard) = acquire_single_instance_guard(options.debug_port)? else {
        activate_existing_codex_app(&options)
            .await
            .map_err(LauncherRunFailure::secondary_existing_instance)?;
        return Ok(LauncherStatusOwnership::SecondaryExistingInstance);
    };
    tokio::spawn(async {
        let _ = notify_manager_when_update_available().await;
    });
    stop_standard_codex_before_administrator_launch().map_err(LauncherRunFailure::primary)?;
    let hooks = LauncherHooks::default();
    let handle = launch_and_inject_with_hooks(options, &hooks)
        .await
        .map_err(LauncherRunFailure::primary)?;
    handle
        .wait_for_codex_exit()
        .await
        .map_err(LauncherRunFailure::primary)?;
    Ok(LauncherStatusOwnership::Primary)
}

fn stop_standard_codex_before_administrator_launch() -> anyhow::Result<()> {
    let settings = codex_plus_core::settings::SettingsStore::default().load()?;
    if settings.administrator_mode_enabled {
        codex_plus_core::watcher::stop_codex_processes_and_wait();
    }
    Ok(())
}

async fn prepare_administrator_mode_startup(_options: &LaunchOptions) -> anyhow::Result<()> {
    let settings = codex_plus_core::settings::SettingsStore::default().load()?;
    let recovery = codex_plus_core::admin_mode::recover_stale_admin_mode(
        &codex_plus_core::codex_home::default_codex_home_dir(),
        &codex_plus_core::paths::default_app_state_dir(),
    );
    if let Err(error) = recovery {
        if settings.administrator_mode_enabled {
            return Err(error.context("administrator_mode:recovery"));
        }
        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
            "launcher.stale_admin_recovery_failed_nonfatal",
            serde_json::json!({
                "message": "stale administrator state could not be fully recovered",
                "component": "administrator_mode"
            }),
        );
    }
    if !settings.administrator_mode_enabled {
        return Ok(());
    }
    let current_exe = std::env::current_exe().context("administrator_mode:shim")?;
    let shim_path = current_exe
        .parent()
        .context("administrator_mode:shim: launcher has no parent directory")?
        .join("codex-plus-admin-shim.exe");
    anyhow::ensure!(
        shim_path.is_file(),
        "administrator_mode:shim: administrator shim is missing"
    );
    let terminal_shim_path = current_exe
        .parent()
        .context("administrator_mode:terminal: launcher has no parent directory")?
        .join("admin-terminal")
        .join("pwsh.exe");
    anyhow::ensure!(
        terminal_shim_path.is_file(),
        "administrator_mode:terminal: PowerShell compatibility shim is missing"
    );
    Ok(())
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn administrator_mode_requested(status_store: &codex_plus_core::status::StatusStore) -> bool {
    codex_plus_core::settings::SettingsStore::default()
        .load()
        .map(|settings| settings.administrator_mode_enabled)
        .or_else(|_| {
            status_store.load_latest().map(|status| {
                status
                    .map(|status| status.administrator_mode.requested)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn administrator_mode_status_for_failure(
    status_store: &codex_plus_core::status::StatusStore,
    error: &anyhow::Error,
) -> AdministratorModeStatus {
    if !administrator_mode_requested(status_store) {
        return AdministratorModeStatus::default();
    }

    let error_component = launcher_failure_component(error)
        .strip_prefix("administrator_mode:")
        .map(str::to_owned)
        .or_else(|| Some("runtime".to_string()));
    AdministratorModeStatus {
        requested: true,
        state: "failed".to_string(),
        exec_elevated: false,
        computer_use_elevated: false,
        error_component,
    }
}

fn acquire_single_instance_guard(
    debug_port: u16,
) -> anyhow::Result<Option<codex_plus_core::ports::LoopbackPortGuard>> {
    acquire_single_instance_guard_with_retry(debug_port, true)
}

fn acquire_single_instance_guard_with_retry(
    debug_port: u16,
    allow_stale_recovery: bool,
) -> anyhow::Result<Option<codex_plus_core::ports::LoopbackPortGuard>> {
    match try_acquire_single_instance_guard() {
        Ok(guard) => {
            if let Some(fallback_lock_path) = guard.fallback_path() {
                log_launcher_guard_fallback(fallback_lock_path);
            }
            Ok(Some(guard))
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            log_launcher_already_running(debug_port);
            Ok(None)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            log_launcher_already_running(debug_port);
            if allow_stale_recovery && should_recover_stale_launcher(debug_port) {
                codex_plus_core::watcher::stop_launcher_processes();
                std::thread::sleep(std::time::Duration::from_millis(250));
                return acquire_single_instance_guard_with_retry(debug_port, false);
            }
            Ok(None)
        }
        Err(error) => Err(error)
            .with_context(|| {
                format!(
                    "failed to acquire launcher guard port {}",
                    codex_plus_core::ports::launcher_guard_port()
                )
            })
            .map(Some),
    }
}

fn try_acquire_single_instance_guard() -> std::io::Result<codex_plus_core::ports::LoopbackPortGuard>
{
    codex_plus_core::ports::acquire_resilient_loopback_port_guard(
        codex_plus_core::ports::launcher_guard_port(),
    )
}

fn log_launcher_guard_fallback(fallback_lock_path: &Path) {
    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
        "launcher.guard_fallback",
        json!({
            "requested_guard_port": codex_plus_core::ports::launcher_guard_port(),
            "fallback_lock_path": fallback_lock_path
        }),
    );
}

fn should_recover_stale_launcher(debug_port: u16) -> bool {
    let has_codex_process = !codex_plus_core::watcher::find_codex_processes().is_empty();
    let cdp_listening = codex_plus_core::watcher::cdp_listening(debug_port);
    let recover =
        codex_plus_core::watcher::should_recover_stale_launcher(has_codex_process, cdp_listening);
    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
        "launcher.stale_recovery_check",
        json!({
            "debug_port": debug_port,
            "has_codex_process": has_codex_process,
            "cdp_listening": cdp_listening,
            "recover": recover
        }),
    );
    recover
}

async fn activate_existing_codex_app(options: &LaunchOptions) -> anyhow::Result<()> {
    let hooks = LauncherHooks::default();
    activate_existing_codex_app_with_hooks(
        &hooks,
        options,
        codex_plus_core::watcher::find_codex_processes,
        |process_id| {
            #[cfg(windows)]
            {
                codex_plus_core::windows_activate_process_window(process_id)
            }
            #[cfg(not(windows))]
            {
                let _ = process_id;
                false
            }
        },
        |outcome| {
            let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                "launcher.activate_existing_codex",
                json!({
                    "app_dir": outcome.app_dir.to_string_lossy(),
                    "debug_port": outcome.debug_port,
                    "helper_port": outcome.helper_port,
                    "requested_helper_port": outcome.helper_port,
                    "process_ids": outcome.process_ids,
                    "activated": outcome.focused_existing_window,
                    "launch_ok": outcome.packaged_activation_succeeded,
                    "launch_error": outcome.launch_error,
                }),
            );
        },
    )
    .await
}

async fn activate_existing_codex_app_with_hooks<H, FindProcesses, FocusWindow, RecordOutcome>(
    hooks: &H,
    options: &LaunchOptions,
    find_processes: FindProcesses,
    mut focus_window: FocusWindow,
    record_outcome: RecordOutcome,
) -> anyhow::Result<()>
where
    H: LaunchHooks + ?Sized,
    FindProcesses: FnOnce() -> Vec<u32>,
    FocusWindow: FnMut(u32) -> bool,
    RecordOutcome: FnOnce(ExistingInstanceActivationOutcome),
{
    let settings = hooks.load_settings().await?;
    let app_dir = hooks.resolve_app_dir(options.app_dir.as_deref(), &settings)?;
    let has_pending_recovery = hooks.has_pending_remote_control_session_recoveries();
    let blocking_process_ids = if has_pending_recovery {
        codex_plus_core::watcher::find_session_index_cleanup_blocking_processes()
    } else {
        Vec::new()
    };
    if should_finalize_pending_remote_control_recovery(has_pending_recovery, &blocking_process_ids)
    {
        hooks.run_remote_control_session_recovery().await?;
    } else if has_pending_recovery {
        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
            "launcher.remote_control_session_finalization_deferred_existing_app",
            json!({"blocking_process_ids": blocking_process_ids}),
        );
    }
    if settings.administrator_mode_enabled {
        return activate_existing_administrator_session(options, &app_dir).await;
    }
    if let Err(error) = hooks.ensure_plugin_marketplace_config(&settings).await {
        let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
            "launcher.plugin_marketplace_config_failed_nonfatal_existing_app",
            json!({"message": error.to_string()}),
        );
    }
    let launch_result = hooks
        .launch_codex(
            &app_dir,
            options.debug_port,
            &settings,
            &settings.codex_extra_args,
        )
        .await;
    let process_ids = find_processes();
    let activated = process_ids.iter().copied().any(&mut focus_window);
    let packaged_activation_succeeded = launch_result.is_ok();
    let activation_succeeded = packaged_activation_succeeded || activated;
    record_outcome(ExistingInstanceActivationOutcome {
        app_dir,
        debug_port: options.debug_port,
        helper_port: options.helper_port,
        process_ids,
        focused_existing_window: activated,
        packaged_activation_succeeded,
        launch_error: launch_result.as_ref().err().map(|error| error.to_string()),
    });
    if activation_succeeded {
        Ok(())
    } else {
        Err(launch_result.expect_err("failed packaged activation must retain its error"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingInstanceActivationOutcome {
    app_dir: PathBuf,
    debug_port: u16,
    helper_port: u16,
    process_ids: Vec<u32>,
    focused_existing_window: bool,
    packaged_activation_succeeded: bool,
    launch_error: Option<String>,
}

async fn activate_existing_administrator_session(
    options: &LaunchOptions,
    app_dir: &Path,
) -> anyhow::Result<()> {
    let activated_process_id =
        codex_plus_core::launcher::activate_existing_administrator_session_with(
            101,
            codex_plus_core::watcher::find_codex_processes,
            |process_id| {
                #[cfg(windows)]
                {
                    codex_plus_core::windows_activate_process_window(process_id)
                }
                #[cfg(not(windows))]
                {
                    let _ = process_id;
                    false
                }
            },
            || tokio::time::sleep(Duration::from_millis(100)),
        )
        .await?;
    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
        "launcher.activate_existing_administrator_session",
        json!({
            "app_dir": app_dir.to_string_lossy(),
            "debug_port": options.debug_port,
            "process_id": activated_process_id,
            "activated": true
        }),
    );
    Ok(())
}

fn should_finalize_pending_remote_control_recovery(
    has_pending_recovery: bool,
    blocking_process_ids: &[u32],
) -> bool {
    has_pending_recovery && blocking_process_ids.is_empty()
}

fn log_launcher_already_running(debug_port: u16) {
    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
        "launcher.already_running",
        json!({
            "guard_port": codex_plus_core::ports::launcher_guard_port(),
            "debug_port": debug_port
        }),
    );
}

async fn notify_manager_when_update_available() -> anyhow::Result<bool> {
    let update =
        codex_plus_core::update::check_for_update(codex_plus_core::version::VERSION).await?;
    if !update.update_available {
        return Ok(false);
    }
    open_manager_with_update_prompt()?;
    Ok(true)
}

fn open_manager_with_update_prompt() -> anyhow::Result<()> {
    codex_plus_core::install::spawn_companion(
        codex_plus_core::install::MANAGER_BINARY,
        ["--show-update"],
    )
    .map(|_| ())
    .map_err(|error| anyhow::anyhow!("启动管理工具失败：{error}"))
}

fn launcher_failure_component(error: &anyhow::Error) -> &'static str {
    const ADMIN_COMPONENTS: &[&str] = &[
        "recovery",
        "identity",
        "shim",
        "terminal",
        "job",
        "exec",
        "computer_use_config",
        "computer_use",
        "app_server",
        "environment",
        "runtime",
        "activation",
    ];
    for message in error.chain().map(ToString::to_string) {
        if let Some(component) = ADMIN_COMPONENTS
            .iter()
            .find(|component| message.contains(&format!("administrator_mode:{component}")))
        {
            return match *component {
                "recovery" => "administrator_mode:recovery",
                "identity" => "administrator_mode:identity",
                "shim" => "administrator_mode:shim",
                "terminal" => "administrator_mode:terminal",
                "job" => "administrator_mode:job",
                "exec" => "administrator_mode:exec",
                "computer_use_config" => "administrator_mode:computer_use_config",
                "computer_use" => "administrator_mode:computer_use",
                "app_server" => "administrator_mode:app_server",
                "environment" => "administrator_mode:environment",
                "runtime" => "administrator_mode:runtime",
                "activation" => "administrator_mode:activation",
                _ => unreachable!(),
            };
        }
    }
    "launcher"
}

fn parse_launch_options<I, S>(args: I) -> LaunchOptions
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut options = LaunchOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_ref() {
            "--app-path" => {
                if let Some(value) = iter.next() {
                    let value = value.as_ref().trim();
                    if !value.is_empty() {
                        options.app_dir = Some(PathBuf::from(value));
                    }
                }
            }
            "--debug-port" => {
                if let Some(value) = iter.next() {
                    if let Ok(port) = value.as_ref().parse::<u16>() {
                        options.debug_port = port;
                    }
                }
            }
            "--helper-port" => {
                if let Some(value) = iter.next() {
                    if let Ok(port) = value.as_ref().parse::<u16>() {
                        options.helper_port = port;
                    }
                }
            }
            _ => {}
        }
    }
    options
}

#[async_trait::async_trait(?Send)]
impl LaunchHooks for LauncherHooks {
    fn resolve_app_dir(
        &self,
        app_dir: Option<&std::path::Path>,
        settings: &codex_plus_core::settings::BackendSettings,
    ) -> anyhow::Result<std::path::PathBuf> {
        self.core.resolve_app_dir(app_dir, settings)
    }

    fn select_debug_port(&self, requested: u16) -> u16 {
        self.core.select_debug_port(requested)
    }

    fn select_helper_port(&self, requested: u16) -> u16 {
        self.core.select_helper_port(requested)
    }

    async fn load_settings(&self) -> anyhow::Result<codex_plus_core::settings::BackendSettings> {
        self.core.load_settings().await
    }

    fn cleanup_unsupported_config(&self) -> anyhow::Result<()> {
        self.core.cleanup_unsupported_config()
    }

    async fn run_provider_sync(&self) -> anyhow::Result<()> {
        let _ = tokio::task::spawn_blocking(|| codex_plus_data::run_provider_sync(None))
            .await
            .map_err(|error| anyhow::anyhow!("provider sync task failed: {error}"))?;
        Ok(())
    }

    fn has_pending_remote_control_session_recoveries(&self) -> bool {
        codex_plus_core::paths::default_pending_remote_control_recovery_path().exists()
    }

    fn remote_control_session_recovery_is_safe_to_run(&self) -> bool {
        codex_plus_core::watcher::find_session_index_cleanup_blocking_processes().is_empty()
    }

    async fn run_remote_control_session_recovery(&self) -> anyhow::Result<()> {
        let outcomes = tokio::task::spawn_blocking(|| {
            let requests = codex_plus_core::remote_control_recovery::load_pending_remote_control_recoveries(None)?;
            let settings = codex_plus_core::settings::SettingsStore::default()
                .load()?;
            let mut outcomes = Vec::with_capacity(requests.len());
            for request in requests {
                let current_profile = settings
                    .relay_profiles
                    .iter()
                    .find(|profile| profile.id == request.profile_id);
                if remote_control_recovery_is_superseded_by_openai(&settings, &request) {
                    let completion_error =
                        codex_plus_core::remote_control_recovery::complete_pending_remote_control_recovery(
                            None,
                            &request.thread_id,
                        )
                        .err()
                        .map(|error| error.to_string());
                    let completed = completion_error.is_none();
                    outcomes.push((
                        request,
                        codex_plus_data::ProviderSyncResult {
                            status: if completed {
                                codex_plus_data::ProviderSyncStatus::Synced
                            } else {
                                codex_plus_data::ProviderSyncStatus::Skipped
                            },
                            message: if completed {
                                "Remote Control session finalization discarded after switching to OpenAI session identity".to_string()
                            } else {
                                "Remote Control session finalization could not discard the superseded recovery request".to_string()
                            },
                            target_provider: "openai".to_string(),
                            backup_dir: None,
                            changed_session_files: 0,
                            sqlite_rows_updated: 0,
                            sqlite_provider_rows_updated: 0,
                            sqlite_user_event_rows_updated: 0,
                            sqlite_cwd_rows_updated: 0,
                            sqlite_catalog_rows_inserted: 0,
                            sqlite_catalog_rows_removed: 0,
                            updated_workspace_roots: 0,
                            skipped_locked_rollout_files: Vec::new(),
                            encrypted_content_warning: None,
                            repair_audit: codex_plus_data::ProviderSyncAudit::default(),
                        },
                        completion_error,
                    ));
                    continue;
                }
                let request_is_current = settings.active_relay_id == request.profile_id
                    && current_profile.is_some_and(|profile| {
                    codex_plus_core::remote_control_recovery::config_generation(
                        profile,
                        &request.target_provider,
                    ) == request.config_generation
                });
                if !request_is_current {
                    outcomes.push((
                        request,
                        codex_plus_data::ProviderSyncResult {
                            status: codex_plus_data::ProviderSyncStatus::Skipped,
                            message: "Remote Control session finalization deferred after relay profile changed".to_string(),
                            target_provider: String::new(),
                            backup_dir: None,
                            changed_session_files: 0,
                            sqlite_rows_updated: 0,
                            sqlite_provider_rows_updated: 0,
                            sqlite_user_event_rows_updated: 0,
                            sqlite_cwd_rows_updated: 0,
                            sqlite_catalog_rows_inserted: 0,
                            sqlite_catalog_rows_removed: 0,
                            updated_workspace_roots: 0,
                            skipped_locked_rollout_files: Vec::new(),
                            encrypted_content_warning: None,
                            repair_audit: codex_plus_data::ProviderSyncAudit::default(),
                        },
                        None,
                    ));
                    continue;
                }
                let result = codex_plus_data::run_remote_control_session_finalization_for_thread_with_target(
                    None,
                    &request.thread_id,
                    &request.target_provider,
                );
                let completed = result.status == codex_plus_data::ProviderSyncStatus::Synced;
                let completion_error = if completed {
                    codex_plus_core::remote_control_recovery::complete_pending_remote_control_recovery(
                        None,
                        &request.thread_id,
                    )
                    .err()
                    .map(|error| error.to_string())
                } else {
                    None
                };
                outcomes.push((request, result, completion_error));
            }
            Ok::<_, anyhow::Error>(outcomes)
        })
        .await
        .map_err(|error| anyhow::anyhow!("Remote Control session recovery task failed: {error}"))?;
        match outcomes {
            Ok(outcomes) => {
                for (request, result, completion_error) in outcomes {
                    let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                        "launcher.remote_control_session_finalization",
                        json!({
                            "thread_id": request.thread_id,
                            "profile_id": request.profile_id,
                            "target_provider": request.target_provider,
                            "config_generation": request.config_generation,
                            "status": result.status,
                            "message": result.message,
                            "completion_error": completion_error
                        }),
                    );
                }
            }
            Err(error) => {
                let _ = codex_plus_core::diagnostic_log::append_diagnostic_log(
                    "launcher.remote_control_session_finalization_failed_nonfatal",
                    json!({"message": error.to_string()}),
                );
            }
        }
        Ok(())
    }

    async fn apply_active_relay_profile(
        &self,
        settings: &codex_plus_core::settings::BackendSettings,
    ) -> anyhow::Result<()> {
        self.core.apply_active_relay_profile(settings).await
    }

    async fn ensure_plugin_marketplace_config(
        &self,
        settings: &codex_plus_core::settings::BackendSettings,
    ) -> anyhow::Result<()> {
        self.core.ensure_plugin_marketplace_config(settings).await
    }

    async fn start_helper(&self, helper_port: u16) -> anyhow::Result<()> {
        self.core.start_helper(helper_port).await
    }

    async fn start_administrator_mode(
        &self,
        settings: &codex_plus_core::settings::BackendSettings,
        app_dir: &Path,
    ) -> anyhow::Result<Option<codex_plus_core::launcher::AdminModeLease>> {
        self.core.start_administrator_mode(settings, app_dir).await
    }

    fn stop_administrator_mode(
        &self,
        lease: codex_plus_core::launcher::AdminModeLease,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        self.core.stop_administrator_mode(lease)
    }

    async fn launch_codex(
        &self,
        app_dir: &Path,
        debug_port: u16,
        settings: &codex_plus_core::settings::BackendSettings,
        extra_args: &[String],
    ) -> anyhow::Result<codex_plus_core::launcher::CodexLaunch> {
        self.core
            .launch_codex(app_dir, debug_port, settings, extra_args)
            .await
    }

    async fn bridge_context(
        &self,
        debug_port: u16,
        app_dir: &Path,
    ) -> anyhow::Result<Option<BridgeContext>> {
        self.runtime.set_debug_port(debug_port);
        let ctx = BridgeContext::core_with_data_and_app_dir(
            self.runtime.clone(),
            self.data.clone(),
            app_dir.to_path_buf(),
        );
        *self
            .bridge_context
            .lock()
            .map_err(|_| anyhow::anyhow!("bridge context lock poisoned"))? = Some(ctx.clone());
        Ok(Some(ctx))
    }

    async fn inject_bridge(
        &self,
        debug_port: u16,
        helper_port: u16,
        ctx: BridgeContext,
    ) -> anyhow::Result<()> {
        inject_with_context(debug_port, helper_port, ctx, self.runtime.clone()).await
    }

    async fn inject(&self, debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
        self.core.inject(debug_port, helper_port).await
    }

    async fn start_bridge_watchdog(&self, debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
        let ctx = self.watchdog_bridge_context()?;
        let runtime = self.runtime.clone();
        let reinjector: BridgeReinjector = Arc::new(move || {
            let ctx = ctx.clone();
            let runtime = runtime.clone();
            Box::pin(
                async move { inject_with_context(debug_port, helper_port, ctx, runtime).await },
            )
        });
        self.core.set_bridge_reinjector(reinjector).await;
        self.core
            .start_bridge_watchdog(debug_port, helper_port)
            .await
    }

    async fn write_status(&self, status: &str) {
        self.core.write_status(status).await;
    }

    async fn wait_for_codex_exit(
        &self,
        launch: &codex_plus_core::launcher::CodexLaunch,
        debug_port: u16,
    ) -> anyhow::Result<()> {
        self.core.wait_for_codex_exit(launch, debug_port).await
    }

    async fn shutdown_helper(&self, helper_port: u16) {
        self.core.shutdown_helper(helper_port).await;
    }

    async fn terminate_codex(&self, launch: &codex_plus_core::launcher::CodexLaunch) {
        self.core.terminate_codex(launch).await;
    }
}

#[derive(Debug, Clone)]
struct LauncherDataService {
    db_path: PathBuf,
    backup_dir: PathBuf,
}

impl Default for LauncherDataService {
    fn default() -> Self {
        Self {
            db_path: default_codex_db_path(),
            backup_dir: codex_plus_core::paths::default_app_state_dir().join("backups"),
        }
    }
}

#[async_trait::async_trait]
impl BridgeDataService for LauncherDataService {
    async fn delete(&self, session: SessionRef) -> anyhow::Result<DeleteResult> {
        let db_paths = self.candidate_db_paths();
        let backup_store = codex_plus_data::BackupStore::new(self.backup_dir.clone());
        tokio::task::spawn_blocking(move || {
            codex_plus_data::delete_local_from_paths(
                db_paths,
                backup_store,
                &session,
                Some(&codex_plus_core::codex_sqlite::default_codex_home_dir()),
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("delete task failed: {error}"))
    }

    async fn undo(&self, undo_token: String) -> anyhow::Result<DeleteResult> {
        let adapter = self.storage_adapter();
        tokio::task::spawn_blocking(move || adapter.undo(&undo_token))
            .await
            .map_err(|error| anyhow::anyhow!("undo task failed: {error}"))
    }

    async fn export_markdown(&self, session: SessionRef) -> anyhow::Result<ExportResult> {
        let db_paths = self.candidate_db_paths();
        tokio::task::spawn_blocking(move || {
            codex_plus_data::export_markdown_from_paths(db_paths, &session)
        })
        .await
        .map_err(|error| anyhow::anyhow!("export markdown task failed: {error}"))
    }

    async fn thread_usage_history(&self, session: SessionRef) -> anyhow::Result<Value> {
        let adapter = self.storage_adapter();
        tokio::task::spawn_blocking(move || adapter.codex_thread_usage_history(&session))
            .await
            .map_err(|error| anyhow::anyhow!("thread usage history task failed: {error}"))
    }

    async fn find_archived_thread_by_title(
        &self,
        title: String,
    ) -> anyhow::Result<Option<SessionRef>> {
        let adapter = self.storage_adapter();
        tokio::task::spawn_blocking(move || adapter.find_archived_thread_by_title(&title))
            .await
            .map_err(|error| anyhow::anyhow!("archived lookup task failed: {error}"))
    }

    async fn move_thread_workspace(
        &self,
        session: SessionRef,
        target_cwd: String,
    ) -> anyhow::Result<Value> {
        let db_paths = self.candidate_db_paths();
        let backup_store = codex_plus_data::BackupStore::new(self.backup_dir.clone());
        tokio::task::spawn_blocking(move || {
            codex_plus_data::move_codex_thread_workspace_from_paths(
                db_paths,
                backup_store,
                &session,
                &target_cwd,
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("move thread workspace task failed: {error}"))
    }

    async fn thread_sort_key(&self, session: SessionRef) -> anyhow::Result<Value> {
        let db_paths = self.candidate_db_paths();
        let backup_dir = self.backup_dir.clone();
        tokio::task::spawn_blocking(move || {
            let mut result = json!({
                "status": "failed",
                "session_id": session.session_id,
                "message": "Thread not found in local storage",
            });
            for db_path in db_paths {
                let adapter = codex_plus_data::SQLiteStorageAdapter::new(
                    db_path,
                    codex_plus_data::BackupStore::new(backup_dir.clone()),
                );
                let candidate = adapter.codex_thread_sort_key(&session);
                if candidate.get("status").and_then(Value::as_str) == Some("ok") {
                    return candidate;
                }
                result = candidate;
            }
            result
        })
        .await
        .map_err(|error| anyhow::anyhow!("thread sort key task failed: {error}"))
    }

    async fn thread_sort_keys(&self, sessions: Vec<SessionRef>) -> anyhow::Result<Value> {
        let db_paths = self.candidate_db_paths();
        let backup_dir = self.backup_dir.clone();
        tokio::task::spawn_blocking(move || {
            let mut sort_keys = Vec::new();
            let mut seen_ids = Vec::new();
            let mut catalog_rows: Vec<Value> = Vec::new();
            let mut seen_catalog_ids: Vec<String> = Vec::new();
            let mut any_supported_db = false;
            for db_path in db_paths {
                let adapter = codex_plus_data::SQLiteStorageAdapter::new(
                    db_path,
                    codex_plus_data::BackupStore::new(backup_dir.clone()),
                );
                let candidate = adapter.codex_thread_sort_keys(&sessions);
                if candidate.get("status").and_then(Value::as_str) == Some("ok") {
                    any_supported_db = true;
                    if let Some(items) = candidate.get("sort_keys").and_then(Value::as_array) {
                        for item in items {
                            let Some(session_id) = item.get("session_id").and_then(Value::as_str)
                            else {
                                continue;
                            };
                            if seen_ids.iter().any(|seen| seen == session_id) {
                                continue;
                            }
                            seen_ids.push(session_id.to_string());
                            sort_keys.push(item.clone());
                        }
                    }
                }

                let catalog = adapter.codex_local_catalog_rows(&sessions);
                if catalog.get("status").and_then(Value::as_str) == Some("ok") {
                    any_supported_db = true;
                    if let Some(items) = catalog.get("catalog_rows").and_then(Value::as_array) {
                        for item in items {
                            let Some(session_id) = item.get("session_id").and_then(Value::as_str)
                            else {
                                continue;
                            };
                            if let Some(existing) = catalog_rows.iter_mut().find(|existing| {
                                existing.get("session_id").and_then(Value::as_str)
                                    == Some(session_id)
                            }) {
                                if let (Some(existing), Some(item)) =
                                    (existing.as_object_mut(), item.as_object())
                                {
                                    for field in [
                                        "source_detail_present",
                                        "rollout_exists",
                                        "internal_subagent",
                                    ] {
                                        let merged = existing
                                            .get(field)
                                            .and_then(Value::as_bool)
                                            .unwrap_or(false)
                                            || item
                                                .get(field)
                                                .and_then(Value::as_bool)
                                                .unwrap_or(false);
                                        existing.insert(field.to_string(), json!(merged));
                                    }
                                }
                            } else if !seen_catalog_ids.iter().any(|seen| seen == session_id) {
                                seen_catalog_ids.push(session_id.to_string());
                                catalog_rows.push(item.clone());
                            }
                        }
                    }
                }
            }
            if any_supported_db {
                json!({"status": "ok", "sort_keys": sort_keys, "catalog_rows": catalog_rows})
            } else {
                json!({
                    "status": "failed",
                    "message": "Unsupported local storage schema",
                    "sort_keys": [],
                    "catalog_rows": [],
                })
            }
        })
        .await
        .map_err(|error| anyhow::anyhow!("thread sort keys task failed: {error}"))
    }

    async fn recover_remote_control_session(&self, thread_id: String) -> anyhow::Result<Value> {
        let settings = codex_plus_core::settings::SettingsStore::default()
            .load()
            .unwrap_or_default();
        let profile = settings.active_relay_profile();
        if !settings.relay_profiles_enabled
            || profile.relay_mode != codex_plus_core::settings::RelayMode::Official
            || !profile.official_mix_api_key
        {
            return Ok(json!({
                "status": "skipped",
                "message": "Remote Control session recovery is disabled for the active profile"
            }));
        }
        let home = codex_plus_core::codex_sqlite::default_codex_home_dir();
        let target_provider =
            codex_plus_core::model_catalog::codex_model_provider_for_relay_profile(&home, &profile);
        if target_provider.trim().is_empty() || target_provider == "openai" {
            return Ok(json!({
                "status": "skipped",
                "message": "Remote Control session recovery requires a non-openai target provider"
            }));
        }
        let candidate_thread_id = thread_id.clone();
        let candidate = tokio::task::spawn_blocking(move || {
            codex_plus_data::remote_control_session_recovery_candidate_exists(
                None,
                &candidate_thread_id,
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("Remote Control candidate check failed: {error}"))??;
        if !candidate {
            return Ok(json!({
                "status": "skipped",
                "message": "Remote Control session recovery is waiting for a recent openai thread"
            }));
        }
        let request = codex_plus_core::remote_control_recovery::PendingRemoteControlRecovery {
            thread_id: thread_id.clone(),
            profile_id: profile.id.clone(),
            target_provider: target_provider.clone(),
            config_generation: codex_plus_core::remote_control_recovery::config_generation(
                &profile,
                &target_provider,
            ),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };
        codex_plus_core::remote_control_recovery::enqueue_pending_remote_control_recovery(
            None, request,
        )?;
        tokio::task::spawn_blocking(move || {
            serde_json::to_value(
                codex_plus_data::run_remote_control_session_catalog_recovery_for_thread_with_target(
                    None,
                    &thread_id,
                    &target_provider,
                ),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .map_err(|error| anyhow::anyhow!("Remote Control session recovery task failed: {error}"))?
    }

    async fn export_session_file(&self, session: SessionRef) -> anyhow::Result<Value> {
        LauncherDataService::export_session_file(self, session).await
    }

    async fn import_session_file(&self, payload: Value) -> anyhow::Result<Value> {
        LauncherDataService::import_session_file(self, payload).await
    }
}

impl LauncherDataService {
    fn candidate_db_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.db_path.clone()];
        for path in codex_plus_core::codex_sqlite::codex_session_db_paths_from_home(
            &codex_plus_core::codex_sqlite::default_codex_home_dir(),
        ) {
            if !paths.iter().any(|candidate| candidate == &path) {
                paths.push(path);
            }
        }
        for path in codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(
            &codex_plus_core::codex_sqlite::default_codex_home_dir(),
        ) {
            if !paths.iter().any(|candidate| candidate == &path) {
                paths.push(path);
            }
        }
        paths
    }

    fn storage_adapter(&self) -> codex_plus_data::SQLiteStorageAdapter {
        let allowed_db_paths = self.candidate_db_paths();
        codex_plus_data::SQLiteStorageAdapter::new(
            self.db_path.clone(),
            codex_plus_data::BackupStore::new(self.backup_dir.clone()),
        )
        .with_allowed_db_paths(allowed_db_paths)
        .with_codex_home(codex_plus_core::codex_sqlite::default_codex_home_dir())
    }

    async fn export_session_file(&self, session: SessionRef) -> anyhow::Result<Value> {
        let home = codex_plus_core::codex_sqlite::default_codex_home_dir();
        tokio::task::spawn_blocking(move || {
            codex_plus_core::session_share::export_rollout(&home, &session.session_id)
        })
        .await
        .map_err(|error| anyhow::anyhow!("session export task failed: {error}"))?
    }

    async fn import_session_file(&self, payload: Value) -> anyhow::Result<Value> {
        let home = codex_plus_core::codex_sqlite::default_codex_home_dir();
        tokio::task::spawn_blocking(move || {
            codex_plus_core::session_share::import_rollout(&home, &payload)
        })
        .await
        .map_err(|error| anyhow::anyhow!("session import task failed: {error}"))?
    }
}

struct LauncherRuntimeService {
    debug_port: Mutex<u16>,
    websocket_url: Mutex<Option<String>>,
    user_scripts: UserScriptManager,
}

impl LauncherRuntimeService {
    fn new(debug_port: u16, user_scripts: UserScriptManager) -> Self {
        Self {
            debug_port: Mutex::new(debug_port),
            websocket_url: Mutex::new(None),
            user_scripts,
        }
    }

    fn set_debug_port(&self, debug_port: u16) {
        *self.debug_port.lock().unwrap() = debug_port;
    }

    fn set_websocket_url(&self, websocket_url: &str) {
        *self.websocket_url.lock().unwrap() = Some(websocket_url.to_string());
    }
}

#[async_trait::async_trait]
impl BridgeRuntimeService for LauncherRuntimeService {
    async fn user_script_inventory(&self) -> anyhow::Result<Value> {
        self.user_scripts.inventory()
    }

    async fn user_script_inventory_with_runtime_status(
        &self,
        payload: Value,
    ) -> anyhow::Result<Value> {
        self.user_scripts
            .inventory_with_runtime_status(payload.get("runtime_status"))
    }

    async fn set_user_scripts_enabled(&self, enabled: bool) -> anyhow::Result<Value> {
        self.user_scripts.set_global_enabled(enabled)?;
        self.user_scripts.inventory()
    }

    async fn set_user_script_enabled(&self, key: String, enabled: bool) -> anyhow::Result<Value> {
        self.user_scripts.set_script_enabled(&key, enabled)?;
        self.user_scripts.inventory()
    }

    async fn delete_user_script(&self, key: String) -> anyhow::Result<Value> {
        self.user_scripts.delete_user_script(&key)?;
        self.user_scripts.inventory()
    }

    async fn reload_user_scripts(&self) -> anyhow::Result<Value> {
        let bundle = self.user_scripts.build_enabled_bundle()?;
        let websocket_url = self.websocket_url.lock().unwrap().clone();
        if let Some(websocket_url) = websocket_url.filter(|_| !bundle.trim().is_empty()) {
            codex_plus_core::bridge::evaluate_script(&websocket_url, &bundle).await?;
        }
        self.user_scripts.inventory()
    }

    async fn open_devtools(&self) -> anyhow::Result<Value> {
        let debug_port = *self.debug_port.lock().unwrap();
        let targets = codex_plus_core::cdp::list_targets(debug_port).await?;
        let target = codex_plus_core::cdp::pick_page_target(&targets)?;
        let url = codex_plus_core::routes::devtools_url(debug_port, &target.id);
        open_url(&url)?;
        Ok(json!({
            "status": "ok",
            "target_id": target.id,
            "url": url
        }))
    }

    async fn open_manager(&self) -> anyhow::Result<Value> {
        let target = codex_plus_core::install::spawn_companion(
            codex_plus_core::install::MANAGER_BINARY,
            std::iter::empty::<&str>(),
        )
        .map_err(|error| anyhow::anyhow!("启动管理工具失败：{error}"))?;
        Ok(json!({
            "status": "ok",
            "path": target
        }))
    }

    async fn open_transient_manager(&self) -> anyhow::Result<Value> {
        let target = codex_plus_core::install::spawn_companion(
            codex_plus_core::install::MANAGER_BINARY,
            ["--transient"],
        )
        .map_err(|error| anyhow::anyhow!("启动管理工具失败：{error}"))?;
        Ok(json!({
            "status": "ok",
            "path": target
        }))
    }

    async fn backend_status(&self) -> anyhow::Result<Value> {
        Ok(
            json!({"status": "ok", "message": "后端已连接", "version": codex_plus_core::version::VERSION}),
        )
    }

    async fn codex_model_catalog(&self) -> anyhow::Result<Value> {
        Ok(codex_plus_core::model_catalog::read_codex_model_catalog().await)
    }

    async fn zed_remote_status(&self) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::zed_remote_status())
    }

    async fn resolve_zed_remote_host(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::resolve_ssh_target_response(
            &payload,
        ))
    }

    async fn fallback_zed_remote_request(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::fallback_open_request_response(
            &payload,
        ))
    }

    async fn open_zed_remote(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::open_zed_remote(&payload))
    }

    async fn list_zed_remote_projects(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::list_zed_remote_projects_response(&payload))
    }

    async fn remember_zed_remote_project(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::remember_zed_remote_project_response(&payload))
    }

    async fn forget_zed_remote_project(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::zed_remote::forget_zed_remote_project_response(&payload))
    }

    async fn upstream_worktree_status(&self) -> anyhow::Result<Value> {
        Ok(codex_plus_core::upstream_worktree::status_response())
    }

    async fn upstream_worktree_defaults(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::upstream_worktree::defaults_response(
            &payload,
        ))
    }

    async fn upstream_worktree_prepare(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::upstream_worktree::prepare_response(
            &payload,
        ))
    }

    async fn upstream_worktree_create(&self, payload: Value) -> anyhow::Result<Value> {
        Ok(codex_plus_core::upstream_worktree::create_response(
            &payload,
        ))
    }
}

async fn inject_with_context(
    debug_port: u16,
    helper_port: u16,
    ctx: BridgeContext,
    runtime: Arc<LauncherRuntimeService>,
) -> anyhow::Result<()> {
    let mut last_error = None;
    for _ in 0..20 {
        match try_inject_with_context(debug_port, helper_port, ctx.clone(), runtime.clone()).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Codex injection failed")))
}

fn remote_control_recovery_is_superseded_by_openai(
    settings: &codex_plus_core::settings::BackendSettings,
    request: &codex_plus_core::remote_control_recovery::PendingRemoteControlRecovery,
) -> bool {
    settings.active_relay_id == request.profile_id
        && settings.active_relay_session_provider()
            == codex_plus_core::settings::RelaySessionProvider::Openai
}

async fn try_inject_with_context(
    debug_port: u16,
    helper_port: u16,
    ctx: BridgeContext,
    runtime: Arc<LauncherRuntimeService>,
) -> anyhow::Result<()> {
    let targets = codex_plus_core::cdp::list_targets(debug_port).await?;
    let target = codex_plus_core::cdp::pick_injectable_codex_page_target(&targets)?;
    let websocket_url = target
        .web_socket_debugger_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("selected CDP target has no websocket URL"))?;
    runtime.set_websocket_url(websocket_url);
    let settings = codex_plus_core::settings::SettingsStore::default()
        .load()
        .unwrap_or_default();
    let script = codex_plus_core::assets::injection_script_with_settings(helper_port, &settings);
    let user_bundle = runtime
        .user_scripts
        .build_enabled_bundle()
        .unwrap_or_default();
    let new_document_scripts = if user_bundle.is_empty() {
        vec![script]
    } else {
        vec![script, user_bundle]
    };
    codex_plus_core::bridge::install_bridge(
        websocket_url,
        codex_plus_core::bridge::BRIDGE_BINDING_NAME,
        Arc::new(move |path, payload| {
            let ctx = ctx.clone();
            Box::pin(async move {
                Ok(codex_plus_core::routes::handle_bridge_request(ctx, &path, payload).await)
            })
        }),
        &new_document_scripts,
    )
    .await
}

fn default_codex_db_path() -> PathBuf {
    codex_plus_core::codex_sqlite::codex_session_db_path()
}

fn open_url(url: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        codex_plus_core::windows_open_url(url)
            .map_err(|error| anyhow::anyhow!("failed to open DevTools URL: {error}"))
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("failed to open DevTools URL: {error}"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("failed to open DevTools URL: {error}"))
    }

    #[cfg(not(any(windows, target_os = "macos", unix)))]
    {
        let _ = url;
        anyhow::bail!("opening DevTools URL is not supported on this platform")
    }
}

fn default_user_script_manager() -> UserScriptManager {
    let config_dir = default_user_scripts_config_dir();
    UserScriptManager::new(
        builtin_user_scripts_dir(),
        config_dir.join("user_scripts"),
        config_dir.join("user_scripts.json"),
    )
}

fn default_user_scripts_config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(roaming) = std::env::var_os("APPDATA") {
            return PathBuf::from(roaming).join("Codex++");
        }
        if let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
            return home.join("AppData").join("Roaming").join("Codex++");
        }
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("Codex++")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STATUS_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn owner_status_fixture() -> (LaunchOptions, PathBuf, Vec<u8>) {
        let path = std::env::temp_dir().join(format!(
            "codex-plus-launcher-secondary-status-{}-{}.json",
            std::process::id(),
            NEXT_STATUS_FIXTURE_ID.fetch_add(1, Ordering::Relaxed),
        ));
        let owner_bytes = br#"{
  "status": "running-owner-sentinel",
  "message": "owner-status-must-remain-byte-identical",
  "started_at_ms": 424242,
  "debug_port": 9229,
  "helper_port": 57321,
  "codex_app": "C:/Owner/Codex.app",
  "administrator_mode": {
    "requested": false,
    "state": "off",
    "exec_elevated": false,
    "computer_use_elevated": false,
    "error_component": null
  }
}
"#
        .to_vec();
        std::fs::write(&path, &owner_bytes).expect("write owner status fixture");
        let mut options = LaunchOptions::default();
        options.status_store = StatusStore::new(path.clone());
        (options, path, owner_bytes)
    }

    fn remove_owner_status_fixture(path: &Path) {
        std::fs::remove_file(path).expect("remove owned status fixture");
    }

    #[test]
    fn parse_launch_options_accepts_manager_forwarded_ports_and_app_path() {
        let options = parse_launch_options([
            "--app-path",
            "C:/Codex/App",
            "--debug-port",
            "9333",
            "--helper-port",
            "57322",
        ]);

        assert_eq!(options.app_dir, Some(PathBuf::from("C:/Codex/App")));
        assert_eq!(options.debug_port, 9333);
        assert_eq!(options.helper_port, 57322);
    }

    #[test]
    fn parse_launch_options_ignores_invalid_ports() {
        let options = parse_launch_options(["--debug-port", "nope", "--helper-port", "70000"]);

        assert_eq!(options.debug_port, LaunchOptions::default().debug_port);
        assert_eq!(options.helper_port, LaunchOptions::default().helper_port);
    }

    #[test]
    fn launcher_uses_single_instance_guard_before_launching() {
        let source = include_str!("main.rs");

        assert!(source.contains("acquire_single_instance_guard(options.debug_port)?"));
        assert!(source.contains("launcher_guard_port"));
        assert!(source.contains("launcher.already_running"));
        assert!(source.contains("Existing Codex instance activated"));
        assert!(source.contains("status: \"failed\".to_string()"));
    }

    #[test]
    fn existing_launcher_path_drains_pending_remote_control_recovery_before_activation() {
        let source = include_str!("main.rs");
        let start = source
            .find("async fn activate_existing_codex_app")
            .expect("existing launcher activation function");
        let body = &source[start..];
        let recovery = body
            .find(
                "let has_pending_recovery = hooks.has_pending_remote_control_session_recoveries()",
            )
            .expect("pending recovery guard");
        let launch = body
            .find("let launch_result = hooks")
            .expect("Codex activation");

        assert!(recovery < launch);
        assert!(body[recovery..launch].contains("find_session_index_cleanup_blocking_processes"));
        assert!(body[recovery..launch].contains("should_finalize_pending_remote_control_recovery"));
        assert!(
            body[recovery..launch].contains("hooks.run_remote_control_session_recovery().await?")
        );
    }

    #[test]
    fn pending_remote_control_finalization_requires_an_idle_desktop() {
        assert!(should_finalize_pending_remote_control_recovery(true, &[]));
        assert!(!should_finalize_pending_remote_control_recovery(false, &[]));
        assert!(!should_finalize_pending_remote_control_recovery(
            true,
            &[42]
        ));
    }

    #[test]
    fn secondary_existing_success_preserves_owner_status_bytes() {
        let (options, path, owner_bytes) = owner_status_fixture();

        finalize_launcher_invocation(
            &options,
            false,
            Ok(LauncherStatusOwnership::SecondaryExistingInstance),
        )
        .expect("secondary success must not need status persistence");

        assert_eq!(
            std::fs::read(&path).expect("read owner status"),
            owner_bytes
        );
        remove_owner_status_fixture(&path);
    }

    #[test]
    fn secondary_existing_failure_preserves_owner_status_bytes() {
        let (options, path, owner_bytes) = owner_status_fixture();

        let error = finalize_launcher_invocation(
            &options,
            false,
            Err(LauncherRunFailure::secondary_existing_instance(
                anyhow::anyhow!("secondary activation fixture failure"),
            )),
        )
        .expect_err("secondary failure must reach the caller without writing owner status");

        assert!(
            error
                .to_string()
                .contains("secondary activation fixture failure")
        );
        assert_eq!(
            std::fs::read(&path).expect("read owner status"),
            owner_bytes
        );
        remove_owner_status_fixture(&path);
    }

    #[test]
    fn primary_failure_persists_failed_status() {
        let (options, path, owner_bytes) = owner_status_fixture();

        let error = finalize_launcher_invocation(
            &options,
            false,
            Err(LauncherRunFailure::primary(anyhow::anyhow!(
                "primary launch fixture failure"
            ))),
        )
        .expect_err("primary failure must persist its failed status");

        assert!(error.to_string().contains("primary launch fixture failure"));
        let bytes = std::fs::read(&path).expect("read persisted failed status");
        assert_ne!(bytes, owner_bytes);
        assert_eq!(
            options
                .status_store
                .load_latest()
                .expect("load persisted failed status")
                .expect("failed status exists")
                .status,
            "failed"
        );
        remove_owner_status_fixture(&path);
    }

    #[derive(Clone)]
    struct ExistingInstanceFakeHooks {
        calls: Arc<Mutex<Vec<String>>>,
        marketplace_fails: bool,
        launch_fails: bool,
    }

    impl ExistingInstanceFakeHooks {
        fn new(marketplace_fails: bool, launch_fails: bool) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                marketplace_fails,
                launch_fails,
            }
        }

        fn record(&self, call: &str) {
            self.calls
                .lock()
                .expect("fake call log")
                .push(call.to_string());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("fake call log").clone()
        }
    }

    #[async_trait::async_trait(?Send)]
    impl LaunchHooks for ExistingInstanceFakeHooks {
        fn resolve_app_dir(
            &self,
            _app_dir: Option<&Path>,
            _settings: &codex_plus_core::settings::BackendSettings,
        ) -> anyhow::Result<PathBuf> {
            self.record("resolve_app_dir");
            Ok(PathBuf::from("C:/Codex/App"))
        }

        fn select_debug_port(&self, requested: u16) -> u16 {
            self.record("select_debug_port");
            requested
        }

        fn select_helper_port(&self, requested: u16) -> u16 {
            self.record("select_helper_port");
            requested.saturating_add(1)
        }

        async fn load_settings(
            &self,
        ) -> anyhow::Result<codex_plus_core::settings::BackendSettings> {
            self.record("load_settings");
            Ok(codex_plus_core::settings::BackendSettings::default())
        }

        async fn run_provider_sync(&self) -> anyhow::Result<()> {
            Ok(())
        }

        async fn run_remote_control_session_recovery(&self) -> anyhow::Result<()> {
            Ok(())
        }

        async fn ensure_plugin_marketplace_config(
            &self,
            _settings: &codex_plus_core::settings::BackendSettings,
        ) -> anyhow::Result<()> {
            self.record("ensure_plugin_marketplace_config");
            if self.marketplace_fails {
                anyhow::bail!("marketplace fixture failure");
            }
            Ok(())
        }

        async fn start_helper(&self, _helper_port: u16) -> anyhow::Result<()> {
            self.record("start_helper");
            Ok(())
        }

        async fn launch_codex(
            &self,
            _app_dir: &Path,
            _debug_port: u16,
            _settings: &codex_plus_core::settings::BackendSettings,
            _extra_args: &[String],
        ) -> anyhow::Result<codex_plus_core::launcher::CodexLaunch> {
            self.record("launch_codex");
            if self.launch_fails {
                anyhow::bail!("packaged activation fixture failure");
            }
            Ok(codex_plus_core::launcher::CodexLaunch::PackagedActivation {
                app_user_model_id: "OpenAI.Codex_fixture".to_string(),
                arguments: String::new(),
                process_id: None,
            })
        }

        async fn inject(&self, _debug_port: u16, _helper_port: u16) -> anyhow::Result<()> {
            self.record("inject");
            Ok(())
        }

        async fn ensure_injection(
            &self,
            _debug_port: u16,
            _helper_port: u16,
            _app_dir: &Path,
        ) -> bool {
            self.record("ensure_injection");
            true
        }

        async fn start_bridge_watchdog(
            &self,
            _debug_port: u16,
            _helper_port: u16,
        ) -> anyhow::Result<()> {
            self.record("start_bridge_watchdog");
            Ok(())
        }

        async fn write_status(&self, _status: &str) {
            self.record("write_status");
        }

        async fn wait_for_codex_exit(
            &self,
            _launch: &codex_plus_core::launcher::CodexLaunch,
            _debug_port: u16,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn shutdown_helper(&self, _helper_port: u16) {}

        async fn terminate_codex(&self, _launch: &codex_plus_core::launcher::CodexLaunch) {}
    }

    fn assert_existing_instance_side_effects_are_absent(calls: &[String]) {
        assert_eq!(
            calls,
            [
                "load_settings",
                "resolve_app_dir",
                "ensure_plugin_marketplace_config",
                "launch_codex",
            ]
        );
        for forbidden in [
            "select_helper_port",
            "start_helper",
            "ensure_injection",
            "start_bridge_watchdog",
            "write_status",
        ] {
            assert!(
                !calls.iter().any(|call| call == forbidden),
                "{forbidden} was called"
            );
        }
    }

    #[tokio::test]
    async fn existing_instance_activation_uses_owner_runtime_and_ignores_marketplace_failure() {
        let options = LaunchOptions {
            debug_port: 9_227,
            helper_port: 9_228,
            ..LaunchOptions::default()
        };
        let hooks = ExistingInstanceFakeHooks::new(true, false);
        let outcome = Arc::new(Mutex::new(None));
        let recorded_outcome = outcome.clone();

        activate_existing_codex_app_with_hooks(
            &hooks,
            &options,
            Vec::new,
            |_| false,
            move |value| *recorded_outcome.lock().expect("activation outcome") = Some(value),
        )
        .await
        .expect("marketplace failure must be nonfatal");

        assert_existing_instance_side_effects_are_absent(&hooks.calls());
        let outcome = outcome
            .lock()
            .expect("activation outcome")
            .clone()
            .expect("outcome");
        assert_eq!(outcome.helper_port, options.helper_port);
        assert!(outcome.packaged_activation_succeeded);
        assert!(!outcome.focused_existing_window);
    }

    #[tokio::test]
    async fn existing_instance_focus_recovers_packaged_activation_failure_without_runtime_side_effects()
     {
        let options = LaunchOptions {
            helper_port: 9_228,
            ..LaunchOptions::default()
        };
        let hooks = ExistingInstanceFakeHooks::new(false, true);

        activate_existing_codex_app_with_hooks(
            &hooks,
            &options,
            || vec![7],
            |process_id| process_id == 7,
            |_| {},
        )
        .await
        .expect("focused existing window must recover packaged activation failure");

        assert_existing_instance_side_effects_are_absent(&hooks.calls());
    }

    #[tokio::test]
    async fn existing_instance_returns_original_packaged_activation_error_when_focus_fails() {
        let options = LaunchOptions::default();
        let hooks = ExistingInstanceFakeHooks::new(false, true);

        let error =
            activate_existing_codex_app_with_hooks(&hooks, &options, || vec![7], |_| false, |_| {})
                .await
                .expect_err(
                    "failed packaged activation without focus must surface its original error",
                );

        assert!(
            error
                .to_string()
                .contains("packaged activation fixture failure")
        );
        assert_existing_instance_side_effects_are_absent(&hooks.calls());
    }

    #[test]
    fn openai_session_identity_supersedes_only_its_active_pending_recovery() {
        let request = codex_plus_core::remote_control_recovery::PendingRemoteControlRecovery {
            thread_id: "mobile".to_string(),
            profile_id: "relay".to_string(),
            target_provider: "custom".to_string(),
            config_generation: "old-generation".to_string(),
            created_at: 1,
        };
        let mut settings = codex_plus_core::settings::BackendSettings {
            active_relay_id: "relay".to_string(),
            relay_profiles: vec![codex_plus_core::settings::RelayProfile {
                id: "relay".to_string(),
                config_contents: "model_provider = \"openai\"\n".to_string(),
                ..codex_plus_core::settings::RelayProfile::default()
            }],
            ..codex_plus_core::settings::BackendSettings::default()
        };

        assert!(remote_control_recovery_is_superseded_by_openai(
            &settings, &request
        ));

        settings.relay_profiles[0].config_contents = "model_provider = \"custom\"\n".to_string();
        assert!(!remote_control_recovery_is_superseded_by_openai(
            &settings, &request
        ));

        settings.relay_profiles[0].config_contents = "model_provider = \"openai\"\n".to_string();
        settings.active_relay_id = "other".to_string();
        assert!(!remote_control_recovery_is_superseded_by_openai(
            &settings, &request
        ));
    }

    #[test]
    fn launcher_hooks_forward_runtime_watchdog_and_marketplace_methods() {
        let source = include_str!("main.rs");

        assert!(source.contains("async fn start_bridge_watchdog"));
        assert!(source.contains("self.watchdog_bridge_context()?"));
        assert!(source.contains("set_bridge_reinjector(reinjector)"));
        assert!(source.contains("inject_with_context(debug_port, helper_port, ctx, runtime)"));
        assert!(source.contains("async fn ensure_plugin_marketplace_config"));
        assert!(source.contains("self.core.ensure_plugin_marketplace_config(settings).await"));
    }

    #[test]
    fn thread_sort_keys_queries_catalog_when_sort_schema_is_missing() {
        let source = include_str!("main.rs");
        let start = source
            .find("async fn thread_sort_keys(&self, sessions: Vec<SessionRef>)")
            .expect("thread sort keys implementation");
        let body = &source[start..];
        let sort_query = body
            .find("let candidate = adapter.codex_thread_sort_keys(&sessions);")
            .expect("thread sort key query");
        let catalog_query = body
            .find("let catalog = adapter.codex_local_catalog_rows(&sessions);")
            .expect("catalog query");
        assert!(sort_query < catalog_query);
        assert!(
            !body[sort_query..catalog_query]
                .contains("if candidate.get(\"status\").and_then(Value::as_str) != Some(\"ok\")")
        );
        assert!(body[catalog_query..].contains("\"internal_subagent\""));
    }

    #[tokio::test]
    async fn watchdog_reuses_bridge_context_with_data_service() {
        let test_dir = std::env::temp_dir().join(format!(
            "codex-plus-launcher-watchdog-test-{}",
            std::process::id()
        ));
        let hooks = LauncherHooks {
            core: Arc::new(DefaultLaunchHooks::default()),
            data: Arc::new(LauncherDataService {
                db_path: test_dir.join("state.sqlite"),
                backup_dir: test_dir.join("backups"),
            }),
            runtime: Arc::new(LauncherRuntimeService::new(
                9229,
                UserScriptManager::new(
                    test_dir.join("builtin"),
                    test_dir.join("user"),
                    test_dir.join("settings.json"),
                ),
            )),
            bridge_context: Arc::new(Mutex::new(None)),
        };

        hooks.bridge_context(9229, &test_dir).await.unwrap();
        let ctx = hooks.watchdog_bridge_context().unwrap();
        let result = codex_plus_core::routes::handle_bridge_request(
            ctx,
            "/move-thread-workspace",
            json!({"session_id": "missing", "title": "Missing", "target_cwd": "/new"}),
        )
        .await;

        assert_ne!(
            result["message"],
            "Move workspace service is not wired in core launcher hooks"
        );
    }
}

fn builtin_user_scripts_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|path| path.join("user_scripts"))
        .unwrap_or_else(|| PathBuf::from("user_scripts"))
}
