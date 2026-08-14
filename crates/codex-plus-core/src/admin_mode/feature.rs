use anyhow::{Context, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

const JOURNAL_FILE_NAME: &str = "administrator-mode-unified-exec.v1.json";
const GUARD_FILE_NAME: &str = "administrator-mode-unified-exec.guard";

pub struct AdminUnifiedExecTransaction {
    codex_home: PathBuf,
    state_dir: PathBuf,
    original_bytes: Option<Vec<u8>>,
    journal: Option<crate::admin_secure_io::SecureFileLease>,
    _guard: FeatureGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedExecRestoreOutcome {
    NoJournal,
    Restored,
}

struct FeatureGuard {
    file: crate::admin_secure_io::SecureFileLease,
}

impl Drop for FeatureGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.file.as_file());
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OriginalFeatureState {
    Missing,
    False,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeatureJournal {
    schema_version: u32,
    target_path: PathBuf,
    original_existed: bool,
    original_feature_state: OriginalFeatureState,
    managed_sha256: String,
    transaction_id: String,
    install_stage_path: PathBuf,
    install_original_path: PathBuf,
    restore_stage_path: PathBuf,
    restore_rollback_path: PathBuf,
}

struct TransactionPaths {
    journal: PathBuf,
    journal_stage: PathBuf,
    install_stage: PathBuf,
    install_original: PathBuf,
    restore_stage: PathBuf,
    restore_rollback: PathBuf,
}

impl AdminUnifiedExecTransaction {
    pub fn install(codex_home: &Path, state_dir: &Path) -> anyhow::Result<Self> {
        let guard = acquire_guard(state_dir)?;
        let journal_path = state_dir.join(JOURNAL_FILE_NAME);
        require_missing_journal(&journal_path)?;
        crate::admin_secure_io::ensure_directory(codex_home)
            .context("failed to create CODEX_HOME for administrator command routing")?;

        let target_path = codex_home.join("config.toml");
        let mut current = open_optional(&target_path, true)
            .context("failed to securely open config.toml for administrator command routing")?;
        let original_bytes = current
            .as_mut()
            .map(crate::admin_secure_io::SecureFileLease::read_all)
            .transpose()
            .context("failed to read config.toml for administrator command routing")?;
        let (mut document, bom) = parse_document(original_bytes.as_deref().unwrap_or_default())?;
        let original_feature_state = match read_unified_exec(&document)? {
            Some(true) => {
                return Ok(Self {
                    codex_home: codex_home.to_path_buf(),
                    state_dir: state_dir.to_path_buf(),
                    original_bytes,
                    journal: None,
                    _guard: guard,
                });
            }
            Some(false) => OriginalFeatureState::False,
            None => OriginalFeatureState::Missing,
        };

        set_unified_exec(&mut document, true)?;
        let managed_bytes = serialize_document(document, bom);
        let transaction_id = uuid::Uuid::new_v4().simple().to_string();
        let paths = transaction_paths(codex_home, state_dir, &transaction_id);
        let journal = FeatureJournal {
            schema_version: 1,
            target_path: target_path.clone(),
            original_existed: original_bytes.is_some(),
            original_feature_state,
            managed_sha256: sha256(&managed_bytes),
            transaction_id,
            install_stage_path: paths.install_stage.clone(),
            install_original_path: paths.install_original.clone(),
            restore_stage_path: paths.restore_stage.clone(),
            restore_rollback_path: paths.restore_rollback.clone(),
        };
        let journal_file = publish_journal(&paths, &journal)?;

        if let Err(error) = replace_target(
            &target_path,
            current.take(),
            &managed_bytes,
            &paths.install_stage,
            &paths.install_original,
        ) {
            let _ = journal_file.delete();
            let _ = cleanup_paths(&paths);
            return Err(error).context("failed to enable unified administrator command routing");
        }

        Ok(Self {
            codex_home: codex_home.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            original_bytes,
            journal: Some(journal_file),
            _guard: guard,
        })
    }

    pub fn restore(mut self) -> anyhow::Result<UnifiedExecRestoreOutcome> {
        let Some(journal) = self.journal.take() else {
            return Ok(UnifiedExecRestoreOutcome::NoJournal);
        };
        restore_from_journal(
            &self.codex_home,
            &self.state_dir,
            Some(journal),
            Some(self.original_bytes.as_deref()),
        )
    }
}

pub fn recover_stale_unified_exec(
    codex_home: &Path,
    state_dir: &Path,
) -> anyhow::Result<UnifiedExecRestoreOutcome> {
    let _guard = acquire_guard(state_dir)?;
    restore_from_journal(codex_home, state_dir, None, None)
}

fn restore_from_journal(
    codex_home: &Path,
    state_dir: &Path,
    owned_journal: Option<crate::admin_secure_io::SecureFileLease>,
    clean_original: Option<Option<&[u8]>>,
) -> anyhow::Result<UnifiedExecRestoreOutcome> {
    let journal_path = state_dir.join(JOURNAL_FILE_NAME);
    let mut journal_file = match owned_journal {
        Some(file) => file,
        None => match crate::admin_secure_io::SecureFileLease::open_for_delete(&journal_path) {
            Ok(file) => file,
            Err(error) if is_not_found(&error) => {
                return Ok(UnifiedExecRestoreOutcome::NoJournal);
            }
            Err(error) => return Err(error).context("failed to open unified exec journal"),
        },
    };
    let journal: FeatureJournal = serde_json::from_slice(
        &journal_file
            .read_all()
            .context("failed to read unified exec journal")?,
    )
    .context("failed to parse unified exec journal")?;
    let paths = validate_journal(codex_home, state_dir, &journal)?;

    recover_interrupted_replace(&journal.target_path, &paths)?;
    let mut current = open_optional(&journal.target_path, true)
        .context("failed to securely open config.toml during unified exec restore")?;
    let current_bytes = current
        .as_mut()
        .map(crate::admin_secure_io::SecureFileLease::read_all)
        .transpose()
        .context("failed to read config.toml during unified exec restore")?;

    match current_bytes {
        None if !journal.original_existed => {}
        None => bail!("config.toml disappeared while unified exec restoration was pending"),
        Some(ref bytes) if sha256(bytes) == journal.managed_sha256 => {
            if let Some(original) = clean_original {
                if let Some(original) = original {
                    replace_target(
                        &journal.target_path,
                        current.take(),
                        original,
                        &paths.restore_stage,
                        &paths.restore_rollback,
                    )?;
                } else if let Some(file) = current.take() {
                    file.delete()?;
                }
            } else {
                restore_semantically(&journal, &paths, current.take(), bytes)?;
            }
        }
        Some(ref bytes) => {
            restore_semantically(&journal, &paths, current.take(), bytes)?;
        }
    }

    journal_file.delete()?;
    cleanup_paths(&paths)?;
    Ok(UnifiedExecRestoreOutcome::Restored)
}

fn restore_semantically(
    journal: &FeatureJournal,
    paths: &TransactionPaths,
    current: Option<crate::admin_secure_io::SecureFileLease>,
    current_bytes: &[u8],
) -> anyhow::Result<()> {
    let (mut document, bom) = parse_document(current_bytes)
        .context("current config.toml is invalid; refusing to overwrite user changes")?;
    if read_unified_exec(&document)? != Some(true) {
        return Ok(());
    }
    match journal.original_feature_state {
        OriginalFeatureState::Missing => remove_unified_exec(&mut document)?,
        OriginalFeatureState::False => set_unified_exec(&mut document, false)?,
    }
    let restored = serialize_document(document, bom);
    replace_target(
        &journal.target_path,
        current,
        &restored,
        &paths.restore_stage,
        &paths.restore_rollback,
    )
}

fn recover_interrupted_replace(target: &Path, paths: &TransactionPaths) -> anyhow::Result<()> {
    let target_exists = open_optional(target, false)?.is_some();
    if !target_exists {
        if let Some(mut rollback) = open_optional(&paths.restore_rollback, true)? {
            rollback
                .rename_to(target)
                .context("failed to roll back an interrupted unified exec restore")?;
        } else if let Some(mut original) = open_optional(&paths.install_original, true)? {
            original
                .rename_to(target)
                .context("failed to roll back an interrupted unified exec install")?;
        }
    }
    remove_if_exists(&paths.install_stage)?;
    remove_if_exists(&paths.restore_stage)?;
    Ok(())
}

fn replace_target(
    target: &Path,
    mut current: Option<crate::admin_secure_io::SecureFileLease>,
    bytes: &[u8],
    stage_path: &Path,
    rollback_path: &Path,
) -> anyhow::Result<()> {
    remove_if_exists(stage_path)?;
    remove_if_exists(rollback_path)?;
    let mut stage = crate::admin_secure_io::SecureFileLease::create(stage_path)
        .context("failed to create config.toml replacement stage")?;
    if let Err(error) = stage.replace_contents(bytes) {
        let _ = stage.delete();
        return Err(error);
    }
    if let Some(file) = current.as_mut()
        && let Err(error) = file.rename_to(rollback_path)
    {
        let _ = stage.delete();
        return Err(error).context("failed to capture config.toml before replacement");
    }
    if let Err(error) = stage.rename_to(target) {
        let _ = stage.delete();
        if let Some(file) = current.as_mut() {
            let _ = file.rename_to(target);
        }
        return Err(error).context("failed to publish config.toml replacement");
    }
    drop(stage);
    if let Some(file) = current {
        file.delete()
            .context("failed to remove the superseded config.toml object")?;
    }
    Ok(())
}

fn parse_document(bytes: &[u8]) -> anyhow::Result<(DocumentMut, bool)> {
    let (bytes, bom) = match bytes.strip_prefix(b"\xef\xbb\xbf") {
        Some(bytes) => (bytes, true),
        None => (bytes, false),
    };
    let text = std::str::from_utf8(bytes).context("config.toml is not valid UTF-8")?;
    let document = if text.trim().is_empty() {
        DocumentMut::new()
    } else {
        text.parse::<DocumentMut>()
            .context("config.toml TOML parse failed")?
    };
    Ok((document, bom))
}

fn serialize_document(document: DocumentMut, bom: bool) -> Vec<u8> {
    let mut bytes = document.to_string().into_bytes();
    if bom {
        let mut with_bom = Vec::with_capacity(bytes.len() + 3);
        with_bom.extend_from_slice(b"\xef\xbb\xbf");
        with_bom.append(&mut bytes);
        with_bom
    } else {
        bytes
    }
}

fn read_unified_exec(document: &DocumentMut) -> anyhow::Result<Option<bool>> {
    let Some(features) = document.get("features") else {
        return Ok(None);
    };
    let features = features
        .as_table_like()
        .context("features must be a TOML table")?;
    let Some(value) = features.get("unified_exec") else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .context("features.unified_exec must be a boolean")
}

fn features_mut(document: &mut DocumentMut) -> anyhow::Result<&mut dyn toml_edit::TableLike> {
    if !document.as_table().contains_key("features") {
        document["features"] = Item::Table(Table::new());
    }
    document
        .get_mut("features")
        .and_then(Item::as_table_like_mut)
        .context("features must be a TOML table")
}

fn set_unified_exec(document: &mut DocumentMut, enabled: bool) -> anyhow::Result<()> {
    features_mut(document)?.insert("unified_exec", toml_edit::value(enabled));
    Ok(())
}

fn remove_unified_exec(document: &mut DocumentMut) -> anyhow::Result<()> {
    let Some(features) = document.get_mut("features") else {
        return Ok(());
    };
    let features = features
        .as_table_like_mut()
        .context("features must be a TOML table")?;
    features.remove("unified_exec");
    if features.is_empty() {
        document.as_table_mut().remove("features");
    }
    Ok(())
}

fn acquire_guard(state_dir: &Path) -> anyhow::Result<FeatureGuard> {
    crate::admin_secure_io::ensure_directory(state_dir)
        .context("failed to create unified exec transaction state")?;
    let path = state_dir.join(GUARD_FILE_NAME);
    let file = match crate::admin_secure_io::SecureFileLease::open(&path, true) {
        Ok(file) => file,
        Err(error) if is_not_found(&error) => {
            crate::admin_secure_io::SecureFileLease::create(&path)?
        }
        Err(error) => return Err(error),
    };
    file.as_file()
        .try_lock_exclusive()
        .context("unified exec transaction is already active")?;
    Ok(FeatureGuard { file })
}

fn publish_journal(
    paths: &TransactionPaths,
    journal: &FeatureJournal,
) -> anyhow::Result<crate::admin_secure_io::SecureFileLease> {
    let bytes =
        serde_json::to_vec_pretty(journal).context("failed to serialize unified exec journal")?;
    let mut stage = crate::admin_secure_io::SecureFileLease::create(&paths.journal_stage)
        .context("failed to stage unified exec journal")?;
    stage.replace_contents(&bytes)?;
    if let Err(error) = stage.rename_to(&paths.journal) {
        let _ = stage.delete();
        return Err(error).context("failed to publish unified exec journal");
    }
    Ok(stage)
}

fn require_missing_journal(path: &Path) -> anyhow::Result<()> {
    match std::fs::metadata(path) {
        Ok(_) => bail!("unified exec recovery is still pending"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to inspect unified exec journal"),
    }
}

fn transaction_paths(codex_home: &Path, state_dir: &Path, id: &str) -> TransactionPaths {
    TransactionPaths {
        journal: state_dir.join(JOURNAL_FILE_NAME),
        journal_stage: state_dir.join(format!(".administrator-mode-unified-exec.{id}.journal")),
        install_stage: codex_home.join(format!(".administrator-mode-unified-exec.{id}.managed")),
        install_original: codex_home
            .join(format!(".administrator-mode-unified-exec.{id}.original")),
        restore_stage: codex_home.join(format!(".administrator-mode-unified-exec.{id}.restore")),
        restore_rollback: codex_home
            .join(format!(".administrator-mode-unified-exec.{id}.rollback")),
    }
}

fn validate_journal(
    codex_home: &Path,
    state_dir: &Path,
    journal: &FeatureJournal,
) -> anyhow::Result<TransactionPaths> {
    ensure!(
        journal.schema_version == 1,
        "unsupported unified exec journal version"
    );
    let id = uuid::Uuid::parse_str(&journal.transaction_id)
        .context("unified exec journal has an invalid transaction id")?
        .simple()
        .to_string();
    ensure!(
        id == journal.transaction_id,
        "unified exec journal id is not canonical"
    );
    let paths = transaction_paths(codex_home, state_dir, &id);
    ensure!(
        journal.target_path == codex_home.join("config.toml"),
        "unified exec journal target is invalid"
    );
    ensure!(
        journal.install_stage_path == paths.install_stage,
        "unified exec journal stage is invalid"
    );
    ensure!(
        journal.install_original_path == paths.install_original,
        "unified exec journal original is invalid"
    );
    ensure!(
        journal.restore_stage_path == paths.restore_stage,
        "unified exec journal restore stage is invalid"
    );
    ensure!(
        journal.restore_rollback_path == paths.restore_rollback,
        "unified exec journal rollback is invalid"
    );
    ensure!(
        journal.managed_sha256.len() == 64
            && journal
                .managed_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "unified exec journal hash is invalid"
    );
    Ok(paths)
}

fn cleanup_paths(paths: &TransactionPaths) -> anyhow::Result<()> {
    for path in [
        &paths.journal_stage,
        &paths.install_stage,
        &paths.install_original,
        &paths.restore_stage,
        &paths.restore_rollback,
    ] {
        remove_if_exists(path)?;
    }
    Ok(())
}

fn open_optional(
    path: &Path,
    writable: bool,
) -> anyhow::Result<Option<crate::admin_secure_io::SecureFileLease>> {
    match crate::admin_secure_io::SecureFileLease::open(path, writable) {
        Ok(file) => Ok(Some(file)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    match crate::admin_secure_io::SecureFileLease::open_for_delete(path) {
        Ok(file) => file.delete(),
        Err(error) if is_not_found(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|source| source.downcast_ref::<std::io::Error>())
        .any(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
