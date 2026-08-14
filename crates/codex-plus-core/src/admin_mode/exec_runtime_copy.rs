use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use base64::Engine;
use sha2::{Digest, Sha256};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_DIRECTORY_SCRIPT: &str =
    include_str!("../../../../scripts/installer/windows/secure-recovery-create.ps1");
const PROTECT_FILE_SCRIPT: &str =
    include_str!("../../../../scripts/installer/windows/secure-recovery-file.ps1");
const RUNTIME_FILE_NAME: &str = "codex-plus-recovery.exe";
const HELPER_RUNTIME_FILE_NAME: &str = "codex-plus-computer-use.exe";

pub(crate) struct RuntimeCopySource<'a> {
    pub(crate) file_name: &'a str,
    pub(crate) file: &'a std::fs::File,
    pub(crate) expected_hash: &'a str,
}

struct RuntimeFile {
    path: PathBuf,
    handle: Option<crate::admin_secure_io::SecureFileLease>,
}

pub(crate) struct AdminExecRuntimeCopy {
    directory: PathBuf,
    executable_path: PathBuf,
    files: Vec<RuntimeFile>,
    #[cfg(test)]
    _temp_guard: Option<tempfile::TempDir>,
}

impl AdminExecRuntimeCopy {
    pub(crate) fn create(
        source: &std::fs::File,
        expected_hash: &str,
        companions: &[RuntimeCopySource<'_>],
    ) -> anyhow::Result<Self> {
        Self::create_with_name(source, expected_hash, RUNTIME_FILE_NAME, companions)
    }

    pub(crate) fn create_for_helper(
        source: &std::fs::File,
        expected_hash: &str,
    ) -> anyhow::Result<Self> {
        Self::create_with_name(source, expected_hash, HELPER_RUNTIME_FILE_NAME, &[])
    }

    fn create_with_name(
        source: &std::fs::File,
        expected_hash: &str,
        runtime_file_name: &str,
        companions: &[RuntimeCopySource<'_>],
    ) -> anyhow::Result<Self> {
        let directory = create_secure_directory()?;
        let executable_path = directory.join(runtime_file_name);
        let result = (|| {
            let sources = runtime_sources(source, expected_hash, runtime_file_name, companions)?;
            let mut staged = Vec::with_capacity(sources.len());
            for source in &sources {
                let path = directory.join(source.file_name);
                let copied = copy_and_verify(source.file, source.expected_hash, &path)?;
                drop(copied);
                staged.push((path, source.expected_hash));
            }
            for (path, _) in &staged {
                protect_runtime_file(path)?;
            }
            let mut files = Vec::with_capacity(staged.len());
            for (path, expected_hash) in staged {
                let verified = crate::admin_secure_io::SecureFileLease::open(&path, false)
                    .context("pin administrator exec runtime bundle file")?;
                ensure!(
                    paths_equal_ignore_ascii_case(&verified.final_path()?, &path),
                    "administrator exec runtime bundle handle path changed"
                );
                ensure!(
                    sha256_file(verified.as_file())?.eq_ignore_ascii_case(expected_hash),
                    "administrator exec runtime bundle changed after ACL protection"
                );
                files.push(RuntimeFile {
                    path,
                    handle: Some(verified),
                });
            }
            Ok(Self {
                directory: directory.clone(),
                executable_path: executable_path.clone(),
                files,
                #[cfg(test)]
                _temp_guard: None,
            })
        })();
        if result.is_err() {
            if let Ok(entries) = std::fs::read_dir(&directory) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            let _ = std::fs::remove_dir(&directory);
        }
        result
    }

    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable_path
    }

    #[cfg(test)]
    pub(crate) fn create_test(source: &std::fs::File, expected_hash: &str) -> anyhow::Result<Self> {
        Self::create_test_with_name(source, expected_hash, RUNTIME_FILE_NAME, &[])
    }

    #[cfg(test)]
    fn create_test_with_companions(
        source: &std::fs::File,
        expected_hash: &str,
        companions: &[RuntimeCopySource<'_>],
    ) -> anyhow::Result<Self> {
        Self::create_test_with_name(source, expected_hash, RUNTIME_FILE_NAME, companions)
    }

    #[cfg(test)]
    fn create_test_for_helper(source: &std::fs::File, expected_hash: &str) -> anyhow::Result<Self> {
        Self::create_test_with_name(source, expected_hash, HELPER_RUNTIME_FILE_NAME, &[])
    }

    #[cfg(test)]
    fn create_test_with_name(
        source: &std::fs::File,
        expected_hash: &str,
        runtime_file_name: &str,
        companions: &[RuntimeCopySource<'_>],
    ) -> anyhow::Result<Self> {
        let temp_guard = tempfile::tempdir()?;
        let directory = temp_guard.path().to_owned();
        let executable_path = directory.join(runtime_file_name);
        let sources = runtime_sources(source, expected_hash, runtime_file_name, companions)?;
        let mut files = Vec::with_capacity(sources.len());
        for source in sources {
            let path = directory.join(source.file_name);
            let copied = copy_and_verify(source.file, source.expected_hash, &path)?;
            drop(copied);
            let handle = crate::admin_secure_io::SecureFileLease::open(&path, false)?;
            files.push(RuntimeFile {
                path,
                handle: Some(handle),
            });
        }
        Ok(Self {
            directory,
            executable_path,
            files,
            _temp_guard: Some(temp_guard),
        })
    }

    pub(crate) fn cleanup(mut self) -> anyhow::Result<()> {
        self.cleanup_inner()
    }

    fn cleanup_inner(&mut self) -> anyhow::Result<()> {
        for file in &mut self.files {
            drop(file.handle.take());
        }
        let mut first_error = None;
        for file in self.files.iter().rev() {
            if let Err(error) = remove_with_retry(&file.path, false).with_context(|| {
                format!("delete administrator runtime file {}", file.path.display())
            }) && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Err(error) = remove_with_retry(&self.directory, true)
            .context("remove administrator exec runtime directory")
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn runtime_sources<'a>(
    executable: &'a std::fs::File,
    executable_hash: &'a str,
    executable_name: &'a str,
    companions: &'a [RuntimeCopySource<'a>],
) -> anyhow::Result<Vec<RuntimeCopySource<'a>>> {
    validate_runtime_file_name(executable_name)?;
    let mut names = vec![executable_name.to_ascii_lowercase()];
    let mut sources = vec![RuntimeCopySource {
        file_name: executable_name,
        file: executable,
        expected_hash: executable_hash,
    }];
    for companion in companions {
        validate_runtime_file_name(companion.file_name)?;
        let normalized = companion.file_name.to_ascii_lowercase();
        ensure!(
            !names.iter().any(|name| name == &normalized),
            "administrator runtime bundle contains a duplicate file name"
        );
        names.push(normalized);
        sources.push(RuntimeCopySource {
            file_name: companion.file_name,
            file: companion.file,
            expected_hash: companion.expected_hash,
        });
    }
    Ok(sources)
}

fn validate_runtime_file_name(name: &str) -> anyhow::Result<()> {
    let path = Path::new(name);
    ensure!(
        !name.is_empty()
            && path
                .file_name()
                .is_some_and(|file_name| file_name == path.as_os_str())
            && path.components().count() == 1,
        "administrator runtime bundle file name is invalid"
    );
    Ok(())
}

fn remove_with_retry(path: &Path, directory: bool) -> std::io::Result<()> {
    const ATTEMPTS: usize = 100;
    const DELAY: std::time::Duration = std::time::Duration::from_millis(10);
    for attempt in 0..ATTEMPTS {
        let result = if directory {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if attempt + 1 < ATTEMPTS
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
                    ) =>
            {
                std::thread::sleep(DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded removal loop always returns")
}

impl Drop for AdminExecRuntimeCopy {
    fn drop(&mut self) {
        let _ = self.cleanup_inner();
    }
}

pub(crate) fn sha256_file(file: &std::fs::File) -> anyhow::Result<String> {
    let mut file = file.try_clone().context("clone executable for hashing")?;
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn copy_and_verify(
    source: &std::fs::File,
    expected_hash: &str,
    target_path: &Path,
) -> anyhow::Result<crate::admin_secure_io::SecureFileLease> {
    let result = (|| {
        let mut source = source
            .try_clone()
            .context("clone locked Store executable")?;
        source.seek(SeekFrom::Start(0))?;
        let target = crate::admin_secure_io::SecureFileLease::create(target_path)
            .context("create administrator exec runtime copy")?;
        let mut target_file = target
            .as_file()
            .try_clone()
            .context("clone administrator exec runtime copy handle")?;
        std::io::copy(&mut source, &mut target_file).context("copy Store executable bytes")?;
        target_file
            .sync_all()
            .context("flush administrator exec runtime copy")?;
        let actual_hash = sha256_file(target.as_file())?;
        ensure!(
            actual_hash.eq_ignore_ascii_case(expected_hash),
            "administrator exec runtime copy hash does not match Store source"
        );
        Ok(target)
    })();
    if result.is_err() {
        if let Ok(target) = crate::admin_secure_io::SecureFileLease::open_for_delete(target_path) {
            let _ = target.delete();
        }
    }
    result
}

fn create_secure_directory() -> anyhow::Result<PathBuf> {
    let output = run_powershell(CREATE_DIRECTORY_SCRIPT, None)
        .context("create administrator exec runtime directory")?;
    let directory = PathBuf::from(output.trim());
    ensure!(
        !directory.as_os_str().is_empty() && directory.is_absolute(),
        "administrator exec runtime directory script returned an invalid path"
    );
    ensure!(
        directory.is_dir(),
        "administrator exec runtime directory is missing"
    );
    Ok(directory)
}

fn protect_runtime_file(path: &Path) -> anyhow::Result<()> {
    run_powershell(PROTECT_FILE_SCRIPT, Some(path))
        .context("protect administrator exec runtime file")?;
    Ok(())
}

fn run_powershell(script: &str, recovery_file: Option<&Path>) -> anyhow::Result<String> {
    let system_directory = trusted_system_directory()?;
    let powershell = system_directory
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let mut command = std::process::Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW);
    if let Some(path) = recovery_file {
        command.env("CODEXPP_RECOVERY_FILE", path);
        command.env(
            "CODEXPP_RECOVERY_FILE_POLICY",
            "administrator-runtime-bundle",
        );
    }
    let output = command.output().context("run trusted Windows PowerShell")?;
    ensure!(
        output.status.success(),
        "trusted Windows PowerShell failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("trusted Windows PowerShell output was not UTF-8")
}

fn trusted_system_directory() -> anyhow::Result<PathBuf> {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buffer = vec![0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    ensure!(
        length > 0 && length < buffer.len(),
        "read trusted Windows system directory"
    );
    buffer.truncate(length);
    Ok(PathBuf::from(String::from_utf16(&buffer)?))
}

fn paths_equal_ignore_ascii_case(left: &Path, right: &Path) -> bool {
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    left.strip_prefix(r"\\?\")
        .unwrap_or(&left)
        .eq_ignore_ascii_case(right.strip_prefix(r"\\?\").unwrap_or(&right))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    #[test]
    fn stream_copy_matches_the_locked_source_hash_and_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.exe");
        let target_path = temp.path().join("target.exe");
        std::fs::write(&source_path, b"trusted-store-codex").unwrap();
        let source = std::fs::File::open(&source_path).unwrap();
        let expected = super::sha256_file(&source).unwrap();

        let target = super::copy_and_verify(&source, &expected, &target_path).unwrap();

        assert_eq!(super::sha256_file(target.as_file()).unwrap(), expected);
        assert_eq!(std::fs::read(target_path).unwrap(), b"trusted-store-codex");
    }

    #[test]
    fn stream_copy_rejects_a_hash_mismatch_and_removes_the_target() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.exe");
        let target_path = temp.path().join("target.exe");
        let mut source = std::fs::File::create(&source_path).unwrap();
        source.write_all(b"trusted-store-codex").unwrap();
        source.sync_all().unwrap();
        drop(source);
        let source = std::fs::File::open(&source_path).unwrap();

        let error = super::copy_and_verify(&source, "wrong-hash", &target_path).unwrap_err();

        assert!(error.to_string().contains("runtime copy hash"));
        assert!(!target_path.exists());
    }

    #[test]
    fn test_runtime_copy_uses_a_distinct_executable_and_cleans_it_up() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.exe");
        std::fs::write(&source_path, b"trusted-store-codex").unwrap();
        let source = std::fs::File::open(&source_path).unwrap();
        let expected = super::sha256_file(&source).unwrap();

        let runtime = super::AdminExecRuntimeCopy::create_test(&source, &expected).unwrap();
        let runtime_path = runtime.executable_path().to_owned();

        assert_ne!(runtime_path, source_path);
        assert_eq!(
            std::fs::read(&runtime_path).unwrap(),
            b"trusted-store-codex"
        );
        runtime.cleanup().unwrap();
        assert!(!runtime_path.exists());
    }

    #[test]
    fn app_server_runtime_copy_contains_code_mode_host_dependency() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("codex.exe");
        let code_mode_host_path = temp.path().join("codex-code-mode-host.exe");
        std::fs::write(&source_path, b"trusted-store-codex").unwrap();
        std::fs::write(&code_mode_host_path, b"trusted-code-mode-host").unwrap();
        let source = std::fs::File::open(&source_path).unwrap();
        let code_mode_host = std::fs::File::open(&code_mode_host_path).unwrap();
        let expected = super::sha256_file(&source).unwrap();
        let code_mode_host_hash = super::sha256_file(&code_mode_host).unwrap();

        let runtime = super::AdminExecRuntimeCopy::create_test_with_companions(
            &source,
            &expected,
            &[super::RuntimeCopySource {
                file_name: "codex-code-mode-host.exe",
                file: &code_mode_host,
                expected_hash: &code_mode_host_hash,
            }],
        )
        .unwrap();
        let runtime_code_mode_host = runtime
            .executable_path()
            .with_file_name("codex-code-mode-host.exe");

        assert!(
            runtime_code_mode_host.is_file(),
            "administrator app-server runtime must include codex-code-mode-host.exe"
        );
        assert_eq!(
            std::fs::read(&runtime_code_mode_host).unwrap(),
            b"trusted-code-mode-host"
        );
        runtime.cleanup().unwrap();
        assert!(!runtime_code_mode_host.exists());
    }

    #[test]
    fn runtime_acl_policy_accepts_every_staged_bundle_file_name() {
        assert!(
            super::PROTECT_FILE_SCRIPT.contains("CODEXPP_RECOVERY_FILE_POLICY"),
            "runtime ACL policy must require the explicit runtime-bundle selector"
        );
        for name in [
            super::RUNTIME_FILE_NAME,
            super::HELPER_RUNTIME_FILE_NAME,
            "codex-code-mode-host.exe",
            "codex-command-runner.exe",
            "codex-windows-sandbox-setup.exe",
            "rg.exe",
        ] {
            assert!(
                super::PROTECT_FILE_SCRIPT.contains(&format!("\"{name}\"")),
                "runtime ACL policy must explicitly accept {name}"
            );
        }
    }

    #[test]
    fn helper_runtime_copy_uses_a_distinct_helper_executable_name() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.exe");
        std::fs::write(&source_path, b"trusted-helper").unwrap();
        let source = std::fs::File::open(&source_path).unwrap();
        let expected = super::sha256_file(&source).unwrap();

        let runtime =
            super::AdminExecRuntimeCopy::create_test_for_helper(&source, &expected).unwrap();
        assert_eq!(
            runtime.executable_path().file_name().unwrap(),
            super::HELPER_RUNTIME_FILE_NAME
        );
        runtime.cleanup().unwrap();
    }
}
