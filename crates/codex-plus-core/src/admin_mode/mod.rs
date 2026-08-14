pub mod computer_use;
pub mod environment;
pub mod exec;
#[cfg(windows)]
mod exec_runtime_copy;
pub mod feature;
pub mod windows;

use std::path::{Path, PathBuf};

use aes_gcm::aead::{OsRng, rand_core::RngCore};
use anyhow::{Context, ensure};
use base64::Engine;

use crate::status::AdministratorModeStatus;

use self::computer_use::{
    AdminComputerUseConfig, AdminComputerUseRuntime, ComputerUseRecoveryOutcome,
};
use self::environment::{
    AdminEnvironmentSpec, AdminEnvironmentTransaction, EnvironmentRestoreOutcome,
};
use self::exec::{AdminExecConfig, AdminExecRuntime};
use self::feature::AdminUnifiedExecTransaction;
use self::windows::{KillOnCloseJob, admin_pipe_name, current_windows_identity};

const COMPUTER_USE_DESCRIPTOR: &str = "administrator-mode-computer-use.v1.json";
const HIGH_INTEGRITY_RID: u32 = 0x3000;

pub struct AdminModeConfig<'a> {
    pub codex_home: &'a Path,
    pub state_dir: &'a Path,
    pub app_dir: &'a Path,
    pub shim_path: &'a Path,
    pub terminal_shim_path: &'a Path,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminAppServerBootstrap {
    pub official_codex_exe: PathBuf,
    pub terminal_shim_path: PathBuf,
    pub terminal_pipe_name: String,
    pub terminal_session_id: String,
    pub terminal_proof_path: PathBuf,
}

pub struct AdminModeRuntime {
    session_id: String,
    job: Option<KillOnCloseJob>,
    exec: Option<AdminExecRuntime>,
    computer_use: Option<AdminComputerUseRuntime>,
    environment: Option<AdminEnvironmentTransaction>,
    unified_exec: Option<AdminUnifiedExecTransaction>,
    app_server: AdminAppServerBootstrap,
}

// SAFETY: the runtime exclusively owns its child processes, tasks, filesystem
// transaction, and Job Object. Their cleanup does not depend on thread affinity.
unsafe impl Send for AdminModeRuntime {}

pub struct AdminModeLease {
    runtime: Option<AdminModeRuntime>,
    fallback_environment: Option<AdminEnvironmentTransaction>,
    health: Option<tokio::sync::watch::Receiver<Option<String>>>,
}

// SAFETY: the lease has exclusive ownership of every runtime resource. Tokio
// child/task handles are Send, filesystem state is path-owned, and the Windows
// Job Object handle may be closed from any thread.
unsafe impl Send for AdminModeLease {}

impl AdminModeLease {
    pub fn new(runtime: AdminModeRuntime) -> Self {
        let health = runtime.health_receiver();
        Self {
            runtime: Some(runtime),
            fallback_environment: None,
            health,
        }
    }

    #[doc(hidden)]
    pub fn testing() -> Self {
        Self {
            runtime: None,
            fallback_environment: None,
            health: None,
        }
    }

    #[doc(hidden)]
    pub fn testing_with_environment(environment: AdminEnvironmentTransaction) -> Self {
        Self {
            runtime: None,
            fallback_environment: Some(environment),
            health: None,
        }
    }

    #[doc(hidden)]
    pub fn testing_with_health(health: tokio::sync::watch::Receiver<Option<String>>) -> Self {
        Self {
            runtime: None,
            fallback_environment: None,
            health: Some(health),
        }
    }

    pub fn health_receiver(&self) -> Option<tokio::sync::watch::Receiver<Option<String>>> {
        self.health.clone()
    }

    pub fn app_server_bootstrap(&self) -> Option<AdminAppServerBootstrap> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.app_server.clone())
    }

    pub async fn shutdown(mut self) -> anyhow::Result<EnvironmentRestoreOutcome> {
        if let Some(runtime) = self.runtime.take() {
            return runtime.shutdown().await;
        }
        match self.fallback_environment.take() {
            Some(environment) => environment.restore(),
            None => Ok(EnvironmentRestoreOutcome::NoJournal),
        }
    }
}

impl Drop for AdminModeLease {
    fn drop(&mut self) {
        if let Some(environment) = self.fallback_environment.take() {
            let _ = environment.restore();
        }
        drop(self.runtime.take());
    }
}

impl AdminModeRuntime {
    fn health_receiver(&self) -> Option<tokio::sync::watch::Receiver<Option<String>>> {
        let exec = self.exec.as_ref().map(AdminExecRuntime::health_receiver);
        let computer_use = self
            .computer_use
            .as_ref()
            .map(AdminComputerUseRuntime::health_receiver);
        merge_health_receivers(exec, computer_use)
    }
    pub async fn start(config: AdminModeConfig<'_>) -> anyhow::Result<Self> {
        recover_stale_admin_mode(config.codex_home, config.state_dir)
            .context("administrator_mode:recovery")?;
        ensure!(
            config.shim_path.is_file(),
            "administrator_mode:shim: administrator shim is missing"
        );
        ensure!(
            config.terminal_shim_path.is_file(),
            "administrator_mode:terminal: PowerShell compatibility shim is missing"
        );

        let identity = current_windows_identity().context("administrator_mode:identity")?;
        ensure!(
            identity.elevated && identity.integrity_rid >= HIGH_INTEGRITY_RID,
            "administrator_mode:identity: launcher is not elevated"
        );

        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let proof = generate_proof();
        let job = KillOnCloseJob::new(&format!("codex-plus-admin-{session_id}"))
            .context("administrator_mode:job")?;
        let exec_pipe = admin_pipe_name(&format!("{session_id}-exec"));
        let computer_use_pipe = admin_pipe_name(&format!("{session_id}-computer-use"));
        let codex_exe = config.app_dir.join("resources").join("codex.exe");
        let descriptor_path = config.state_dir.join(COMPUTER_USE_DESCRIPTOR);
        let proof_path = descriptor_path.with_extension("proof");

        let mut exec = None;
        let mut computer_use = None;
        let mut environment: Option<AdminEnvironmentTransaction> = None;
        let mut unified_exec: Option<AdminUnifiedExecTransaction> = None;

        let startup = async {
            let started_exec = AdminExecRuntime::start(
                AdminExecConfig {
                    codex_exe: &codex_exe,
                    readiness_probe_exe: config.shim_path,
                    pipe_name: &exec_pipe,
                    session_id: &session_id,
                    session_proof: &proof,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
            )
            .await
            .context("administrator_mode:exec")?;
            exec = Some(started_exec);
            exec.as_mut()
                .expect("exec runtime was just installed")
                .verify_ready()
                .await
                .context("administrator_mode:exec")?;

            let artifacts = match crate::computer_use_guard::resolve_admin_computer_use_artifacts(
                config.codex_home,
            ) {
                Ok(artifacts) => artifacts,
                Err(resolve_error) => {
                    #[cfg(windows)]
                    {
                        crate::computer_use_guard::ensure_packaged_admin_computer_use_artifacts(
                            config.app_dir,
                        )
                        .with_context(|| {
                            format!(
                                "administrator_mode:computer_use: packaged runtime bootstrap failed after {resolve_error}"
                            )
                        })?
                    }
                    #[cfg(not(windows))]
                    {
                        return Err(resolve_error).context("administrator_mode:computer_use");
                    }
                }
            };
            #[cfg(windows)]
            crate::computer_use_guard::ensure_admin_computer_use_config_for_artifacts(
                config.codex_home,
                config.app_dir,
                &artifacts,
            )
            .context("administrator_mode:computer_use_config")?;
            let started_computer_use = AdminComputerUseRuntime::start(
                AdminComputerUseConfig {
                    home: config.codex_home,
                    descriptor_path: &descriptor_path,
                    shim_path: config.shim_path,
                    helper_exe: &artifacts.helper_exe,
                    helper_transport: &artifacts.helper_transport,
                    pipe_name: &computer_use_pipe,
                    session_id: &session_id,
                    session_proof: &proof,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
            )
            .await
            .context("administrator_mode:computer_use")?;
            computer_use = Some(started_computer_use);
            computer_use
                .as_ref()
                .expect("Computer Use runtime was just installed")
                .verify_ready()
                .await
                .context("administrator_mode:computer_use")?;

            environment = Some(
                AdminEnvironmentTransaction::install(
                    config.codex_home,
                    config.state_dir,
                    &AdminEnvironmentSpec {
                        shim_path: config.shim_path,
                        pipe_name: &exec_pipe,
                        session_id: &session_id,
                        proof_path: &proof_path,
                    },
                )
                .context("administrator_mode:environment")?,
            );
            unified_exec = Some(
                AdminUnifiedExecTransaction::install(config.codex_home, config.state_dir)
                    .context("administrator_mode:unified_exec")?,
            );

            Ok::<(), anyhow::Error>(())
        }
        .await;

        if let Err(error) = startup {
            if let Some(transaction) = unified_exec.take() {
                let _ = transaction.restore();
            }
            if let Some(transaction) = environment.take() {
                let _ = transaction.restore();
            }
            if let Some(runtime) = computer_use.take() {
                let _ = runtime.shutdown().await;
            }
            if let Some(runtime) = exec.take() {
                let _ = runtime.shutdown().await;
            }
            return Err(error);
        }

        let app_server = AdminAppServerBootstrap {
            official_codex_exe: exec
                .as_ref()
                .expect("exec runtime was verified before app-server bootstrap")
                .official_executable_path()
                .to_path_buf(),
            terminal_shim_path: std::fs::canonicalize(config.terminal_shim_path)
                .context("administrator_mode:terminal: compatibility shim is unavailable")?,
            terminal_pipe_name: exec_pipe,
            terminal_session_id: session_id.clone(),
            terminal_proof_path: proof_path,
        };

        Ok(Self {
            session_id,
            job: Some(job),
            exec,
            computer_use,
            environment,
            unified_exec,
            app_server,
        })
    }

    pub fn status(&self) -> AdministratorModeStatus {
        AdministratorModeStatus {
            requested: true,
            state: "active".to_string(),
            exec_elevated: self.exec.is_some()
                && self.environment.is_some()
                && self.unified_exec.is_some(),
            computer_use_elevated: self.computer_use.is_some(),
            error_component: None,
        }
    }

    pub fn session_id_prefix(&self) -> &str {
        &self.session_id[..self.session_id.len().min(8)]
    }

    pub async fn shutdown(mut self) -> anyhow::Result<EnvironmentRestoreOutcome> {
        let unified_exec_result = match self.unified_exec.take() {
            Some(transaction) => transaction.restore(),
            None => Ok(feature::UnifiedExecRestoreOutcome::NoJournal),
        };
        let environment_result = match self.environment.take() {
            Some(transaction) => transaction.restore(),
            None => Ok(EnvironmentRestoreOutcome::NoJournal),
        };
        let computer_use_result = match self.computer_use.take() {
            Some(runtime) => runtime.shutdown().await,
            None => Ok(()),
        };
        let exec_result = match self.exec.take() {
            Some(runtime) => runtime.shutdown().await,
            None => Ok(()),
        };
        drop(self.job.take());
        let environment_outcome =
            environment_result.context("administrator_mode:environment_cleanup")?;
        unified_exec_result.context("administrator_mode:unified_exec_cleanup")?;
        computer_use_result.context("administrator_mode:computer_use_cleanup")?;
        exec_result.context("administrator_mode:exec_cleanup")?;
        Ok(environment_outcome)
    }
}

fn merge_health_receivers(
    exec: Option<tokio::sync::watch::Receiver<Option<String>>>,
    computer_use: Option<tokio::sync::watch::Receiver<Option<String>>>,
) -> Option<tokio::sync::watch::Receiver<Option<String>>> {
    match (exec, computer_use) {
        (None, None) => None,
        (Some(receiver), None) | (None, Some(receiver)) => Some(receiver),
        (Some(mut exec), Some(mut computer_use)) => {
            let (fatal_tx, fatal) = tokio::sync::watch::channel(None);
            tokio::spawn(async move {
                loop {
                    if let Some(failure) = exec.borrow().clone() {
                        let _ = fatal_tx.send(Some(failure));
                        return;
                    }
                    if let Some(failure) = computer_use.borrow().clone() {
                        let _ = fatal_tx.send(Some(failure));
                        return;
                    }
                    tokio::select! {
                        changed = exec.changed() => {
                            if changed.is_err() {
                                let _ = fatal_tx.send(Some(
                                    "administrator exec broker health channel closed".to_owned(),
                                ));
                                return;
                            }
                        }
                        changed = computer_use.changed() => {
                            if changed.is_err() {
                                let _ = fatal_tx.send(Some(
                                    "administrator Computer Use broker health channel closed".to_owned(),
                                ));
                                return;
                            }
                        }
                    }
                }
            });
            Some(fatal)
        }
    }
}

impl Drop for AdminModeRuntime {
    fn drop(&mut self) {
        if let Some(unified_exec) = self.unified_exec.take() {
            let _ = unified_exec.restore();
        }
        if let Some(environment) = self.environment.take() {
            let _ = environment.restore();
        }
        drop(self.computer_use.take());
        drop(self.exec.take());
        drop(self.job.take());
    }
}

pub fn computer_use_descriptor_path(state_dir: &Path) -> PathBuf {
    state_dir.join(COMPUTER_USE_DESCRIPTOR)
}

pub fn recover_stale_admin_mode(
    codex_home: &Path,
    state_dir: &Path,
) -> anyhow::Result<EnvironmentRestoreOutcome> {
    let descriptor = computer_use_descriptor_path(state_dir);
    match computer_use::recover_stale_admin_computer_use(codex_home, state_dir, &descriptor)? {
        ComputerUseRecoveryOutcome::ActiveBroker => Ok(EnvironmentRestoreOutcome::NoJournal),
        ComputerUseRecoveryOutcome::NothingToRecover | ComputerUseRecoveryOutcome::Recovered => {
            feature::recover_stale_unified_exec(codex_home, state_dir)
                .context("administrator_mode:unified_exec_recovery")?;
            environment::recover_stale_environment(codex_home, state_dir)
        }
    }
}

pub fn recover_stale_admin_mode_for_shutdown(
    codex_home: &Path,
    state_dir: &Path,
) -> anyhow::Result<EnvironmentRestoreOutcome> {
    let descriptor = computer_use_descriptor_path(state_dir);
    match computer_use::recover_stale_admin_computer_use_for_shutdown(
        codex_home,
        state_dir,
        &descriptor,
    )? {
        ComputerUseRecoveryOutcome::ActiveBroker => {
            anyhow::bail!("administrator_mode:recovery: administrator broker is still active")
        }
        ComputerUseRecoveryOutcome::NothingToRecover | ComputerUseRecoveryOutcome::Recovered => {
            feature::recover_stale_unified_exec(codex_home, state_dir)
                .context("administrator_mode:unified_exec_recovery")?;
            environment::recover_stale_environment(codex_home, state_dir)
        }
    }
}

fn generate_proof() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    bytes.fill(0);
    proof
}

#[cfg(test)]
mod tests {
    use super::{AdminModeConfig, AdminModeRuntime, merge_health_receivers};

    #[tokio::test]
    async fn elevated_production_admin_runtime_bootstraps_first_pure_api_launch() {
        if !super::windows::process_has_high_integrity(std::process::id()).unwrap_or(false) {
            eprintln!("SKIP: production administrator runtime smoke requires elevation");
            return;
        }
        let temp = tempfile::tempdir().expect("create administrator runtime smoke home");
        let home = temp.path().join("home");
        let state = temp.path().join("state");
        std::fs::create_dir_all(&home).expect("create smoke CODEX_HOME");
        std::fs::create_dir_all(&state).expect("create smoke state directory");
        std::fs::write(
            home.join("config.toml"),
            r#"model = "gpt-fixture"
model_provider = "custom"
notify = ["C:\\missing-runtime\\codex-computer-use.exe", "turn-ended"]

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://pure-api.example/v1"
"#,
        )
        .expect("write pure API smoke config");
        let auth_fixture = br#"{"OPENAI_API_KEY":"sk-pure-api-fixture"}"#;
        std::fs::write(home.join("auth.json"), auth_fixture).expect("write pure API smoke auth");
        let app_dir = crate::app_paths::find_latest_codex_app_dir_default()
            .expect("locate installed official Codex app");
        let shim_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root")
            .join("target/release/codex-plus-admin-shim.exe");
        assert!(
            shim_path.is_file(),
            "release administrator shim is required"
        );

        let runtime = AdminModeRuntime::start(AdminModeConfig {
            codex_home: &home,
            state_dir: &state,
            app_dir: &app_dir,
            shim_path: &shim_path,
            terminal_shim_path: &shim_path,
        })
        .await;
        let runtime = match runtime {
            Ok(runtime) => runtime,
            Err(error)
                if error.chain().any(|source| {
                    source
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.raw_os_error() == Some(32))
                }) =>
            {
                eprintln!("SKIP: active Codex Computer Use runtime holds the real transport open");
                return;
            }
            Err(error) => panic!("start complete administrator runtime: {error:#}"),
        };
        let status = runtime.status();
        assert!(status.exec_elevated);
        assert!(status.computer_use_elevated);
        assert!(runtime.app_server.official_codex_exe.is_file());
        assert_eq!(
            runtime.app_server.official_codex_exe,
            app_dir
                .join("resources")
                .join("codex.exe")
                .canonicalize()
                .expect("canonicalize official Codex executable")
        );
        assert!(
            !runtime
                .app_server
                .official_codex_exe
                .to_string_lossy()
                .contains("CodexPlusPlus-Recovery-")
        );
        assert!(home.join("environments.toml").is_file());
        assert!(
            state
                .join("administrator-mode-environment.v1.json")
                .is_file()
        );
        assert!(
            state
                .join("administrator-mode-unified-exec.v1.json")
                .is_file()
        );
        let active_config = std::fs::read_to_string(home.join("config.toml"))
            .expect("administrator first launch writes Computer Use runtime config");
        assert!(active_config.contains("notify"));
        assert!(active_config.contains("openai-bundled"));
        assert!(active_config.contains(r#"model_provider = "custom""#));
        assert!(active_config.contains(r#"base_url = "https://pure-api.example/v1""#));
        assert!(active_config.contains("unified_exec = true"));
        assert!(!active_config.contains("missing-runtime"));
        assert!(!active_config.contains("OPENAI_API_KEY"));
        assert_eq!(std::fs::read(home.join("auth.json")).unwrap(), auth_fixture);

        runtime
            .shutdown()
            .await
            .expect("shutdown complete administrator runtime");
        assert!(!home.join("environments.toml").exists());
        let restored_config = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(restored_config.contains("notify"));
        assert!(restored_config.contains("openai-bundled"));
        assert!(restored_config.contains(r#"model_provider = "custom""#));
        assert!(restored_config.contains(r#"base_url = "https://pure-api.example/v1""#));
        assert!(!restored_config.contains("unified_exec"));
        assert!(!restored_config.contains("missing-runtime"));
        assert!(!restored_config.contains("OPENAI_API_KEY"));
        assert_eq!(std::fs::read(home.join("auth.json")).unwrap(), auth_fixture);
        for name in [
            "administrator-mode-environment.v1.json",
            "administrator-mode-unified-exec.v1.json",
            "administrator-mode-computer-use.v1.json",
            "administrator-mode-computer-use.v1.proof",
            "administrator-mode-computer-use-recovery.required",
        ] {
            assert!(!state.join(name).exists(), "managed state remained: {name}");
        }
    }

    #[tokio::test]
    #[ignore = "manual isolated Codex administrator terminal QA"]
    async fn isolated_administrator_terminal_qa_broker() {
        let status_path = std::path::PathBuf::from(
            std::env::var_os("CODEXPP_TERMINAL_QA_STATUS")
                .expect("CODEXPP_TERMINAL_QA_STATUS is required"),
        );
        let panic_status = status_path.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = std::fs::write(&panic_status, format!("panic: {info}"));
        }));
        std::fs::write(&status_path, b"started")
            .expect("publish administrator terminal QA startup");
        if !super::windows::process_has_high_integrity(std::process::id()).unwrap_or(false) {
            panic!("administrator terminal QA broker must run elevated");
        }
        std::fs::write(&status_path, b"elevated")
            .expect("publish administrator terminal QA elevation");
        let inspector_port = std::env::var("CODEXPP_TERMINAL_QA_INSPECTOR")
            .expect("CODEXPP_TERMINAL_QA_INSPECTOR is required")
            .parse::<u16>()
            .expect("administrator terminal QA inspector port is invalid");
        let shim_path = std::path::PathBuf::from(
            std::env::var_os("CODEXPP_ADMIN_SHIM_TEST_EXE")
                .expect("CODEXPP_ADMIN_SHIM_TEST_EXE is required"),
        );
        let terminal_shim_path = std::path::PathBuf::from(
            std::env::var_os("CODEXPP_TERMINAL_SHIM_TEST_EXE")
                .expect("CODEXPP_TERMINAL_SHIM_TEST_EXE is required"),
        );
        let qa_root = std::path::PathBuf::from(
            std::env::var_os("CODEXPP_TERMINAL_QA_ROOT")
                .expect("CODEXPP_TERMINAL_QA_ROOT is required"),
        );
        let state = qa_root.join("state");
        std::fs::create_dir_all(&state).expect("create terminal QA state directory");
        let app_dir = crate::app_paths::find_latest_codex_app_dir_default()
            .expect("locate installed official Codex app");
        assert!(shim_path.is_file(), "administrator QA shim is missing");
        assert!(
            terminal_shim_path.is_file(),
            "administrator terminal QA compatibility shim is missing"
        );

        let identity = super::windows::current_windows_identity()
            .expect("read administrator terminal QA identity");
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let proof = super::generate_proof();
        let proof_path = state.join("terminal.proof");
        std::fs::write(&proof_path, &proof).expect("write administrator terminal QA proof");
        let pipe_name = super::windows::admin_pipe_name(&format!("{session_id}-terminal-qa"));
        let job = super::windows::KillOnCloseJob::new(&format!(
            "codex-plus-admin-terminal-qa-{session_id}"
        ))
        .expect("create administrator terminal QA job");
        let codex_exe = app_dir.join("resources").join("codex.exe");
        let mut runtime = super::exec::AdminExecRuntime::start(
            super::exec::AdminExecConfig {
                codex_exe: &codex_exe,
                readiness_probe_exe: &shim_path,
                pipe_name: &pipe_name,
                session_id: &session_id,
                session_proof: &proof,
                expected_user_sid: &identity.user_sid,
                expected_logon_sid: &identity.logon_sid,
            },
            &job,
        )
        .await
        .expect("start isolated administrator terminal QA broker");
        runtime
            .verify_ready()
            .await
            .expect("verify isolated administrator terminal QA broker");
        std::fs::write(&status_path, b"broker-ready")
            .expect("publish administrator terminal QA broker readiness");
        let bootstrap = super::AdminAppServerBootstrap {
            official_codex_exe: runtime.official_executable_path().to_path_buf(),
            terminal_shim_path: terminal_shim_path
                .canonicalize()
                .expect("canonicalize administrator terminal QA shim"),
            terminal_pipe_name: pipe_name,
            terminal_session_id: session_id,
            terminal_proof_path: proof_path,
        };
        crate::admin_app_server::install_and_resume(inspector_port, &bootstrap)
            .await
            .expect("install administrator terminal QA bootstrap");
        std::fs::write(&status_path, b"ready")
            .expect("publish administrator terminal QA readiness");

        tokio::time::sleep(std::time::Duration::from_secs(45)).await;
        runtime
            .shutdown()
            .await
            .expect("shutdown isolated administrator terminal QA broker");
        std::fs::write(status_path, b"complete")
            .expect("publish administrator terminal QA completion");
    }

    #[tokio::test]
    async fn merged_health_publishes_exec_failure() {
        let (exec_tx, exec_rx) = tokio::sync::watch::channel(None);
        let (_computer_tx, computer_rx) = tokio::sync::watch::channel(None);
        let mut merged = merge_health_receivers(Some(exec_rx), Some(computer_rx)).unwrap();

        exec_tx
            .send(Some(
                "administrator exec broker stopped unexpectedly".to_owned(),
            ))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), merged.changed())
            .await
            .expect("merged health timeout")
            .expect("merged health channel");
        assert_eq!(
            merged.borrow().as_deref(),
            Some("administrator exec broker stopped unexpectedly")
        );
    }

    #[tokio::test]
    async fn merged_health_publishes_computer_use_failure() {
        let (_exec_tx, exec_rx) = tokio::sync::watch::channel(None);
        let (computer_tx, computer_rx) = tokio::sync::watch::channel(None);
        let mut merged = merge_health_receivers(Some(exec_rx), Some(computer_rx)).unwrap();

        computer_tx
            .send(Some(
                "administrator Computer Use broker stopped unexpectedly".to_owned(),
            ))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), merged.changed())
            .await
            .expect("merged health timeout")
            .expect("merged health channel");
        assert_eq!(
            merged.borrow().as_deref(),
            Some("administrator Computer Use broker stopped unexpectedly")
        );
    }
}
