use codex_plus_core::admin_mode::feature::{
    AdminUnifiedExecTransaction, UnifiedExecRestoreOutcome, recover_stale_unified_exec,
};
use std::fs;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    codex_home: std::path::PathBuf,
    state_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let codex_home = temp.path().join("codex-home");
        let state_dir = temp.path().join("state");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        Self {
            _temp: temp,
            codex_home,
            state_dir,
        }
    }

    fn config(&self) -> std::path::PathBuf {
        self.codex_home.join("config.toml")
    }

    fn journal(&self) -> std::path::PathBuf {
        self.state_dir
            .join("administrator-mode-unified-exec.v1.json")
    }

    fn install(&self) -> AdminUnifiedExecTransaction {
        AdminUnifiedExecTransaction::install(&self.codex_home, &self.state_dir).unwrap()
    }
}

fn unified_exec(config: &std::path::Path) -> Option<bool> {
    let text = fs::read_to_string(config).unwrap();
    let value: toml::Value = toml::from_str(&text).unwrap();
    value
        .get("features")
        .and_then(|features| features.get("unified_exec"))
        .and_then(toml::Value::as_bool)
}

#[test]
fn install_enables_unified_exec_and_clean_restore_is_byte_exact() {
    let fixture = Fixture::new();
    let original = b"# preserve comments and formatting\r\nmodel_provider = \"custom\"\r\n\r\n[features]\r\nshell_tool = true\r\n";
    fs::write(fixture.config(), original).unwrap();

    let transaction = fixture.install();

    assert_eq!(unified_exec(&fixture.config()), Some(true));
    assert!(fixture.journal().is_file());
    assert_eq!(
        transaction.restore().unwrap(),
        UnifiedExecRestoreOutcome::Restored
    );
    assert_eq!(fs::read(fixture.config()).unwrap(), original);
    assert!(!fixture.journal().exists());
}

#[test]
fn restore_preserves_provider_and_api_key_edits_made_while_admin_mode_is_active() {
    let fixture = Fixture::new();
    fs::write(
        fixture.config(),
        "model_provider = \"first\"\n[features]\nunified_exec = false\n",
    )
    .unwrap();
    let transaction = fixture.install();

    fs::write(
        fixture.config(),
        "model_provider = \"second\"\napi_key_marker = \"must-survive\"\n[features]\nunified_exec = true\nshell_tool = true\n",
    )
    .unwrap();

    assert_eq!(
        transaction.restore().unwrap(),
        UnifiedExecRestoreOutcome::Restored
    );
    let restored = fs::read_to_string(fixture.config()).unwrap();
    assert!(restored.contains("model_provider = \"second\""));
    assert!(restored.contains("api_key_marker = \"must-survive\""));
    assert!(restored.contains("shell_tool = true"));
    assert_eq!(unified_exec(&fixture.config()), Some(false));
}

#[test]
fn restore_removes_only_the_inserted_feature_from_an_externally_edited_config() {
    let fixture = Fixture::new();
    fs::write(fixture.config(), "model = \"before\"\n").unwrap();
    let transaction = fixture.install();

    fs::write(
        fixture.config(),
        "model = \"after\"\n[features]\nunified_exec = true\nshell_snapshot = true\n",
    )
    .unwrap();

    transaction.restore().unwrap();
    let restored = fs::read_to_string(fixture.config()).unwrap();
    assert!(restored.contains("model = \"after\""));
    assert!(restored.contains("shell_snapshot = true"));
    assert_eq!(unified_exec(&fixture.config()), None);
}

#[test]
fn existing_true_value_is_left_byte_exact_and_needs_no_journal() {
    let fixture = Fixture::new();
    let original = b"[features]\nunified_exec = true\n";
    fs::write(fixture.config(), original).unwrap();

    let transaction = fixture.install();

    assert_eq!(fs::read(fixture.config()).unwrap(), original);
    assert!(!fixture.journal().exists());
    assert_eq!(
        transaction.restore().unwrap(),
        UnifiedExecRestoreOutcome::NoJournal
    );
    assert_eq!(fs::read(fixture.config()).unwrap(), original);
}

#[test]
fn clean_restore_removes_config_when_admin_mode_created_it() {
    let fixture = Fixture::new();

    let transaction = fixture.install();

    assert_eq!(unified_exec(&fixture.config()), Some(true));
    assert_eq!(
        transaction.restore().unwrap(),
        UnifiedExecRestoreOutcome::Restored
    );
    assert!(!fixture.config().exists());
    assert!(!fixture.journal().exists());
}

#[test]
fn stale_recovery_reverts_the_feature_without_copying_config_secrets_to_the_journal() {
    let fixture = Fixture::new();
    let secret = "sk-test-secret-must-not-enter-journal";
    fs::write(
        fixture.config(),
        format!("api_key_marker = \"{secret}\"\n[features]\nunified_exec = false\n"),
    )
    .unwrap();
    let transaction = fixture.install();
    let journal = fs::read(&fixture.journal()).unwrap();
    assert!(
        !journal
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
    );
    drop(transaction);

    assert_eq!(
        recover_stale_unified_exec(&fixture.codex_home, &fixture.state_dir).unwrap(),
        UnifiedExecRestoreOutcome::Restored
    );
    assert_eq!(unified_exec(&fixture.config()), Some(false));
    assert!(!fixture.journal().exists());
}

#[test]
fn invalid_config_fails_before_writing_a_journal_or_changing_bytes() {
    let fixture = Fixture::new();
    let invalid = b"[features\nunified_exec = false\n";
    fs::write(fixture.config(), invalid).unwrap();

    assert!(AdminUnifiedExecTransaction::install(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(fixture.config()).unwrap(), invalid);
    assert!(!fixture.journal().exists());
}

#[cfg(windows)]
#[test]
fn install_rejects_a_hardlinked_config_without_copying_its_secret() {
    let fixture = Fixture::new();
    let outside = fixture._temp.path().join("outside-secret.toml");
    let secret = b"api_key_marker = \"must-not-be-copied\"\n";
    fs::write(&outside, secret).unwrap();
    fs::hard_link(&outside, fixture.config()).unwrap();

    assert!(AdminUnifiedExecTransaction::install(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(&outside).unwrap(), secret);
    assert_eq!(fs::read(fixture.config()).unwrap(), secret);
    for entry in fs::read_dir(&fixture.state_dir).unwrap().flatten() {
        if entry.path().is_file() {
            assert!(
                !fs::read(entry.path())
                    .unwrap()
                    .windows(secret.len())
                    .any(|window| window == secret)
            );
        }
    }
}
