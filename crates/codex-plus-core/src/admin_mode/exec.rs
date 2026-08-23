use std::path::Path;

use super::windows::KillOnCloseJob;

pub struct AdminExecConfig<'a> {
    pub codex_exe: &'a Path,
    pub readiness_probe_exe: &'a Path,
    pub pipe_name: &'a str,
    pub session_id: &'a str,
    pub session_proof: &'a str,
    pub expected_user_sid: &'a str,
    pub expected_logon_sid: &'a str,
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek};
    use std::mem::size_of;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::{Context, ensure};
    use base64::Engine;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tokio::io::{
        AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
        BufReader,
    };
    use tokio::net::windows::named_pipe::NamedPipeServer;
    use tokio::process::{Child, Command};
    use tokio::task::{JoinHandle, JoinSet};
    use windows::Win32::Foundation::{
        CloseHandle, FALSE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_NAME_NORMALIZED,
        GetFinalPathNameByHandleW, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows::Win32::System::Pipes::{
        CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::{PCWSTR, PWSTR};

    use super::{AdminExecConfig, KillOnCloseJob};
    use crate::admin_mode::windows::{
        WindowsIdentity, admin_pipe_sddl, process_has_high_integrity, process_windows_identity,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const MAX_HELLO_BYTES: usize = 64 * 1024;
    const MAX_READY_LINE_BYTES: usize = 1024 * 1024;
    const MAX_READY_STDERR_BYTES: usize = 16 * 1024;
    const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
    const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
    const MAX_CONCURRENT_CLIENTS: usize = 8;
    const HIGH_INTEGRITY_RID: u32 = 0x3000;
    const PROBE_PROCESS_ID: &str = "codex-plus-admin-readiness-probe";
    const OFFICIAL_PACKAGE_NAME: &str = "OpenAI.Codex";
    const OFFICIAL_PACKAGE_FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";
    const OFFICIAL_PUBLISHER_ID: &str = "2p2nqsd0c76g0";
    const POWERSHELL_STORE_PACKAGE_NAME: &str = "Microsoft.PowerShell";
    const POWERSHELL_STORE_PUBLISHER_ID: &str = "8wekyb3d8bbwe";
    const TERMINAL_SHELL_MODE_FILE: &str = "shell-mode.txt";
    const OFFICIAL_RUNTIME_COMPANION_NAMES: [&str; 4] = [
        "codex-code-mode-host.exe",
        "codex-command-runner.exe",
        "codex-windows-sandbox-setup.exe",
        "rg.exe",
    ];
    const FILE_ATTRIBUTE_REPARSE_POINT_VALUE: u32 = 0x400;

    type IntegrityChecker = dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync;
    type PipeFactory = dyn Fn(&str, &str, bool) -> anyhow::Result<NamedPipeServer> + Send + Sync;
    type ImageVerifier = dyn Fn(&mut Child, &Path) -> anyhow::Result<()> + Send + Sync;
    #[cfg(test)]
    type TestPipeFactory = dyn Fn(&str, &str) -> anyhow::Result<NamedPipeServer> + Send + Sync;
    type TrustedExecutableResolver =
        dyn Fn(&Path) -> anyhow::Result<VerifiedExecutableLease> + Send + Sync;
    type RuntimeCopyFactory = dyn Fn(
            &VerifiedExecutableLease,
        ) -> anyhow::Result<crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy>
        + Send
        + Sync;

    struct OwnedHandle(HANDLE);

    unsafe impl Send for OwnedHandle {}

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    struct ExecHooks {
        integrity_checker: Arc<IntegrityChecker>,
        pipe_factory: Arc<PipeFactory>,
        trusted_executable: Arc<TrustedExecutableResolver>,
        runtime_copy: Arc<RuntimeCopyFactory>,
        image_verifier: Arc<ImageVerifier>,
    }

    impl ExecHooks {
        #[cfg(test)]
        fn new(
            integrity_checker: Arc<IntegrityChecker>,
            pipe_factory: Arc<TestPipeFactory>,
        ) -> Self {
            Self {
                integrity_checker,
                pipe_factory: Arc::new(move |name, sid, first| {
                    if first {
                        pipe_factory(name, sid)
                    } else {
                        create_test_pipe(name, sid, false)
                    }
                }),
                trusted_executable: Arc::new(|path| {
                    let package_root = path
                        .parent()
                        .and_then(Path::parent)
                        .context("test executable package root missing")?;
                    let expected = package_root.join("app").join("resources");
                    if path.parent() == Some(expected.as_path()) {
                        VerifiedExecutableLease::open(path, package_root)
                    } else {
                        VerifiedExecutableLease::open_test(path, package_root)
                    }
                }),
                runtime_copy: Arc::new(|source| {
                    crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy::create_test(
                        &source._file,
                        &source._sha256,
                    )
                }),
                image_verifier: Arc::new(verify_spawned_image),
            }
        }

        fn production() -> Self {
            Self {
                integrity_checker: Arc::new(process_has_high_integrity),
                pipe_factory: Arc::new(create_restricted_pipe),
                trusted_executable: Arc::new(resolve_official_codex_lease),
                runtime_copy: Arc::new(|source| {
                    let companions = source.runtime_companions()?;
                    let companion_sources = companions
                        .iter()
                        .map(|companion| {
                            let file_name = companion
                                .canonical_path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .context("official runtime companion name is not Unicode")?;
                            Ok(crate::admin_mode::exec_runtime_copy::RuntimeCopySource {
                                file_name,
                                file: &companion._file,
                                expected_hash: &companion._sha256,
                            })
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy::create(
                        &source._file,
                        &source._sha256,
                        &companion_sources,
                    )
                }),
                image_verifier: Arc::new(verify_spawned_image),
            }
        }

        #[cfg(test)]
        fn with_pipe_factory(
            integrity_checker: Arc<IntegrityChecker>,
            pipe_factory: Arc<PipeFactory>,
        ) -> Self {
            Self {
                integrity_checker,
                pipe_factory,
                trusted_executable: Arc::new(|path| {
                    let package_root = path
                        .parent()
                        .and_then(Path::parent)
                        .context("test executable package root missing")?;
                    VerifiedExecutableLease::open_test(path, package_root)
                }),
                runtime_copy: Arc::new(|source| {
                    crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy::create_test(
                        &source._file,
                        &source._sha256,
                    )
                }),
                image_verifier: Arc::new(verify_spawned_image),
            }
        }

        #[cfg(test)]
        fn with_image_verifier(
            integrity_checker: Arc<IntegrityChecker>,
            image_verifier: Arc<ImageVerifier>,
        ) -> Self {
            let mut hooks = Self::new(
                integrity_checker,
                Arc::new(|name, sid| create_test_pipe(name, sid, true)),
            );
            hooks.image_verifier = image_verifier;
            hooks
        }
    }

    #[derive(Clone, Debug)]
    struct OfficialPackageRecord {
        name: String,
        package_full_name: String,
        package_family_name: String,
        version: String,
        signature_kind: String,
        is_development_mode: bool,
        install_location: PathBuf,
    }

    struct VerifiedExecutableLease {
        #[allow(dead_code)]
        path: PathBuf,
        #[allow(dead_code)]
        canonical_path: PathBuf,
        package_root: PathBuf,
        _file: File,
        _sha256: String,
    }

    impl VerifiedExecutableLease {
        fn open(path: &Path, package_root: &Path) -> anyhow::Result<Self> {
            let canonical_root = package_root
                .canonicalize()
                .context("canonicalize official Codex package root")?;
            let expected = canonical_root
                .join("app")
                .join("resources")
                .join("codex.exe");
            Self::open_expected(path, package_root, &expected)
        }

        fn open_expected(
            path: &Path,
            package_root: &Path,
            expected: &Path,
        ) -> anyhow::Result<Self> {
            ensure_no_reparse_components(package_root, path)?;
            let canonical_root = package_root
                .canonicalize()
                .context("canonicalize official Codex package root")?;
            let canonical_path = path
                .canonicalize()
                .context("canonicalize official Codex runtime file")?;
            ensure!(
                canonical_path.starts_with(&canonical_root),
                "official Codex runtime file escaped package install root"
            );
            ensure!(
                paths_equal_ignore_ascii_case(&canonical_path, &expected),
                "official Codex runtime file path does not match package layout"
            );
            let mut file = OpenOptions::new()
                .read(true)
                .share_mode(windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0)
                .open(&canonical_path)
                .context("lock official Codex runtime file")?;
            let final_path = final_path_for_handle(&file)?;
            ensure!(
                paths_equal_ignore_ascii_case(&final_path, &canonical_path),
                "official Codex runtime file changed during verification: handle={} expected={}",
                final_path.display(),
                canonical_path.display()
            );
            let sha256 = sha256_file(&mut file)?;
            Ok(Self {
                path: canonical_path.clone(),
                canonical_path,
                package_root: canonical_root,
                _file: file,
                _sha256: sha256,
            })
        }

        fn runtime_companions(&self) -> anyhow::Result<Vec<Self>> {
            let resources = self
                .canonical_path
                .parent()
                .context("official Codex runtime file has no resource directory")?;
            OFFICIAL_RUNTIME_COMPANION_NAMES
                .iter()
                .map(|name| {
                    let expected = resources.join(name);
                    Self::open_expected(&expected, &self.package_root, &expected)
                        .with_context(|| format!("verify official runtime companion {name}"))
                })
                .collect()
        }

        #[cfg(test)]
        fn open_test(path: &Path, package_root: &Path) -> anyhow::Result<Self> {
            let canonical_root = package_root.canonicalize()?;
            let canonical_path = path.canonicalize()?;
            ensure!(
                canonical_path.starts_with(&canonical_root),
                "test executable escaped root"
            );
            let mut file = OpenOptions::new()
                .read(true)
                .share_mode(windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0)
                .open(&canonical_path)?;
            let sha256 = sha256_file(&mut file)?;
            Ok(Self {
                path: canonical_path.clone(),
                canonical_path,
                package_root: canonical_root,
                _file: file,
                _sha256: sha256,
            })
        }
    }

    struct PendingChild {
        child: Option<Child>,
    }

    impl PendingChild {
        fn new(child: Child) -> Self {
            Self { child: Some(child) }
        }

        fn child_mut(&mut self) -> &mut Child {
            self.child.as_mut().expect("pending child must exist")
        }

        async fn terminate_and_wait(mut self) -> anyhow::Result<()> {
            terminate_and_wait(self.child_mut()).await
        }
    }

    pub struct AdminExecRuntime {
        pub pipe_name: String,
        pub session_id: String,
        pub session_proof: String,
        relay_task: Option<JoinHandle<anyhow::Result<()>>>,
        shutdown: Option<tokio::sync::watch::Sender<bool>>,
        fatal: tokio::sync::watch::Receiver<Option<String>>,
        _executable_lease: Arc<VerifiedExecutableLease>,
        runtime_copy: Option<Arc<crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy>>,
    }

    impl AdminExecRuntime {
        pub async fn start(
            config: AdminExecConfig<'_>,
            job: &KillOnCloseJob,
        ) -> anyhow::Result<Self> {
            Self::start_with_hooks(config, job, ExecHooks::production()).await
        }

        #[cfg(test)]
        async fn start_with_integrity_checker(
            config: AdminExecConfig<'_>,
            job: &KillOnCloseJob,
            integrity_checker: Arc<dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync>,
        ) -> anyhow::Result<Self> {
            Self::start_with_hooks(
                config,
                job,
                ExecHooks::new(
                    integrity_checker,
                    Arc::new(|name, sid| create_test_pipe(name, sid, true)),
                ),
            )
            .await
        }

        async fn start_with_hooks(
            config: AdminExecConfig<'_>,
            job: &KillOnCloseJob,
            hooks: ExecHooks,
        ) -> anyhow::Result<Self> {
            let executable_lease = Arc::new(
                (hooks.trusted_executable)(config.codex_exe)
                    .context("admin_exec_trust: official Store package verification failed")?,
            );
            let runtime_copy = Arc::new(
                (hooks.runtime_copy)(&executable_lease)
                    .context("admin_exec_trust: stage verified runtime copy")?,
            );

            let mut probe = PendingChild::new(spawn_official(runtime_copy.executable_path())?);
            let probe_result = async {
                own_elevated_child(
                    probe.child_mut(),
                    job.raw_handle().0 as isize,
                    config.expected_user_sid,
                    config.expected_logon_sid,
                    hooks.integrity_checker.as_ref(),
                )
                .await?;
                (hooks.image_verifier)(probe.child_mut(), runtime_copy.executable_path())
                    .context("admin_exec_readiness: probe image is not trusted")?;
                verify_probe(
                    probe.child_mut(),
                    config.expected_user_sid,
                    config.expected_logon_sid,
                    config.readiness_probe_exe,
                )
                .await
            }
            .await;
            let probe_cleanup = probe.terminate_and_wait().await;
            merge_primary_and_cleanup(probe_result, probe_cleanup)?;

            let pipe = (hooks.pipe_factory)(config.pipe_name, config.expected_user_sid, true)
                .context("admin_exec_readiness: create administrator pipe")?;
            let terminal_host = Arc::new(
                std::fs::canonicalize(config.readiness_probe_exe)
                    .context("administrator terminal host is unavailable")?,
            );
            let terminal_shell = Arc::new(
                resolve_terminal_shell(&terminal_host)
                    .context("administrator PowerShell is unavailable")?,
            );
            let session_id = config.session_id.to_owned();
            let session_proof = config.session_proof.to_owned();
            let expected_sid = config.expected_user_sid.to_owned();
            let expected_logon_sid = config.expected_logon_sid.to_owned();
            let relay_session = session_id.clone();
            let relay_proof = session_proof.clone();
            let relay_integrity_checker = Arc::clone(&hooks.integrity_checker);
            let relay_pipe_factory = Arc::clone(&hooks.pipe_factory);
            let relay_image_verifier = Arc::clone(&hooks.image_verifier);
            let relay_runtime_copy = Arc::clone(&runtime_copy);
            let relay_terminal_host = Arc::clone(&terminal_host);
            let relay_terminal_shell = Arc::clone(&terminal_shell);
            let job_handle = job.raw_handle().0 as isize;
            let pipe_name = config.pipe_name.to_owned();
            let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
            let (fatal_tx, fatal) = tokio::sync::watch::channel(None);
            let relay_task = tokio::spawn(async move {
                let result = serve_clients(
                    pipe,
                    &pipe_name,
                    &relay_session,
                    &relay_proof,
                    &expected_sid,
                    &expected_logon_sid,
                    relay_runtime_copy,
                    relay_terminal_host,
                    relay_terminal_shell,
                    job_handle,
                    relay_integrity_checker,
                    relay_image_verifier,
                    relay_pipe_factory,
                    shutdown_rx,
                )
                .await;
                if result.is_err() {
                    let _ = fatal_tx.send(Some(
                        "administrator exec broker stopped unexpectedly".to_owned(),
                    ));
                }
                Ok(())
            });

            Ok(Self {
                pipe_name: config.pipe_name.to_owned(),
                session_id,
                session_proof,
                relay_task: Some(relay_task),
                shutdown: Some(shutdown),
                fatal,
                _executable_lease: executable_lease,
                runtime_copy: Some(runtime_copy),
            })
        }

        pub async fn verify_ready(&mut self) -> anyhow::Result<()> {
            ensure!(
                self.fatal.borrow().is_none(),
                "admin_exec_readiness: administrator exec broker failed"
            );
            ensure!(
                !self
                    .relay_task
                    .as_ref()
                    .is_some_and(JoinHandle::is_finished),
                "admin_exec_readiness: administrator pipe broker exited early"
            );
            Ok(())
        }

        pub fn health_receiver(&self) -> tokio::sync::watch::Receiver<Option<String>> {
            self.fatal.clone()
        }

        pub fn official_executable_path(&self) -> &Path {
            &self._executable_lease.canonical_path
        }

        pub async fn shutdown(mut self) -> anyhow::Result<()> {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(true);
            }
            let mut relay_task = self.relay_task.take().context("exec broker task missing")?;
            let broker_result =
                match tokio::time::timeout(Duration::from_secs(3), &mut relay_task).await {
                    Err(error) => {
                        relay_task.abort();
                        let _ = relay_task.await;
                        Err(anyhow::Error::new(error)
                            .context("timed out stopping administrator exec broker"))
                    }
                    Ok(result) => match result {
                        Ok(result) => result,
                        Err(error) if error.is_cancelled() => Ok(()),
                        Err(error) => Err(error).context("administrator relay task failed"),
                    },
                };
            let runtime_copy = self
                .runtime_copy
                .take()
                .context("administrator exec runtime copy missing")?;
            let cleanup = Arc::try_unwrap(runtime_copy)
                .map_err(|_| anyhow::anyhow!("administrator exec runtime copy is still in use"))?
                .cleanup();
            merge_primary_and_cleanup(broker_result, cleanup)
        }
    }

    impl Drop for AdminExecRuntime {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(true);
            }
            if let Some(task) = self.relay_task.take() {
                task.abort();
            }
            self.runtime_copy.take();
        }
    }

    fn resolve_official_codex_lease(requested: &Path) -> anyhow::Result<VerifiedExecutableLease> {
        let record = discover_official_package()?;
        validate_package_record_structure(&record)?;
        let package_root = record
            .install_location
            .canonicalize()
            .context("canonicalize Store package install location")?;
        let expected_requested_path = record
            .install_location
            .join("app")
            .join("resources")
            .join("codex.exe");
        ensure!(
            paths_equal_ignore_ascii_case(requested, &expected_requested_path),
            "administrator exec requires the exact official package executable path"
        );
        let expected = package_root.join("app").join("resources").join("codex.exe");
        let requested_canonical = requested
            .canonicalize()
            .context("canonicalize requested administrator Codex executable")?;
        ensure!(
            paths_equal_ignore_ascii_case(&requested_canonical, &expected),
            "administrator exec requires the official OpenAI.Codex Store package"
        );
        VerifiedExecutableLease::open(&expected_requested_path, &record.install_location)
    }

    fn discover_official_package() -> anyhow::Result<OfficialPackageRecord> {
        discover_official_package_if_installed()?
            .context("official OpenAI.Codex Store package was not found")
    }

    fn discover_official_package_if_installed() -> anyhow::Result<Option<OfficialPackageRecord>> {
        let system_directory = trusted_system_directory()?;
        let powershell = system_directory
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let trusted_appx_module = system_directory
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("Modules")
            .join("Appx")
            .join("Appx.psd1");
        let module = trusted_appx_module.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$ErrorActionPreference='Stop'; Import-Module -Name '{module}' -Force -ErrorAction Stop; $p=Appx\\Get-AppxPackage -Name OpenAI.Codex | Sort-Object Version -Descending | Select-Object -First 1; if($null -eq $p){{exit 3}}; @($p.Name,$p.PackageFullName,$p.PackageFamilyName,$p.Version.ToString(),$p.SignatureKind.ToString(),$p.IsDevelopmentMode.ToString(),$p.InstallLocation) -join \"`t\""
        );
        let output = std::process::Command::new(&powershell)
            .creation_flags(CREATE_NO_WINDOW)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .output()
            .context("query official OpenAI.Codex Store package")?;
        if output.status.code() == Some(3) {
            return Ok(None);
        }
        ensure!(
            output.status.success(),
            "official OpenAI.Codex Store package query failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let stdout = String::from_utf8(output.stdout).context("package query was not UTF-8")?;
        let fields = stdout.trim().split('\t').collect::<Vec<_>>();
        ensure!(
            fields.len() == 7,
            "official package query returned invalid data"
        );
        let is_development_mode = match fields[5] {
            value if value.eq_ignore_ascii_case("true") => true,
            value if value.eq_ignore_ascii_case("false") => false,
            _ => anyhow::bail!("official package query returned invalid development mode"),
        };
        Ok(Some(OfficialPackageRecord {
            name: fields[0].to_owned(),
            package_full_name: fields[1].to_owned(),
            package_family_name: fields[2].to_owned(),
            version: fields[3].to_owned(),
            signature_kind: fields[4].to_owned(),
            is_development_mode,
            install_location: PathBuf::from(fields[6]),
        }))
    }

    fn trusted_system_directory() -> anyhow::Result<PathBuf> {
        let mut buffer = vec![0u16; 32768];
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        ensure!(
            length > 0 && length < buffer.len(),
            "read trusted Windows system directory"
        );
        buffer.truncate(length);
        Ok(PathBuf::from(String::from_utf16(&buffer)?))
    }

    fn validate_package_record_structure(record: &OfficialPackageRecord) -> anyhow::Result<()> {
        ensure!(
            record.name == OFFICIAL_PACKAGE_NAME,
            "package identity is not OpenAI.Codex"
        );
        ensure!(
            record.package_family_name == OFFICIAL_PACKAGE_FAMILY,
            "package family is not the official OpenAI.Codex publisher"
        );
        ensure!(
            record.signature_kind.eq_ignore_ascii_case("Store"),
            "OpenAI.Codex package is not Microsoft Store signed"
        );
        ensure!(
            !record.is_development_mode,
            "OpenAI.Codex package is registered in development mode"
        );
        let version_parts = record.version.split('.').collect::<Vec<_>>();
        ensure!(
            version_parts.len() == 4
                && version_parts
                    .iter()
                    .all(|part| !part.is_empty() && part.parse::<u32>().is_ok()),
            "package version is invalid"
        );
        let full_prefix = format!("{OFFICIAL_PACKAGE_NAME}_{}_", record.version);
        let full_suffix = format!("__{OFFICIAL_PUBLISHER_ID}");
        ensure!(
            record.package_full_name.starts_with(&full_prefix)
                && record.package_full_name.ends_with(&full_suffix),
            "package full name does not match identity/version/publisher"
        );
        ensure!(
            record
                .install_location
                .file_name()
                .and_then(|value| value.to_str())
                == Some(record.package_full_name.as_str()),
            "package install location does not match package full name"
        );
        ensure!(
            record
                .install_location
                .parent()
                .and_then(Path::file_name)
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("WindowsApps")),
            "package install location is not under WindowsApps"
        );
        Ok(())
    }

    fn ensure_no_reparse_components(package_root: &Path, path: &Path) -> anyhow::Result<()> {
        ensure!(
            path.starts_with(package_root),
            "executable is outside package root"
        );
        let mut current = package_root.to_path_buf();
        let root_metadata =
            std::fs::symlink_metadata(&current).context("inspect official package install root")?;
        ensure!(
            root_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT_VALUE == 0,
            "official package install root is a reparse point"
        );
        for relative in path.strip_prefix(package_root)?.components() {
            current.push(relative.as_os_str());
            let metadata = std::fs::symlink_metadata(&current).with_context(|| {
                format!("inspect trusted executable component {}", current.display())
            })?;
            ensure!(
                metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT_VALUE == 0,
                "official executable path contains a reparse point"
            );
        }
        Ok(())
    }

    fn final_path_for_handle(file: &File) -> anyhow::Result<PathBuf> {
        let handle = HANDLE(file.as_raw_handle());
        let mut buffer = vec![0u16; 32768];
        let length = unsafe { GetFinalPathNameByHandleW(handle, &mut buffer, FILE_NAME_NORMALIZED) }
            as usize;
        ensure!(
            length > 0 && length < buffer.len(),
            "read locked executable final path"
        );
        buffer.truncate(length);
        Ok(normalize_extended_path(PathBuf::from(String::from_utf16(
            &buffer,
        )?)))
    }

    fn normalize_extended_path(path: PathBuf) -> PathBuf {
        let value = path.to_string_lossy();
        value
            .strip_prefix(r"\\?\")
            .map(PathBuf::from)
            .unwrap_or(path)
    }

    fn paths_equal_ignore_ascii_case(left: &Path, right: &Path) -> bool {
        let left = left.to_string_lossy();
        let right = right.to_string_lossy();
        left.strip_prefix(r"\\?\")
            .unwrap_or(&left)
            .eq_ignore_ascii_case(right.strip_prefix(r"\\?\").unwrap_or(&right))
    }

    fn sha256_file(file: &mut File) -> anyhow::Result<String> {
        use sha2::{Digest, Sha256};
        file.rewind().context("rewind official executable")?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).context("hash official executable")?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        file.rewind()
            .context("rewind official executable after hash")?;
        Ok(format!("{:x}", digest.finalize()))
    }

    fn verify_spawned_image(child: &mut Child, expected_path: &Path) -> anyhow::Result<()> {
        let handle = HANDLE(
            child
                .raw_handle()
                .context("exec-server has no process handle")?,
        );
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
            .context("inspect spawned exec-server image")?;
        }
        buffer.truncate(length as usize);
        let image = PathBuf::from(String::from_utf16(&buffer)?);
        ensure!(
            paths_equal_ignore_ascii_case(&image, expected_path),
            "spawned exec-server image does not match locked official executable"
        );
        Ok(())
    }

    fn process_image_path(process_id: u32) -> anyhow::Result<PathBuf> {
        let process = OwnedHandle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, process_id)
                .context("open administrator terminal process")?
        });
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
            .context("inspect administrator terminal process image")?;
        }
        buffer.truncate(length as usize);
        Ok(PathBuf::from(String::from_utf16(&buffer)?))
    }

    fn verify_terminal_client_image(client_pid: u32, terminal_host: &Path) -> anyhow::Result<()> {
        let install_dir = terminal_host
            .parent()
            .context("administrator terminal host install directory is unavailable")?;
        let expected = install_dir.join("admin-terminal").join("pwsh.exe");
        let image = process_image_path(client_pid)?;
        ensure!(
            paths_equal_ignore_ascii_case(&image, &expected),
            "administrator terminal client image is not trusted"
        );
        Ok(())
    }

    fn spawn_official(executable_path: &Path) -> anyhow::Result<Child> {
        spawn_official_with_args(
            executable_path,
            &[
                "exec-server".to_owned(),
                "--listen".to_owned(),
                "stdio".to_owned(),
            ],
        )
        .context("failed to start official Codex exec-server")
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TerminalShellPreference {
        PowerShell7,
        WindowsPowerShell,
    }

    fn parse_terminal_shell_preference(value: &str) -> Option<TerminalShellPreference> {
        match value.trim() {
            "powershell7" => Some(TerminalShellPreference::PowerShell7),
            "windows-powershell" => Some(TerminalShellPreference::WindowsPowerShell),
            _ => None,
        }
    }

    fn configured_terminal_shell_preference(
        terminal_host: &Path,
    ) -> anyhow::Result<Option<TerminalShellPreference>> {
        let install_dir = terminal_host
            .parent()
            .context("administrator terminal host install directory is unavailable")?;
        let preference_path = install_dir
            .join("admin-terminal")
            .join(TERMINAL_SHELL_MODE_FILE);
        let value = match std::fs::read_to_string(&preference_path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "read administrator terminal shell preference {}",
                        preference_path.display()
                    )
                });
            }
        };
        parse_terminal_shell_preference(&value)
            .map(Some)
            .context("administrator terminal shell preference is invalid")
    }

    fn select_terminal_shell(
        preference: Option<TerminalShellPreference>,
        powershell7_candidates: impl IntoIterator<Item = PathBuf>,
        windows_powershell: PathBuf,
    ) -> Option<PathBuf> {
        match preference {
            Some(TerminalShellPreference::PowerShell7) => {
                first_existing_terminal_shell(powershell7_candidates)
            }
            Some(TerminalShellPreference::WindowsPowerShell) => {
                first_existing_terminal_shell([windows_powershell])
            }
            None => first_existing_terminal_shell(
                powershell7_candidates
                    .into_iter()
                    .chain(std::iter::once(windows_powershell)),
            ),
        }
    }

    fn resolve_terminal_shell(terminal_host: &Path) -> anyhow::Result<PathBuf> {
        let preference = configured_terminal_shell_preference(terminal_host)?;
        let windows_powershell = trusted_system_directory()?
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let powershell7_candidates = match preference {
            Some(TerminalShellPreference::WindowsPowerShell) => Vec::new(),
            _ => system_powershell7_candidates(),
        };
        select_terminal_shell(preference, powershell7_candidates, windows_powershell).with_context(
            || match preference {
                Some(TerminalShellPreference::PowerShell7) => {
                    "PowerShell 7 was selected during installation but is unavailable"
                }
                Some(TerminalShellPreference::WindowsPowerShell) => {
                    "Windows PowerShell 5.1 was selected during installation but is unavailable"
                }
                None => "PowerShell 7 and Windows PowerShell 5.1 were not found",
            },
        )
    }

    fn system_powershell7_candidates() -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(Some(store)) = discover_store_powershell7_path() {
            candidates.push(store);
        }
        for variable in ["ProgramFiles", "ProgramW6432"] {
            if let Some(root) = std::env::var_os(variable) {
                let root = PathBuf::from(root);
                candidates.push(root.join("PowerShell").join("7").join("pwsh.exe"));
                candidates.extend(store_powershell7_candidates(&root.join("WindowsApps")));
            }
        }
        if let Some(root) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(root)
                    .join("Programs")
                    .join("PowerShell")
                    .join("7")
                    .join("pwsh.exe"),
            );
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates
                .extend(std::env::split_paths(&path).map(|directory| directory.join("pwsh.exe")));
        }
        candidates
    }

    fn discover_store_powershell7_path() -> anyhow::Result<Option<PathBuf>> {
        let system_directory = trusted_system_directory()?;
        let powershell = system_directory
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let trusted_appx_module = system_directory
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("Modules")
            .join("Appx")
            .join("Appx.psd1");
        let module = trusted_appx_module.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$ErrorActionPreference='Stop'; Import-Module -Name '{module}' -Force -ErrorAction Stop; $p=Appx\\Get-AppxPackage -Name {POWERSHELL_STORE_PACKAGE_NAME} | Where-Object {{ $_.Architecture.ToString() -match 'X64|X86|Arm64' }} | Sort-Object Version -Descending | Select-Object -First 1; if($null -eq $p){{exit 3}}; $root=[IO.Path]::GetFullPath([string]$p.InstallLocation); @($p.Name,$p.PackageFullName,$p.Version.ToString(),$p.Architecture.ToString(),$root) -join \"`t\""
        );
        let output = std::process::Command::new(&powershell)
            .creation_flags(CREATE_NO_WINDOW)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .output()
            .context("query Microsoft.PowerShell Store package")?;
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Microsoft.PowerShell Store query was not UTF-8")?;
        let fields = stdout.trim().split('\t').collect::<Vec<_>>();
        ensure!(
            fields.len() == 5,
            "Microsoft.PowerShell Store query returned invalid data"
        );
        validate_store_powershell7_record(
            fields[0],
            fields[1],
            fields[2],
            fields[3],
            Path::new(fields[4]),
        )
        .map(Some)
    }

    fn validate_store_powershell7_record(
        name: &str,
        package_full_name: &str,
        version: &str,
        architecture: &str,
        install_location: &Path,
    ) -> anyhow::Result<PathBuf> {
        ensure!(
            name == POWERSHELL_STORE_PACKAGE_NAME,
            "PowerShell Store package identity is invalid"
        );
        let version_parts = version.split('.').collect::<Vec<_>>();
        ensure!(
            version_parts.len() == 4
                && version_parts
                    .iter()
                    .all(|part| !part.is_empty() && part.parse::<u32>().is_ok())
                && version_parts[0] == "7",
            "PowerShell Store package version is not PowerShell 7"
        );
        let architecture = match architecture.to_ascii_lowercase().as_str() {
            "x64" => "x64",
            "x86" => "x86",
            "arm64" => "arm64",
            _ => anyhow::bail!("PowerShell Store package architecture is invalid"),
        };
        let expected_full_name = format!(
            "{POWERSHELL_STORE_PACKAGE_NAME}_{version}_{architecture}__{POWERSHELL_STORE_PUBLISHER_ID}"
        );
        ensure!(
            package_full_name == expected_full_name,
            "PowerShell Store package full name is invalid"
        );
        ensure!(
            install_location.is_absolute(),
            "PowerShell Store install location is not absolute"
        );
        ensure!(
            install_location
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value == package_full_name),
            "PowerShell Store install location does not match package full name"
        );
        ensure!(
            install_location
                .parent()
                .and_then(Path::file_name)
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("WindowsApps")),
            "PowerShell Store install location is not under WindowsApps"
        );
        let executable = install_location.join("pwsh.exe");
        ensure!(
            executable.is_file(),
            "PowerShell Store package does not contain pwsh.exe"
        );
        ensure_no_reparse_components(install_location, &executable)?;
        Ok(executable)
    }

    fn first_existing_terminal_shell(
        candidates: impl IntoIterator<Item = PathBuf>,
    ) -> Option<PathBuf> {
        candidates.into_iter().find(|candidate| {
            candidate.is_file()
                && match candidate.file_name().and_then(|name| name.to_str()) {
                    Some(name) if name.eq_ignore_ascii_case("pwsh.exe") => {
                        !is_windows_app_execution_alias(candidate)
                    }
                    Some(name) => name.eq_ignore_ascii_case("powershell.exe"),
                    None => false,
                }
        })
    }

    fn store_powershell7_candidates(windows_apps: &Path) -> Vec<PathBuf> {
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "x86",
            other => other,
        };
        let architecture_marker = format!("_{architecture}__8wekyb3d8bbwe");
        let mut packages = std::fs::read_dir(windows_apps)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("Microsoft.PowerShell_")
                    || !name.ends_with(&architecture_marker)
                {
                    return None;
                }
                let version = name
                    .strip_prefix("Microsoft.PowerShell_")?
                    .split('_')
                    .next()?
                    .split('.')
                    .map(str::parse::<u64>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                Some((version, entry.path().join("pwsh.exe")))
            })
            .filter(|(_, executable)| executable.is_file())
            .collect::<Vec<_>>();
        packages.sort_by(|left, right| right.0.cmp(&left.0));
        packages
            .into_iter()
            .map(|(_, executable)| executable)
            .collect()
    }

    fn is_windows_app_execution_alias(path: &Path) -> bool {
        let normalized = path.to_string_lossy().replace('/', "\\");
        normalized
            .to_ascii_lowercase()
            .contains(r"\microsoft\windowsapps\")
    }

    fn spawn_official_with_args(executable_path: &Path, args: &[String]) -> anyhow::Result<Child> {
        let mut command = Command::new(executable_path);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        command
            .spawn()
            .context("failed to start official Codex CLI")
    }

    fn terminal_host_args(
        client_pid: u32,
        cwd: &Path,
        shell: &Path,
        shell_args: &[String],
    ) -> Vec<std::ffi::OsString> {
        let mut args = vec![
            "terminal-host".into(),
            "--client-pid".into(),
            client_pid.to_string().into(),
            "--cwd".into(),
            cwd.as_os_str().to_owned(),
            "--shell".into(),
            shell.as_os_str().to_owned(),
        ];
        if !shell_args.is_empty() {
            args.push("--".into());
            args.extend(shell_args.iter().map(Into::into));
        }
        args
    }

    fn spawn_terminal_host(
        terminal_host: &Path,
        client_pid: u32,
        cwd: &Path,
        shell: &Path,
        shell_args: &[String],
    ) -> anyhow::Result<Child> {
        let mut command = Command::new(terminal_host);
        command
            .args(terminal_host_args(client_pid, cwd, shell, shell_args))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW);
        command
            .spawn()
            .context("failed to start administrator terminal host")
    }

    async fn own_elevated_child(
        child: &mut Child,
        job_handle: isize,
        expected_user_sid: &str,
        expected_logon_sid: &str,
        integrity_checker: &(dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync),
    ) -> anyhow::Result<()> {
        let pid = child.id().context("exec-server has no process id")?;
        let raw_handle = child
            .raw_handle()
            .context("exec-server has no process handle")?;
        unsafe {
            AssignProcessToJobObject(
                HANDLE(job_handle as *mut std::ffi::c_void),
                HANDLE(raw_handle),
            )
            .context("failed to assign exec-server to administrator job")?;
        }
        if !integrity_checker(pid).context("failed to inspect exec-server integrity")? {
            anyhow::bail!("admin_exec_readiness: exec-server is not high integrity");
        }
        ensure!(
            trusted_client_identity(
                process_windows_identity(pid),
                expected_user_sid,
                expected_logon_sid,
            ),
            "admin_exec_readiness: exec-server identity is not trusted"
        );
        Ok(())
    }

    async fn terminate_and_wait(child: &mut Child) -> anyhow::Result<()> {
        if child.try_wait().ok().flatten().is_none() {
            child
                .start_kill()
                .context("failed to terminate exec-server")?;
        }
        child
            .wait()
            .await
            .context("failed to wait for exec-server")?;
        Ok(())
    }

    fn merge_primary_and_cleanup<T>(
        primary: anyhow::Result<T>,
        cleanup: anyhow::Result<()>,
    ) -> anyhow::Result<T> {
        match (primary, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("cleanup also failed: {cleanup:#}")))
            }
        }
    }

    fn drain_stderr(stderr: Option<tokio::process::ChildStderr>) {
        if let Some(mut stderr) = stderr {
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                while stderr.read(&mut buffer).await.unwrap_or(0) != 0 {}
            });
        }
    }

    async fn verify_probe(
        child: &mut Child,
        expected_sid: &str,
        expected_logon_sid: &str,
        readiness_probe_exe: &Path,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(
            READINESS_TIMEOUT,
            verify_probe_inner(child, expected_sid, expected_logon_sid, readiness_probe_exe),
        )
        .await
        .context("admin_exec_readiness: probe timed out")?
        .context("admin_exec_readiness: official protocol probe failed")
    }

    async fn verify_probe_inner(
        child: &mut Child,
        expected_sid: &str,
        expected_logon_sid: &str,
        readiness_probe_exe: &Path,
    ) -> anyhow::Result<()> {
        let mut stdin = child.stdin.take().context("probe stdin missing")?;
        let stdout = child.stdout.take().context("probe stdout missing")?;
        drain_stderr(child.stderr.take());
        let mut stdout = BufReader::new(stdout);

        write_json_line(&mut stdin, &json!({"id":1,"method":"initialize","params":{"clientName":"codex-plus-admin-readiness"}})).await?;
        let response = read_json_line(&mut stdout).await?;
        ensure!(
            response["id"] == 1
                && response.get("result").is_some()
                && response.get("error").is_none(),
            "initialize was rejected"
        );
        write_json_line(&mut stdin, &json!({"method":"initialized","params":{}})).await?;

        write_json_line(&mut stdin, &readiness_process_request(readiness_probe_exe)?).await?;

        let mut state = ProbeResponseState::default();
        while !state.exited {
            state.consume(read_json_line(&mut stdout).await?)?;
        }
        state.finish(expected_sid, expected_logon_sid)
    }

    fn readiness_process_request(readiness_probe_exe: &Path) -> anyhow::Result<Value> {
        let cwd = std::env::current_dir().context("read readiness cwd")?;
        let cwd_uri = format!("file:///{}", cwd.to_string_lossy().replace('\\', "/"));
        ensure!(
            readiness_probe_exe.is_absolute(),
            "administrator identity probe path is not absolute"
        );
        Ok(json!({
            "id":2,
            "method":"process/start",
            "params":{
                "processId":PROBE_PROCESS_ID,
                "argv":[readiness_probe_exe,"identity-probe"],
                "cwd":cwd_uri,
                "env":{},
                "tty":false,
                "pipeStdin":false,
                "arg0":null
            }
        }))
    }

    #[derive(Default)]
    struct ProbeResponseState {
        start_accepted: bool,
        output: Vec<u8>,
        stderr: Vec<u8>,
        exited: bool,
    }

    impl ProbeResponseState {
        fn consume(&mut self, message: Value) -> anyhow::Result<()> {
            if message["id"] == 2 {
                ensure!(!self.start_accepted, "duplicate process/start response");
                ensure!(message.get("error").is_none(), "process/start was rejected");
                ensure!(
                    message["result"]["processId"] == PROBE_PROCESS_ID,
                    "process/start returned the wrong process id"
                );
                self.start_accepted = true;
                return Ok(());
            }

            ensure!(
                self.start_accepted,
                "readiness notification arrived before process/start"
            );
            let method = message["method"]
                .as_str()
                .context("unexpected readiness message")?;
            let params = &message["params"];
            ensure!(
                params["processId"] == PROBE_PROCESS_ID,
                "readiness notification used the wrong process id"
            );
            match method {
                "process/output" => match params["stream"].as_str() {
                    Some("stdout") => {
                        let chunk = params["chunk"]
                            .as_str()
                            .context("missing readiness output chunk")?;
                        self.output.extend(
                            base64::engine::general_purpose::STANDARD
                                .decode(chunk)
                                .context("invalid readiness output")?,
                        );
                    }
                    Some("stderr") => {
                        let chunk = params["chunk"]
                            .as_str()
                            .context("missing readiness stderr chunk")?;
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(chunk)
                            .context("invalid readiness stderr")?;
                        let remaining = MAX_READY_STDERR_BYTES.saturating_sub(self.stderr.len());
                        self.stderr.extend(&decoded[..decoded.len().min(remaining)]);
                    }
                    _ => anyhow::bail!("unexpected readiness output stream"),
                },
                "process/exited" => {
                    ensure!(!self.exited, "duplicate readiness process exit");
                    if params["exitCode"] != 0 {
                        anyhow::bail!(
                            "readiness process failed: {}",
                            decode_readiness_text(&self.stderr)
                        );
                    }
                    ensure!(
                        params["sandboxDenied"] == false,
                        "readiness process was sandbox denied"
                    );
                    self.exited = true;
                }
                _ => anyhow::bail!("unexpected readiness notification"),
            }
            Ok(())
        }

        fn finish(self, expected_sid: &str, expected_logon_sid: &str) -> anyhow::Result<()> {
            ensure!(
                self.start_accepted && self.exited,
                "readiness process did not complete"
            );
            let output = decode_readiness_text(&self.output);
            let expected = format!("SID={expected_sid};LOGON={expected_logon_sid};RID=");
            let rid = output
                .split(&expected)
                .nth(1)
                .and_then(|value| value.trim().lines().next())
                .and_then(|value| value.parse::<u32>().ok())
                .context("readiness output did not contain expected SID/integrity")?;
            ensure!(
                rid >= HIGH_INTEGRITY_RID,
                "readiness process is not high integrity"
            );
            Ok(())
        }
    }

    fn decode_readiness_text(bytes: &[u8]) -> String {
        if bytes.len() >= 2
            && bytes.len().is_multiple_of(2)
            && bytes.chunks_exact(2).take(32).any(|pair| pair[1] == 0)
        {
            let words = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            return String::from_utf16_lossy(&words).trim().to_owned();
        }
        String::from_utf8_lossy(bytes).trim().to_string()
    }

    async fn write_json_line(
        writer: &mut (impl AsyncWrite + Unpin),
        value: &Value,
    ) -> anyhow::Result<()> {
        let mut bytes = serde_json::to_vec(value).context("serialize exec-server request")?;
        bytes.push(b'\n');
        writer
            .write_all(&bytes)
            .await
            .context("write exec-server request")?;
        writer.flush().await.context("flush exec-server request")
    }

    async fn read_json_line(reader: &mut (impl AsyncBufRead + Unpin)) -> anyhow::Result<Value> {
        let mut bytes = Vec::new();
        (&mut *reader)
            .take(MAX_READY_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .await
            .context("read exec-server response")?;
        ensure!(!bytes.is_empty(), "exec-server closed during readiness");
        ensure!(
            bytes.len() <= MAX_READY_LINE_BYTES,
            "exec-server readiness message is too large"
        );
        serde_json::from_slice(&bytes).context("invalid exec-server readiness JSON")
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Hello {
        protocol: u8,
        session_id: String,
        mode: String,
        client_pid: u32,
        proof: String,
        #[serde(default)]
        helper_args: Option<Vec<String>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ClientRequest {
        Exec,
        AppServer(Vec<String>),
        Terminal {
            client_pid: u32,
            cwd: PathBuf,
            shell_args: Vec<String>,
        },
    }

    async fn serve_clients(
        mut pipe: NamedPipeServer,
        pipe_name: &str,
        session_id: &str,
        proof: &str,
        expected_sid: &str,
        expected_logon_sid: &str,
        runtime_copy: Arc<crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy>,
        terminal_host: Arc<PathBuf>,
        terminal_shell: Arc<PathBuf>,
        job_handle: isize,
        integrity_checker: Arc<dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync>,
        image_verifier: Arc<ImageVerifier>,
        pipe_factory: Arc<PipeFactory>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let session_id: Arc<str> = Arc::from(session_id);
        let proof: Arc<str> = Arc::from(proof);
        let expected_sid: Arc<str> = Arc::from(expected_sid);
        let expected_logon_sid: Arc<str> = Arc::from(expected_logon_sid);
        let mut clients: JoinSet<anyhow::Result<ClientOutcome>> = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    changed.context("administrator exec shutdown channel closed")?;
                    clients.abort_all();
                    while clients.join_next().await.is_some() {}
                    return Ok(());
                },
                completed = clients.join_next(), if !clients.is_empty() => {
                    let joined = completed
                        .context("administrator exec client task disappeared")?;
                    let outcome = joined
                        .context("administrator exec client task panicked")?
                        .context("administrator exec client failed")?;
                    match outcome {
                        ClientOutcome::Disconnected | ClientOutcome::Rejected | ClientOutcome::Shutdown => {}
                    }
                },
                accepted = pipe.connect(), if clients.len() < MAX_CONCURRENT_CLIENTS => {
                    accepted.context("admin_exec_accept: failed to accept administrator exec client")?;
                    let next_pipe = pipe_factory(pipe_name, &expected_sid, false)
                        .context("admin_exec_accept: failed to recreate administrator pipe")?;
                    let mut connected_pipe = std::mem::replace(&mut pipe, next_pipe);
                    let client_session = Arc::clone(&session_id);
                    let client_proof = Arc::clone(&proof);
                    let client_sid = Arc::clone(&expected_sid);
                    let client_logon_sid = Arc::clone(&expected_logon_sid);
                    let client_runtime_copy = Arc::clone(&runtime_copy);
                    let client_terminal_host = Arc::clone(&terminal_host);
                    let client_terminal_shell = Arc::clone(&terminal_shell);
                    let client_integrity_checker = Arc::clone(&integrity_checker);
                    let client_image_verifier = Arc::clone(&image_verifier);
                    let client_shutdown = shutdown.clone();
                    clients.spawn(async move {
                        admit_and_relay_client(
                            &mut connected_pipe,
                            &client_session,
                            &client_proof,
                            &client_sid,
                            &client_logon_sid,
                            &client_runtime_copy,
                            &client_terminal_host,
                            &client_terminal_shell,
                            job_handle,
                            client_integrity_checker.as_ref(),
                            client_image_verifier.as_ref(),
                            client_shutdown,
                        )
                        .await
                    });
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ClientOutcome {
        Rejected,
        Disconnected,
        Shutdown,
    }

    async fn admit_and_relay_client(
        pipe: &mut NamedPipeServer,
        session_id: &str,
        proof: &str,
        expected_sid: &str,
        expected_logon_sid: &str,
        runtime_copy: &crate::admin_mode::exec_runtime_copy::AdminExecRuntimeCopy,
        terminal_host: &Path,
        terminal_shell: &Path,
        job_handle: isize,
        integrity_checker: &(dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync),
        image_verifier: &ImageVerifier,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<ClientOutcome> {
        let request = match tokio::time::timeout(
            AUTH_TIMEOUT,
            authenticate(pipe, session_id, proof, expected_sid, expected_logon_sid),
        )
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(_)) | Err(_) => {
                reject_client(pipe, "authentication-rejected").await;
                return Ok(ClientOutcome::Rejected);
            }
        };
        let Some(request) = request else {
            reject_client(pipe, "authentication-rejected").await;
            return Ok(ClientOutcome::Rejected);
        };

        if let ClientRequest::Terminal {
            client_pid,
            cwd,
            shell_args,
        } = &request
        {
            return admit_terminal_client(
                pipe,
                *client_pid,
                cwd,
                terminal_host,
                terminal_shell,
                shell_args,
                job_handle,
                expected_sid,
                expected_logon_sid,
                integrity_checker,
                shutdown,
            )
            .await;
        }

        let mut child = PendingChild::new(match &request {
            ClientRequest::Exec => spawn_official(runtime_copy.executable_path())?,
            ClientRequest::AppServer(args) => {
                spawn_official_with_args(runtime_copy.executable_path(), args)?
            }
            ClientRequest::Terminal { .. } => unreachable!("terminal request handled above"),
        });
        let setup = async {
            own_elevated_child(
                child.child_mut(),
                job_handle,
                expected_sid,
                expected_logon_sid,
                integrity_checker,
            )
            .await?;
            image_verifier(child.child_mut(), runtime_copy.executable_path())?;
            let stdin = child
                .child_mut()
                .stdin
                .take()
                .context("production exec-server stdin missing")?;
            let stdout = child
                .child_mut()
                .stdout
                .take()
                .context("production exec-server stdout missing")?;
            Ok::<_, anyhow::Error>((stdin, stdout))
        }
        .await;
        let (mut child_stdin, mut child_stdout) = match setup {
            Ok(io) => io,
            Err(error) => {
                reject_client(pipe, "production-start-rejected").await;
                let cleanup = child.terminate_and_wait().await;
                return merge_primary_and_cleanup(Err(error), cleanup);
            }
        };
        drain_stderr(child.child_mut().stderr.take());
        if write_frame(pipe, &serde_json::to_vec(&json!({"accepted":true}))?)
            .await
            .is_err()
        {
            child.terminate_and_wait().await?;
            return Ok(ClientOutcome::Disconnected);
        }

        let (mut pipe_reader, mut pipe_writer) = tokio::io::split(&mut *pipe);
        let client_to_child = async {
            tokio::io::copy(&mut pipe_reader, &mut child_stdin)
                .await
                .context("relay client to exec-server")?;
            Ok::<(), anyhow::Error>(())
        };
        let child_to_client = async {
            tokio::io::copy(&mut child_stdout, &mut pipe_writer)
                .await
                .context("relay exec-server to client")?;
            Ok::<(), anyhow::Error>(())
        };
        tokio::pin!(client_to_child);
        tokio::pin!(child_to_client);
        let outcome = tokio::select! {
            changed = shutdown.changed() => {
                changed.context("administrator exec shutdown channel closed")?;
                ClientOutcome::Shutdown
            }
            result = &mut client_to_child => {
                result?;
                ClientOutcome::Disconnected
            }
            result = &mut child_to_client => {
                result?;
                match &request {
                    ClientRequest::Exec => anyhow::bail!("admin_exec_runtime: active exec-server output closed unexpectedly"),
                    ClientRequest::AppServer(_) => ClientOutcome::Disconnected,
                    ClientRequest::Terminal { .. } => unreachable!(),
                }
            }
            result = child.child_mut().wait() => {
                result.context("admin_exec_runtime: wait for active exec-server")?;
                match &request {
                    ClientRequest::Exec => anyhow::bail!("admin_exec_runtime: active exec-server exited unexpectedly"),
                    ClientRequest::AppServer(_) => ClientOutcome::Disconnected,
                    ClientRequest::Terminal { .. } => unreachable!(),
                }
            }
        };
        child.terminate_and_wait().await?;
        Ok(outcome)
    }

    async fn admit_terminal_client(
        pipe: &mut NamedPipeServer,
        client_pid: u32,
        cwd: &Path,
        terminal_host: &Path,
        terminal_shell: &Path,
        shell_args: &[String],
        job_handle: isize,
        expected_sid: &str,
        expected_logon_sid: &str,
        integrity_checker: &(dyn Fn(u32) -> anyhow::Result<bool> + Send + Sync),
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<ClientOutcome> {
        if !cwd.is_dir() || verify_terminal_client_image(client_pid, terminal_host).is_err() {
            reject_client(pipe, "terminal-start-rejected").await;
            return Ok(ClientOutcome::Rejected);
        }
        let mut terminal =
            match spawn_terminal_host(terminal_host, client_pid, cwd, terminal_shell, shell_args) {
                Ok(child) => PendingChild::new(child),
                Err(_) => {
                    reject_client(pipe, "terminal-start-rejected").await;
                    return Ok(ClientOutcome::Rejected);
                }
            };
        let setup = async {
            own_elevated_child(
                terminal.child_mut(),
                job_handle,
                expected_sid,
                expected_logon_sid,
                integrity_checker,
            )
            .await?;
            verify_spawned_image(terminal.child_mut(), terminal_host)
                .context("administrator terminal host image is not trusted")
        }
        .await;
        if let Err(error) = setup {
            reject_client(pipe, "terminal-start-rejected").await;
            let cleanup = terminal.terminate_and_wait().await;
            return merge_primary_and_cleanup(Err(error), cleanup);
        }
        let process_id = terminal
            .child_mut()
            .id()
            .context("administrator terminal host has no process id")?;
        if write_frame(
            pipe,
            &serde_json::to_vec(&json!({
                "accepted": true,
                "processId": process_id,
            }))?,
        )
        .await
        .is_err()
        {
            terminal.terminate_and_wait().await?;
            return Ok(ClientOutcome::Disconnected);
        }

        enum TerminalOutcome {
            Exited(i32),
            Shutdown,
            Disconnected,
        }
        let outcome = tokio::select! {
            changed = shutdown.changed() => {
                changed.context("administrator exec shutdown channel closed")?;
                TerminalOutcome::Shutdown
            }
            _ = read_frame(pipe, MAX_HELLO_BYTES) => {
                TerminalOutcome::Disconnected
            }
            status = terminal.child_mut().wait() => {
                let status = status.context("failed to wait for administrator terminal host")?;
                TerminalOutcome::Exited(status.code().unwrap_or(1))
            }
        };
        match outcome {
            TerminalOutcome::Exited(exit_code) => {
                let _ =
                    write_frame(pipe, &serde_json::to_vec(&json!({"exitCode": exit_code}))?).await;
                Ok(ClientOutcome::Disconnected)
            }
            TerminalOutcome::Shutdown => {
                terminal.terminate_and_wait().await?;
                Ok(ClientOutcome::Shutdown)
            }
            TerminalOutcome::Disconnected => {
                terminal.terminate_and_wait().await?;
                Ok(ClientOutcome::Disconnected)
            }
        }
    }

    async fn reject_client(pipe: &mut NamedPipeServer, reason: &'static str) {
        let payload = serde_json::to_vec(&json!({"accepted":false,"reason":reason}))
            .expect("fixed rejection payload must serialize");
        let _ = write_frame(pipe, &payload).await;
        let _ = pipe.shutdown().await;
    }

    async fn authenticate(
        pipe: &mut NamedPipeServer,
        session_id: &str,
        proof: &str,
        expected_sid: &str,
        expected_logon_sid: &str,
    ) -> anyhow::Result<Option<ClientRequest>> {
        let mut payload = read_frame(pipe, MAX_HELLO_BYTES).await?;
        let mut hello: Hello =
            serde_json::from_slice(&payload).context("invalid administrator hello")?;
        let pipe_pid = named_pipe_client_pid(pipe)?;
        let identity = process_windows_identity(pipe_pid);
        let proof_matches = constant_time_proof_eq(hello.proof.as_bytes(), proof.as_bytes());
        unsafe {
            hello.proof.as_bytes_mut().fill(0);
        }
        payload.fill(0);
        let valid_identity = hello.protocol == 1
            && hello.session_id == session_id
            && proof_matches
            && hello.client_pid == pipe_pid
            && trusted_client_identity(identity, expected_sid, expected_logon_sid);
        if !valid_identity {
            return Ok(None);
        }
        match hello.mode.as_str() {
            "exec" if hello.helper_args.is_none() => Ok(Some(ClientRequest::Exec)),
            "app-server" => {
                let args = hello.helper_args.unwrap_or_default();
                if args.is_empty() || !args.iter().any(|arg| arg == "app-server") {
                    return Ok(None);
                }
                Ok(Some(ClientRequest::AppServer(args)))
            }
            "terminal" => {
                let args = hello.helper_args.unwrap_or_default();
                if args.is_empty() || args[0].is_empty() {
                    return Ok(None);
                }
                Ok(Some(ClientRequest::Terminal {
                    client_pid: hello.client_pid,
                    cwd: PathBuf::from(&args[0]),
                    shell_args: args[1..].to_vec(),
                }))
            }
            _ => Ok(None),
        }
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

    fn constant_time_proof_eq(provided: &[u8], expected: &[u8]) -> bool {
        use sha2::{Digest, Sha256};

        let mut provided_digest = Sha256::digest(provided);
        let mut expected_digest = Sha256::digest(expected);
        let mut difference = 0u8;
        for index in 0..32 {
            difference |= provided_digest[index] ^ expected_digest[index];
        }
        provided_digest.fill(0);
        expected_digest.fill(0);
        difference == 0
    }

    async fn read_frame(
        reader: &mut (impl AsyncRead + Unpin),
        maximum: usize,
    ) -> anyhow::Result<Vec<u8>> {
        let length = reader.read_u32_le().await.context("read hello length")? as usize;
        ensure!(length <= maximum, "administrator hello is too large");
        let mut payload = vec![0; length];
        reader
            .read_exact(&mut payload)
            .await
            .context("read hello payload")?;
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

    fn create_restricted_pipe(
        pipe_name: &str,
        user_sid: &str,
        first_instance: bool,
    ) -> anyhow::Result<NamedPipeServer> {
        let sddl = exec_pipe_sddl(user_sid)?;
        create_pipe_with_sddl(pipe_name, &sddl, first_instance)
    }

    #[cfg(test)]
    fn create_test_pipe(
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
            )
            .context("failed to build administrator pipe security descriptor")?;
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
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: FALSE,
        };
        let wide_name = pipe_name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let open_mode = if first_instance {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED
        };
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide_name.as_ptr()),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                Some(&attributes),
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "failed to create restricted administrator pipe"
        );
        unsafe {
            NamedPipeServer::from_raw_handle(handle.0 as _)
                .context("register administrator pipe with Tokio")
        }
    }

    fn exec_pipe_sddl(user_sid: &str) -> anyhow::Result<String> {
        // Reuse the central strict SID parser, but do not grant the client
        // FILE_CREATE_PIPE_INSTANCE (0x4). SYSTEM/Administrators retain full
        // control so only the elevated broker can create subsequent instances.
        admin_pipe_sddl(user_sid)?;
        const CLIENT_READ_WRITE_WITHOUT_CREATE_INSTANCE: u32 = 0x0012_019b;
        Ok(format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x{CLIENT_READ_WRITE_WITHOUT_CREATE_INSTANCE:08X};;;{user_sid})"
        ))
    }

    fn named_pipe_client_pid(pipe: &NamedPipeServer) -> anyhow::Result<u32> {
        let mut pid = 0;
        unsafe {
            GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()), &mut pid)
                .context("read administrator client PID")?;
        }
        Ok(pid)
    }

    #[cfg(test)]
    mod tests {
        use std::path::{Path, PathBuf};
        use std::process::Command as StdCommand;
        use std::sync::OnceLock;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

        use super::*;
        use crate::admin_mode::windows::{admin_pipe_name, current_windows_identity};

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

        #[test]
        fn store_powershell_resolution_prefers_the_newest_real_package_binary() {
            let temp = tempfile::tempdir().unwrap();
            let architecture = match std::env::consts::ARCH {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                "x86" => "x86",
                other => other,
            };
            for version in ["7.5.4.0", "7.10.1.0"] {
                let package = temp.path().join(format!(
                    "Microsoft.PowerShell_{version}_{architecture}__8wekyb3d8bbwe"
                ));
                std::fs::create_dir_all(&package).unwrap();
                std::fs::write(package.join("pwsh.exe"), b"fixture").unwrap();
            }
            let ignored = temp.path().join(format!(
                "Microsoft.PowerShellPreview_9.0.0.0_{architecture}__8wekyb3d8bbwe"
            ));
            std::fs::create_dir_all(&ignored).unwrap();
            std::fs::write(ignored.join("pwsh.exe"), b"fixture").unwrap();

            let candidates = store_powershell7_candidates(temp.path());

            assert_eq!(candidates.len(), 2);
            assert!(candidates[0].to_string_lossy().contains("7.10.1.0"));
            assert!(candidates[1].to_string_lossy().contains("7.5.4.0"));
        }

        #[test]
        fn store_powershell_alias_fixture_resolves_the_real_package_binary() {
            let temp = tempfile::tempdir().unwrap();
            let alias = temp
                .path()
                .join("Users")
                .join("test")
                .join("AppData")
                .join("Local")
                .join("Microsoft")
                .join("WindowsApps")
                .join("pwsh.exe");
            std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
            std::fs::write(&alias, []).unwrap();

            let package_root = temp
                .path()
                .join("Program Files")
                .join("WindowsApps")
                .join("Microsoft.PowerShell_7.6.4.0_x64__8wekyb3d8bbwe");
            let package = package_root.join("pwsh.exe");
            std::fs::create_dir_all(&package_root).unwrap();
            std::fs::write(&package, b"store-pwsh-fixture").unwrap();

            assert_eq!(std::fs::metadata(&alias).unwrap().len(), 0);
            assert_eq!(
                validate_store_powershell7_record(
                    "Microsoft.PowerShell",
                    "Microsoft.PowerShell_7.6.4.0_x64__8wekyb3d8bbwe",
                    "7.6.4.0",
                    "X64",
                    &package_root,
                )
                .unwrap(),
                package
            );
            assert_ne!(alias, package);
        }

        #[test]
        fn app_execution_alias_is_not_a_terminal_shell_candidate() {
            assert!(is_windows_app_execution_alias(Path::new(
                r"C:\Users\test\AppData\Local\Microsoft\WindowsApps\pwsh.exe"
            )));
            assert!(!is_windows_app_execution_alias(Path::new(
                r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.3.0_x64__8wekyb3d8bbwe\pwsh.exe"
            )));
        }

        #[test]
        fn terminal_shell_selection_honors_installer_preference_and_legacy_auto_mode() {
            let temp = tempfile::tempdir().unwrap();
            let system = temp.path().join("system").join("pwsh.exe");
            let legacy = temp.path().join("system32").join("powershell.exe");
            for path in [&system, &legacy] {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"fixture").unwrap();
            }

            assert_eq!(
                select_terminal_shell(
                    Some(TerminalShellPreference::PowerShell7),
                    [system.clone()],
                    legacy.clone(),
                ),
                Some(system.clone())
            );
            assert_eq!(
                select_terminal_shell(
                    Some(TerminalShellPreference::WindowsPowerShell),
                    [system.clone()],
                    legacy.clone(),
                ),
                Some(legacy.clone())
            );
            assert_eq!(
                select_terminal_shell(None, [system.clone()], legacy.clone()),
                Some(system)
            );
            assert_eq!(
                select_terminal_shell(None, [temp.path().join("missing-pwsh.exe")], legacy.clone(),),
                Some(legacy)
            );
        }

        #[test]
        fn terminal_shell_preference_parser_accepts_only_installer_values() {
            assert_eq!(
                parse_terminal_shell_preference("powershell7\r\n"),
                Some(TerminalShellPreference::PowerShell7)
            );
            assert_eq!(
                parse_terminal_shell_preference("windows-powershell\n"),
                Some(TerminalShellPreference::WindowsPowerShell)
            );
            assert_eq!(parse_terminal_shell_preference("auto"), None);
            assert_eq!(parse_terminal_shell_preference("cmd"), None);
        }

        #[test]
        fn terminal_shell_preference_reads_the_installer_marker_next_to_the_shim() {
            let temp = tempfile::tempdir().unwrap();
            let terminal_dir = temp.path().join("admin-terminal");
            std::fs::create_dir_all(&terminal_dir).unwrap();
            let terminal_host = temp.path().join("codex-plus-admin-shim.exe");

            assert_eq!(
                configured_terminal_shell_preference(&terminal_host).unwrap(),
                None
            );

            std::fs::write(
                terminal_dir.join(TERMINAL_SHELL_MODE_FILE),
                "powershell7\r\n",
            )
            .unwrap();
            assert_eq!(
                configured_terminal_shell_preference(&terminal_host).unwrap(),
                Some(TerminalShellPreference::PowerShell7)
            );

            std::fs::write(terminal_dir.join(TERMINAL_SHELL_MODE_FILE), "cmd\n").unwrap();
            assert!(configured_terminal_shell_preference(&terminal_host).is_err());
        }

        #[test]
        fn terminal_shell_selection_rejects_unknown_executables() {
            let temp = tempfile::tempdir().unwrap();
            let cmd = temp.path().join("cmd.exe");
            std::fs::write(&cmd, b"fixture").unwrap();

            assert_eq!(first_existing_terminal_shell([cmd]), None);
        }

        #[test]
        fn store_powershell_is_passed_to_the_elevated_terminal_host() {
            let cwd = Path::new(r"D:\workspace with spaces");
            let shell = Path::new(
                r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7.6.3.0_x64__8wekyb3d8bbwe\pwsh.exe",
            );

            let args = terminal_host_args(
                4242,
                cwd,
                shell,
                &["-NoLogo".to_string(), "-NoProfile".to_string()],
            );

            assert_eq!(args[0], "terminal-host");
            assert_eq!(args[1], "--client-pid");
            assert_eq!(args[2], "4242");
            assert_eq!(args[3], "--cwd");
            assert_eq!(args[4], cwd.as_os_str());
            assert_eq!(args[5], "--shell");
            assert_eq!(args[6], shell.as_os_str());
            assert_eq!(args[7], "--");
            assert_eq!(args[8], "-NoLogo");
            assert_eq!(args[9], "-NoProfile");
        }

        const SESSION: &str = "admin-exec-session";
        const PROOF: &str = "admin-exec-proof-token";

        fn fake_codex_exe() -> &'static Path {
            static EXE: OnceLock<PathBuf> = OnceLock::new();
            EXE.get_or_init(|| {
                let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("target")
                    .join("admin-exec-unit-fixture");
                let resources = root.join("resources");
                std::fs::create_dir_all(&resources).expect("create fixture directory");
                let source = root.join("fake_codex.rs");
                let exe = resources.join("codex.exe");
                std::fs::write(&source, FAKE_CODEX_SOURCE).expect("write fixture source");
                let status = StdCommand::new("rustc")
                    .args(["--edition=2024", "-O"])
                    .arg(&source)
                    .arg("-o")
                    .arg(&exe)
                    .status()
                    .expect("compile fixture");
                assert!(status.success());
                exe
            })
        }

        async fn runtime() -> (AdminExecRuntime, KillOnCloseJob) {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-unit-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let runtime = AdminExecRuntime::start_with_integrity_checker(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                Arc::new(|_| Ok(true)),
            )
            .await
            .expect("runtime");
            (runtime, job)
        }

        async fn connect(pipe_name: &str) -> NamedPipeClient {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                match ClientOptions::new().open(pipe_name) {
                    Ok(client) => return client,
                    Err(_) if tokio::time::Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    Err(error) => panic!("connect pipe: {error}"),
                }
            }
        }

        async fn authenticate_response(
            client: &mut NamedPipeClient,
            session: &str,
            proof: &str,
        ) -> Value {
            let hello = serde_json::to_vec(&json!({
                "protocol":1,
                "sessionId":session,
                "mode":"exec",
                "clientPid":std::process::id(),
                "proof":proof
            }))
            .expect("hello");
            client
                .write_all(&(hello.len() as u32).to_le_bytes())
                .await
                .expect("length");
            client.write_all(&hello).await.expect("hello payload");
            let length = client.read_u32_le().await.expect("response length") as usize;
            let mut response = vec![0; length];
            client.read_exact(&mut response).await.expect("response");
            serde_json::from_slice::<Value>(&response).expect("response JSON")
        }

        async fn authenticate(client: &mut NamedPipeClient, session: &str, proof: &str) -> bool {
            authenticate_response(client, session, proof).await["accepted"] == true
        }

        async fn authenticate_app_server(client: &mut NamedPipeClient, args: &[&str]) -> bool {
            let hello = serde_json::to_vec(&json!({
                "protocol":1,
                "sessionId":SESSION,
                "mode":"app-server",
                "clientPid":std::process::id(),
                "proof":PROOF,
                "helperArgs":args,
            }))
            .expect("hello");
            client
                .write_all(&(hello.len() as u32).to_le_bytes())
                .await
                .expect("length");
            client.write_all(&hello).await.expect("hello payload");
            let length = client.read_u32_le().await.expect("response length") as usize;
            let mut response = vec![0; length];
            client.read_exact(&mut response).await.expect("response");
            serde_json::from_slice::<Value>(&response).expect("response JSON")["accepted"] == true
        }

        #[tokio::test]
        async fn valid_authentication_relays_raw_bytes() {
            let (mut runtime, _job) = runtime().await;
            runtime.verify_ready().await.expect("ready");
            let mut client = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut client, SESSION, PROOF).await);
            let payload = b"{\"id\":77,\"opaque\":\"payload\"}\n";
            client.write_all(payload).await.expect("write payload");
            let mut echoed = vec![0; payload.len()];
            client.read_exact(&mut echoed).await.expect("read payload");
            assert_eq!(echoed, payload);
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn app_server_client_preserves_args_and_relays_stdio() {
            let (runtime, _job) = runtime().await;
            let mut client = connect(&runtime.pipe_name).await;
            assert!(
                authenticate_app_server(
                    &mut client,
                    &["-c", "features.code_mode_host=true", "app-server"],
                )
                .await
            );
            client.write_all(b"app-server-ping\n").await.unwrap();
            let mut echoed = vec![0; "app-server-ping\n".len()];
            client.read_exact(&mut echoed).await.unwrap();
            assert_eq!(echoed, b"app-server-ping\n");
            client.write_all(b"EXIT\n").await.unwrap();
            drop(client);
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn bad_proof_is_rejected() {
            let (runtime, _job) = runtime().await;
            let mut client = connect(&runtime.pipe_name).await;
            assert!(!authenticate(&mut client, SESSION, "wrong-proof").await);
            drop(client);
            let mut retry = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut retry, SESSION, PROOF).await);
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn pipe_recreation_failure_is_fatal_and_secret_free() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-recreate-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let runtime = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::with_pipe_factory(
                    Arc::new(|_| Ok(true)),
                    Arc::new(|name, sid, first| {
                        if first {
                            create_test_pipe(name, sid, true)
                        } else {
                            anyhow::bail!("synthetic reconnect failure containing {PROOF}")
                        }
                    }),
                ),
            )
            .await
            .expect("runtime");
            let mut health = runtime.health_receiver();
            let _client = connect(&runtime.pipe_name).await;
            tokio::time::timeout(Duration::from_secs(3), health.changed())
                .await
                .expect("health timeout")
                .expect("health channel");
            let failure = health.borrow().clone().expect("fatal health");
            assert_eq!(failure, "administrator exec broker stopped unexpectedly");
            assert!(!failure.contains(PROOF));
            runtime.shutdown().await.expect("shutdown");
        }

        #[test]
        fn exec_pipe_acl_allows_client_io_without_pipe_instance_creation() {
            let identity = current_windows_identity().expect("identity");
            let sddl = exec_pipe_sddl(&identity.user_sid).expect("exec pipe SDDL");
            assert!(sddl.contains("0x0012019B"));
            assert!(!sddl.contains(&format!("GA;;;{}", identity.user_sid)));
            assert_eq!(0x0012_019b_u32 & 0x4, 0);
        }

        #[tokio::test]
        async fn medium_integrity_peer_cannot_create_a_squatting_pipe_instance() {
            if process_has_high_integrity(std::process::id()).expect("current integrity") {
                eprintln!("SKIP: adversarial pipe-instance test requires a medium-integrity peer");
                return;
            }
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let owner = create_restricted_pipe(&pipe_name, &identity.user_sid, true)
                .expect("broker must create first instance");
            assert!(
                create_restricted_pipe(&pipe_name, &identity.user_sid, false).is_err(),
                "medium-integrity peer created a squatting server instance"
            );
            let _client = ClientOptions::new()
                .read(true)
                .write(true)
                .open(&pipe_name)
                .expect("client read/write access must remain allowed");
            owner
                .connect()
                .await
                .or_else(|error| {
                    (error.raw_os_error() == Some(535))
                        .then_some(())
                        .ok_or(error)
                })
                .expect("broker must observe the client connection");
        }

        #[tokio::test]
        async fn listener_replacement_is_created_before_first_instance_is_released() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-anchor-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let checked_replacement = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let checked_for_factory = Arc::clone(&checked_replacement);
            let runtime = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::with_pipe_factory(
                    Arc::new(|_| Ok(true)),
                    Arc::new(move |name, sid, first| {
                        if first {
                            return create_test_pipe(name, sid, true);
                        }
                        assert!(
                            create_test_pipe(name, sid, true).is_err(),
                            "FIRST_PIPE_INSTANCE succeeded, exposing an ownership gap"
                        );
                        checked_for_factory.store(true, Ordering::SeqCst);
                        create_test_pipe(name, sid, false)
                    }),
                ),
            )
            .await
            .expect("runtime");
            let mut rejected = connect(&runtime.pipe_name).await;
            assert!(!authenticate(&mut rejected, SESSION, "wrong-proof").await);
            drop(rejected);
            let mut retry = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut retry, SESSION, PROOF).await);
            assert!(checked_replacement.load(Ordering::SeqCst));
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn wrong_session_is_rejected() {
            let (runtime, _job) = runtime().await;
            let mut client = connect(&runtime.pipe_name).await;
            assert!(!authenticate(&mut client, "wrong-session", PROOF).await);
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn sequential_clients_are_admitted_after_disconnect() {
            let (runtime, _job) = runtime().await;
            let mut first = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut first, SESSION, PROOF).await);
            drop(first);
            let mut second = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut second, SESSION, PROOF).await);
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn concurrent_app_server_clients_are_admitted_before_the_first_disconnects() {
            let (runtime, _job) = runtime().await;
            let mut first = connect(&runtime.pipe_name).await;
            assert!(authenticate_app_server(&mut first, &["app-server"]).await);
            first.write_all(b"first-stays-open\n").await.unwrap();
            let mut first_echo = vec![0; "first-stays-open\n".len()];
            first.read_exact(&mut first_echo).await.unwrap();

            let pipe_name = runtime.pipe_name.clone();
            let mut second = tokio::time::timeout(Duration::from_secs(1), async move {
                let mut client = connect(&pipe_name).await;
                assert!(authenticate_app_server(&mut client, &["app-server"]).await);
                client
            })
            .await
            .expect("second app-server client must not wait for the first to disconnect");

            second.write_all(b"second-is-live\n").await.unwrap();
            let mut second_echo = vec![0; "second-is-live\n".len()];
            second.read_exact(&mut second_echo).await.unwrap();
            assert_eq!(second_echo, b"second-is-live\n");
            drop(second);
            drop(first);
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn active_exec_server_exit_is_published_as_fatal_health() {
            let (runtime, _job) = runtime().await;
            let mut health = runtime.health_receiver();
            let mut client = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut client, SESSION, PROOF).await);
            client.write_all(b"EXIT\n").await.expect("exit request");
            tokio::time::timeout(Duration::from_secs(3), health.changed())
                .await
                .expect("health timeout")
                .expect("health channel");
            assert_eq!(
                health.borrow().as_deref(),
                Some("administrator exec broker stopped unexpectedly")
            );
            runtime.shutdown().await.expect("shutdown");
        }

        #[test]
        fn official_package_record_requires_exact_codex_identity_version_and_location() {
            let record = OfficialPackageRecord {
                name: "OpenAI.Codex".to_owned(),
                package_full_name: "OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0".to_owned(),
                package_family_name: "OpenAI.Codex_2p2nqsd0c76g0".to_owned(),
                version: "26.707.3748.0".to_owned(),
                signature_kind: "Store".to_owned(),
                is_development_mode: false,
                install_location: PathBuf::from(
                    r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0",
                ),
            };
            validate_package_record_structure(&record).expect("official package");

            for mut invalid in [
                OfficialPackageRecord {
                    name: "Other.Codex".to_owned(),
                    ..record.clone()
                },
                OfficialPackageRecord {
                    package_full_name: "OpenAI.Codex_1.0.0.0_x64__wrongpublisher".to_owned(),
                    ..record.clone()
                },
                OfficialPackageRecord {
                    version: "1.0.0.0".to_owned(),
                    ..record.clone()
                },
                OfficialPackageRecord {
                    signature_kind: "Developer".to_owned(),
                    ..record.clone()
                },
                OfficialPackageRecord {
                    is_development_mode: true,
                    ..record.clone()
                },
                OfficialPackageRecord {
                    install_location: PathBuf::from(
                        r"C:\Users\me\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0",
                    ),
                    ..record.clone()
                },
            ] {
                assert!(validate_package_record_structure(&invalid).is_err());
                invalid.name.clear();
            }
        }

        #[test]
        fn readiness_request_uses_windowless_shim_and_empty_environment() {
            let shim = PathBuf::from(r"C:\Program Files\Codex++\codex-plus-admin-shim.exe");
            let request = readiness_process_request(&shim).expect("build readiness request");
            request["params"]["argv"].as_array().expect("argv");
            assert_eq!(request["params"]["argv"], json!([shim, "identity-probe"]));
            let environment = request["params"]["env"].as_object().expect("environment");
            assert!(environment.is_empty());
            let serialized = request.to_string().to_ascii_lowercase();
            assert!(!serialized.contains("powershell"));
            assert!(!serialized.contains("whoami"));
        }

        #[test]
        fn store_package_discovery_hides_its_powershell_process() {
            let source = include_str!("exec.rs");
            let discovery = source
                .split("fn discover_official_package_if_installed")
                .nth(1)
                .expect("package discovery function");
            let body = discovery.split("fn ").next().unwrap_or(discovery);
            assert!(body.contains("creation_flags(CREATE_NO_WINDOW)"));
        }

        #[test]
        fn production_discovery_loads_the_trusted_appx_module_on_windows_powershell() {
            let system_directory = trusted_system_directory().expect("trusted system directory");
            let powershell = system_directory
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            let module = system_directory
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("Modules")
                .join("Appx")
                .join("Appx.psd1")
                .to_string_lossy()
                .replace('\'', "''");
            let script = format!(
                "$ErrorActionPreference='Stop'; Import-Module -Name '{module}' -Force; $p=Appx\\Get-AppxPackage -Name OpenAI.Codex | Sort-Object Version -Descending | Select-Object -First 1; if($null -eq $p){{exit 3}}; $p.PackageFullName"
            );
            let installed = StdCommand::new(powershell)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &script,
                ])
                .output()
                .expect("execute trusted Appx package probe");
            if installed.status.code() == Some(3) {
                eprintln!("SKIP: OpenAI.Codex is explicitly not installed");
                return;
            }
            assert!(
                installed.status.success(),
                "trusted Appx package probe failed: {}",
                String::from_utf8_lossy(&installed.stderr)
            );

            let record = discover_official_package()
                .expect("production discovery must find the installed OpenAI.Codex package");
            assert_eq!(record.signature_kind, "Store");
            assert!(!record.is_development_mode);
            validate_package_record_structure(&record).expect("trusted installed package record");
        }

        #[test]
        fn production_resolver_accepts_installed_store_codex_and_rejects_user_directory() {
            let record = match discover_official_package_if_installed()
                .expect("trusted Appx package query must complete")
            {
                Some(record) => record,
                None => {
                    eprintln!(
                        "SKIP: production Store trust smoke requires an installed OpenAI.Codex package"
                    );
                    return;
                }
            };
            let official = record
                .install_location
                .join("app")
                .join("resources")
                .join("codex.exe");
            let lease = resolve_official_codex_lease(&official)
                .expect("installed official Store executable must be trusted");
            assert!(paths_equal_ignore_ascii_case(&lease.path, &official));
            let companions = lease
                .runtime_companions()
                .expect("installed official runtime companions must be trusted");
            assert_eq!(companions.len(), OFFICIAL_RUNTIME_COMPANION_NAMES.len());
            for (companion, expected_name) in
                companions.iter().zip(OFFICIAL_RUNTIME_COMPANION_NAMES)
            {
                assert_eq!(
                    companion
                        .canonical_path
                        .file_name()
                        .and_then(|name| name.to_str()),
                    Some(expected_name)
                );
            }
            drop(lease);

            let temp = tempfile::tempdir().unwrap();
            let malicious = temp.path().join("resources").join("codex.exe");
            std::fs::create_dir_all(malicious.parent().unwrap()).unwrap();
            std::fs::write(&malicious, b"malicious").unwrap();
            assert!(resolve_official_codex_lease(&malicious).is_err());
        }

        #[tokio::test]
        async fn elevated_production_runtime_copy_completes_real_exec_readiness() {
            if !process_has_high_integrity(std::process::id()).unwrap_or(false) {
                eprintln!("SKIP: production administrator exec smoke requires elevation");
                return;
            }
            let record = discover_official_package()
                .expect("production smoke requires the installed Store Codex package");
            let codex_exe = record
                .install_location
                .join("app")
                .join("resources")
                .join("codex.exe");
            let Some(readiness_probe_exe) = std::env::var_os("CODEXPP_ADMIN_SHIM_TEST_EXE")
                .map(PathBuf::from)
                .filter(|path| path.is_file())
            else {
                eprintln!("SKIP: set CODEXPP_ADMIN_SHIM_TEST_EXE to the built administrator shim");
                return;
            };
            let identity = current_windows_identity().expect("read elevated identity");
            let job = KillOnCloseJob::new(&format!(
                "admin-exec-production-smoke-{}",
                uuid::Uuid::new_v4()
            ))
            .expect("create production smoke job");
            let pipe_name = format!(
                r"\\.\pipe\codex-plus-admin-production-smoke-{}",
                uuid::Uuid::new_v4()
            );
            let mut runtime = AdminExecRuntime::start(
                AdminExecConfig {
                    codex_exe: &codex_exe,
                    readiness_probe_exe: &readiness_probe_exe,
                    pipe_name: &pipe_name,
                    session_id: "production-smoke-session",
                    session_proof: "production-smoke-proof",
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
            )
            .await
            .expect("real administrator exec runtime readiness");
            runtime.verify_ready().await.expect("runtime health");
            runtime.shutdown().await.expect("runtime cleanup");
        }

        #[test]
        fn verified_executable_lease_rejects_reparse_and_prevents_replacement() {
            use std::os::windows::fs::symlink_file;

            let temp = tempfile::tempdir().unwrap();
            let package = temp
                .path()
                .join("WindowsApps")
                .join("OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0");
            let resources = package.join("app").join("resources");
            std::fs::create_dir_all(&resources).unwrap();
            let executable = resources.join("codex.exe");
            std::fs::write(&executable, b"trusted").unwrap();
            let lease = VerifiedExecutableLease::open(&executable, &package).unwrap();
            assert!(std::fs::rename(&executable, resources.join("replaced.exe")).is_err());
            drop(lease);

            let target = resources.join("target.exe");
            let link = resources.join("codex-link.exe");
            std::fs::write(&target, b"target").unwrap();
            if symlink_file(&target, &link).is_ok() {
                assert!(VerifiedExecutableLease::open(&link, &package).is_err());
            }
        }

        #[tokio::test]
        async fn child_exit_closes_client_stream() {
            let (runtime, _job) = runtime().await;
            let mut client = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut client, SESSION, PROOF).await);
            client.write_all(b"EXIT\n").await.expect("exit request");
            let mut output = Vec::new();
            tokio::time::timeout(Duration::from_secs(3), client.read_to_end(&mut output))
                .await
                .expect("exit timeout")
                .expect("read EOF");
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn shutdown_closes_client_stream() {
            let (runtime, _job) = runtime().await;
            let mut client = connect(&runtime.pipe_name).await;
            assert!(authenticate(&mut client, SESSION, PROOF).await);
            runtime.shutdown().await.expect("shutdown");
            let mut byte = [0; 1];
            let read = tokio::time::timeout(Duration::from_secs(3), client.read(&mut byte))
                .await
                .expect("shutdown timeout");
            assert!(matches!(read, Ok(0) | Err(_)));
        }

        #[tokio::test]
        async fn probe_checker_error_terminates_spawned_child() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-cleanup-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let observed_pid = Arc::new(std::sync::Mutex::new(None));
            let observed_for_checker = Arc::clone(&observed_pid);
            let result = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::new(
                    Arc::new(move |pid| {
                        *observed_for_checker.lock().expect("pid lock") = Some(pid);
                        anyhow::bail!("synthetic probe checker failure")
                    }),
                    Arc::new(|name, sid| create_test_pipe(name, sid, true)),
                ),
            )
            .await;
            assert!(result.is_err());
            let pid = observed_pid.lock().expect("pid lock").expect("probe pid");
            assert_pid_exits(pid).await;
        }

        #[tokio::test]
        async fn probe_image_mismatch_terminates_before_protocol_readiness() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-image-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let observed_pid = Arc::new(std::sync::Mutex::new(None));
            let observed_for_verifier = Arc::clone(&observed_pid);
            let result = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::with_image_verifier(
                    Arc::new(|_| Ok(true)),
                    Arc::new(move |child, _lease| {
                        *observed_for_verifier.lock().expect("pid lock") = child.id();
                        anyhow::bail!("synthetic image mismatch")
                    }),
                ),
            )
            .await;
            assert!(result.is_err());
            let pid = observed_pid.lock().expect("pid lock").expect("probe pid");
            assert_pid_exits(pid).await;
        }

        #[tokio::test]
        async fn production_checker_error_terminates_spawned_child() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-cleanup-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let calls = Arc::new(AtomicUsize::new(0));
            let production_pid = Arc::new(std::sync::Mutex::new(None));
            let calls_for_checker = Arc::clone(&calls);
            let pid_for_checker = Arc::clone(&production_pid);
            let runtime = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::new(
                    Arc::new(move |pid| {
                        if calls_for_checker.fetch_add(1, Ordering::SeqCst) == 0 {
                            Ok(true)
                        } else {
                            *pid_for_checker.lock().expect("pid lock") = Some(pid);
                            anyhow::bail!("synthetic production checker failure")
                        }
                    }),
                    Arc::new(|name, sid| create_test_pipe(name, sid, true)),
                ),
            )
            .await
            .expect("probe and listener startup");
            let mut health = runtime.health_receiver();
            let mut client = connect(&runtime.pipe_name).await;
            let response = authenticate_response(&mut client, SESSION, PROOF).await;
            assert_eq!(response["accepted"], false);
            tokio::time::timeout(Duration::from_secs(3), health.changed())
                .await
                .expect("health timeout")
                .expect("health channel");
            assert_eq!(
                health.borrow().as_deref(),
                Some("administrator exec broker stopped unexpectedly")
            );
            let pid = production_pid
                .lock()
                .expect("pid lock")
                .expect("production pid");
            assert_pid_exits(pid).await;
            runtime.shutdown().await.expect("shutdown");
        }

        #[tokio::test]
        async fn initial_pipe_creation_error_never_spawns_production_child() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job = KillOnCloseJob::new(&format!("admin-exec-cleanup-{}", uuid::Uuid::new_v4()))
                .expect("job");
            let calls = Arc::new(AtomicUsize::new(0));
            let production_pid = Arc::new(std::sync::Mutex::new(None));
            let calls_for_checker = Arc::clone(&calls);
            let pid_for_checker = Arc::clone(&production_pid);
            let result = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::new(
                    Arc::new(move |pid| {
                        if calls_for_checker.fetch_add(1, Ordering::SeqCst) == 1 {
                            *pid_for_checker.lock().expect("pid lock") = Some(pid);
                        }
                        Ok(true)
                    }),
                    Arc::new(|_, _| anyhow::bail!("synthetic pipe creation failure")),
                ),
            )
            .await;
            assert!(result.is_err());
            assert!(production_pid.lock().expect("pid lock").is_none());
            assert_eq!(calls.load(Ordering::SeqCst), 1, "only probe may be spawned");
        }

        #[tokio::test]
        async fn admission_integrity_query_error_returns_explicit_rejection() {
            let identity = current_windows_identity().expect("identity");
            let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
            let job =
                KillOnCloseJob::new(&format!("admin-exec-admission-{}", uuid::Uuid::new_v4()))
                    .expect("job");
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_for_checker = Arc::clone(&calls);
            let runtime = AdminExecRuntime::start_with_hooks(
                AdminExecConfig {
                    codex_exe: fake_codex_exe(),
                    readiness_probe_exe: fake_codex_exe(),
                    pipe_name: &pipe_name,
                    session_id: SESSION,
                    session_proof: PROOF,
                    expected_user_sid: &identity.user_sid,
                    expected_logon_sid: &identity.logon_sid,
                },
                &job,
                ExecHooks::new(
                    Arc::new(move |_| {
                        if calls_for_checker.fetch_add(1, Ordering::SeqCst) < 1 {
                            Ok(true)
                        } else {
                            anyhow::bail!("synthetic admission query failure")
                        }
                    }),
                    Arc::new(|name, sid| create_test_pipe(name, sid, true)),
                ),
            )
            .await
            .expect("runtime");
            let mut client = connect(&runtime.pipe_name).await;
            let response = authenticate_response(&mut client, SESSION, PROOF).await;
            assert_eq!(response["accepted"], false);
            assert_eq!(response["reason"], "production-start-rejected");
            let mut health = runtime.health_receiver();
            if health.borrow().is_none() {
                tokio::time::timeout(Duration::from_secs(3), health.changed())
                    .await
                    .expect("health timeout")
                    .expect("health channel");
            }
            assert_eq!(
                health.borrow().as_deref(),
                Some("administrator exec broker stopped unexpectedly")
            );
            runtime.shutdown().await.expect("shutdown");
        }

        #[test]
        fn cleanup_error_is_returned_without_hiding_primary_error() {
            let primary = format!(
                "{:#}",
                merge_primary_and_cleanup::<()>(
                    Err(anyhow::anyhow!("primary")),
                    Err(anyhow::anyhow!("cleanup")),
                )
                .expect_err("both errors must fail")
            );
            assert!(primary.contains("primary"));
            assert!(primary.contains("cleanup also failed"));
            let cleanup =
                merge_primary_and_cleanup::<()>(Ok(()), Err(anyhow::anyhow!("cleanup-only")))
                    .expect_err("cleanup error must fail")
                    .to_string();
            assert!(cleanup.contains("cleanup-only"));
        }

        #[test]
        fn readiness_rejects_output_before_start_response() {
            let identity = current_windows_identity().expect("identity");
            let mut state = ProbeResponseState::default();
            assert!(state.consume(json!({"method":"process/output","params":{"processId":PROBE_PROCESS_ID,"stream":"stdout","chunk":""}})).is_err());
            assert!(
                state
                    .finish(&identity.user_sid, &identity.logon_sid)
                    .is_err()
            );
        }

        #[test]
        fn readiness_rejects_wrong_process_id() {
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            assert!(state.consume(json!({"method":"process/output","params":{"processId":"wrong","stream":"stdout","chunk":""}})).is_err());
        }

        #[test]
        fn readiness_rejects_sandbox_denied_exit() {
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            assert!(state.consume(json!({"method":"process/exited","params":{"processId":PROBE_PROCESS_ID,"exitCode":0,"sandboxDenied":true}})).is_err());
        }

        #[test]
        fn readiness_stderr_cannot_contribute_identity() {
            let identity = current_windows_identity().expect("identity");
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            let chunk = base64::engine::general_purpose::STANDARD
                .encode(format!("SID={};RID=12288", identity.user_sid));
            state.consume(json!({"method":"process/output","params":{"processId":PROBE_PROCESS_ID,"stream":"stderr","chunk":chunk}})).expect("stderr ignored");
            state.consume(json!({"method":"process/exited","params":{"processId":PROBE_PROCESS_ID,"exitCode":0,"sandboxDenied":false}})).expect("exit");
            assert!(
                state
                    .finish(&identity.user_sid, &identity.logon_sid)
                    .is_err()
            );
        }

        #[test]
        fn readiness_failure_reports_bounded_stderr_context() {
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            let chunk = base64::engine::general_purpose::STANDARD.encode("diagnostic failure");
            state
                .consume(json!({"method":"process/output","params":{"processId":PROBE_PROCESS_ID,"stream":"stderr","chunk":chunk}}))
                .expect("stderr collection");
            let error = state
                .consume(json!({"method":"process/exited","params":{"processId":PROBE_PROCESS_ID,"exitCode":1,"sandboxDenied":false}}))
                .expect_err("nonzero readiness exit must fail")
                .to_string();
            assert!(error.contains("diagnostic failure"));
        }

        #[test]
        fn readiness_rejects_wrong_logon_session_from_actual_command_output() {
            let identity = current_windows_identity().expect("identity");
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            let chunk = base64::engine::general_purpose::STANDARD.encode(format!(
                "SID={};LOGON=S-1-5-5-999-999;RID=12288",
                identity.user_sid
            ));
            state.consume(json!({"method":"process/output","params":{"processId":PROBE_PROCESS_ID,"stream":"stdout","chunk":chunk}})).expect("stdout");
            state.consume(json!({"method":"process/exited","params":{"processId":PROBE_PROCESS_ID,"exitCode":0,"sandboxDenied":false}})).expect("exit");
            assert!(
                state
                    .finish(&identity.user_sid, &identity.logon_sid)
                    .is_err()
            );
        }

        #[test]
        fn readiness_rejects_missing_logon_session_from_actual_command_output() {
            let identity = current_windows_identity().expect("identity");
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            let chunk = base64::engine::general_purpose::STANDARD
                .encode(format!("SID={};LOGON=;RID=12288", identity.user_sid));
            state.consume(json!({"method":"process/output","params":{"processId":PROBE_PROCESS_ID,"stream":"stdout","chunk":chunk}})).expect("stdout");
            state.consume(json!({"method":"process/exited","params":{"processId":PROBE_PROCESS_ID,"exitCode":0,"sandboxDenied":false}})).expect("exit");
            assert!(
                state
                    .finish(&identity.user_sid, &identity.logon_sid)
                    .is_err()
            );
        }

        #[test]
        fn readiness_accepts_utf16_windows_powershell_identity_output() {
            let identity = current_windows_identity().expect("identity");
            let mut state = ProbeResponseState::default();
            state
                .consume(json!({"id":2,"result":{"processId":PROBE_PROCESS_ID}}))
                .expect("start response");
            let output = format!(
                "SID={};LOGON={};RID=12288\r\n",
                identity.user_sid, identity.logon_sid
            );
            let bytes = output
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            let chunk = base64::engine::general_purpose::STANDARD.encode(bytes);
            state
                .consume(json!({"method":"process/output","params":{"processId":PROBE_PROCESS_ID,"stream":"stdout","chunk":chunk}}))
                .expect("stdout");
            state
                .consume(json!({"method":"process/exited","params":{"processId":PROBE_PROCESS_ID,"exitCode":0,"sandboxDenied":false}}))
                .expect("exit");
            state
                .finish(&identity.user_sid, &identity.logon_sid)
                .expect("UTF-16 identity output");
        }

        #[test]
        fn proof_digest_comparison_handles_different_lengths() {
            assert!(constant_time_proof_eq(b"same", b"same"));
            assert!(!constant_time_proof_eq(b"short", b"a much longer proof"));
            assert!(!constant_time_proof_eq(b"a much longer proof", b"short"));
        }

        async fn assert_pid_exits(pid: u32) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                if process_windows_identity(pid).is_err() {
                    return;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "child {pid} did not exit"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        const FAKE_CODEX_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
use std::process::Command;
fn b64(bytes: &[u8]) -> String {
    const T: &[u8;64]=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o=String::new();
    for c in bytes.chunks(3) { let a=c[0] as u32; let b=c.get(1).copied().unwrap_or(0) as u32; let d=c.get(2).copied().unwrap_or(0) as u32; let n=(a<<16)|(b<<8)|d; o.push(T[((n>>18)&63)as usize]as char); o.push(T[((n>>12)&63)as usize]as char); o.push(if c.len()>1{T[((n>>6)&63)as usize]as char}else{'='}); o.push(if c.len()>2{T[(n&63)as usize]as char}else{'='}); }
    o
}
fn main() {
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    let app_server=args.iter().any(|arg| arg=="app-server");
    if !app_server && args != ["exec-server","--listen","stdio"] { std::process::exit(64); }
    let stdin=io::stdin(); let mut input=stdin.lock(); let mut output=io::stdout().lock(); let mut line=String::new(); let mut initialized=false;
    while input.read_line(&mut line).unwrap_or(0)!=0 {
        if app_server { if line=="EXIT\n" { break; } output.write_all(line.as_bytes()).unwrap(); output.flush().unwrap(); line.clear(); continue; }
        if !initialized && line.contains("\"method\":\"initialize\"") { writeln!(output,"{{\"id\":1,\"result\":{{}}}}").unwrap(); output.flush().unwrap(); }
        else if !initialized && line.contains("\"method\":\"initialized\"") { initialized=true; }
        else if initialized && line.contains("\"method\":\"process/start\"") { let p=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command","$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $logon=[regex]::Match((whoami /logonid | Out-String),'S-1-5-5-\\d+-\\d+').Value; Write-Output ('SID='+$sid+';LOGON='+$logon+';RID=12288')"]).output().unwrap(); writeln!(output,"{{\"id\":2,\"result\":{{\"processId\":\"codex-plus-admin-readiness-probe\"}}}}").unwrap(); writeln!(output,"{{\"method\":\"process/output\",\"params\":{{\"processId\":\"codex-plus-admin-readiness-probe\",\"seq\":1,\"stream\":\"stdout\",\"chunk\":\"{}\"}}}}",b64(&p.stdout)).unwrap(); writeln!(output,"{{\"method\":\"process/exited\",\"params\":{{\"processId\":\"codex-plus-admin-readiness-probe\",\"seq\":2,\"exitCode\":0,\"sandboxDenied\":false}}}}").unwrap(); output.flush().unwrap(); }
        else { if line=="EXIT\n" { break; } output.write_all(line.as_bytes()).unwrap(); output.flush().unwrap(); }
        line.clear();
    }
}
"#;
    }
}

#[cfg(windows)]
pub use platform::AdminExecRuntime;

#[cfg(not(windows))]
pub struct AdminExecRuntime {
    pub pipe_name: String,
    pub session_id: String,
    pub session_proof: String,
    child: tokio::process::Child,
}

#[cfg(not(windows))]
impl AdminExecRuntime {
    pub async fn start(
        _config: AdminExecConfig<'_>,
        _job: &KillOnCloseJob,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("administrator exec runtime is unsupported on non-Windows platforms")
    }

    pub async fn verify_ready(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("administrator exec runtime is unsupported on non-Windows platforms")
    }

    pub fn health_receiver(&self) -> tokio::sync::watch::Receiver<Option<String>> {
        tokio::sync::watch::channel(Some(
            "administrator exec runtime is unsupported off Windows".to_owned(),
        ))
        .1
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        anyhow::bail!("administrator exec runtime is unsupported on non-Windows platforms")
    }
}
