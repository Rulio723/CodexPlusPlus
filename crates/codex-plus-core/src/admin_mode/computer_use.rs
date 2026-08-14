use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::windows::KillOnCloseJob;

const RECOVERY_EVIDENCE_FILE: &str = "administrator-mode-computer-use-recovery.required";
const RECOVERY_EVIDENCE_BYTES: &[u8] = b"codex-plus-admin-computer-use-recovery-v1\n";

pub struct AdminComputerUseConfig<'a> {
    pub home: &'a Path,
    pub descriptor_path: &'a Path,
    pub shim_path: &'a Path,
    pub helper_exe: &'a Path,
    pub helper_transport: &'a Path,
    pub pipe_name: &'a str,
    pub session_id: &'a str,
    pub session_proof: &'a str,
    pub expected_user_sid: &'a str,
    pub expected_logon_sid: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerUseAdminDescriptor {
    pub broker_pid: u32,
    pub broker_creation_time: u64,
    pub shim_path: PathBuf,
    pub pipe_name: String,
    pub session_id: String,
    pub proof_path: PathBuf,
    pub proof_hash: String,
    pub transport_path: PathBuf,
    pub backup_path: PathBuf,
    pub original_hash: String,
    pub patched_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerUseHookOutcome {
    Installed,
    AlreadyInstalled,
    Removed,
    NotInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerUseRecoveryOutcome {
    NothingToRecover,
    ActiveBroker,
    Recovered,
}

pub fn install_admin_computer_use_hook(
    home: &Path,
    descriptor_path: &Path,
) -> anyhow::Result<ComputerUseHookOutcome> {
    crate::computer_use_guard::install_admin_computer_use_hook(home, descriptor_path)
}

pub fn remove_admin_computer_use_hook(home: &Path) -> anyhow::Result<ComputerUseHookOutcome> {
    crate::computer_use_guard::remove_admin_computer_use_hook(home)
}

fn write_descriptor_and_proof(
    descriptor_path: &Path,
    descriptor: &ComputerUseAdminDescriptor,
    proof: &str,
    user_sid: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(!proof.is_empty(), "administrator proof must not be empty");
    anyhow::ensure!(
        descriptor.proof_path == descriptor_path.with_extension("proof"),
        "administrator proof path is not owned by the descriptor"
    );
    anyhow::ensure!(
        descriptor.proof_hash == sha256_bytes(proof.as_bytes()),
        "administrator proof fingerprint mismatch"
    );
    let mut proof_bytes = proof.as_bytes().to_vec();
    let mut proof_file = crate::admin_secure_io::SecureFileLease::create(&descriptor.proof_path)?;
    let proof_result = proof_file
        .replace_contents(&proof_bytes)
        .and_then(|_| protect_current_user_file(&descriptor.proof_path, user_sid));
    proof_bytes.fill(0);
    if let Err(error) = proof_result {
        let _ = proof_file.delete();
        return Err(error);
    }
    drop(proof_file);
    let bytes = serde_json::to_vec(descriptor)?;
    let mut descriptor_file = match crate::admin_secure_io::SecureFileLease::create(descriptor_path)
    {
        Ok(file) => file,
        Err(error) => {
            let _ =
                remove_owned_file_if_hash_matches(&descriptor.proof_path, &descriptor.proof_hash);
            return Err(error);
        }
    };
    if let Err(error) = descriptor_file
        .replace_contents(&bytes)
        .and_then(|_| protect_current_user_file(descriptor_path, user_sid))
    {
        let _ = descriptor_file.delete();
        let _ = remove_owned_file_if_hash_matches(&descriptor.proof_path, &descriptor.proof_hash);
        return Err(error);
    }
    Ok(())
}

fn recovery_evidence_path(descriptor_path: &Path) -> anyhow::Result<PathBuf> {
    Ok(descriptor_path
        .parent()
        .context("administrator descriptor has no state directory")?
        .join(RECOVERY_EVIDENCE_FILE))
}

fn read_recovery_evidence(descriptor_path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let path = recovery_evidence_path(descriptor_path)?;
    match crate::admin_secure_io::SecureFileLease::open(&path, false) {
        Ok(mut file) => {
            let bytes = file.read_all()?;
            ensure_trusted_state_file(
                descriptor_path
                    .parent()
                    .context("descriptor has no state directory")?,
                &path,
            )?;
            anyhow::ensure!(
                bytes == RECOVERY_EVIDENCE_BYTES,
                "administrator recovery evidence ownership changed"
            );
            Ok(Some(path))
        }
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_recovery_evidence(path: &Path) -> anyhow::Result<()> {
    let mut file = crate::admin_secure_io::SecureFileLease::open_for_delete(path)?;
    anyhow::ensure!(
        file.read_all()? == RECOVERY_EVIDENCE_BYTES,
        "administrator recovery evidence ownership changed"
    );
    file.delete()?;
    Ok(())
}

fn publish_recovery_evidence(path: &Path, user_sid: &str) -> anyhow::Result<()> {
    let mut file = crate::admin_secure_io::SecureFileLease::create(path)?;
    if let Err(error) = file
        .replace_contents(RECOVERY_EVIDENCE_BYTES)
        .and_then(|_| protect_current_user_file(path, user_sid))
    {
        let _ = file.delete();
        return Err(error);
    }
    Ok(())
}

fn ensure_recovery_evidence(path: &Path, user_sid: &str) -> anyhow::Result<()> {
    match crate::admin_secure_io::SecureFileLease::open(path, false) {
        Ok(mut file) => anyhow::ensure!(
            file.read_all()? == RECOVERY_EVIDENCE_BYTES,
            "administrator recovery evidence ownership changed"
        ),
        Err(error) if is_not_found(&error) => publish_recovery_evidence(path, user_sid)?,
        Err(error) => return Err(error),
    }
    Ok(())
}

fn cleanup_with_recovery_evidence(
    descriptor_path: &Path,
    user_sid: &str,
    cleanup: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let evidence_path = recovery_evidence_path(descriptor_path)?;
    ensure_recovery_evidence(&evidence_path, user_sid)?;
    cleanup()?;
    remove_recovery_evidence(&evidence_path)
}

pub fn recover_stale_admin_computer_use(
    home: &Path,
    state_dir: &Path,
    descriptor_path: &Path,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_with_process_control(
        home,
        state_dir,
        descriptor_path,
        process_creation_time,
        |_| Ok(false),
    )
}

pub fn recover_stale_admin_computer_use_for_shutdown(
    home: &Path,
    state_dir: &Path,
    descriptor_path: &Path,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_with_process_control(
        home,
        state_dir,
        descriptor_path,
        process_creation_time,
        |descriptor| {
            terminate_process_with_creation_time(
                descriptor.broker_pid,
                descriptor.broker_creation_time,
            )?;
            Ok(true)
        },
    )
}

fn recover_stale_admin_computer_use_with_process_control(
    home: &Path,
    state_dir: &Path,
    descriptor_path: &Path,
    process_lookup: impl FnMut(u32) -> anyhow::Result<Option<u64>>,
    on_active_broker: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<bool>,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_impl_with_process_control(
        home,
        state_dir,
        descriptor_path,
        |inspect_marked_transports| {
            crate::computer_use_guard::recover_descriptorless_admin_computer_use(
                home,
                inspect_marked_transports,
                Some(descriptor_path),
            )
            .map(|_| ())
        },
        |descriptor| {
            crate::computer_use_guard::verify_stale_admin_computer_use_hook(
                home,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        |descriptor| {
            crate::computer_use_guard::restore_stale_admin_computer_use_hook(
                home,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        process_lookup,
        on_active_broker,
    )
}

#[cfg(test)]
fn recover_stale_admin_computer_use_impl(
    home: &Path,
    state_dir: &Path,
    descriptor_path: &Path,
    recover_descriptorless_rename_window: impl FnOnce(bool) -> anyhow::Result<()>,
    verify_hook: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<()>,
    restore_hook: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<()>,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_impl_with_process_lookup(
        home,
        state_dir,
        descriptor_path,
        recover_descriptorless_rename_window,
        verify_hook,
        restore_hook,
        process_creation_time,
    )
}

#[cfg(test)]
fn recover_stale_admin_computer_use_impl_with_process_lookup(
    _home: &Path,
    state_dir: &Path,
    descriptor_path: &Path,
    recover_descriptorless_rename_window: impl FnOnce(bool) -> anyhow::Result<()>,
    verify_hook: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<()>,
    restore_hook: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<()>,
    process_lookup: impl FnMut(u32) -> anyhow::Result<Option<u64>>,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_impl_with_process_control(
        _home,
        state_dir,
        descriptor_path,
        recover_descriptorless_rename_window,
        verify_hook,
        restore_hook,
        process_lookup,
        |_| Ok(false),
    )
}

fn recover_stale_admin_computer_use_impl_with_process_control(
    _home: &Path,
    state_dir: &Path,
    descriptor_path: &Path,
    recover_descriptorless_rename_window: impl FnOnce(bool) -> anyhow::Result<()>,
    verify_hook: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<()>,
    restore_hook: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<()>,
    mut process_lookup: impl FnMut(u32) -> anyhow::Result<Option<u64>>,
    on_active_broker: impl FnOnce(&ComputerUseAdminDescriptor) -> anyhow::Result<bool>,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    ensure_trusted_state_file(state_dir, descriptor_path)?;
    let mut descriptor_file =
        match crate::admin_secure_io::SecureFileLease::open_for_delete(descriptor_path) {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => {
                let evidence_path = read_recovery_evidence(descriptor_path)?;
                recover_descriptorless_rename_window(evidence_path.is_some())?;
                if let Some(evidence_path) = evidence_path {
                    remove_unpublished_proof(descriptor_path)?;
                    remove_recovery_evidence(&evidence_path)?;
                }
                return Ok(ComputerUseRecoveryOutcome::NothingToRecover);
            }
            Err(error) => return Err(error),
        };
    let bytes = descriptor_file.read_all()?;
    let descriptor: ComputerUseAdminDescriptor = match serde_json::from_slice(&bytes) {
        Ok(descriptor) => descriptor,
        Err(parse_error) => {
            let Some(evidence_path) = read_recovery_evidence(descriptor_path)? else {
                return Err(parse_error.into());
            };
            recover_descriptorless_rename_window(true)?;
            descriptor_file.delete()?;
            remove_unpublished_proof(descriptor_path)?;
            remove_recovery_evidence(&evidence_path)?;
            return Ok(ComputerUseRecoveryOutcome::NothingToRecover);
        }
    };
    ensure_trusted_state_file(state_dir, &descriptor.proof_path)?;
    anyhow::ensure!(
        descriptor.proof_path == descriptor_path.with_extension("proof"),
        "administrator proof path is not owned by the descriptor"
    );
    let mut proof_file =
        crate::admin_secure_io::SecureFileLease::open_for_delete(&descriptor.proof_path)?;
    anyhow::ensure!(
        sha256_bytes(&proof_file.read_all()?) == descriptor.proof_hash,
        "administrator proof ownership changed"
    );
    if process_lookup(descriptor.broker_pid)? == Some(descriptor.broker_creation_time) {
        verify_hook(&descriptor)?;
        if !on_active_broker(&descriptor)? {
            return Ok(ComputerUseRecoveryOutcome::ActiveBroker);
        }
        anyhow::ensure!(
            process_lookup(descriptor.broker_pid)? != Some(descriptor.broker_creation_time),
            "administrator_mode:recovery: administrator broker remained active after termination"
        );
    }
    let user_sid = super::windows::current_windows_identity()?.user_sid;
    cleanup_with_recovery_evidence(descriptor_path, &user_sid, || {
        restore_hook(&descriptor)?;
        descriptor_file.delete()?;
        proof_file.delete()?;
        Ok(())
    })?;
    if let Some(evidence_path) = read_recovery_evidence(descriptor_path)? {
        remove_recovery_evidence(&evidence_path)?;
    }
    Ok(ComputerUseRecoveryOutcome::Recovered)
}

fn remove_unpublished_proof(descriptor_path: &Path) -> anyhow::Result<()> {
    let proof_path = descriptor_path.with_extension("proof");
    match crate::admin_secure_io::SecureFileLease::open_for_delete(&proof_path) {
        Ok(file) => {
            file.delete()?;
        }
        Err(error) if is_not_found(&error) => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

#[cfg(all(test, windows))]
fn recover_stale_admin_computer_use_with_artifacts(
    home: &Path,
    descriptor_path: &Path,
    artifacts: &crate::computer_use_guard::AdminComputerUseArtifacts,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_impl(
        home,
        descriptor_path
            .parent()
            .context("descriptor has no state directory")?,
        descriptor_path,
        |_| {
            crate::computer_use_guard::recover_descriptorless_admin_computer_use_artifacts_for_test(
                artifacts,
            )
        },
        |descriptor| {
            crate::computer_use_guard::verify_stale_admin_computer_use_hook_with_artifacts(
                artifacts,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        |descriptor| {
            crate::computer_use_guard::restore_stale_admin_computer_use_hook_with_artifacts(
                artifacts,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
    )
}

#[cfg(all(test, windows))]
fn recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
    home: &Path,
    descriptor_path: &Path,
    artifacts: &crate::computer_use_guard::AdminComputerUseArtifacts,
    process_lookup: impl FnMut(u32) -> anyhow::Result<Option<u64>>,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_impl_with_process_lookup(
        home,
        descriptor_path
            .parent()
            .context("descriptor has no state directory")?,
        descriptor_path,
        |_| {
            crate::computer_use_guard::recover_descriptorless_admin_computer_use_artifacts_for_test(
                artifacts,
            )
        },
        |descriptor| {
            crate::computer_use_guard::verify_stale_admin_computer_use_hook_with_artifacts(
                artifacts,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        |descriptor| {
            crate::computer_use_guard::restore_stale_admin_computer_use_hook_with_artifacts(
                artifacts,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        process_lookup,
    )
}

#[cfg(all(test, windows))]
fn recover_stale_admin_computer_use_for_shutdown_with_artifacts_and_process_control(
    home: &Path,
    descriptor_path: &Path,
    artifacts: &crate::computer_use_guard::AdminComputerUseArtifacts,
    process_lookup: impl FnMut(u32) -> anyhow::Result<Option<u64>>,
    terminate_process: impl FnOnce(u32, u64) -> anyhow::Result<()>,
) -> anyhow::Result<ComputerUseRecoveryOutcome> {
    recover_stale_admin_computer_use_impl_with_process_control(
        home,
        descriptor_path
            .parent()
            .context("descriptor has no state directory")?,
        descriptor_path,
        |_| {
            crate::computer_use_guard::recover_descriptorless_admin_computer_use_artifacts_for_test(
                artifacts,
            )
        },
        |descriptor| {
            crate::computer_use_guard::verify_stale_admin_computer_use_hook_with_artifacts(
                artifacts,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        |descriptor| {
            crate::computer_use_guard::restore_stale_admin_computer_use_hook_with_artifacts(
                artifacts,
                descriptor_path,
                &descriptor.transport_path,
                &descriptor.backup_path,
                &descriptor.original_hash,
                &descriptor.patched_hash,
            )
        },
        process_lookup,
        |descriptor| {
            terminate_process(descriptor.broker_pid, descriptor.broker_creation_time)?;
            Ok(true)
        },
    )
}

fn ensure_trusted_state_file(state_dir: &Path, path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        path.parent() == Some(state_dir),
        "administrator state path escapes trusted state directory"
    );
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "administrator state path must not be a symbolic link"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !state_dir.exists() {
        return Ok(());
    }
    let state_dir = std::fs::canonicalize(state_dir)?;
    let parent = path
        .parent()
        .context("administrator state path has no parent")?;
    let parent = std::fs::canonicalize(parent)?;
    anyhow::ensure!(
        parent == state_dir,
        "administrator state path escapes trusted state directory"
    );
    Ok(())
}

#[cfg(windows)]
fn process_creation_time(pid: u32) -> anyhow::Result<Option<u64>> {
    use windows::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND, FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
        WaitForSingleObject,
    };
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let access = PROCESS_ACCESS_RIGHTS(PROCESS_QUERY_LIMITED_INFORMATION.0 | SYNCHRONIZE_ACCESS);
    let process = match unsafe { OpenProcess(access, false, pid) } {
        Ok(process) => process,
        Err(error)
            if error.code() == ERROR_INVALID_PARAMETER.to_hresult()
                || error.code() == ERROR_NOT_FOUND.to_hresult() =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    match unsafe { WaitForSingleObject(process, 0) } {
        WAIT_OBJECT_0 => {
            unsafe { windows::Win32::Foundation::CloseHandle(process) }?;
            return Ok(None);
        }
        WAIT_TIMEOUT => {}
        status => {
            unsafe { windows::Win32::Foundation::CloseHandle(process) }?;
            anyhow::bail!(
                "administrator_mode:recovery: unexpected broker liveness wait status {status:?}"
            );
        }
    }
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let times =
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
    let closed = unsafe { windows::Win32::Foundation::CloseHandle(process) };
    times?;
    closed?;
    Ok(Some(
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
    ))
}

#[cfg(not(windows))]
fn process_creation_time(_pid: u32) -> anyhow::Result<Option<u64>> {
    Ok(None)
}

#[cfg(windows)]
fn terminate_process_with_creation_time(
    pid: u32,
    expected_creation_time: u64,
) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND, FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
        TerminateProcess, WaitForSingleObject,
    };

    const PROCESS_TERMINATE_ACCESS: u32 = 0x0001;
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    const TERMINATION_TIMEOUT_MS: u32 = 10_000;

    let access = PROCESS_ACCESS_RIGHTS(
        PROCESS_QUERY_LIMITED_INFORMATION.0 | PROCESS_TERMINATE_ACCESS | SYNCHRONIZE_ACCESS,
    );
    let process = match unsafe { OpenProcess(access, false, pid) } {
        Ok(process) => process,
        Err(error)
            if error.code() == ERROR_INVALID_PARAMETER.to_hresult()
                || error.code() == ERROR_NOT_FOUND.to_hresult() =>
        {
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    let result = (|| -> anyhow::Result<()> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }?;
        let actual_creation_time =
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        anyhow::ensure!(
            actual_creation_time == expected_creation_time,
            "administrator_mode:recovery: broker process identity changed before termination"
        );

        unsafe { TerminateProcess(process, 1) }?;
        match unsafe { WaitForSingleObject(process, TERMINATION_TIMEOUT_MS) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => anyhow::bail!(
                "administrator_mode:recovery: timed out waiting for administrator broker termination"
            ),
            status => anyhow::bail!(
                "administrator_mode:recovery: unexpected broker termination wait status {status:?}"
            ),
        }
    })();
    let close_result = unsafe { windows::Win32::Foundation::CloseHandle(process) };
    result?;
    close_result?;
    Ok(())
}

#[cfg(not(windows))]
fn terminate_process_with_creation_time(
    _pid: u32,
    _expected_creation_time: u64,
) -> anyhow::Result<()> {
    anyhow::bail!("administrator broker termination is unsupported off Windows")
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|source| source.downcast_ref::<std::io::Error>())
        .any(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn validate_configured_artifact_paths(
    artifacts: &crate::computer_use_guard::AdminComputerUseArtifacts,
    configured_helper: &Path,
    configured_transport: &Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::fs::canonicalize(configured_helper)? == std::fs::canonicalize(&artifacts.helper_exe)?
            && std::fs::canonicalize(configured_transport)?
                == std::fs::canonicalize(&artifacts.helper_transport)?,
        "computer_use_contract_incompatible"
    );
    Ok(())
}

fn remove_owned_file_if_hash_matches(path: &Path, expected_hash: &str) -> anyhow::Result<()> {
    match crate::admin_secure_io::SecureFileLease::open_for_delete(path) {
        Ok(mut file) => {
            let bytes = file.read_all()?;
            anyhow::ensure!(
                sha256_bytes(&bytes) == expected_hash,
                "administrator file ownership changed"
            );
            file.delete()?;
        }
        Err(error) if is_not_found(&error) => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn remove_descriptor_and_proof(
    descriptor_path: &Path,
    expected: &ComputerUseAdminDescriptor,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        expected.proof_path == descriptor_path.with_extension("proof"),
        "administrator proof path is not owned by the descriptor"
    );
    let expected_descriptor = serde_json::to_vec(expected)?;
    let mut descriptor_file =
        crate::admin_secure_io::SecureFileLease::open_for_delete(descriptor_path)?;
    anyhow::ensure!(
        descriptor_file.read_all()? == expected_descriptor,
        "administrator descriptor ownership changed"
    );
    let mut proof_file =
        crate::admin_secure_io::SecureFileLease::open_for_delete(&expected.proof_path)?;
    anyhow::ensure!(
        sha256_bytes(&proof_file.read_all()?) == expected.proof_hash,
        "administrator proof ownership changed"
    );
    descriptor_file.delete()?;
    proof_file.delete()?;
    Ok(())
}

#[cfg(windows)]
fn protect_current_user_file(path: &Path, user_sid: &str) -> anyhow::Result<()> {
    use std::process::{Command, Stdio};
    let user_grant = format!("*{user_sid}:(R,D)");
    for args in [
        vec!["/reset"],
        vec!["/inheritance:r"],
        vec![
            "/grant:r",
            user_grant.as_str(),
            "*S-1-5-32-544:F",
            "*S-1-5-18:F",
        ],
    ] {
        let status = Command::new("icacls.exe")
            .arg(path)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        anyhow::ensure!(
            status.success(),
            "failed to restrict administrator session file ACL"
        );
    }
    let output = Command::new("icacls.exe").arg(path).output()?;
    anyhow::ensure!(
        output.status.success(),
        "failed to verify administrator session file ACL"
    );
    anyhow::ensure!(
        acl_listing_has_expected_entries(&output.stdout),
        "administrator session file ACL contains unexpected entries"
    );
    Ok(())
}

#[cfg(windows)]
fn acl_listing_has_expected_entries(listing: &[u8]) -> bool {
    fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|window| *window == needle)
            .count()
    }

    occurrences(listing, b"(I)") == 0
        && occurrences(listing, b":(") == 3
        && occurrences(listing, b"(F)") == 2
}

#[cfg(not(windows))]
fn protect_current_user_file(_path: &Path, _user_sid: &str) -> anyhow::Result<()> {
    anyhow::bail!("administrator session file ACL is unsupported off Windows")
}

#[cfg(all(test, windows))]
mod descriptor_tests {
    use super::*;
    use crate::admin_mode::windows::current_windows_identity;

    #[test]
    fn acl_verification_accepts_non_utf8_localized_account_names() {
        let listing = b"\x81:(R,D)\r\nBUILTIN\\Administrators:(F)\r\nSYSTEM:(F)\r\n";
        assert!(acl_listing_has_expected_entries(listing));
    }

    fn junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn descriptor_publish_rejects_prepositioned_proof_reparse_point() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let descriptor_path = temp.path().join("computer-use-admin.json");
        let proof_path = descriptor_path.with_extension("proof");
        std::fs::create_dir(&outside).unwrap();
        junction(&proof_path, &outside);
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: 1,
            broker_creation_time: 1,
            shim_path: PathBuf::from(r"C:\shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path,
            proof_hash: sha256_bytes(b"secret"),
            transport_path: temp.path().join("transport"),
            backup_path: temp.path().join("backup"),
            original_hash: "original".to_owned(),
            patched_hash: "patched".to_owned(),
        };

        assert!(
            write_descriptor_and_proof(&descriptor_path, &descriptor, "secret", "S-1-5-21-1")
                .is_err()
        );
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        assert!(!descriptor_path.exists());
    }

    #[test]
    fn missing_descriptor_without_recovery_evidence_uses_narrow_evidence_scan() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".codex");
        let state = temp.path().join(".codex-session-delete");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let descriptor = state.join("administrator-mode-computer-use.v1.json");
        let mut inspected_owned_evidence = false;

        let outcome = recover_stale_admin_computer_use_impl(
            &home,
            &state,
            &descriptor,
            |inspect_marked_transports| {
                assert!(!inspect_marked_transports);
                inspected_owned_evidence = true;
                Ok(())
            },
            |_| panic!("missing descriptor must not verify a hook"),
            |_| panic!("missing descriptor must not restore a hook"),
        )
        .unwrap();

        assert_eq!(outcome, ComputerUseRecoveryOutcome::NothingToRecover);
        assert!(inspected_owned_evidence);
    }

    #[test]
    fn trusted_state_files_accept_production_sibling_of_codex_home() {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join(".codex");
        let state_dir = temp.path().join(".codex-session-delete");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        let descriptor = state_dir.join("administrator-mode-computer-use.v1.json");
        let proof = descriptor.with_extension("proof");
        std::fs::write(&descriptor, b"descriptor").unwrap();
        std::fs::write(&proof, b"proof").unwrap();

        ensure_trusted_state_file(&state_dir, &descriptor).unwrap();
        ensure_trusted_state_file(&state_dir, &proof).unwrap();
    }

    #[test]
    fn trusted_state_files_reject_outside_and_symlink_paths() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("descriptor.json");
        std::fs::write(&outside_file, b"outside").unwrap();
        assert!(ensure_trusted_state_file(&state_dir, &outside_file).is_err());

        let link = state_dir.join("descriptor.json");
        match std::os::windows::fs::symlink_file(&outside_file, &link) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(1314) => return,
            Err(error) => panic!("failed to create symlink fixture: {error}"),
        }
        assert!(ensure_trusted_state_file(&state_dir, &link).is_err());
        assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside");
    }

    #[test]
    fn recovery_rejects_a_hardlinked_descriptor_before_restoring_the_hook() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let descriptor_path = home.join("computer-use-admin.json");
        let proof_path = descriptor_path.with_extension("proof");
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: u32::MAX,
            broker_creation_time: 0,
            shim_path: home.join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"proof"),
            transport_path: home.join("transport.js"),
            backup_path: home.join("transport.backup.js"),
            original_hash: "original".to_owned(),
            patched_hash: "patched".to_owned(),
        };
        std::fs::write(&descriptor_path, serde_json::to_vec(&descriptor).unwrap()).unwrap();
        std::fs::write(&proof_path, b"proof").unwrap();
        let outside = home.join("outside-descriptor-copy.json");
        std::fs::hard_link(&descriptor_path, &outside).unwrap();
        let outside_bytes = std::fs::read(&outside).unwrap();

        let result = recover_stale_admin_computer_use_impl(
            &home,
            descriptor_path.parent().unwrap(),
            &descriptor_path,
            |_| Ok(()),
            |_| panic!("hardlinked descriptor must not verify a live hook"),
            |_| panic!("hardlinked descriptor must not restore a hook"),
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), outside_bytes);
        assert_eq!(std::fs::read(&descriptor_path).unwrap(), outside_bytes);
    }

    #[test]
    fn cleanup_evidence_recovers_an_orphaned_proof_after_descriptor_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let descriptor_path = temp.path().join("computer-use-admin.json");
        let proof_path = descriptor_path.with_extension("proof");
        let identity = current_windows_identity().unwrap();
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: u32::MAX,
            broker_creation_time: 0,
            shim_path: temp.path().join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"proof"),
            transport_path: temp.path().join("transport.js"),
            backup_path: temp.path().join("transport.backup.js"),
            original_hash: "original".to_owned(),
            patched_hash: "patched".to_owned(),
        };
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();

        let error = cleanup_with_recovery_evidence(&descriptor_path, &identity.user_sid, || {
            crate::admin_secure_io::SecureFileLease::open_for_delete(&descriptor_path)?.delete()?;
            anyhow::bail!("synthetic crash before proof deletion")
        })
        .unwrap_err();
        assert!(error.to_string().contains("synthetic crash"));
        assert!(!descriptor_path.exists());
        assert!(proof_path.exists());
        assert!(recovery_evidence_path(&descriptor_path).unwrap().exists());

        assert_eq!(
            recover_stale_admin_computer_use_impl(
                temp.path(),
                temp.path(),
                &descriptor_path,
                |_| Ok(()),
                |_| panic!("missing descriptor must not verify"),
                |_| panic!("missing descriptor must not restore"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert!(!proof_path.exists());
        assert!(!recovery_evidence_path(&descriptor_path).unwrap().exists());
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();
    }

    #[test]
    fn descriptor_is_secret_free_and_files_are_current_user_acl_protected() {
        let temp = tempfile::tempdir().unwrap();
        let descriptor_path = temp.path().join("computer-use-admin.json");
        let proof_path = temp.path().join("computer-use-admin.proof");
        let identity = current_windows_identity().unwrap();
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: std::process::id(),
            broker_creation_time: process_creation_time(std::process::id()).unwrap().unwrap(),
            shim_path: PathBuf::from(r"C:\Program Files\CodexPlusPlus\codex-plus-admin-shim.exe"),
            pipe_name: r"\\.\pipe\codex-plus-admin-test".to_owned(),
            session_id: "session-123".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"secret-proof-token"),
            transport_path: temp.path().join("helper_transport.js"),
            backup_path: temp.path().join("helper_transport.backup.js"),
            original_hash: "original".to_owned(),
            patched_hash: "patched".to_owned(),
        };
        write_descriptor_and_proof(
            &descriptor_path,
            &descriptor,
            "secret-proof-token",
            &identity.user_sid,
        )
        .unwrap();
        let descriptor_bytes = std::fs::read(&descriptor_path).unwrap();
        assert!(
            !descriptor_bytes
                .windows("secret-proof-token".len())
                .any(|value| value == b"secret-proof-token")
        );
        assert_eq!(
            std::fs::read_to_string(&proof_path).unwrap(),
            "secret-proof-token"
        );
        for path in [&descriptor_path, &proof_path] {
            let output = std::process::Command::new("icacls.exe")
                .arg(path)
                .output()
                .unwrap();
            assert!(output.status.success());
            let listing = String::from_utf8_lossy(&output.stdout);
            assert!(!listing.contains("(I)"), "ACL inheritance must be removed");
            assert!(!listing.to_ascii_lowercase().contains("everyone"));
            assert!(!listing.to_ascii_lowercase().contains("anonymous"));
            assert!(listing.matches("(F)").count() >= 2);
            assert!(listing.contains("(R,D)") || listing.contains("(D,R)"));
        }
        remove_descriptor_and_proof(&descriptor_path, &descriptor).unwrap();
        assert!(!descriptor_path.exists());
        assert!(!proof_path.exists());
    }

    #[test]
    fn cleanup_does_not_delete_unknown_descriptor_or_proof_files() {
        let temp = tempfile::tempdir().unwrap();
        let descriptor_path = temp.path().join("computer-use-admin.json");
        let proof_path = temp.path().join("computer-use-admin.proof");
        std::fs::write(&descriptor_path, b"unknown descriptor").unwrap();
        std::fs::write(&proof_path, b"unknown proof").unwrap();

        let expected = ComputerUseAdminDescriptor {
            broker_pid: std::process::id(),
            broker_creation_time: process_creation_time(std::process::id()).unwrap().unwrap(),
            shim_path: PathBuf::from("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"expected proof"),
            transport_path: temp.path().join("helper_transport.js"),
            backup_path: temp.path().join("helper_transport.backup.js"),
            original_hash: "original".to_owned(),
            patched_hash: "patched".to_owned(),
        };
        assert!(remove_descriptor_and_proof(&descriptor_path, &expected).is_err());
        assert_eq!(
            std::fs::read(&descriptor_path).unwrap(),
            b"unknown descriptor"
        );
        assert_eq!(std::fs::read(&proof_path).unwrap(), b"unknown proof");
    }

    #[test]
    fn proof_acl_removes_unrelated_explicit_aces() {
        let temp = tempfile::tempdir().unwrap();
        let proof_path = temp.path().join("computer-use-admin.proof");
        std::fs::write(&proof_path, b"proof").unwrap();
        let identity = current_windows_identity().unwrap();
        let status = std::process::Command::new("icacls.exe")
            .arg(&proof_path)
            .args(["/grant", "*S-1-1-0:R"])
            .status()
            .unwrap();
        assert!(status.success());

        protect_current_user_file(&proof_path, &identity.user_sid).unwrap();
        let output = std::process::Command::new("icacls.exe")
            .arg(&proof_path)
            .output()
            .unwrap();
        assert!(output.status.success());
        let listing = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        assert!(!listing.contains("everyone"));
        assert!(!listing.contains("s-1-1-0"));
    }

    #[test]
    fn valid_live_broker_recovery_leaves_all_owned_files_byte_for_byte_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let transport = home.join("helper_transport.js");
        let helper = home.join("codex-computer-use.exe");
        const FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const P=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},P().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;
        std::fs::write(&transport, FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: "0.4.20".to_owned(),
        };
        let state_dir = temp.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let descriptor_path = state_dir.join("computer-use-admin.json");
        crate::computer_use_guard::install_admin_computer_use_hook_with_artifacts(
            &artifacts,
            &descriptor_path,
        )
        .unwrap();
        let backup = transport.with_file_name("helper_transport.js.bak-codex-plus-admin");
        let proof_path = descriptor_path.with_extension("proof");
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: std::process::id(),
            broker_creation_time: process_creation_time(std::process::id()).unwrap().unwrap(),
            shim_path: home.join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"proof"),
            transport_path: std::fs::canonicalize(&transport).unwrap(),
            backup_path: std::fs::canonicalize(&backup).unwrap(),
            original_hash: sha256_bytes(&std::fs::read(&backup).unwrap()),
            patched_hash: sha256_bytes(&std::fs::read(&transport).unwrap()),
        };
        let identity = current_windows_identity().unwrap();
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();
        let original = std::fs::read(&backup).unwrap();
        std::fs::write(&transport, original).unwrap();
        let before = [
            std::fs::read(&descriptor_path).unwrap(),
            std::fs::read(&proof_path).unwrap(),
            std::fs::read(&transport).unwrap(),
            std::fs::read(&backup).unwrap(),
        ];

        let outcome = recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
            &home,
            &descriptor_path,
            &artifacts,
            process_creation_time,
        )
        .unwrap();

        assert_eq!(outcome, ComputerUseRecoveryOutcome::ActiveBroker);
        assert_eq!(before[0], std::fs::read(&descriptor_path).unwrap());
        assert_eq!(before[1], std::fs::read(&proof_path).unwrap());
        assert_eq!(before[2], std::fs::read(&transport).unwrap());
        assert_eq!(before[3], std::fs::read(&backup).unwrap());
    }

    #[test]
    fn shutdown_recovery_terminates_exact_live_broker_before_restoring_owned_state() {
        use std::cell::Cell;

        let (_temp, home, artifacts, descriptor_path, descriptor) =
            recovery_fixture(42_424, 77_777);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        let lookup_count = Cell::new(0);
        let terminated = Cell::new(false);

        let outcome =
            recover_stale_admin_computer_use_for_shutdown_with_artifacts_and_process_control(
                &home,
                &descriptor_path,
                &artifacts,
                |pid| {
                    assert_eq!(pid, descriptor.broker_pid);
                    let count = lookup_count.get();
                    lookup_count.set(count + 1);
                    Ok((count == 0).then_some(descriptor.broker_creation_time))
                },
                |pid, creation_time| {
                    assert_eq!(pid, descriptor.broker_pid);
                    assert_eq!(creation_time, descriptor.broker_creation_time);
                    terminated.set(true);
                    Ok(())
                },
            )
            .unwrap();

        assert_eq!(outcome, ComputerUseRecoveryOutcome::Recovered);
        assert!(terminated.get());
        assert_eq!(lookup_count.get(), 2);
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
    }

    #[test]
    fn shutdown_recovery_never_terminates_a_reused_pid() {
        let (_temp, home, artifacts, descriptor_path, descriptor) =
            recovery_fixture(42_425, 88_888);
        let original = std::fs::read(&descriptor.backup_path).unwrap();

        let outcome =
            recover_stale_admin_computer_use_for_shutdown_with_artifacts_and_process_control(
                &home,
                &descriptor_path,
                &artifacts,
                |pid| {
                    assert_eq!(pid, descriptor.broker_pid);
                    Ok(Some(descriptor.broker_creation_time + 1))
                },
                |_, _| panic!("a reused PID must never be terminated"),
            )
            .unwrap();

        assert_eq!(outcome, ComputerUseRecoveryOutcome::Recovered);
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
    }

    #[test]
    fn shutdown_recovery_preserves_owned_state_when_broker_does_not_exit() {
        let (_temp, home, artifacts, descriptor_path, descriptor) =
            recovery_fixture(42_426, 99_999);
        let before = [
            std::fs::read(&descriptor_path).unwrap(),
            std::fs::read(&descriptor.proof_path).unwrap(),
            std::fs::read(&descriptor.transport_path).unwrap(),
            std::fs::read(&descriptor.backup_path).unwrap(),
        ];

        let error =
            recover_stale_admin_computer_use_for_shutdown_with_artifacts_and_process_control(
                &home,
                &descriptor_path,
                &artifacts,
                |_| Ok(Some(descriptor.broker_creation_time)),
                |_, _| Ok(()),
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("administrator broker remained active after termination")
        );
        assert_eq!(before[0], std::fs::read(&descriptor_path).unwrap());
        assert_eq!(before[1], std::fs::read(&descriptor.proof_path).unwrap());
        assert_eq!(
            before[2],
            std::fs::read(&descriptor.transport_path).unwrap()
        );
        assert_eq!(before[3], std::fs::read(&descriptor.backup_path).unwrap());
    }

    #[test]
    fn exact_broker_termination_waits_for_the_matching_process_to_exit() {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let pid = child.id();
        let creation_time = process_creation_time(pid).unwrap().unwrap();

        terminate_process_with_creation_time(pid, creation_time).unwrap();

        assert!(child.wait().unwrap().code().is_some());
        assert_eq!(process_creation_time(pid).unwrap(), None);
    }

    #[test]
    fn exact_broker_termination_rejects_a_creation_time_mismatch() {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let pid = child.id();
        let creation_time = process_creation_time(pid).unwrap().unwrap();

        let result = terminate_process_with_creation_time(pid, creation_time + 1);
        let still_running = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();

        assert!(result.is_err());
        assert!(still_running);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("broker process identity changed before termination")
        );
    }

    #[test]
    fn reused_live_pid_with_wrong_creation_time_is_recovered_as_stale() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let transport = home.join("helper_transport.js");
        let helper = home.join("codex-computer-use.exe");
        const FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const P=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},P().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;
        std::fs::write(&transport, FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: "0.4.20".to_owned(),
        };
        let descriptor_path = home.join("computer-use-admin.json");
        crate::computer_use_guard::install_admin_computer_use_hook_with_artifacts(
            &artifacts,
            &descriptor_path,
        )
        .unwrap();
        let backup = transport.with_file_name("helper_transport.js.bak-codex-plus-admin");
        let proof_path = descriptor_path.with_extension("proof");
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: std::process::id(),
            broker_creation_time: process_creation_time(std::process::id()).unwrap().unwrap() + 1,
            shim_path: home.join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"proof"),
            transport_path: std::fs::canonicalize(&transport).unwrap(),
            backup_path: std::fs::canonicalize(&backup).unwrap(),
            original_hash: sha256_bytes(&std::fs::read(&backup).unwrap()),
            patched_hash: sha256_bytes(&std::fs::read(&transport).unwrap()),
        };
        let identity = current_windows_identity().unwrap();
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts(&home, &descriptor_path, &artifacts)
                .unwrap(),
            ComputerUseRecoveryOutcome::Recovered
        );
        assert_eq!(std::fs::read_to_string(&transport).unwrap(), FIXTURE);
        assert!(!backup.exists());
        assert!(!descriptor_path.exists());
        assert!(!proof_path.exists());
    }

    #[test]
    fn complete_dead_recovery_restores_hook_before_deleting_owned_state() {
        let (temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let proof_path = descriptor.proof_path.clone();
        let transport = descriptor.transport_path.clone();
        let backup = descriptor.backup_path.clone();
        let original = std::fs::read(&backup).unwrap();
        let descriptor_before = std::fs::read(&descriptor_path).unwrap();
        let proof_before = std::fs::read(&proof_path).unwrap();

        let outcome = recover_stale_admin_computer_use_impl(
            &home,
            descriptor_path.parent().unwrap(),
            &descriptor_path,
            |_| Ok(()),
            |value| {
                crate::computer_use_guard::verify_stale_admin_computer_use_hook_with_artifacts(
                    &artifacts,
                    &descriptor_path,
                    &value.transport_path,
                    &value.backup_path,
                    &value.original_hash,
                    &value.patched_hash,
                )
            },
            |value| {
                assert_eq!(std::fs::read(&descriptor_path).unwrap(), descriptor_before);
                assert_eq!(std::fs::read(&proof_path).unwrap(), proof_before);
                crate::computer_use_guard::restore_stale_admin_computer_use_hook_with_artifacts(
                    &artifacts,
                    &descriptor_path,
                    &value.transport_path,
                    &value.backup_path,
                    &value.original_hash,
                    &value.patched_hash,
                )
            },
        )
        .unwrap();

        assert_eq!(outcome, ComputerUseRecoveryOutcome::Recovered);
        assert_eq!(std::fs::read(&transport).unwrap(), original);
        assert!(!backup.exists());
        assert!(!descriptor_path.exists());
        assert!(!proof_path.exists());
        drop(temp);
    }

    #[test]
    fn complete_dead_recovery_accepts_a_legacy_hook_that_strips_back_to_the_owned_backup() {
        let (_temp, home, artifacts, descriptor_path, mut descriptor) =
            recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        let legacy_patched = std::fs::read_to_string(&descriptor.transport_path)
            .unwrap()
            .replacen(
                "/* codex-plus-admin-computer-use:begin */",
                "/* codex-plus-admin-computer-use:begin */void 0;",
                1,
            );
        std::fs::write(&descriptor.transport_path, legacy_patched.as_bytes()).unwrap();
        descriptor.patched_hash = sha256_bytes(legacy_patched.as_bytes());
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor.proof_path).unwrap();
        let identity = current_windows_identity().unwrap();
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts(&home, &descriptor_path, &artifacts)
                .unwrap(),
            ComputerUseRecoveryOutcome::Recovered
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
    }

    #[test]
    fn complete_dead_recovery_uses_descriptor_bound_runtime_without_the_old_helper()
    -> anyhow::Result<()> {
        let source = match crate::computer_use_guard::resolve_admin_computer_use_artifacts(
            &crate::codex_home::default_codex_home_dir(),
        ) {
            Ok(artifacts) => artifacts,
            Err(error) => {
                eprintln!("SKIP: installed Computer Use runtime is unavailable: {error}");
                return Ok(());
            }
        };
        let source_sky = source
            .helper_exe
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("installed Computer Use helper has no @oai/sky root")?;
        let source_backup = source
            .helper_transport
            .with_file_name("helper_transport.js.bak-codex-plus-admin");
        let source_original = if source_backup.is_file() {
            source_backup
        } else {
            source.helper_transport.clone()
        };
        let original = std::fs::read(&source_original)?;

        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let sky = home
            .join("plugins/cache/openai-bundled/computer-use/old-runtime/node_modules/@oai/sky");
        let transport =
            sky.join("dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap())?;
        std::fs::copy(source_sky.join("package.json"), sky.join("package.json"))?;
        std::fs::write(&transport, &original)?;
        let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
            helper_exe: sky.join("bin/windows/codex-computer-use.exe"),
            helper_transport: transport.clone(),
            sky_version: source.sky_version.clone(),
        };
        let state_dir = temp.path().join(".codex-session-delete");
        std::fs::create_dir_all(&state_dir)?;
        let descriptor_path = state_dir.join("administrator-mode-computer-use.v1.json");
        crate::computer_use_guard::install_admin_computer_use_hook_with_artifacts(
            &artifacts,
            &descriptor_path,
        )?;
        let backup = transport.with_file_name("helper_transport.js.bak-codex-plus-admin");
        let identity = current_windows_identity()?;
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: u32::MAX,
            broker_creation_time: 0,
            shim_path: temp.path().join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: descriptor_path.with_extension("proof"),
            proof_hash: sha256_bytes(b"proof"),
            transport_path: PathBuf::from(
                std::fs::canonicalize(&transport)?
                    .to_string_lossy()
                    .trim_start_matches(r"\\?\")
                    .replace('\\', "/"),
            ),
            backup_path: std::fs::canonicalize(&backup)?,
            original_hash: sha256_bytes(&original),
            patched_hash: sha256_bytes(&std::fs::read(&transport)?),
        };
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)?;

        assert_eq!(
            recover_stale_admin_computer_use(&home, &state_dir, &descriptor_path)?,
            ComputerUseRecoveryOutcome::Recovered
        );
        assert_eq!(std::fs::read(&transport)?, original);
        assert!(!backup.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
        Ok(())
    }

    #[test]
    fn forged_complete_recovery_descriptor_fails_closed_without_mutation() {
        let (_temp, home, artifacts, descriptor_path, mut descriptor) =
            recovery_fixture(u32::MAX, 0);
        let forged = home.join("forged-transport.js");
        std::fs::write(&forged, b"unknown transport").unwrap();
        descriptor.transport_path = forged.clone();
        descriptor.patched_hash = sha256_bytes(b"forged hash");
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor.proof_path).unwrap();
        let identity = current_windows_identity().unwrap();
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::write(&artifacts.helper_transport, original).unwrap();
        let before = [
            std::fs::read(&descriptor_path).unwrap(),
            std::fs::read(&descriptor.proof_path).unwrap(),
            std::fs::read(&artifacts.helper_transport).unwrap(),
            std::fs::read(&descriptor.backup_path).unwrap(),
            std::fs::read(&forged).unwrap(),
        ];

        assert!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                process_creation_time,
            )
            .is_err()
        );

        assert_eq!(before[0], std::fs::read(&descriptor_path).unwrap());
        assert_eq!(before[1], std::fs::read(&descriptor.proof_path).unwrap());
        assert_eq!(
            before[2],
            std::fs::read(&artifacts.helper_transport).unwrap()
        );
        assert_eq!(before[3], std::fs::read(&descriptor.backup_path).unwrap());
        assert_eq!(before[4], std::fs::read(&forged).unwrap());
    }

    #[test]
    fn recovery_completes_after_transport_restore_crash_before_backup_cleanup() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::write(&descriptor.transport_path, &original).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts(&home, &descriptor_path, &artifacts)
                .unwrap(),
            ComputerUseRecoveryOutcome::Recovered
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
    }

    #[test]
    fn dead_descriptor_with_already_restored_transport_removes_owned_state() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::write(&descriptor.transport_path, &original).unwrap();
        std::fs::remove_file(&descriptor.backup_path).unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts(&home, &descriptor_path, &artifacts)
                .unwrap(),
            ComputerUseRecoveryOutcome::Recovered
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
        assert!(!evidence.exists());
    }

    #[test]
    fn dead_descriptor_with_unknown_transport_and_missing_backup_fails_closed() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        std::fs::remove_file(&descriptor.backup_path).unwrap();
        std::fs::write(&descriptor.transport_path, b"unknown transport").unwrap();
        let descriptor_before = std::fs::read(&descriptor_path).unwrap();
        let proof_before = std::fs::read(&descriptor.proof_path).unwrap();
        let transport_before = std::fs::read(&descriptor.transport_path).unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();

        assert!(
            recover_stale_admin_computer_use_with_artifacts(&home, &descriptor_path, &artifacts)
                .is_err()
        );
        assert_eq!(std::fs::read(&descriptor_path).unwrap(), descriptor_before);
        assert_eq!(std::fs::read(&descriptor.proof_path).unwrap(), proof_before);
        assert_eq!(
            std::fs::read(&descriptor.transport_path).unwrap(),
            transport_before
        );
        assert!(!descriptor.backup_path.exists());
        assert!(evidence.exists());
    }

    #[test]
    fn recovery_completes_after_partial_transport_restore_write() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::write(
            &descriptor.transport_path,
            &original[..original.len().saturating_sub(1)],
        )
        .unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts(&home, &descriptor_path, &artifacts)
                .unwrap(),
            ComputerUseRecoveryOutcome::Recovered
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
    }

    #[test]
    fn process_lookup_access_denied_preserves_all_recovery_evidence() {
        let (_temp, home, artifacts, descriptor_path, descriptor) =
            recovery_fixture(std::process::id(), 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::write(&descriptor.transport_path, original).unwrap();
        let before = [
            std::fs::read(&descriptor_path).unwrap(),
            std::fs::read(&descriptor.proof_path).unwrap(),
            std::fs::read(&descriptor.transport_path).unwrap(),
            std::fs::read(&descriptor.backup_path).unwrap(),
        ];

        assert!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| anyhow::bail!("synthetic access denied"),
            )
            .is_err()
        );
        assert_eq!(before[0], std::fs::read(&descriptor_path).unwrap());
        assert_eq!(before[1], std::fs::read(&descriptor.proof_path).unwrap());
        assert_eq!(
            before[2],
            std::fs::read(&descriptor.transport_path).unwrap()
        );
        assert_eq!(before[3], std::fs::read(&descriptor.backup_path).unwrap());
    }

    #[test]
    fn descriptor_absent_recovery_repairs_only_the_owned_rename_window() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::remove_file(&descriptor.transport_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor.proof_path).unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();
        std::fs::write(&evidence, RECOVERY_EVIDENCE_BYTES).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| panic!("descriptor-less recovery must not inspect a process"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!evidence.exists());
    }

    #[test]
    fn descriptor_absent_recovery_removes_crash_orphaned_proof_when_evidence_is_owned() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();
        std::fs::write(&evidence, RECOVERY_EVIDENCE_BYTES).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| panic!("descriptor-less recovery must not inspect a process"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor.proof_path.exists());
        assert!(!evidence.exists());
    }

    #[test]
    fn partial_descriptor_recovery_uses_owned_evidence_and_restores_the_hook() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::write(&descriptor_path, b"{\"schemaVersion\":").unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();
        std::fs::write(&evidence, RECOVERY_EVIDENCE_BYTES).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| panic!("partial descriptor recovery must not inspect a process"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!descriptor_path.exists());
        assert!(!descriptor.proof_path.exists());
        assert!(!evidence.exists());
    }

    #[test]
    fn descriptor_absent_recovery_accepts_legacy_rename_window_without_marker() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::remove_file(&descriptor.transport_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor.proof_path).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| panic!("descriptor-less recovery must not inspect a process"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
    }

    #[test]
    fn descriptor_absent_recovery_finishes_restored_transport_backup_cleanup() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::write(&descriptor.transport_path, &original).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor.proof_path).unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();
        std::fs::write(&evidence, RECOVERY_EVIDENCE_BYTES).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| panic!("descriptor-less recovery must not inspect a process"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!evidence.exists());
    }

    #[test]
    fn descriptor_absent_recovery_restores_owned_patched_transport() {
        let (_temp, home, artifacts, descriptor_path, descriptor) = recovery_fixture(u32::MAX, 0);
        let original = std::fs::read(&descriptor.backup_path).unwrap();
        std::fs::remove_file(&descriptor_path).unwrap();
        std::fs::remove_file(&descriptor.proof_path).unwrap();
        let evidence = recovery_evidence_path(&descriptor_path).unwrap();
        std::fs::write(&evidence, RECOVERY_EVIDENCE_BYTES).unwrap();

        assert_eq!(
            recover_stale_admin_computer_use_with_artifacts_and_process_lookup(
                &home,
                &descriptor_path,
                &artifacts,
                |_| panic!("descriptor-less recovery must not inspect a process"),
            )
            .unwrap(),
            ComputerUseRecoveryOutcome::NothingToRecover
        );
        assert_eq!(std::fs::read(&descriptor.transport_path).unwrap(), original);
        assert!(!descriptor.backup_path.exists());
        assert!(!evidence.exists());
    }

    #[test]
    fn configured_helper_and_transport_must_match_resolved_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join("codex-computer-use.exe");
        let transport = temp.path().join("helper_transport.js");
        let other = temp.path().join("other-helper_transport.js");
        std::fs::write(&helper, b"helper").unwrap();
        std::fs::write(&transport, b"transport").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
            helper_exe: helper.clone(),
            helper_transport: transport.clone(),
            sky_version: "0.4.20".to_owned(),
        };

        assert!(validate_configured_artifact_paths(&artifacts, &helper, &transport).is_ok());
        assert!(validate_configured_artifact_paths(&artifacts, &helper, &other).is_err());
    }

    fn recovery_fixture(
        broker_pid: u32,
        broker_creation_time: u64,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        crate::computer_use_guard::AdminComputerUseArtifacts,
        PathBuf,
        ComputerUseAdminDescriptor,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let transport = home.join("helper_transport.js");
        let helper = home.join("codex-computer-use.exe");
        const FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const P=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},P().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;
        std::fs::write(&transport, FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: "0.4.20".to_owned(),
        };
        let descriptor_path = home.join("computer-use-admin.json");
        crate::computer_use_guard::install_admin_computer_use_hook_with_artifacts(
            &artifacts,
            &descriptor_path,
        )
        .unwrap();
        let backup = transport.with_file_name("helper_transport.js.bak-codex-plus-admin");
        let proof_path = descriptor_path.with_extension("proof");
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid,
            broker_creation_time,
            shim_path: home.join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path,
            proof_hash: sha256_bytes(b"proof"),
            transport_path: std::fs::canonicalize(&transport).unwrap(),
            backup_path: std::fs::canonicalize(&backup).unwrap(),
            original_hash: sha256_bytes(&std::fs::read(&backup).unwrap()),
            patched_hash: sha256_bytes(&std::fs::read(&transport).unwrap()),
        };
        let identity = current_windows_identity().unwrap();
        write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
            .unwrap();
        (temp, home, artifacts, descriptor_path, descriptor)
    }

    #[test]
    fn startup_descriptor_write_failure_restores_hook_and_owned_proof() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join("codex-computer-use.exe");
        const FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const P=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},P().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;
        std::fs::write(&transport, FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: "0.4.20".to_owned(),
        };
        let descriptor_path = temp.path().join("descriptor.json");
        std::fs::create_dir(&descriptor_path).unwrap();
        let rollback =
            crate::computer_use_guard::install_admin_computer_use_hook_transaction_with_artifacts(
                &artifacts,
                &descriptor_path,
            )
            .unwrap();
        let hook = rollback.installed();
        let proof_path = descriptor_path.with_extension("proof");
        let descriptor = ComputerUseAdminDescriptor {
            broker_pid: std::process::id(),
            broker_creation_time: process_creation_time(std::process::id()).unwrap().unwrap(),
            shim_path: temp.path().join("shim.exe"),
            pipe_name: "pipe".to_owned(),
            session_id: "session".to_owned(),
            proof_path: proof_path.clone(),
            proof_hash: sha256_bytes(b"proof"),
            transport_path: hook.transport_path.clone(),
            backup_path: hook.backup_path.clone(),
            original_hash: hook.original_hash.clone(),
            patched_hash: hook.patched_hash.clone(),
        };
        let identity = current_windows_identity().unwrap();

        assert!(
            write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
                .is_err()
        );
        drop(rollback);

        assert_eq!(std::fs::read_to_string(&transport).unwrap(), FIXTURE);
        assert!(!hook_backup_path(&transport).exists());
        assert!(!proof_path.exists());
        assert!(descriptor_path.is_dir());
    }

    fn hook_backup_path(transport: &Path) -> PathBuf {
        transport.with_file_name("helper_transport.js.bak-codex-plus-admin")
    }
}

#[cfg(windows)]
mod platform {
    use std::io::{Read, Seek, SeekFrom};
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::{Context, bail, ensure};
    use serde::Deserialize;
    use serde_json::json;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use tokio::process::{Child, Command};
    use tokio::task::JoinHandle;
    use windows::Win32::Foundation::{FALSE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows::Win32::System::Pipes::{
        CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::core::PCWSTR;

    use super::{
        AdminComputerUseConfig, ComputerUseAdminDescriptor, KillOnCloseJob,
        cleanup_with_recovery_evidence, process_creation_time, publish_recovery_evidence,
        recovery_evidence_path, remove_descriptor_and_proof, remove_owned_file_if_hash_matches,
        remove_recovery_evidence, sha256_bytes, validate_configured_artifact_paths,
        write_descriptor_and_proof,
    };
    use crate::admin_mode::windows::{
        WindowsIdentity, admin_pipe_sddl, process_has_high_integrity, process_windows_identity,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const MAX_HELLO_BYTES: usize = 64 * 1024;
    const MAX_MUX_PAYLOAD: usize = 64 * 1024;
    const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
    const MUX_STDIN_DATA: u8 = 1;
    const MUX_STDIN_EOF: u8 = 2;
    const MUX_STDOUT_DATA: u8 = 3;
    const MUX_STDOUT_EOF: u8 = 4;
    const MUX_STDERR_DATA: u8 = 5;
    const MUX_STDERR_EOF: u8 = 6;
    const MUX_EXIT: u8 = 7;
    type IntegrityChecker = dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync;
    type PipeFactory = dyn Fn(&str, &str, bool) -> anyhow::Result<NamedPipeServer> + Send + Sync;

    struct VerifiedExecutableLease {
        path: std::path::PathBuf,
        _file: std::fs::File,
    }

    impl VerifiedExecutableLease {
        fn open(path: &Path, expected_hash: &str) -> anyhow::Result<Self> {
            use sha2::{Digest, Sha256};
            let path = std::fs::canonicalize(path)?;
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0)
                .open(&path)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            file.seek(SeekFrom::Start(0))?;
            let hash = format!("{:x}", Sha256::digest(&bytes));
            bytes.fill(0);
            ensure!(hash == expected_hash, "computer_use_contract_incompatible");
            Ok(Self { path, _file: file })
        }
    }

    pub struct AdminComputerUseRuntime {
        pub pipe_name: String,
        pub session_id: String,
        relay_task: Option<JoinHandle<anyhow::Result<()>>>,
        shutdown: Option<tokio::sync::oneshot::Sender<()>>,
        fatal: tokio::sync::watch::Receiver<Option<String>>,
        _helper_runtime_copy: crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy,
        _helper_lease: VerifiedExecutableLease,
        hook: Option<crate::computer_use_guard::OwnedAdminComputerUseHook>,
        descriptor_path: std::path::PathBuf,
        descriptor: ComputerUseAdminDescriptor,
        expected_user_sid: String,
    }

    impl AdminComputerUseRuntime {
        pub async fn start(
            config: AdminComputerUseConfig<'_>,
            job: &KillOnCloseJob,
        ) -> anyhow::Result<Self> {
            let artifacts =
                crate::computer_use_guard::resolve_admin_computer_use_artifacts(config.home)?;
            validate_configured_artifact_paths(
                &artifacts,
                config.helper_exe,
                config.helper_transport,
            )?;
            let helper_exe = std::fs::canonicalize(&artifacts.helper_exe)?;
            let helper_hash = {
                use sha2::{Digest, Sha256};

                let helper_hash = format!("{:x}", Sha256::digest(std::fs::read(&helper_exe)?));
                let supported_hashes =
                    crate::computer_use_guard::supported_helper_sha256s(&artifacts.sky_version)
                        .context("computer_use_contract_incompatible")?;
                anyhow::ensure!(
                    supported_hashes
                        .iter()
                        .any(|expected| helper_hash.eq_ignore_ascii_case(expected)),
                    "computer_use_contract_incompatible"
                );
                helper_hash
            };
            let helper_lease = VerifiedExecutableLease::open(&helper_exe, &helper_hash)?;
            let helper_runtime_copy =
                crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy::create_for_helper(
                    &helper_lease._file,
                    &helper_hash,
                )
                .context("stage administrator Computer Use runtime copy")?;
            let helper_runtime_lease =
                VerifiedExecutableLease::open(helper_runtime_copy.executable_path(), &helper_hash)?;
            preflight_helper(
                &helper_runtime_lease,
                job.raw_handle().0 as isize,
                config.expected_user_sid,
                config.expected_logon_sid,
                Arc::new(process_has_high_integrity),
            )
            .await
            .context("Computer Use readiness probe failed")?;
            let pipe = create_restricted_pipe(config.pipe_name, config.expected_user_sid)?;
            let proof_path = config.descriptor_path.with_extension("proof");
            let mut recovery_evidence = RecoveryEvidenceGuard::create(
                &recovery_evidence_path(config.descriptor_path)?,
                config.expected_user_sid,
            )?;
            let hook_rollback = crate::computer_use_guard::install_admin_computer_use_hook_transaction_with_resolved_artifacts(
                &artifacts,
                config.descriptor_path,
            )?;
            let hook = hook_rollback.installed();
            let descriptor = ComputerUseAdminDescriptor {
                broker_pid: std::process::id(),
                broker_creation_time: process_creation_time(std::process::id())?
                    .context("administrator broker creation time is unavailable")?,
                shim_path: std::fs::canonicalize(config.shim_path)
                    .context("administrator shim path is unavailable")?,
                pipe_name: config.pipe_name.to_owned(),
                session_id: config.session_id.to_owned(),
                proof_path: proof_path.clone(),
                proof_hash: sha256_bytes(config.session_proof.as_bytes()),
                transport_path: hook.transport_path.clone(),
                backup_path: hook.backup_path.clone(),
                original_hash: hook.original_hash.clone(),
                patched_hash: hook.patched_hash.clone(),
            };
            if let Err(error) = write_descriptor_and_proof(
                config.descriptor_path,
                &descriptor,
                config.session_proof,
                config.expected_user_sid,
            ) {
                let _ = remove_owned_file_if_hash_matches(&proof_path, &descriptor.proof_hash);
                return Err(error);
            }
            if let Err(error) = recovery_evidence.complete() {
                let _ = remove_descriptor_and_proof(config.descriptor_path, &descriptor);
                return Err(error);
            }
            let session_id = config.session_id.to_owned();
            let proof = config.session_proof.to_owned();
            let expected_sid = config.expected_user_sid.to_owned();
            let expected_logon_sid = config.expected_logon_sid.to_owned();
            let relay_session = session_id.clone();
            let job_handle = job.raw_handle().0 as isize;
            let pipe_name = config.pipe_name.to_owned();
            let authorized_helper_path = helper_exe.clone();
            let relay_helper_path = helper_runtime_lease.path.clone();
            let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
            let (fatal_tx, fatal) = tokio::sync::watch::channel(None);
            let relay_task = tokio::spawn(async move {
                supervise_broker(
                    serve_clients(
                        pipe,
                        &pipe_name,
                        &relay_session,
                        &proof,
                        &expected_sid,
                        &expected_logon_sid,
                        &authorized_helper_path,
                        &relay_helper_path,
                        helper_hash.as_str(),
                        job_handle,
                        Arc::new(process_has_high_integrity),
                        Arc::new(create_restricted_pipe_instance),
                        shutdown_rx,
                    ),
                    fatal_tx,
                )
                .await
            });
            Ok(Self {
                pipe_name: config.pipe_name.to_owned(),
                session_id,
                relay_task: Some(relay_task),
                shutdown: Some(shutdown),
                fatal,
                _helper_runtime_copy: helper_runtime_copy,
                _helper_lease: helper_runtime_lease,
                hook: Some(hook_rollback.commit()),
                descriptor_path: config.descriptor_path.to_owned(),
                descriptor,
                expected_user_sid: config.expected_user_sid.to_owned(),
            })
        }

        pub async fn verify_ready(&self) -> anyhow::Result<()> {
            ensure!(
                self.fatal.borrow().is_none(),
                "administrator Computer Use broker failed readiness"
            );
            ensure!(
                !self
                    .relay_task
                    .as_ref()
                    .is_some_and(JoinHandle::is_finished),
                "administrator Computer Use broker exited early"
            );
            Ok(())
        }

        pub fn health_receiver(&self) -> tokio::sync::watch::Receiver<Option<String>> {
            self.fatal.clone()
        }

        pub async fn shutdown(mut self) -> anyhow::Result<()> {
            let relay_task = self.relay_task.take().context("broker task missing")?;
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            let mut relay_task = relay_task;
            let relay = match tokio::time::timeout(Duration::from_secs(3), &mut relay_task).await {
                Ok(result) => match result {
                    Ok(result) => result,
                    Err(error) if error.is_cancelled() => Ok(()),
                    Err(error) => Err(error.into()),
                },
                Err(error) => {
                    relay_task.abort();
                    let _ = relay_task.await;
                    Err(anyhow::Error::new(error).context("timed out stopping Computer Use broker"))
                }
            };
            let hook = self.hook.take().context("owned hook missing")?;
            let cleanup = cleanup_with_recovery_evidence(
                &self.descriptor_path,
                &self.expected_user_sid,
                || {
                    hook.restore().and_then(|_| {
                        remove_descriptor_and_proof(&self.descriptor_path, &self.descriptor)
                    })
                },
            );
            relay.and(cleanup)
        }
    }

    struct RecoveryEvidenceGuard {
        path: PathBuf,
        armed: bool,
    }

    impl RecoveryEvidenceGuard {
        fn create(path: &Path, user_sid: &str) -> anyhow::Result<Self> {
            publish_recovery_evidence(path, user_sid)?;
            Ok(Self {
                path: path.to_owned(),
                armed: true,
            })
        }

        fn complete(&mut self) -> anyhow::Result<()> {
            remove_recovery_evidence(&self.path)?;
            self.armed = false;
            Ok(())
        }
    }

    impl Drop for RecoveryEvidenceGuard {
        fn drop(&mut self) {
            if self.armed {
                let _ = remove_recovery_evidence(&self.path);
            }
        }
    }

    impl Drop for AdminComputerUseRuntime {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            if let Some(task) = self.relay_task.take() {
                task.abort();
            }
            if let Some(hook) = self.hook.take() {
                let _ = cleanup_with_recovery_evidence(
                    &self.descriptor_path,
                    &self.expected_user_sid,
                    || {
                        hook.restore().and_then(|_| {
                            remove_descriptor_and_proof(&self.descriptor_path, &self.descriptor)
                        })
                    },
                );
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Hello {
        protocol: u8,
        session_id: String,
        mode: String,
        client_pid: u32,
        proof: String,
        helper_args: Vec<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ClientFailureStage {
        Accept,
        Authentication,
        HelperStart,
        HelperIntegrity,
        Relay,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ClientFailurePolicy {
        Recoverable,
        Fatal,
    }

    fn client_failure_policy(stage: ClientFailureStage) -> ClientFailurePolicy {
        match stage {
            ClientFailureStage::Authentication | ClientFailureStage::Relay => {
                ClientFailurePolicy::Recoverable
            }
            ClientFailureStage::Accept
            | ClientFailureStage::HelperStart
            | ClientFailureStage::HelperIntegrity => ClientFailurePolicy::Fatal,
        }
    }

    #[derive(Debug)]
    struct ClientServeFailure {
        stage: ClientFailureStage,
        error: anyhow::Error,
    }

    impl ClientServeFailure {
        fn new(stage: ClientFailureStage, error: impl Into<anyhow::Error>) -> Self {
            Self {
                stage,
                error: error.into(),
            }
        }
    }

    async fn serve_one_client(
        mut pipe: NamedPipeServer,
        session_id: &str,
        proof: &str,
        expected_sid: &str,
        expected_logon_sid: &str,
        authorized_helper_exe: &Path,
        runtime_helper_exe: &Path,
        helper_hash: &str,
        job_handle: isize,
        integrity_checker: Arc<IntegrityChecker>,
    ) -> Result<(), ClientServeFailure> {
        pipe.connect().await.map_err(|error| {
            ClientServeFailure::new(
                ClientFailureStage::Accept,
                anyhow::Error::new(error).context("accept Computer Use client"),
            )
        })?;
        let authenticated = tokio::time::timeout(
            AUTH_TIMEOUT,
            authenticate(
                &mut pipe,
                session_id,
                proof,
                expected_sid,
                expected_logon_sid,
                authorized_helper_exe,
            ),
        )
        .await
        .context("Computer Use authentication timed out")
        .and_then(|result| result)
        .map_err(|error| ClientServeFailure::new(ClientFailureStage::Authentication, error))?;
        let Some(helper_args) = authenticated else {
            let rejected =
                serde_json::to_vec(&json!({"accepted":false,"reason":"authentication-rejected"}))
                    .map_err(|error| ClientServeFailure::new(ClientFailureStage::Relay, error))?;
            write_frame(&mut pipe, &rejected)
                .await
                .map_err(|error| ClientServeFailure::new(ClientFailureStage::Relay, error))?;
            pipe.shutdown().await.ok();
            return Ok(());
        };

        let lease = VerifiedExecutableLease::open(runtime_helper_exe, helper_hash)
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::HelperStart, error))?;
        let mut child = spawn_helper(&lease, &helper_args)
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::HelperStart, error))?;
        drop(lease);
        let prepared = prepare_helper_for_relay(
            &mut pipe,
            &mut child,
            job_handle,
            expected_sid,
            expected_logon_sid,
            integrity_checker,
        )
        .await?;
        if !prepared {
            return Err(ClientServeFailure::new(
                ClientFailureStage::HelperIntegrity,
                anyhow::anyhow!("administrator Computer Use helper failed integrity verification"),
            ));
        }
        relay_helper(pipe, child)
            .await
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::Relay, error))
    }

    async fn supervise_broker<F>(
        broker: F,
        fatal: tokio::sync::watch::Sender<Option<String>>,
    ) -> anyhow::Result<()>
    where
        F: std::future::Future<Output = anyhow::Result<()>>,
    {
        let result = broker.await;
        if result.is_err() {
            let _ = fatal.send(Some(
                "administrator Computer Use broker stopped unexpectedly".to_owned(),
            ));
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn serve_clients(
        first_pipe: NamedPipeServer,
        pipe_name: &str,
        session_id: &str,
        proof: &str,
        expected_sid: &str,
        expected_logon_sid: &str,
        authorized_helper_exe: &Path,
        runtime_helper_exe: &Path,
        helper_hash: &str,
        job_handle: isize,
        integrity_checker: Arc<IntegrityChecker>,
        pipe_factory: Arc<PipeFactory>,
        mut shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        let mut current_pipe = Some(first_pipe);
        loop {
            let replacement_pipe = pipe_factory(pipe_name, expected_sid, false)
                .context("recreate Computer Use broker pipe")?;
            let pipe = current_pipe
                .take()
                .context("Computer Use broker listener ownership was lost")?;
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                result = serve_one_client(
                    pipe,
                    session_id,
                    proof,
                    expected_sid,
                    expected_logon_sid,
                    authorized_helper_exe,
                    runtime_helper_exe,
                    helper_hash,
                    job_handle,
                    Arc::clone(&integrity_checker),
                ) => {
                    if let Err(failure) = result {
                        if client_failure_policy(failure.stage) == ClientFailurePolicy::Fatal {
                            return Err(failure.error.context("Computer Use broker invariant failed"));
                        }
                        let _ = crate::diagnostic_log::append_diagnostic_log(
                            "administrator_mode.computer_use_client_failed",
                            serde_json::json!({"message": sanitize_broker_error(&failure.error)}),
                        );
                    }
                }
            }
            current_pipe = Some(replacement_pipe);
        }
    }

    fn sanitize_broker_error(_error: &anyhow::Error) -> &'static str {
        "administrator Computer Use client failed"
    }

    async fn prepare_helper_for_relay(
        pipe: &mut (impl AsyncWrite + Unpin),
        child: &mut Child,
        job_handle: isize,
        expected_sid: &str,
        expected_logon_sid: &str,
        integrity_checker: Arc<IntegrityChecker>,
    ) -> Result<bool, ClientServeFailure> {
        prepare_helper_for_relay_with_identity_checker(
            pipe,
            child,
            job_handle,
            expected_sid,
            expected_logon_sid,
            Arc::new(process_windows_identity),
            integrity_checker,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_helper_for_relay_with_identity_checker(
        pipe: &mut (impl AsyncWrite + Unpin),
        child: &mut Child,
        job_handle: isize,
        expected_sid: &str,
        expected_logon_sid: &str,
        identity_checker: Arc<dyn Fn(u32) -> anyhow::Result<WindowsIdentity> + Send + Sync>,
        integrity_checker: Arc<IntegrityChecker>,
    ) -> Result<bool, ClientServeFailure> {
        let child_pid = child
            .id()
            .context("Computer Use helper has no PID")
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::HelperIntegrity, error))?;
        let raw_handle = child
            .raw_handle()
            .context("Computer Use helper has no handle")
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::HelperIntegrity, error))?;
        if let Err(error) =
            unsafe { AssignProcessToJobObject(HANDLE(job_handle as _), HANDLE(raw_handle)) }
                .context("assign Computer Use helper to administrator job")
        {
            terminate_child(child).await;
            return Err(ClientServeFailure::new(
                ClientFailureStage::HelperIntegrity,
                error,
            ));
        }
        let high_integrity =
            match integrity_checker(child_pid).context("inspect Computer Use integrity") {
                Ok(high_integrity) => high_integrity,
                Err(error) => {
                    terminate_child(child).await;
                    return Err(ClientServeFailure::new(
                        ClientFailureStage::HelperIntegrity,
                        error,
                    ));
                }
            };
        if !high_integrity {
            terminate_child(child).await;
            write_frame(
                pipe,
                &serde_json::to_vec(
                    &json!({"accepted":false,"reason":"helper-integrity-rejected"}),
                )
                .map_err(|error| ClientServeFailure::new(ClientFailureStage::Relay, error))?,
            )
            .await
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::Relay, error))?;
            return Ok(false);
        }
        if !trusted_client_identity(
            identity_checker(child_pid),
            expected_sid,
            expected_logon_sid,
        ) {
            terminate_child(child).await;
            return Err(ClientServeFailure::new(
                ClientFailureStage::HelperIntegrity,
                anyhow::anyhow!("administrator Computer Use helper identity is not trusted"),
            ));
        }
        let accepted = serde_json::to_vec(&json!({"accepted":true}))
            .map_err(|error| ClientServeFailure::new(ClientFailureStage::Relay, error))?;
        if let Err(error) = write_frame(pipe, &accepted).await {
            terminate_child(child).await;
            return Err(ClientServeFailure::new(ClientFailureStage::Relay, error));
        }
        Ok(true)
    }

    async fn authenticate(
        pipe: &mut NamedPipeServer,
        session_id: &str,
        proof: &str,
        expected_sid: &str,
        expected_logon_sid: &str,
        helper_exe: &Path,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let mut payload = read_frame(pipe, MAX_HELLO_BYTES).await?;
        let mut hello: Hello =
            serde_json::from_slice(&payload).context("invalid Computer Use hello")?;
        let pipe_pid = named_pipe_client_pid(pipe)?;
        let identity = process_windows_identity(pipe_pid);
        let proof_matches = constant_time_proof_eq(hello.proof.as_bytes(), proof.as_bytes());
        unsafe {
            hello.proof.as_bytes_mut().fill(0);
        }
        payload.fill(0);
        let valid_identity = hello.protocol == 1
            && hello.mode == "computer-use"
            && hello.session_id == session_id
            && proof_matches
            && hello.client_pid == pipe_pid
            && trusted_client_identity(identity, expected_sid, expected_logon_sid);
        if !valid_identity || hello.helper_args.is_empty() {
            return Ok(None);
        }
        let provided = std::fs::canonicalize(&hello.helper_args[0]).ok();
        if provided.as_deref() != Some(helper_exe) {
            return Ok(None);
        }
        Ok(Some(hello.helper_args.into_iter().skip(1).collect()))
    }

    fn spawn_helper(lease: &VerifiedExecutableLease, args: &[String]) -> anyhow::Result<Child> {
        let mut command = Command::new(&lease.path);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        command
            .spawn()
            .context("start official Computer Use helper")
    }

    fn readiness_probe_args(parent_pid: u32) -> [String; 2] {
        ["--parent-pid".to_owned(), parent_pid.to_string()]
    }

    async fn preflight_helper(
        lease: &VerifiedExecutableLease,
        job_handle: isize,
        expected_sid: &str,
        expected_logon_sid: &str,
        integrity_checker: Arc<IntegrityChecker>,
    ) -> anyhow::Result<()> {
        let args = readiness_probe_args(std::process::id());
        let mut child = spawn_helper(lease, &args)?;
        let result = async {
            let child_pid = child
                .id()
                .context("Computer Use readiness helper has no PID")?;
            let raw_handle = child
                .raw_handle()
                .context("Computer Use readiness helper has no handle")?;
            unsafe { AssignProcessToJobObject(HANDLE(job_handle as _), HANDLE(raw_handle)) }
                .context("assign Computer Use readiness helper to administrator job")?;
            ensure!(
                trusted_client_identity(
                    process_windows_identity(child_pid),
                    expected_sid,
                    expected_logon_sid,
                ),
                "administrator Computer Use helper user is not trusted"
            );
            ensure!(
                integrity_checker(child_pid).context("inspect Computer Use readiness integrity")?,
                "administrator Computer Use helper is not elevated"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        terminate_child(&mut child).await;
        result
    }

    fn trusted_client_identity(
        identity: anyhow::Result<WindowsIdentity>,
        expected_user_sid: &str,
        expected_logon_sid: &str,
    ) -> bool {
        identity.is_ok_and(|identity| {
            identity.user_sid.eq_ignore_ascii_case(expected_user_sid)
                && identity.logon_sid.eq_ignore_ascii_case(expected_logon_sid)
        })
    }

    #[derive(Debug)]
    struct MuxFrame {
        channel: u8,
        payload: Vec<u8>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum RelayTaskKind {
        Input,
        Stdout,
        Stderr,
        Writer,
    }

    async fn relay_helper<S>(pipe: S, mut child: Child) -> anyhow::Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let child_stdin = child.stdin.take().context("helper stdin missing")?;
        let mut child_stdout = child.stdout.take().context("helper stdout missing")?;
        let mut child_stderr = child.stderr.take().context("helper stderr missing")?;
        let (mut pipe_reader, mut pipe_writer) = tokio::io::split(pipe);
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<MuxFrame>(16);

        let stdout_sender = sender.clone();
        let mut stdout_task = tokio::spawn(async move {
            copy_output_frames(
                &mut child_stdout,
                stdout_sender,
                MUX_STDOUT_DATA,
                MUX_STDOUT_EOF,
            )
            .await
        });
        let stderr_sender = sender.clone();
        let mut stderr_task = tokio::spawn(async move {
            copy_output_frames(
                &mut child_stderr,
                stderr_sender,
                MUX_STDERR_DATA,
                MUX_STDERR_EOF,
            )
            .await
        });
        let mut input_task: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut child_stdin = Some(child_stdin);
            loop {
                let frame = read_mux_frame(&mut pipe_reader).await?;
                match frame.channel {
                    MUX_STDIN_DATA => {
                        let stdin = child_stdin.as_mut().context("stdin data after EOF")?;
                        ensure!(!frame.payload.is_empty(), "empty stdin data frame");
                        stdin.write_all(&frame.payload).await?;
                        stdin.flush().await?;
                    }
                    MUX_STDIN_EOF => {
                        ensure!(frame.payload.is_empty(), "stdin EOF payload must be empty");
                        let mut stdin = child_stdin.take().context("duplicate stdin EOF")?;
                        stdin.shutdown().await?;
                        drop(stdin);
                    }
                    _ => bail!("invalid client-to-broker multiplex channel"),
                }
            }
        });
        let mut writer_task = tokio::spawn(async move {
            while let Some(frame) = receiver.recv().await {
                write_mux_frame(&mut pipe_writer, frame.channel, &frame.payload).await?;
            }
            pipe_writer.shutdown().await?;
            Ok::<_, anyhow::Error>(())
        });
        let mut stdout_done = false;
        let mut stderr_done = false;
        let first = {
            let child_wait = child.wait();
            tokio::pin!(child_wait);
            loop {
                tokio::select! {
                    status = &mut child_wait => break Ok(status.context("wait for Computer Use helper")?),
                    result = &mut input_task => break Err((RelayTaskKind::Input, premature_task("stdin relay", result))),
                    result = &mut writer_task => break Err((RelayTaskKind::Writer, premature_task("multiplex writer", result))),
                    result = &mut stdout_task, if !stdout_done => {
                        match join_task("stdout relay", result) {
                            Ok(()) => stdout_done = true,
                            Err(error) => break Err((RelayTaskKind::Stdout, error)),
                        }
                    }
                    result = &mut stderr_task, if !stderr_done => {
                        match join_task("stderr relay", result) {
                            Ok(()) => stderr_done = true,
                            Err(error) => break Err((RelayTaskKind::Stderr, error)),
                        }
                    }
                }
            }
        };
        let status = match first {
            Ok(status) => status,
            Err((source, error)) => {
                terminate_child(&mut child).await;
                if source != RelayTaskKind::Input {
                    abort_and_drain(&mut input_task).await;
                }
                if source != RelayTaskKind::Stdout {
                    abort_and_drain(&mut stdout_task).await;
                }
                if source != RelayTaskKind::Stderr {
                    abort_and_drain(&mut stderr_task).await;
                }
                if source != RelayTaskKind::Writer {
                    abort_and_drain(&mut writer_task).await;
                }
                return Err(error);
            }
        };
        abort_and_drain(&mut input_task).await;
        drain_output_tasks(
            &mut stdout_task,
            &mut stderr_task,
            &mut writer_task,
            stdout_done,
            stderr_done,
        )
        .await?;
        if let Err(error) = sender
            .send(MuxFrame {
                channel: MUX_EXIT,
                payload: status.code().unwrap_or(1).to_le_bytes().to_vec(),
            })
            .await
            .context("queue helper exit")
        {
            drop(sender);
            abort_and_drain(&mut writer_task).await;
            return Err(error);
        }
        drop(sender);
        match tokio::time::timeout(Duration::from_secs(3), &mut writer_task).await {
            Ok(result) => result.context("multiplex writer task")??,
            Err(error) => {
                abort_and_drain(&mut writer_task).await;
                return Err(
                    anyhow::Error::new(error).context("timed out flushing Computer Use output")
                );
            }
        }
        Ok(())
    }

    async fn drain_output_tasks(
        stdout_task: &mut JoinHandle<anyhow::Result<()>>,
        stderr_task: &mut JoinHandle<anyhow::Result<()>>,
        writer_task: &mut JoinHandle<anyhow::Result<()>>,
        stdout_done: bool,
        stderr_done: bool,
    ) -> anyhow::Result<()> {
        let drain = tokio::time::timeout(Duration::from_secs(3), async {
            let stdout = async {
                if stdout_done {
                    Ok(())
                } else {
                    join_task("stdout relay", (&mut *stdout_task).await)
                        .map_err(|error| (RelayTaskKind::Stdout, error))
                }
            };
            let stderr = async {
                if stderr_done {
                    Ok(())
                } else {
                    join_task("stderr relay", (&mut *stderr_task).await)
                        .map_err(|error| (RelayTaskKind::Stderr, error))
                }
            };
            tokio::try_join!(stdout, stderr)?;
            Ok::<_, (RelayTaskKind, anyhow::Error)>(())
        })
        .await;
        let (source, error) = match drain {
            Ok(Ok(())) => return Ok(()),
            Ok(Err((source, error))) => (Some(source), error),
            Err(error) => (
                None,
                anyhow::Error::new(error).context("timed out draining Computer Use output"),
            ),
        };
        if !stdout_done && source != Some(RelayTaskKind::Stdout) {
            abort_and_drain(stdout_task).await;
        }
        if !stderr_done && source != Some(RelayTaskKind::Stderr) {
            abort_and_drain(stderr_task).await;
        }
        abort_and_drain(writer_task).await;
        Err(error)
    }

    fn join_task(
        name: &str,
        result: Result<anyhow::Result<()>, tokio::task::JoinError>,
    ) -> anyhow::Result<()> {
        result
            .with_context(|| format!("{name} task failed"))?
            .with_context(|| format!("{name} failed"))
    }

    fn premature_task(
        name: &str,
        result: Result<anyhow::Result<()>, tokio::task::JoinError>,
    ) -> anyhow::Error {
        match join_task(name, result) {
            Ok(()) => anyhow::anyhow!("{name} ended before the helper exited"),
            Err(error) => error,
        }
    }

    async fn abort_and_drain(task: &mut JoinHandle<anyhow::Result<()>>) {
        if !task.is_finished() {
            task.abort();
        }
        let _ = task.await;
    }

    async fn terminate_child(child: &mut Child) {
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.start_kill();
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    }

    async fn copy_output_frames(
        reader: &mut (impl AsyncRead + Unpin),
        sender: tokio::sync::mpsc::Sender<MuxFrame>,
        data_channel: u8,
        eof_channel: u8,
    ) -> anyhow::Result<()> {
        let mut buffer = vec![0; MAX_MUX_PAYLOAD];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                sender
                    .send(MuxFrame {
                        channel: eof_channel,
                        payload: Vec::new(),
                    })
                    .await
                    .context("send output EOF")?;
                return Ok(());
            }
            sender
                .send(MuxFrame {
                    channel: data_channel,
                    payload: buffer[..read].to_vec(),
                })
                .await
                .context("send output data")?;
        }
    }

    async fn read_mux_frame(reader: &mut (impl AsyncRead + Unpin)) -> anyhow::Result<MuxFrame> {
        let channel = reader.read_u8().await?;
        let flags = reader.read_u8().await?;
        ensure!(flags == 0, "unsupported multiplex flags");
        let length = reader.read_u32_le().await? as usize;
        ensure!(length <= MAX_MUX_PAYLOAD, "multiplex payload is too large");
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload).await?;
        Ok(MuxFrame { channel, payload })
    }

    async fn write_mux_frame(
        writer: &mut (impl AsyncWrite + Unpin),
        channel: u8,
        payload: &[u8],
    ) -> anyhow::Result<()> {
        ensure!(
            payload.len() <= MAX_MUX_PAYLOAD,
            "multiplex payload is too large"
        );
        writer.write_all(&[channel, 0]).await?;
        writer
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await?;
        writer.write_all(payload).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn read_frame(
        reader: &mut (impl AsyncRead + Unpin),
        maximum: usize,
    ) -> anyhow::Result<Vec<u8>> {
        let length = reader.read_u32_le().await? as usize;
        ensure!(length <= maximum, "administrator hello is too large");
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload).await?;
        Ok(payload)
    }

    async fn write_frame(
        writer: &mut (impl AsyncWrite + Unpin),
        payload: &[u8],
    ) -> anyhow::Result<()> {
        writer
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await?;
        writer.write_all(payload).await?;
        writer.flush().await?;
        Ok(())
    }

    fn constant_time_proof_eq(provided: &[u8], expected: &[u8]) -> bool {
        use sha2::{Digest, Sha256};
        let mut left = Sha256::digest(provided);
        let mut right = Sha256::digest(expected);
        let difference = left
            .iter()
            .zip(right.iter())
            .fold(0u8, |value, (left, right)| value | left ^ right);
        left.fill(0);
        right.fill(0);
        difference == 0
    }

    fn create_restricted_pipe(pipe_name: &str, user_sid: &str) -> anyhow::Result<NamedPipeServer> {
        create_restricted_pipe_instance(pipe_name, user_sid, true)
    }

    fn create_restricted_pipe_instance(
        pipe_name: &str,
        user_sid: &str,
        first_instance: bool,
    ) -> anyhow::Result<NamedPipeServer> {
        let sddl = computer_use_pipe_sddl(user_sid)?;
        create_pipe_with_sddl(pipe_name, &sddl, first_instance)
    }

    #[cfg(test)]
    fn create_test_pipe_instance(
        pipe_name: &str,
        user_sid: &str,
        first_instance: bool,
    ) -> anyhow::Result<NamedPipeServer> {
        let sddl = admin_pipe_sddl(user_sid)?;
        create_pipe_with_sddl(pipe_name, &sddl, first_instance)
    }

    fn create_pipe_with_sddl(
        pipe_name: &str,
        sddl: &str,
        first_instance: bool,
    ) -> anyhow::Result<NamedPipeServer> {
        let wide_sddl = sddl.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide_sddl.as_ptr()),
                1,
                &mut descriptor,
                None,
            )?;
        }
        struct Descriptor(PSECURITY_DESCRIPTOR);
        impl Drop for Descriptor {
            fn drop(&mut self) {
                unsafe {
                    let _ = LocalFree(HLOCAL(self.0.0));
                }
            }
        }
        let descriptor = Descriptor(descriptor);
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: FALSE,
        };
        let wide_name = pipe_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let first_flag = if first_instance {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0)
        };
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide_name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first_flag,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                Some(&mut attributes),
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "failed to create restricted administrator pipe"
        );
        unsafe {
            NamedPipeServer::from_raw_handle(handle.0 as _).context("register administrator pipe")
        }
    }

    fn computer_use_pipe_sddl(user_sid: &str) -> anyhow::Result<String> {
        // Validate through the shared strict SID parser, but do not grant the
        // client FILE_CREATE_PIPE_INSTANCE (0x4). SYSTEM and Administrators
        // retain full control so only the elevated broker can create another
        // server instance in this namespace.
        admin_pipe_sddl(user_sid)?;
        const CLIENT_READ_WRITE_WITHOUT_CREATE_INSTANCE: u32 = 0x0012_019b;
        Ok(format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x{CLIENT_READ_WRITE_WITHOUT_CREATE_INSTANCE:08X};;;{user_sid})"
        ))
    }

    fn named_pipe_client_pid(pipe: &NamedPipeServer) -> anyhow::Result<u32> {
        let mut pid = 0;
        unsafe {
            GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()), &mut pid)?;
        }
        Ok(pid)
    }

    #[cfg(test)]
    mod tests {
        use std::pin::Pin;
        use std::task::{Context as TaskContext, Poll};

        use super::*;
        use crate::admin_mode::windows::{admin_pipe_name, current_windows_identity};

        #[tokio::test]
        async fn elevated_production_helper_runtime_copy_completes_real_readiness() {
            if !process_has_high_integrity(std::process::id()).unwrap_or(false) {
                eprintln!("SKIP: production Computer Use smoke requires elevation");
                return;
            }
            let home = crate::codex_home::default_codex_home_dir();
            let artifacts = crate::computer_use_guard::resolve_admin_computer_use_artifacts(&home)
                .expect("resolve installed Computer Use artifacts");
            let helper_bytes =
                std::fs::read(&artifacts.helper_exe).expect("read installed Computer Use helper");
            let expected_helper_sha256 = format!("{:x}", Sha256::digest(&helper_bytes));
            let supported_helper_sha256s =
                crate::computer_use_guard::supported_helper_sha256s(&artifacts.sky_version)
                    .expect("resolve supported Computer Use helper hashes");
            assert!(
                supported_helper_sha256s
                    .iter()
                    .any(|hash| expected_helper_sha256.eq_ignore_ascii_case(hash)),
                "installed Computer Use helper hash is not supported"
            );
            let source =
                VerifiedExecutableLease::open(&artifacts.helper_exe, &expected_helper_sha256)
                    .expect("lock Store Computer Use helper");
            let runtime_copy =
                crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy::create_for_helper(
                    &source._file,
                    &expected_helper_sha256,
                )
                .expect("stage Computer Use helper runtime copy");
            let runtime = VerifiedExecutableLease::open(
                runtime_copy.executable_path(),
                &expected_helper_sha256,
            )
            .expect("lock Computer Use helper runtime copy");
            let identity = current_windows_identity().expect("read elevated identity");
            let job = KillOnCloseJob::new(&format!(
                "admin-computer-use-production-smoke-{}",
                uuid::Uuid::new_v4()
            ))
            .expect("create Computer Use smoke job");

            preflight_helper(
                &runtime,
                job.raw_handle().0 as isize,
                &identity.user_sid,
                &identity.logon_sid,
                Arc::new(process_has_high_integrity),
            )
            .await
            .expect("real Computer Use helper readiness");
            drop(runtime);
            runtime_copy.cleanup().expect("runtime copy cleanup");
        }

        #[tokio::test]
        async fn elevated_production_computer_use_runtime_restores_the_real_transport() {
            if !process_has_high_integrity(std::process::id()).unwrap_or(false) {
                eprintln!("SKIP: production Computer Use lifecycle smoke requires elevation");
                return;
            }
            let home = crate::codex_home::default_codex_home_dir();
            let artifacts = crate::computer_use_guard::resolve_admin_computer_use_artifacts(&home)
                .expect("resolve installed Computer Use artifacts");
            let original_transport = std::fs::read(&artifacts.helper_transport)
                .expect("read original Computer Use transport");
            let identity = current_windows_identity().expect("read elevated identity");
            let session_id = uuid::Uuid::new_v4().simple().to_string();
            let pipe_name = admin_pipe_name(&format!("{session_id}-computer-use-smoke"));
            let descriptor_path = home.join(format!(
                "administrator-mode-computer-use-smoke-{session_id}.json"
            ));
            let shim_path = std::env::current_exe().expect("locate lifecycle smoke executable");
            let job =
                KillOnCloseJob::new(&format!("admin-computer-use-lifecycle-smoke-{session_id}"))
                    .expect("create Computer Use lifecycle smoke job");

            let runtime = AdminComputerUseRuntime::start(
                AdminComputerUseConfig {
                    home: &home,
                    descriptor_path: &descriptor_path,
                    shim_path: &shim_path,
                    helper_exe: &artifacts.helper_exe,
                    helper_transport: &artifacts.helper_transport,
                    pipe_name: &pipe_name,
                    session_id: &session_id,
                    session_proof: "production-computer-use-lifecycle-smoke-proof",
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
            )
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
                    eprintln!(
                        "SKIP: active Codex Computer Use runtime holds the real transport open"
                    );
                    return;
                }
                Err(error) => panic!("start real Computer Use administrator runtime: {error:#}"),
            };
            runtime.verify_ready().await.expect("runtime readiness");
            assert_ne!(
                std::fs::read(&artifacts.helper_transport).expect("read patched transport"),
                original_transport
            );

            runtime
                .shutdown()
                .await
                .expect("shutdown real Computer Use runtime");
            assert_eq!(
                std::fs::read(&artifacts.helper_transport).expect("read restored transport"),
                original_transport
            );
            assert!(!descriptor_path.exists());
            assert!(!descriptor_path.with_extension("proof").exists());
        }

        #[test]
        fn client_identity_accepts_same_account_and_logon_session() {
            let identity = current_windows_identity().expect("identity");
            assert!(trusted_client_identity(
                Ok(identity.clone()),
                &identity.user_sid,
                &identity.logon_sid,
            ));
        }

        #[test]
        fn client_identity_rejects_same_account_from_wrong_logon_session() {
            let identity = current_windows_identity().expect("identity");
            let mut wrong_logon = identity.clone();
            wrong_logon.logon_sid = "S-1-5-5-999-999".to_owned();
            assert!(!trusted_client_identity(
                Ok(wrong_logon),
                &identity.user_sid,
                &identity.logon_sid,
            ));
        }

        #[test]
        fn client_identity_rejects_missing_logon_sid() {
            let identity = current_windows_identity().expect("identity");
            assert!(!trusted_client_identity(
                Err(anyhow::anyhow!("missing logon SID")),
                &identity.user_sid,
                &identity.logon_sid,
            ));
        }
        use sha2::{Digest, Sha256};
        use tokio::net::windows::named_pipe::ClientOptions;
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_ACCESS_RIGHTS, WaitForSingleObject,
        };

        fn compile_reconnect_helper(temp: &tempfile::TempDir) -> PathBuf {
            let source = temp.path().join("reconnect_helper.rs");
            let helper = temp.path().join("codex-computer-use.exe");
            std::fs::write(
                &source,
                r#"use std::io::{Read,Write};
fn main(){let mut b=Vec::new();std::io::stdin().read_to_end(&mut b).unwrap();std::io::stdout().write_all(&b).unwrap()}"#,
            )
            .unwrap();
            assert!(
                std::process::Command::new("rustc")
                    .args(["--edition=2024", "-O"])
                    .arg(&source)
                    .arg("-o")
                    .arg(&helper)
                    .status()
                    .unwrap()
                    .success()
            );
            std::fs::canonicalize(helper).unwrap()
        }

        async fn connect_test_client(
            pipe_name: &str,
        ) -> tokio::net::windows::named_pipe::NamedPipeClient {
            loop {
                match ClientOptions::new().open(pipe_name) {
                    Ok(client) => return client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        }

        #[tokio::test]
        async fn medium_integrity_peer_cannot_create_a_squatting_computer_use_pipe_instance() {
            if process_has_high_integrity(std::process::id()).expect("current integrity") {
                eprintln!(
                    "SKIP: adversarial Computer Use pipe-instance test requires a medium-integrity peer"
                );
                return;
            }
            let identity = current_windows_identity().expect("identity");
            let pipe_name =
                admin_pipe_name(&format!("computer-use-owner-{}", uuid::Uuid::new_v4()));
            let owner = create_restricted_pipe(&pipe_name, &identity.user_sid)
                .expect("broker must create first instance");
            assert!(
                create_restricted_pipe_instance(&pipe_name, &identity.user_sid, false).is_err(),
                "medium-integrity peer created a squatting Computer Use server instance"
            );
            let _client = ClientOptions::new()
                .read(true)
                .write(true)
                .open(&pipe_name)
                .expect("Computer Use client read/write access must remain allowed");
            owner
                .connect()
                .await
                .or_else(|error| {
                    (error.raw_os_error() == Some(535))
                        .then_some(())
                        .ok_or(error)
                })
                .expect("broker must observe the Computer Use client connection");
        }

        #[test]
        fn computer_use_pipe_acl_allows_client_io_without_pipe_instance_creation() {
            let identity = current_windows_identity().expect("identity");
            let sddl = computer_use_pipe_sddl(&identity.user_sid).expect("Computer Use pipe SDDL");
            assert!(sddl.contains("0x0012019B"));
            assert!(!sddl.contains(&format!("GA;;;{}", identity.user_sid)));
            assert_eq!(0x0012_019b_u32 & 0x4, 0);
        }

        async fn run_reconnect_client(pipe_name: &str, helper: &Path, payload: &[u8]) {
            let mut client = connect_test_client(pipe_name).await;
            let hello = serde_json::to_vec(&json!({
                "protocol":1,"sessionId":"session","mode":"computer-use",
                "clientPid":std::process::id(),"proof":"proof",
                "helperArgs":[helper.to_string_lossy()]
            }))
            .unwrap();
            write_frame(&mut client, &hello).await.unwrap();
            let accepted: serde_json::Value =
                serde_json::from_slice(&read_frame(&mut client, 4096).await.unwrap()).unwrap();
            assert_eq!(accepted["accepted"], true);
            write_mux_frame(&mut client, MUX_STDIN_DATA, payload)
                .await
                .unwrap();
            write_mux_frame(&mut client, MUX_STDIN_EOF, &[])
                .await
                .unwrap();
            let mut output = Vec::new();
            loop {
                let frame = read_mux_frame(&mut client).await.unwrap();
                match frame.channel {
                    MUX_STDOUT_DATA => output.extend(frame.payload),
                    MUX_EXIT => break,
                    _ => {}
                }
            }
            assert_eq!(output, payload);
        }

        #[tokio::test]
        async fn broker_survives_malformed_client_and_serves_two_sequential_helpers() {
            let temp = tempfile::tempdir().unwrap();
            let helper = compile_reconnect_helper(&temp);
            let helper_hash = format!(
                "{:x}",
                sha2::Sha256::digest(std::fs::read(&helper).unwrap())
            );
            let identity = current_windows_identity().unwrap();
            let pipe_name = admin_pipe_name(&format!("computer-use-loop-{}", uuid::Uuid::new_v4()));
            let first_pipe =
                create_test_pipe_instance(&pipe_name, &identity.user_sid, true).unwrap();
            let job = KillOnCloseJob::new(&format!("computer-use-loop-{}", uuid::Uuid::new_v4()))
                .unwrap();
            let job_handle = job.raw_handle().0 as isize;
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let server_pipe_name = pipe_name.clone();
            let server_helper = helper.clone();
            let server_sid = identity.user_sid.clone();
            let server_logon_sid = identity.logon_sid.clone();
            let server = tokio::spawn(async move {
                serve_clients(
                    first_pipe,
                    &server_pipe_name,
                    "session",
                    "proof",
                    &server_sid,
                    &server_logon_sid,
                    &server_helper,
                    &server_helper,
                    &helper_hash,
                    job_handle,
                    Arc::new(|_| Ok(true)),
                    Arc::new(create_test_pipe_instance),
                    shutdown_rx,
                )
                .await
            });

            let mut malformed = connect_test_client(&pipe_name).await;
            write_frame(&mut malformed, b"{").await.unwrap();
            drop(malformed);
            tokio::time::timeout(
                Duration::from_secs(10),
                run_reconnect_client(&pipe_name, &helper, b"first"),
            )
            .await
            .expect("first reconnect client timed out");
            tokio::time::timeout(
                Duration::from_secs(10),
                run_reconnect_client(&pipe_name, &helper, b"second"),
            )
            .await
            .expect("second reconnect client timed out");
            shutdown_tx.send(()).unwrap();
            server.await.unwrap().unwrap();
        }

        #[tokio::test]
        async fn listener_replacement_is_created_before_current_instance_is_released() {
            use std::sync::atomic::{AtomicBool, Ordering};

            let temp = tempfile::tempdir().unwrap();
            let helper = temp.path().join("codex-computer-use.exe");
            std::fs::write(&helper, b"unused helper fixture").unwrap();
            let helper = std::fs::canonicalize(helper).unwrap();
            let identity = current_windows_identity().unwrap();
            let pipe_name =
                admin_pipe_name(&format!("computer-use-anchor-{}", uuid::Uuid::new_v4()));
            let first_pipe =
                create_test_pipe_instance(&pipe_name, &identity.user_sid, true).unwrap();
            let checked_replacement = Arc::new(AtomicBool::new(false));
            let checked_for_factory = Arc::clone(&checked_replacement);
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let server_pipe_name = pipe_name.clone();
            let server_sid = identity.user_sid.clone();
            let server_logon_sid = identity.logon_sid.clone();
            let server = tokio::spawn(async move {
                serve_clients(
                    first_pipe,
                    &server_pipe_name,
                    "session",
                    "proof",
                    &server_sid,
                    &server_logon_sid,
                    &helper,
                    &helper,
                    "unused",
                    0,
                    Arc::new(|_| Ok(true)),
                    Arc::new(move |name, sid, first| {
                        assert!(!first, "replacement must not claim first-instance mode");
                        if create_test_pipe_instance(name, sid, true).is_ok() {
                            anyhow::bail!("Computer Use pipe namespace ownership gap detected");
                        }
                        checked_for_factory.store(true, Ordering::SeqCst);
                        create_test_pipe_instance(name, sid, false)
                    }),
                    shutdown_rx,
                )
                .await
            });

            tokio::time::timeout(Duration::from_secs(1), async {
                while !checked_replacement.load(Ordering::SeqCst) && !server.is_finished() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("broker did not pre-create its replacement listener");
            assert!(checked_replacement.load(Ordering::SeqCst));
            let mut malformed = connect_test_client(&pipe_name).await;
            write_frame(&mut malformed, b"{").await.unwrap();
            drop(malformed);
            shutdown_tx.send(()).expect("broker must remain alive");
            server.await.unwrap().unwrap();
        }

        #[tokio::test]
        async fn replacement_listener_failure_sets_fatal_health_without_leaking_details() {
            let temp = tempfile::tempdir().unwrap();
            let helper = temp.path().join("codex-computer-use.exe");
            std::fs::write(&helper, b"unused helper fixture").unwrap();
            let helper = std::fs::canonicalize(helper).unwrap();
            let identity = current_windows_identity().unwrap();
            let pipe_name =
                admin_pipe_name(&format!("computer-use-fatal-{}", uuid::Uuid::new_v4()));
            let first_pipe =
                create_test_pipe_instance(&pipe_name, &identity.user_sid, true).unwrap();
            let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let (fatal_tx, mut fatal_rx) = tokio::sync::watch::channel(None);
            let failure = supervise_broker(
                serve_clients(
                    first_pipe,
                    &pipe_name,
                    "session",
                    "proof",
                    &identity.user_sid,
                    &identity.logon_sid,
                    &helper,
                    &helper,
                    "unused",
                    0,
                    Arc::new(|_| Ok(true)),
                    Arc::new(|_, _, _| {
                        anyhow::bail!("synthetic replacement failure containing secret-proof")
                    }),
                    shutdown_rx,
                ),
                fatal_tx,
            )
            .await
            .expect_err("replacement ownership loss must be fatal");
            assert!(
                failure
                    .to_string()
                    .contains("recreate Computer Use broker pipe")
            );
            fatal_rx.changed().await.unwrap();
            let health = fatal_rx.borrow().clone().expect("fatal health");
            assert_eq!(
                health,
                "administrator Computer Use broker stopped unexpectedly"
            );
            assert!(!health.contains("secret-proof"));
        }

        #[test]
        fn readiness_probe_uses_official_parent_pid_argument_shape() {
            assert_eq!(
                readiness_probe_args(4242),
                ["--parent-pid".to_owned(), "4242".to_owned()]
            );
        }

        #[tokio::test]
        async fn readiness_probe_fails_when_helper_is_not_high_integrity() {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("probe_helper.rs");
            let helper = temp.path().join("codex-computer-use.exe");
            std::fs::write(
                &source,
                "fn main(){std::thread::sleep(std::time::Duration::from_secs(30))}",
            )
            .unwrap();
            assert!(
                std::process::Command::new("rustc")
                    .arg(&source)
                    .arg("-o")
                    .arg(&helper)
                    .status()
                    .unwrap()
                    .success()
            );
            let hash = format!(
                "{:x}",
                sha2::Sha256::digest(std::fs::read(&helper).unwrap())
            );
            let lease = VerifiedExecutableLease::open(&helper, &hash).unwrap();
            let job = KillOnCloseJob::new(&format!("computer-use-probe-{}", uuid::Uuid::new_v4()))
                .unwrap();
            let identity = current_windows_identity().unwrap();
            assert!(
                preflight_helper(
                    &lease,
                    job.raw_handle().0 as isize,
                    &identity.user_sid,
                    &identity.logon_sid,
                    Arc::new(|_| Ok(false)),
                )
                .await
                .is_err()
            );
        }

        #[tokio::test]
        async fn readiness_probe_rejects_wrong_helper_sid() {
            let temp = tempfile::tempdir().unwrap();
            let helper = compile_reconnect_helper(&temp);
            let hash = format!(
                "{:x}",
                sha2::Sha256::digest(std::fs::read(&helper).unwrap())
            );
            let lease = VerifiedExecutableLease::open(&helper, &hash).unwrap();
            let job =
                KillOnCloseJob::new(&format!("computer-use-probe-sid-{}", uuid::Uuid::new_v4()))
                    .unwrap();
            assert!(
                preflight_helper(
                    &lease,
                    job.raw_handle().0 as isize,
                    "S-1-5-21-1-2-3-1001",
                    &current_windows_identity().unwrap().logon_sid,
                    Arc::new(|_| Ok(true)),
                )
                .await
                .is_err()
            );
        }

        #[tokio::test]
        async fn broker_fatal_result_is_published_without_secret_details() {
            let (fatal_tx, fatal_rx) = tokio::sync::watch::channel(None);
            let result = supervise_broker(
                async { anyhow::bail!("fatal proof-token-secret") },
                fatal_tx,
            )
            .await;
            assert!(result.is_err());
            let published = fatal_rx.borrow().clone().unwrap();
            assert_eq!(
                published,
                "administrator Computer Use broker stopped unexpectedly"
            );
            assert!(!published.contains("proof-token-secret"));
        }

        #[test]
        fn client_failure_policy_only_recovers_protocol_and_relay_failures() {
            assert_eq!(
                client_failure_policy(ClientFailureStage::Accept),
                ClientFailurePolicy::Fatal
            );
            assert_eq!(
                client_failure_policy(ClientFailureStage::HelperStart),
                ClientFailurePolicy::Fatal
            );
            assert_eq!(
                client_failure_policy(ClientFailureStage::HelperIntegrity),
                ClientFailurePolicy::Fatal
            );
            assert_eq!(
                client_failure_policy(ClientFailureStage::Authentication),
                ClientFailurePolicy::Recoverable
            );
            assert_eq!(
                client_failure_policy(ClientFailureStage::Relay),
                ClientFailurePolicy::Recoverable
            );
        }

        #[tokio::test]
        async fn official_helper_path_authenticates_while_staged_copy_executes() {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("fake_helper.rs");
            let helper = temp.path().join("codex-computer-use.exe");
            std::fs::write(
                &source,
                r#"use std::io::{Read,Write};
fn main(){let a:Vec<_>=std::env::args().skip(1).collect();if a!=["--expected","value with spaces"]{std::process::exit(64)}let mut b=Vec::new();std::io::stdin().read_to_end(&mut b).unwrap();std::io::stdout().write_all(b"out:").unwrap();std::io::stdout().write_all(&b).unwrap();std::io::stderr().write_all(b"warn").unwrap();std::process::exit(23)}"#,
            )
            .unwrap();
            assert!(
                std::process::Command::new("rustc")
                    .args(["--edition=2024", "-O"])
                    .arg(&source)
                    .arg("-o")
                    .arg(&helper)
                    .status()
                    .unwrap()
                    .success()
            );
            let helper = std::fs::canonicalize(helper).unwrap();
            let staged_helper = temp.path().join("staged-codex-computer-use.exe");
            std::fs::copy(&helper, &staged_helper).unwrap();
            let staged_helper = std::fs::canonicalize(staged_helper).unwrap();
            let identity = current_windows_identity().unwrap();
            let pipe_name = admin_pipe_name(&format!("computer-use-e2e-{}", uuid::Uuid::new_v4()));
            let pipe = create_restricted_pipe(&pipe_name, &identity.user_sid).unwrap();
            let job =
                KillOnCloseJob::new(&format!("computer-use-e2e-{}", uuid::Uuid::new_v4())).unwrap();
            let job_handle = job.raw_handle().0 as isize;
            let authorized_helper = helper.clone();
            let expected_helper = staged_helper.clone();
            let expected_hash = format!(
                "{:x}",
                sha2::Sha256::digest(std::fs::read(&expected_helper).unwrap())
            );
            let expected_sid = identity.user_sid.clone();
            let expected_logon_sid = identity.logon_sid.clone();
            let server = tokio::spawn(async move {
                serve_one_client(
                    pipe,
                    "session",
                    "proof",
                    &expected_sid,
                    &expected_logon_sid,
                    &authorized_helper,
                    &expected_helper,
                    &expected_hash,
                    job_handle,
                    Arc::new(|_| Ok(true)),
                )
                .await
            });
            let mut client = loop {
                match ClientOptions::new().open(&pipe_name) {
                    Ok(client) => break client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            };
            let hello = serde_json::to_vec(&json!({
                "protocol":1,
                "sessionId":"session",
                "mode":"computer-use",
                "clientPid":std::process::id(),
                "proof":"proof",
                "helperArgs":[helper.to_string_lossy(),"--expected","value with spaces"]
            }))
            .unwrap();
            write_frame(&mut client, &hello).await.unwrap();
            let accepted: serde_json::Value =
                serde_json::from_slice(&read_frame(&mut client, 4096).await.unwrap()).unwrap();
            assert_eq!(accepted["accepted"], true);
            write_mux_frame(&mut client, MUX_STDIN_DATA, b"request\n")
                .await
                .unwrap();
            write_mux_frame(&mut client, MUX_STDIN_EOF, &[])
                .await
                .unwrap();

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = loop {
                let frame = read_mux_frame(&mut client).await.unwrap();
                match frame.channel {
                    MUX_STDOUT_DATA => stdout.extend(frame.payload),
                    MUX_STDERR_DATA => stderr.extend(frame.payload),
                    MUX_STDOUT_EOF | MUX_STDERR_EOF => assert!(frame.payload.is_empty()),
                    MUX_EXIT => break i32::from_le_bytes(frame.payload.try_into().unwrap()),
                    _ => panic!("unexpected channel"),
                }
            };
            assert_eq!(stdout, b"out:request\n");
            assert_eq!(stderr, b"warn");
            assert_eq!(exit, 23);
            server.await.unwrap().unwrap();
        }

        #[tokio::test]
        async fn authentication_rejects_bad_proof_session_pid_sid_and_path() {
            let temp = tempfile::tempdir().unwrap();
            let helper = temp.path().join("codex-computer-use.exe");
            std::fs::write(&helper, b"fixture").unwrap();
            let helper = std::fs::canonicalize(helper).unwrap();
            let identity = current_windows_identity().unwrap();
            for (session, proof, pid, expected_sid, path) in [
                (
                    "session",
                    "bad",
                    std::process::id(),
                    identity.user_sid.as_str(),
                    helper.clone(),
                ),
                (
                    "bad",
                    "proof",
                    std::process::id(),
                    identity.user_sid.as_str(),
                    helper.clone(),
                ),
                (
                    "session",
                    "proof",
                    std::process::id() + 1,
                    identity.user_sid.as_str(),
                    helper.clone(),
                ),
                (
                    "session",
                    "proof",
                    std::process::id(),
                    "S-1-5-21-1-2-3-1001",
                    helper.clone(),
                ),
                (
                    "session",
                    "proof",
                    std::process::id(),
                    identity.user_sid.as_str(),
                    temp.path().join("other.exe"),
                ),
            ] {
                let pipe_name =
                    admin_pipe_name(&format!("computer-use-auth-{}", uuid::Uuid::new_v4()));
                let pipe = create_restricted_pipe(&pipe_name, &identity.user_sid).unwrap();
                let server_helper = helper.clone();
                let expected_sid = expected_sid.to_owned();
                let expected_logon_sid = identity.logon_sid.clone();
                let server = tokio::spawn(async move {
                    let mut pipe = pipe;
                    pipe.connect().await.unwrap();
                    authenticate(
                        &mut pipe,
                        "session",
                        "proof",
                        &expected_sid,
                        &expected_logon_sid,
                        &server_helper,
                    )
                    .await
                    .unwrap()
                });
                let mut client = loop {
                    match ClientOptions::new().open(&pipe_name) {
                        Ok(client) => break client,
                        Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                    }
                };
                let hello = serde_json::to_vec(&json!({
                    "protocol":1,"sessionId":session,"mode":"computer-use",
                    "clientPid":pid,"proof":proof,
                    "helperArgs":[path.to_string_lossy()]
                }))
                .unwrap();
                write_frame(&mut client, &hello).await.unwrap();
                assert!(server.await.unwrap().is_none());
            }
        }

        #[test]
        fn verified_executable_lease_blocks_write_and_replace_until_drop() {
            let temp = tempfile::tempdir().unwrap();
            let executable = temp.path().join("codex-computer-use.exe");
            std::fs::write(&executable, b"official bytes").unwrap();
            let hash = format!("{:x}", sha2::Sha256::digest(b"official bytes"));
            let lease = VerifiedExecutableLease::open(&executable, &hash).unwrap();
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&executable)
                    .is_err()
            );
            let replacement = temp.path().join("replacement.exe");
            std::fs::write(&replacement, b"replacement").unwrap();
            assert!(std::fs::rename(&replacement, &executable).is_err());
            drop(lease);
            std::fs::write(&executable, b"changed").unwrap();
            assert_eq!(std::fs::read(&executable).unwrap(), b"changed");
        }

        fn tampered_runtime_fixture() -> (
            tempfile::TempDir,
            AdminComputerUseRuntime,
            std::path::PathBuf,
            std::path::PathBuf,
            std::path::PathBuf,
            std::path::PathBuf,
        ) {
            let temp = tempfile::tempdir().unwrap();
            let transport = temp.path().join("helper_transport.js");
            let helper = temp.path().join("codex-computer-use.exe");
            const FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const P=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},P().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;
            std::fs::write(&transport, FIXTURE).unwrap();
            std::fs::write(&helper, b"fixture helper").unwrap();
            let artifacts = crate::computer_use_guard::AdminComputerUseArtifacts {
                helper_exe: helper,
                helper_transport: transport.clone(),
                sky_version: "0.4.20".to_owned(),
            };
            let descriptor_path = temp.path().join("descriptor.json");
            let rollback = crate::computer_use_guard::install_admin_computer_use_hook_transaction_with_artifacts(
                &artifacts,
                &descriptor_path,
            )
            .unwrap();
            let installed = rollback.installed();
            let proof_path = descriptor_path.with_extension("proof");
            let descriptor = ComputerUseAdminDescriptor {
                broker_pid: std::process::id(),
                broker_creation_time: process_creation_time(std::process::id()).unwrap().unwrap(),
                shim_path: temp.path().join("shim.exe"),
                pipe_name: "pipe".to_owned(),
                session_id: "session".to_owned(),
                proof_path: proof_path.clone(),
                proof_hash: sha256_bytes(b"proof"),
                transport_path: installed.transport_path.clone(),
                backup_path: installed.backup_path.clone(),
                original_hash: installed.original_hash.clone(),
                patched_hash: installed.patched_hash.clone(),
            };
            let identity = current_windows_identity().unwrap();
            write_descriptor_and_proof(&descriptor_path, &descriptor, "proof", &identity.user_sid)
                .unwrap();
            let backup = descriptor.backup_path.clone();
            let helper_hash = format!("{:x}", sha2::Sha256::digest(b"fixture helper"));
            let helper_lease =
                VerifiedExecutableLease::open(&artifacts.helper_exe, &helper_hash).unwrap();
            let helper_runtime_copy =
                crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy::create_test(
                    &helper_lease._file,
                    &helper_hash,
                )
                .unwrap();
            let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
            let (_fatal_tx, fatal) = tokio::sync::watch::channel(None);
            let runtime = AdminComputerUseRuntime {
                pipe_name: "pipe".to_owned(),
                session_id: "session".to_owned(),
                relay_task: Some(tokio::spawn(async move {
                    let _ = shutdown_rx.await;
                    Ok(())
                })),
                shutdown: Some(shutdown),
                fatal,
                _helper_runtime_copy: helper_runtime_copy,
                _helper_lease: helper_lease,
                hook: Some(rollback.commit()),
                descriptor_path: descriptor_path.clone(),
                descriptor,
                expected_user_sid: identity.user_sid,
            };
            assert!(
                std::fs::write(&transport, b"unknown tampered hook").is_err(),
                "the hook lease must block same-user replacement while elevated"
            );
            (
                temp,
                runtime,
                transport,
                backup,
                descriptor_path,
                proof_path,
            )
        }

        #[tokio::test]
        async fn shutdown_restores_a_hook_that_was_pinned_against_tampering() {
            let (_temp, runtime, transport, backup, descriptor, proof) = tampered_runtime_fixture();
            runtime.shutdown().await.unwrap();
            assert!(
                !std::fs::read_to_string(&transport)
                    .unwrap()
                    .contains("codex-plus-admin-computer-use:begin")
            );
            assert!(!backup.exists());
            assert!(!descriptor.exists());
            assert!(!proof.exists());
        }

        #[tokio::test]
        async fn drop_restores_a_hook_that_was_pinned_against_tampering() {
            let (_temp, runtime, transport, backup, descriptor, proof) = tampered_runtime_fixture();
            drop(runtime);
            assert!(
                !std::fs::read_to_string(&transport)
                    .unwrap()
                    .contains("codex-plus-admin-computer-use:begin")
            );
            assert!(!backup.exists());
            assert!(!descriptor.exists());
            assert!(!proof.exists());
        }

        fn long_running_child(emits_output: bool) -> Child {
            let command = if emits_output {
                "Write-Output output; Start-Sleep -Seconds 30"
            } else {
                "Start-Sleep -Seconds 30"
            };
            let mut child = Command::new("powershell.exe");
            child
                .args(["-NoProfile", "-NonInteractive", "-Command", command])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .creation_flags(CREATE_NO_WINDOW);
            child.spawn().unwrap()
        }

        async fn assert_relay_failure_terminates_child<S>(stream: S, child: Child)
        where
            S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        {
            let pid = child.id().unwrap();
            let process = unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(0x0010_0000), false, pid) }
                .expect("open original helper process for termination assertion");
            let process_handle = process.0 as isize;
            let result = tokio::time::timeout(Duration::from_secs(3), relay_helper(stream, child))
                .await
                .expect("relay cleanup must not deadlock");
            assert!(result.is_err());
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while unsafe { WaitForSingleObject(HANDLE(process_handle as _), 0) } != WAIT_OBJECT_0
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                unsafe { WaitForSingleObject(HANDLE(process_handle as _), 0) },
                WAIT_OBJECT_0
            );
            unsafe { CloseHandle(HANDLE(process_handle as _)).unwrap() };
        }

        async fn assert_process_handle_signaled_within_three_seconds(process_handle: isize) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while unsafe { WaitForSingleObject(HANDLE(process_handle as _), 0) } != WAIT_OBJECT_0
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                unsafe { WaitForSingleObject(HANDLE(process_handle as _), 0) },
                WAIT_OBJECT_0
            );
            unsafe { CloseHandle(HANDLE(process_handle as _)).unwrap() };
        }

        fn exact_process_handle(child: &Child) -> isize {
            let process = unsafe {
                OpenProcess(
                    PROCESS_ACCESS_RIGHTS(0x0010_0000),
                    false,
                    child.id().unwrap(),
                )
            }
            .expect("open exact helper process for termination assertion");
            process.0 as isize
        }

        #[tokio::test]
        async fn job_assignment_failure_terminates_exact_helper_before_returning() {
            let (mut server, _client) = tokio::io::duplex(128);
            let mut child = long_running_child(false);
            let process = exact_process_handle(&child);
            let identity = current_windows_identity().unwrap();

            assert!(
                prepare_helper_for_relay(
                    &mut server,
                    &mut child,
                    0,
                    &identity.user_sid,
                    &identity.logon_sid,
                    Arc::new(|_| Ok(true)),
                )
                .await
                .is_err()
            );
            assert_process_handle_signaled_within_three_seconds(process).await;
        }

        #[tokio::test]
        async fn integrity_error_terminates_exact_helper_before_returning() {
            let (mut server, _client) = tokio::io::duplex(128);
            let mut child = long_running_child(false);
            let process = exact_process_handle(&child);
            let job =
                KillOnCloseJob::new(&format!("computer-use-pre-relay-{}", uuid::Uuid::new_v4()))
                    .unwrap();
            let identity = current_windows_identity().unwrap();

            assert!(
                prepare_helper_for_relay(
                    &mut server,
                    &mut child,
                    job.raw_handle().0 as isize,
                    &identity.user_sid,
                    &identity.logon_sid,
                    Arc::new(|_| anyhow::bail!("synthetic integrity failure")),
                )
                .await
                .is_err()
            );
            assert_process_handle_signaled_within_three_seconds(process).await;
        }

        #[tokio::test]
        async fn actual_helper_rejects_wrong_logon_session_before_acceptance() {
            let (mut server, _client) = tokio::io::duplex(128);
            let mut child = long_running_child(false);
            let process = exact_process_handle(&child);
            let job = KillOnCloseJob::new(&format!(
                "computer-use-logon-boundary-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap();
            let identity = current_windows_identity().unwrap();
            let mut wrong_logon = identity.clone();
            wrong_logon.logon_sid = "S-1-5-5-999-999".to_owned();

            assert!(
                prepare_helper_for_relay_with_identity_checker(
                    &mut server,
                    &mut child,
                    job.raw_handle().0 as isize,
                    &identity.user_sid,
                    &identity.logon_sid,
                    Arc::new(move |_| Ok(wrong_logon.clone())),
                    Arc::new(|_| Ok(true)),
                )
                .await
                .is_err()
            );
            assert_process_handle_signaled_within_three_seconds(process).await;
        }

        #[tokio::test]
        async fn actual_helper_rejects_missing_logon_identity_before_acceptance() {
            let (mut server, _client) = tokio::io::duplex(128);
            let mut child = long_running_child(false);
            let process = exact_process_handle(&child);
            let job = KillOnCloseJob::new(&format!(
                "computer-use-missing-logon-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap();
            let identity = current_windows_identity().unwrap();

            assert!(
                prepare_helper_for_relay_with_identity_checker(
                    &mut server,
                    &mut child,
                    job.raw_handle().0 as isize,
                    &identity.user_sid,
                    &identity.logon_sid,
                    Arc::new(|_| anyhow::bail!("synthetic missing logon identity")),
                    Arc::new(|_| Ok(true)),
                )
                .await
                .is_err()
            );
            assert_process_handle_signaled_within_three_seconds(process).await;
        }

        #[tokio::test]
        async fn accepted_write_failure_terminates_exact_helper_before_returning() {
            let mut stream = FailingWriteStream;
            let mut child = long_running_child(false);
            let process = exact_process_handle(&child);
            let job =
                KillOnCloseJob::new(&format!("computer-use-pre-relay-{}", uuid::Uuid::new_v4()))
                    .unwrap();
            let identity = current_windows_identity().unwrap();

            assert!(
                prepare_helper_for_relay(
                    &mut stream,
                    &mut child,
                    job.raw_handle().0 as isize,
                    &identity.user_sid,
                    &identity.logon_sid,
                    Arc::new(|_| Ok(true)),
                )
                .await
                .is_err()
            );
            assert_process_handle_signaled_within_three_seconds(process).await;
        }

        #[tokio::test]
        async fn invalid_direction_terminates_helper_without_deadlock() {
            let (server, mut client) = tokio::io::duplex(128);
            let child = long_running_child(false);
            let relay = tokio::spawn(assert_relay_failure_terminates_child(server, child));
            write_mux_frame(&mut client, MUX_STDOUT_DATA, b"invalid")
                .await
                .unwrap();
            relay.await.unwrap();
        }

        #[tokio::test]
        async fn client_disconnect_terminates_helper_without_deadlock() {
            let (server, client) = tokio::io::duplex(128);
            let child = long_running_child(false);
            drop(client);
            assert_relay_failure_terminates_child(server, child).await;
        }

        struct FailingWriteStream;
        impl AsyncRead for FailingWriteStream {
            fn poll_read(
                self: Pin<&mut Self>,
                _cx: &mut TaskContext<'_>,
                _buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Pending
            }
        }
        impl AsyncWrite for FailingWriteStream {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut TaskContext<'_>,
                _buf: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                Poll::Ready(Err(std::io::Error::other("synthetic writer failure")))
            }
            fn poll_flush(
                self: Pin<&mut Self>,
                _cx: &mut TaskContext<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: Pin<&mut Self>,
                _cx: &mut TaskContext<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        #[tokio::test]
        async fn writer_failure_terminates_helper_without_deadlock() {
            assert_relay_failure_terminates_child(FailingWriteStream, long_running_child(true))
                .await;
        }

        #[tokio::test]
        async fn output_drain_timeout_aborts_writer_and_remaining_output_tasks() {
            use std::sync::atomic::{AtomicUsize, Ordering};

            struct Dropped(Arc<AtomicUsize>);
            impl Drop for Dropped {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }

            let dropped = Arc::new(AtomicUsize::new(0));
            let spawn_pending = |dropped: Arc<AtomicUsize>| {
                tokio::spawn(async move {
                    let _guard = Dropped(dropped);
                    std::future::pending::<()>().await;
                    Ok::<_, anyhow::Error>(())
                })
            };
            let mut stdout = spawn_pending(dropped.clone());
            let mut stderr = spawn_pending(dropped.clone());
            let mut writer = spawn_pending(dropped.clone());

            assert!(
                drain_output_tasks(&mut stdout, &mut stderr, &mut writer, false, false)
                    .await
                    .is_err()
            );
            assert_eq!(dropped.load(Ordering::SeqCst), 3);
            assert!(stdout.is_finished());
            assert!(stderr.is_finished());
            assert!(writer.is_finished());
        }

        #[tokio::test]
        async fn output_drain_error_aborts_writer_and_other_output_task() {
            use std::sync::atomic::{AtomicUsize, Ordering};

            struct Dropped(Arc<AtomicUsize>);
            impl Drop for Dropped {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }

            let dropped = Arc::new(AtomicUsize::new(0));
            let mut stdout = tokio::spawn(async { anyhow::bail!("synthetic output failure") });
            let spawn_pending = |dropped: Arc<AtomicUsize>| {
                tokio::spawn(async move {
                    let _guard = Dropped(dropped);
                    std::future::pending::<()>().await;
                    Ok::<_, anyhow::Error>(())
                })
            };
            let mut stderr = spawn_pending(dropped.clone());
            let mut writer = spawn_pending(dropped.clone());

            assert!(
                drain_output_tasks(&mut stdout, &mut stderr, &mut writer, false, false)
                    .await
                    .is_err()
            );
            assert_eq!(dropped.load(Ordering::SeqCst), 2);
            assert!(stderr.is_finished());
            assert!(writer.is_finished());
        }
    }
}

#[cfg(windows)]
pub use platform::AdminComputerUseRuntime;

#[cfg(not(windows))]
pub struct AdminComputerUseRuntime;

#[cfg(not(windows))]
impl AdminComputerUseRuntime {
    pub async fn start(
        _config: AdminComputerUseConfig<'_>,
        _job: &KillOnCloseJob,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("administrator Computer Use runtime is unsupported off Windows")
    }

    pub fn health_receiver(&self) -> tokio::sync::watch::Receiver<Option<String>> {
        let (_sender, receiver) = tokio::sync::watch::channel(Some(
            "administrator Computer Use runtime is unsupported off Windows".to_owned(),
        ));
        receiver
    }
}
