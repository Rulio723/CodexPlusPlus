use anyhow::{Context, bail};
use base64::Engine;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const JOURNAL_FILE_NAME: &str = "administrator-mode-environment.v1.json";
const GUARD_FILE_NAME: &str = "administrator-mode-environment.guard";
const MANAGED_ENVIRONMENT_ID: &str = "codex-plus-admin";

pub struct AdminEnvironmentSpec<'a> {
    pub shim_path: &'a Path,
    pub pipe_name: &'a str,
    pub session_id: &'a str,
    pub proof_path: &'a Path,
}

pub struct AdminEnvironmentTransaction {
    codex_home: PathBuf,
    state_dir: PathBuf,
    managed_sha256: String,
    transaction_id: String,
    backup: Option<crate::admin_secure_io::SecureFileLease>,
    managed: Option<crate::admin_secure_io::SecureFileLease>,
    journal: Option<crate::admin_secure_io::SecureFileLease>,
    managed_snapshot: Option<crate::admin_secure_io::SecureFileLease>,
    _guard: EnvironmentGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentRestoreOutcome {
    NoJournal,
    Restored,
    Conflict {
        original_path: PathBuf,
        managed_path: PathBuf,
        conflicting_paths: Vec<PathBuf>,
    },
}

struct EnvironmentGuard {
    file: crate::admin_secure_io::SecureFileLease,
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.file.as_file());
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentJournal {
    schema_version: u32,
    target_path: PathBuf,
    original_existed: bool,
    original_bytes: String,
    #[serde(default)]
    original_sha256: Option<String>,
    managed_sha256: String,
    transaction_id: String,
    backup_path: PathBuf,
    managed_snapshot_path: PathBuf,
    managed_stage_path: PathBuf,
}

struct TransactionPaths {
    journal: PathBuf,
    journal_stage: PathBuf,
    backup: PathBuf,
    managed_snapshot: PathBuf,
    managed_stage: PathBuf,
    restoring: PathBuf,
    original_stage: PathBuf,
    intervening: PathBuf,
    original_recovery: PathBuf,
    conflicting_recovery: PathBuf,
    captured_conflicting_recovery: PathBuf,
}

impl AdminEnvironmentTransaction {
    pub fn install(
        codex_home: &Path,
        state_dir: &Path,
        spec: &AdminEnvironmentSpec<'_>,
    ) -> anyhow::Result<Self> {
        Self::install_with_test_hook(codex_home, state_dir, spec, |_, _| Ok(()))
    }

    #[doc(hidden)]
    pub fn install_with_test_hook<F, G>(
        codex_home: &Path,
        state_dir: &Path,
        spec: &AdminEnvironmentSpec<'_>,
        before_managed_link: F,
    ) -> anyhow::Result<Self>
    where
        F: FnOnce(&Path, &Path) -> anyhow::Result<G>,
    {
        let target_path = codex_home.join("environments.toml");
        let journal_path = journal_path(state_dir);
        let guard = acquire_guard(state_dir)?;
        require_missing_journal(&journal_path)?;
        let mut original = open_optional_file(&target_path, true)
            .context("failed to read and pin the existing environment file")?;
        let original_bytes = original
            .as_mut()
            .map(|file| file.read_all())
            .transpose()
            .context("failed to read the existing environment file")?;
        let managed_bytes = managed_environment_bytes(spec)?;
        let managed_sha256 = sha256(&managed_bytes);
        let transaction_id = uuid::Uuid::new_v4().simple().to_string();
        let paths = transaction_paths(codex_home, state_dir, &transaction_id);
        let mut managed_snapshot =
            crate::admin_secure_io::SecureFileLease::create(&paths.managed_snapshot)
                .context("failed to create the managed environment snapshot")?;
        managed_snapshot
            .replace_contents(&managed_bytes)
            .context("failed to write the managed environment snapshot")?;
        let mut managed_stage = match crate::admin_secure_io::SecureFileLease::create(
            &paths.managed_stage,
        )
        .and_then(|mut file| {
            file.replace_contents(&managed_bytes)?;
            Ok(file)
        }) {
            Ok(file) => file,
            Err(error) => {
                let _ = managed_snapshot.delete();
                return Err(error).context("failed to stage the managed environment file");
            }
        };

        let journal = EnvironmentJournal {
            schema_version: 2,
            target_path,
            original_existed: original_bytes.is_some(),
            original_bytes: String::new(),
            original_sha256: original_bytes.as_deref().map(sha256),
            managed_sha256: managed_sha256.clone(),
            transaction_id: transaction_id.clone(),
            backup_path: paths.backup.clone(),
            managed_snapshot_path: paths.managed_snapshot.clone(),
            managed_stage_path: paths.managed_stage.clone(),
        };
        let journal_file = match publish_journal(&paths, &journal) {
            Ok(file) => file,
            Err(error) => {
                let _ = managed_snapshot.delete();
                let _ = managed_stage.delete();
                return Err(error);
            }
        };

        if let Some(original) = original.as_mut() {
            original
                .rename_to(&paths.backup)
                .context("failed to capture the environment file atomically")?;
            original
                .read_all()
                .context("failed to read the captured environment backup")?;
        }

        let _hook_guard = before_managed_link(&journal.target_path, &paths.backup)?;
        if let Err(error) = managed_stage.rename_to(&journal.target_path) {
            let _ = managed_stage.delete();
            let intervening =
                restore_backup_after_failed_install(&journal, &paths, original.as_mut())?;
            if let Some(intervening) = intervening {
                bail!(
                    "failed to install the administrator environment file; intervening edit preserved at {}: {error}",
                    intervening.display()
                );
            }
            return Err(error).context("failed to install the administrator environment file");
        }

        Ok(Self {
            codex_home: codex_home.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            managed_sha256,
            transaction_id,
            backup: original,
            managed: Some(managed_stage),
            journal: Some(journal_file),
            managed_snapshot: Some(managed_snapshot),
            _guard: guard,
        })
    }

    pub fn restore(mut self) -> anyhow::Result<EnvironmentRestoreOutcome> {
        let backup = self.backup.take();
        let managed = self.managed.take();
        let journal = self.journal.take();
        let managed_snapshot = self.managed_snapshot.take();
        restore_from_journal(
            &self.codex_home,
            &self.state_dir,
            Some((&self.managed_sha256, &self.transaction_id)),
            backup,
            managed,
            journal,
            managed_snapshot,
        )
    }
}

pub fn recover_stale_environment(
    codex_home: &Path,
    state_dir: &Path,
) -> anyhow::Result<EnvironmentRestoreOutcome> {
    let _guard = acquire_guard(state_dir)?;
    restore_from_journal(codex_home, state_dir, None, None, None, None, None)
}

fn managed_environment_bytes(spec: &AdminEnvironmentSpec<'_>) -> anyhow::Result<Vec<u8>> {
    let shim_path = spec
        .shim_path
        .to_str()
        .context("administrator shim path is not valid Unicode")?;
    let proof_path = spec
        .proof_path
        .to_str()
        .context("administrator proof path is not valid Unicode")?;
    let program = toml_basic_string(shim_path)?;
    let args = [
        "exec-client",
        "--pipe",
        spec.pipe_name,
        "--session",
        spec.session_id,
        "--proof-file",
        proof_path,
    ]
    .into_iter()
    .map(toml_basic_string)
    .collect::<anyhow::Result<Vec<_>>>()?
    .join(", ");
    let managed = format!(
        "default = \"{MANAGED_ENVIRONMENT_ID}\"\ninclude_local = false\n\n[[environments]]\nid = \"{MANAGED_ENVIRONMENT_ID}\"\nprogram = {program}\nargs = [{args}]\n"
    );
    toml::from_str::<toml::Value>(&managed)
        .context("generated administrator environment file is not valid TOML")?;
    Ok(managed.into_bytes())
}

fn toml_basic_string(value: &str) -> anyhow::Result<String> {
    let value = toml::Value::String(value.to_string());
    serde_json::to_string(
        value
            .as_str()
            .context("administrator environment value is not a TOML string")?,
    )
    .context("failed to serialize administrator environment value")
}

fn restore_from_journal(
    codex_home: &Path,
    state_dir: &Path,
    expected_transaction: Option<(&str, &str)>,
    owned_backup: Option<crate::admin_secure_io::SecureFileLease>,
    owned_current: Option<crate::admin_secure_io::SecureFileLease>,
    owned_journal: Option<crate::admin_secure_io::SecureFileLease>,
    owned_managed_snapshot: Option<crate::admin_secure_io::SecureFileLease>,
) -> anyhow::Result<EnvironmentRestoreOutcome> {
    let journal_path = journal_path(state_dir);
    let mut journal_file = match owned_journal {
        Some(file) => file,
        None => match crate::admin_secure_io::SecureFileLease::open_for_delete(&journal_path) {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => return Ok(EnvironmentRestoreOutcome::NoJournal),
            Err(error) => {
                return Err(error).context("failed to pin environment recovery journal");
            }
        },
    };
    let journal_bytes = journal_file
        .read_all()
        .context("failed to read environment recovery journal")?;
    let journal: EnvironmentJournal = serde_json::from_slice(&journal_bytes)
        .context("failed to parse environment recovery journal")?;
    let paths = validate_journal(codex_home, state_dir, &journal)?;
    if let Some((expected_sha256, expected_transaction_id)) = expected_transaction
        && (expected_sha256 != journal.managed_sha256
            || expected_transaction_id != journal.transaction_id)
    {
        bail!("environment transaction does not match its recovery journal");
    }

    let mut backup = match owned_backup {
        Some(backup) => Some(backup),
        None => open_optional_file(&paths.backup, true)
            .context("failed to pin the captured environment backup")?,
    };
    let mut current = match owned_current {
        Some(current) => Some(current),
        None => open_optional_file(&journal.target_path, true)
            .context("failed to read and pin the current environment file")?,
    };
    let current_bytes = current
        .as_mut()
        .map(|file| file.read_all())
        .transpose()
        .context("failed to read the current environment file")?;
    if journal.original_existed && backup.is_none() {
        let expected_original_hash = journal
            .original_sha256
            .as_deref()
            .context("environment recovery journal is missing the original fingerprint")?;
        anyhow::ensure!(
            current_bytes
                .as_deref()
                .is_some_and(|bytes| sha256(bytes) == expected_original_hash),
            "captured environment backup is missing or was replaced"
        );
        journal_file.delete()?;
        if let Some(snapshot) = owned_managed_snapshot {
            snapshot.delete()?;
        }
        let _ = cleanup_transaction_files(&paths, false);
        return Ok(EnvironmentRestoreOutcome::Restored);
    }
    let (original_existed, original_bytes) = original_state(&journal, backup.as_mut())?;
    if original_existed && current_bytes.is_none() && backup.is_some() {
        let mut backup = backup.take().expect("checked backup presence");
        anyhow::ensure!(
            backup.read_all()? == original_bytes,
            "captured environment backup ownership changed"
        );
        backup.rename_to(&journal.target_path)?;
        journal_file.delete()?;
        if let Some(snapshot) = owned_managed_snapshot {
            snapshot.delete()?;
        }
        let _ = cleanup_transaction_files(&paths, false);
        return Ok(EnvironmentRestoreOutcome::Restored);
    }
    let current_matches_managed = current_bytes
        .as_deref()
        .is_some_and(|bytes| sha256(bytes) == journal.managed_sha256);

    let restore_already_completed = match (original_existed, current_bytes.as_deref()) {
        (true, Some(current)) => current == original_bytes,
        (false, None) => true,
        _ => false,
    };
    if restore_already_completed {
        journal_file.delete()?;
        if let Some(backup) = backup {
            backup.delete()?;
        }
        if let Some(snapshot) = owned_managed_snapshot {
            snapshot.delete()?;
        }
        let _ = cleanup_transaction_files(&paths, false);
        return Ok(EnvironmentRestoreOutcome::Restored);
    }

    if current_matches_managed {
        return restore_owned_target(
            &journal,
            &paths,
            original_existed,
            &original_bytes,
            current.expect("managed bytes require a pinned current file"),
            backup,
            journal_file,
            owned_managed_snapshot,
        );
    }

    preserve_conflict(
        &paths,
        &journal,
        &original_bytes,
        current_bytes.as_deref(),
        journal_file,
        owned_managed_snapshot,
    )
}

fn restore_owned_target(
    journal: &EnvironmentJournal,
    paths: &TransactionPaths,
    original_existed: bool,
    original_bytes: &[u8],
    mut captured: crate::admin_secure_io::SecureFileLease,
    backup: Option<crate::admin_secure_io::SecureFileLease>,
    journal_file: crate::admin_secure_io::SecureFileLease,
    managed_snapshot: Option<crate::admin_secure_io::SecureFileLease>,
) -> anyhow::Result<EnvironmentRestoreOutcome> {
    let captured_bytes = captured
        .read_all()
        .context("failed to verify the captured managed environment")?;
    if sha256(&captured_bytes) != journal.managed_sha256 {
        return preserve_conflict(
            paths,
            journal,
            original_bytes,
            Some(&captured_bytes),
            journal_file,
            managed_snapshot,
        );
    }
    captured
        .rename_to(&paths.restoring)
        .context("failed to capture the managed environment during restore")?;

    if original_existed {
        let mut original_stage =
            crate::admin_secure_io::SecureFileLease::create(&paths.original_stage)
                .context("failed to stage the original environment file")?;
        original_stage
            .replace_contents(original_bytes)
            .context("failed to stage the original environment file")?;
        if original_stage.rename_to(&journal.target_path).is_err() {
            let _ = original_stage.delete();
            let current = read_optional_file(
                &journal.target_path,
                "failed to read an intervening environment edit",
            )?;
            captured.delete()?;
            return preserve_conflict(
                paths,
                journal,
                original_bytes,
                current.as_deref(),
                journal_file,
                managed_snapshot,
            );
        }
    } else if read_optional_file(
        &journal.target_path,
        "failed to verify the restored environment path",
    )?
    .is_some()
    {
        let current = read_optional_file(
            &journal.target_path,
            "failed to read an intervening environment edit",
        )?;
        captured.delete()?;
        return preserve_conflict(
            paths,
            journal,
            original_bytes,
            current.as_deref(),
            journal_file,
            managed_snapshot,
        );
    }

    captured.delete()?;
    journal_file.delete()?;
    if let Some(backup) = backup {
        backup.delete()?;
    }
    if let Some(snapshot) = managed_snapshot {
        snapshot.delete()?;
    }
    let _ = cleanup_transaction_files(paths, false);
    Ok(EnvironmentRestoreOutcome::Restored)
}

fn preserve_conflict(
    paths: &TransactionPaths,
    journal: &EnvironmentJournal,
    original_bytes: &[u8],
    current_bytes: Option<&[u8]>,
    journal_file: crate::admin_secure_io::SecureFileLease,
    managed_snapshot: Option<crate::admin_secure_io::SecureFileLease>,
) -> anyhow::Result<EnvironmentRestoreOutcome> {
    let mut managed_snapshot = match managed_snapshot {
        Some(snapshot) => snapshot,
        None => crate::admin_secure_io::SecureFileLease::open(&paths.managed_snapshot, false)
            .context("failed to pin the managed environment snapshot")?,
    };
    let managed_bytes = managed_snapshot
        .read_all()
        .context("failed to read the managed environment snapshot")?;
    if sha256(&managed_bytes) != journal.managed_sha256 {
        bail!("managed environment snapshot does not match its journal");
    }
    crate::admin_secure_io::create_new(&paths.original_recovery, &original_bytes)
        .context("failed to preserve the original environment recovery copy")?;
    let mut conflicting_paths = Vec::new();
    if let Some(current_bytes) = current_bytes {
        crate::admin_secure_io::create_new(&paths.conflicting_recovery, current_bytes)
            .context("failed to preserve the conflicting environment recovery copy")?;
        conflicting_paths.push(paths.conflicting_recovery.clone());
    }
    if let Some(captured_bytes) = read_optional_file(
        &paths.restoring,
        "failed to read the captured conflicting environment edit",
    )? && sha256(&captured_bytes) != journal.managed_sha256
    {
        crate::admin_secure_io::create_new(&paths.captured_conflicting_recovery, &captured_bytes)
            .context("failed to preserve the captured conflicting environment recovery copy")?;
        conflicting_paths.push(paths.captured_conflicting_recovery.clone());
    }
    journal_file.delete()?;
    let _ = cleanup_transaction_files(paths, true);
    Ok(EnvironmentRestoreOutcome::Conflict {
        original_path: paths.original_recovery.clone(),
        managed_path: paths.managed_snapshot.clone(),
        conflicting_paths,
    })
}

fn require_missing_journal(path: &Path) -> anyhow::Result<()> {
    match fs::metadata(path) {
        Ok(_) => bail!("administrator environment recovery is still pending"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to inspect the environment recovery journal"),
    }
}

fn acquire_guard(state_dir: &Path) -> anyhow::Result<EnvironmentGuard> {
    crate::admin_secure_io::ensure_directory(state_dir)
        .context("failed to create administrator environment state")?;
    let guard_path = state_dir.join(GUARD_FILE_NAME);
    let file = match crate::admin_secure_io::SecureFileLease::open(&guard_path, true) {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => {
            crate::admin_secure_io::SecureFileLease::create(&guard_path)?
        }
        Err(error) => return Err(error),
    };
    file.as_file()
        .try_lock_exclusive()
        .context("administrator environment transaction is already active")?;
    Ok(EnvironmentGuard { file })
}

fn read_optional_file(path: &Path, message: &'static str) -> anyhow::Result<Option<Vec<u8>>> {
    match open_optional_file(path, false)? {
        Some(mut file) => file.read_all().map(Some).context(message),
        None => Ok(None),
    }
}

fn open_optional_file(
    path: &Path,
    writable: bool,
) -> anyhow::Result<Option<crate::admin_secure_io::SecureFileLease>> {
    match crate::admin_secure_io::SecureFileLease::open(path, writable) {
        Ok(file) => Ok(Some(file)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn serialize_journal(journal: &EnvironmentJournal) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec_pretty(journal).context("failed to serialize environment journal")
}

fn publish_journal(
    paths: &TransactionPaths,
    journal: &EnvironmentJournal,
) -> anyhow::Result<crate::admin_secure_io::SecureFileLease> {
    let bytes = serialize_journal(journal)?;
    let mut stage = crate::admin_secure_io::SecureFileLease::create(&paths.journal_stage)
        .context("failed to stage environment recovery journal")?;
    stage
        .replace_contents(&bytes)
        .context("failed to stage environment recovery journal")?;
    if let Err(error) = stage.rename_to(&paths.journal) {
        let _ = stage.delete();
        return Err(error).context("failed to acquire environment recovery journal ownership");
    }
    Ok(stage)
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|source| source.downcast_ref::<std::io::Error>())
        .any(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn original_state(
    journal: &EnvironmentJournal,
    backup: Option<&mut crate::admin_secure_io::SecureFileLease>,
) -> anyhow::Result<(bool, Vec<u8>)> {
    if let Some(backup) = backup {
        let bytes = backup
            .read_all()
            .context("failed to read the captured environment backup")?;
        let expected = journal
            .original_sha256
            .as_deref()
            .context("environment recovery journal is missing the original fingerprint")?;
        anyhow::ensure!(
            sha256(&bytes) == expected,
            "captured environment backup ownership changed"
        );
        return Ok((true, bytes));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&journal.original_bytes)
        .context("environment recovery journal contains invalid backup bytes")?;
    Ok((journal.original_existed, bytes))
}

fn restore_backup_after_failed_install(
    journal: &EnvironmentJournal,
    paths: &TransactionPaths,
    backup: Option<&mut crate::admin_secure_io::SecureFileLease>,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(backup) = backup else {
        return Ok(None);
    };
    let mut intervening_file = open_optional_file(&journal.target_path, true)
        .context("failed to inspect the environment target after install failure")?;
    let intervening = if let Some(intervening_file) = intervening_file.as_mut() {
        intervening_file
            .read_all()
            .context("failed to read an intervening environment edit")?;
        intervening_file
            .rename_to(&paths.intervening)
            .with_context(|| {
                format!(
                    "failed to preserve an intervening environment edit at {}",
                    paths.intervening.display()
                )
            })?;
        Some(paths.intervening.clone())
    } else {
        None
    };
    if let Err(error) = backup.read_all() {
        if let Some(intervening) = intervening.as_deref() {
            bail!(
                "failed to validate the environment backup after install failure; intervening edit preserved at {}: {error}",
                intervening.display()
            );
        }
        return Err(error).context("failed to read the environment backup after install failure");
    }
    if let Err(error) = backup.rename_to(&journal.target_path) {
        if let Some(intervening) = intervening.as_deref() {
            bail!(
                "failed to restore the environment backup after install failure; intervening edit preserved at {}: {error}",
                intervening.display()
            );
        }
        return Err(error)
            .context("failed to restore the environment backup after install failure");
    }
    Ok(intervening)
}

fn transaction_paths(
    codex_home: &Path,
    state_dir: &Path,
    transaction_id: &str,
) -> TransactionPaths {
    TransactionPaths {
        journal: journal_path(state_dir),
        journal_stage: state_dir.join(format!(
            ".administrator-mode-environment.{transaction_id}.journal"
        )),
        backup: codex_home.join(format!(
            ".administrator-mode-environment.{transaction_id}.backup"
        )),
        managed_snapshot: state_dir.join(format!(
            "administrator-mode-environment.{transaction_id}.managed.toml"
        )),
        managed_stage: codex_home.join(format!(
            ".administrator-mode-environment.{transaction_id}.managed"
        )),
        restoring: codex_home.join(format!(
            ".administrator-mode-environment.{transaction_id}.restoring"
        )),
        original_stage: codex_home.join(format!(
            ".administrator-mode-environment.{transaction_id}.original"
        )),
        intervening: codex_home.join(format!(
            ".administrator-mode-environment.{transaction_id}.intervening"
        )),
        original_recovery: state_dir.join(format!(
            "administrator-mode-environment.{transaction_id}.original.toml"
        )),
        conflicting_recovery: state_dir.join(format!(
            "administrator-mode-environment.{transaction_id}.conflicting.toml"
        )),
        captured_conflicting_recovery: state_dir.join(format!(
            "administrator-mode-environment.{transaction_id}.captured-conflicting.toml"
        )),
    }
}

fn validate_journal(
    codex_home: &Path,
    state_dir: &Path,
    journal: &EnvironmentJournal,
) -> anyhow::Result<TransactionPaths> {
    if journal.schema_version != 2 {
        bail!("unsupported environment recovery journal version");
    }
    let transaction_id = uuid::Uuid::parse_str(&journal.transaction_id)
        .context("environment recovery journal has an invalid transaction id")?;
    let canonical_id = transaction_id.simple().to_string();
    if canonical_id != journal.transaction_id {
        bail!("environment recovery journal has a non-canonical transaction id");
    }
    let paths = transaction_paths(codex_home, state_dir, &canonical_id);
    if journal.target_path != codex_home.join("environments.toml")
        || journal.backup_path != paths.backup
        || journal.managed_stage_path != paths.managed_stage
        || journal.managed_snapshot_path != paths.managed_snapshot
    {
        bail!("environment recovery journal contains an unexpected path");
    }
    Ok(paths)
}

fn cleanup_transaction_files(
    paths: &TransactionPaths,
    keep_managed_snapshot: bool,
) -> anyhow::Result<()> {
    remove_if_exists(&paths.backup)?;
    remove_if_exists(&paths.managed_stage)?;
    remove_if_exists(&paths.restoring)?;
    remove_if_exists(&paths.original_stage)?;
    remove_if_exists(&paths.journal_stage)?;
    if !keep_managed_snapshot {
        remove_if_exists(&paths.managed_snapshot)?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    match crate::admin_secure_io::SecureFileLease::open_for_delete(path) {
        Ok(file) => file
            .delete()
            .with_context(|| format!("failed to remove {}", path.display())),
        Err(error) if is_not_found(&error) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn journal_path(state_dir: &Path) -> PathBuf {
    state_dir.join(JOURNAL_FILE_NAME)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
