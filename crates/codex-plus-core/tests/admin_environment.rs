use codex_plus_core::admin_mode::environment::{
    AdminEnvironmentSpec, AdminEnvironmentTransaction, EnvironmentRestoreOutcome,
    recover_stale_environment,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success());
}

const MANAGED_TOML: &str = r#"default = "codex-plus-admin"
include_local = false

[[environments]]
id = "codex-plus-admin"
program = "C:\\Program Files\\CodexPlusPlus\\codex-plus-admin-shim.exe"
args = ["exec-client", "--pipe", "codex-plus-admin-abc", "--session", "session-123", "--proof-file", "C:\\Users\\me\\AppData\\Local\\CodexPlusPlus\\admin-session.proof"]
"#;

struct Fixture {
    _temp: TempDir,
    codex_home: PathBuf,
    state_dir: PathBuf,
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

    fn target(&self) -> PathBuf {
        self.codex_home.join("environments.toml")
    }

    fn journal(&self) -> PathBuf {
        self.state_dir
            .join("administrator-mode-environment.v1.json")
    }

    fn install(&self) -> AdminEnvironmentTransaction {
        AdminEnvironmentTransaction::install(
            &self.codex_home,
            &self.state_dir,
            &AdminEnvironmentSpec {
                shim_path: Path::new(r"C:\Program Files\CodexPlusPlus\codex-plus-admin-shim.exe"),
                pipe_name: "codex-plus-admin-abc",
                session_id: "session-123",
                proof_path: Path::new(
                    r"C:\Users\me\AppData\Local\CodexPlusPlus\admin-session.proof",
                ),
            },
        )
        .unwrap()
    }
}

#[test]
fn install_writes_exact_managed_toml_and_journal() {
    let fixture = Fixture::new();

    let _transaction = fixture.install();

    assert_eq!(fs::read(fixture.target()).unwrap(), MANAGED_TOML.as_bytes());
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    assert_eq!(journal["schemaVersion"], 2);
    assert_eq!(
        journal["targetPath"],
        fixture.target().to_string_lossy().as_ref()
    );
    assert_eq!(journal["originalExisted"], false);
    assert_eq!(journal["originalBytes"], "");
    assert!(journal["originalSha256"].is_null());
    assert_eq!(journal["managedSha256"].as_str().unwrap().len(), 64);
    assert!(!journal["transactionId"].as_str().unwrap().is_empty());
    let managed_snapshot = PathBuf::from(journal["managedSnapshotPath"].as_str().unwrap());
    assert_eq!(fs::read(managed_snapshot).unwrap(), MANAGED_TOML.as_bytes());
    assert!(journal["backupPath"].as_str().is_some());
}

#[cfg(windows)]
#[test]
fn install_ignores_a_prepositioned_predictable_journal_tmp_reparse_point() {
    let fixture = Fixture::new();
    let outside = fixture._temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    junction(
        &fixture
            .state_dir
            .join("administrator-mode-environment.v1.json.tmp"),
        &outside,
    );

    let transaction = fixture.install();
    assert_eq!(fs::read(fixture.target()).unwrap(), MANAGED_TOML.as_bytes());
    assert!(outside.is_dir());
    transaction.restore().unwrap();
}

#[cfg(windows)]
#[test]
fn install_rejects_a_reparse_state_directory_without_writing_outside() {
    let temp = tempfile::tempdir().unwrap();
    let codex_home = temp.path().join("codex-home");
    let outside = temp.path().join("outside");
    let state_dir = temp.path().join("state");
    fs::create_dir(&codex_home).unwrap();
    fs::create_dir(&outside).unwrap();
    junction(&state_dir, &outside);

    let result = AdminEnvironmentTransaction::install(
        &codex_home,
        &state_dir,
        &AdminEnvironmentSpec {
            shim_path: Path::new(r"C:\Program Files\CodexPlusPlus\codex-plus-admin-shim.exe"),
            pipe_name: "codex-plus-admin-abc",
            session_id: "session-123",
            proof_path: Path::new(r"C:\proof"),
        },
    );
    assert!(result.is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}

#[cfg(windows)]
#[test]
fn install_rejects_a_hardlinked_environment_without_copying_external_secret() {
    let fixture = Fixture::new();
    let outside = fixture._temp.path().join("outside-secret.toml");
    let secret = b"api_key = 'must-not-be-copied'\n";
    fs::write(&outside, secret).unwrap();
    fs::hard_link(&outside, fixture.target()).unwrap();

    let result = AdminEnvironmentTransaction::install(
        &fixture.codex_home,
        &fixture.state_dir,
        &AdminEnvironmentSpec {
            shim_path: Path::new(r"C:\Program Files\CodexPlusPlus\codex-plus-admin-shim.exe"),
            pipe_name: "codex-plus-admin-abc",
            session_id: "session-123",
            proof_path: Path::new(r"C:\proof"),
        },
    );

    assert!(result.is_err());
    assert_eq!(fs::read(&outside).unwrap(), secret);
    assert_eq!(fs::read(fixture.target()).unwrap(), secret);
    for entry in fs::read_dir(&fixture.state_dir).unwrap().flatten() {
        if entry.path().is_file() {
            assert!(
                !fs::read(entry.path())
                    .unwrap()
                    .windows(secret.len())
                    .any(|w| w == secret)
            );
        }
    }
}

#[test]
fn restore_preserves_original_bytes_exactly() {
    let fixture = Fixture::new();
    let original = b"# user bytes\r\ndefault = 'local'\r\n\x00";
    fs::write(fixture.target(), original).unwrap();
    let transaction = fixture.install();

    let outcome = transaction.restore().unwrap();

    assert_eq!(outcome, EnvironmentRestoreOutcome::Restored);
    assert_eq!(fs::read(fixture.target()).unwrap(), original);
    assert!(!fixture.journal().exists());
}

#[test]
fn published_journal_is_not_rewritten_after_the_original_is_captured() {
    let fixture = Fixture::new();
    let original = b"original bytes are authoritative in the captured backup\r\n";
    fs::write(fixture.target(), original).unwrap();

    let transaction = fixture.install();
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    assert_eq!(journal["originalExisted"], true);
    assert_eq!(journal["originalBytes"], "");
    assert_eq!(journal["originalSha256"].as_str().unwrap().len(), 64);
    let backup = PathBuf::from(journal["backupPath"].as_str().unwrap());
    assert_eq!(fs::read(backup).unwrap(), original);

    transaction.restore().unwrap();
    assert_eq!(fs::read(fixture.target()).unwrap(), original);
}

#[cfg(windows)]
#[test]
fn active_transaction_keeps_the_captured_backup_pinned_against_replacement() {
    let fixture = Fixture::new();
    let original = b"pinned-original";
    fs::write(fixture.target(), original).unwrap();
    let transaction = fixture.install();
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    let backup = PathBuf::from(journal["backupPath"].as_str().unwrap());
    let replacement = fixture._temp.path().join("replacement.toml");
    fs::write(&replacement, b"attacker").unwrap();

    assert!(fs::write(&backup, b"attacker").is_err());
    assert!(fs::rename(&replacement, &backup).is_err());

    transaction.restore().unwrap();
    assert_eq!(fs::read(fixture.target()).unwrap(), original);
    assert_eq!(fs::read(replacement).unwrap(), b"attacker");
}

#[cfg(windows)]
#[test]
fn active_transaction_keeps_the_managed_target_pinned_against_replacement() {
    let fixture = Fixture::new();
    let transaction = fixture.install();
    let replacement = fixture._temp.path().join("managed-replacement.toml");
    fs::write(&replacement, b"attacker").unwrap();

    assert!(fs::write(fixture.target(), b"attacker").is_err());
    assert!(fs::rename(&replacement, fixture.target()).is_err());

    transaction.restore().unwrap();
    assert!(!fixture.target().exists());
    assert_eq!(fs::read(replacement).unwrap(), b"attacker");
}

#[cfg(windows)]
#[test]
fn active_transaction_keeps_journal_and_snapshot_pinned_against_replacement() {
    let fixture = Fixture::new();
    let transaction = fixture.install();
    let journal_bytes = fs::read(fixture.journal()).unwrap();
    let journal: serde_json::Value = serde_json::from_slice(&journal_bytes).unwrap();
    let snapshot = PathBuf::from(journal["managedSnapshotPath"].as_str().unwrap());
    let journal_replacement = fixture._temp.path().join("journal-replacement.json");
    let snapshot_replacement = fixture._temp.path().join("snapshot-replacement.toml");
    fs::write(&journal_replacement, b"attacker").unwrap();
    fs::write(&snapshot_replacement, b"attacker").unwrap();

    assert!(fs::write(fixture.journal(), b"attacker").is_err());
    assert!(fs::rename(&journal_replacement, fixture.journal()).is_err());
    assert!(fs::write(&snapshot, b"attacker").is_err());
    assert!(fs::rename(&snapshot_replacement, &snapshot).is_err());

    transaction.restore().unwrap();
    assert_eq!(fs::read(journal_replacement).unwrap(), b"attacker");
    assert_eq!(fs::read(snapshot_replacement).unwrap(), b"attacker");
}

#[cfg(windows)]
#[test]
fn stale_recovery_rejects_a_replaced_backup_instead_of_restoring_attacker_bytes() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"trusted-original").unwrap();
    let transaction = fixture.install();
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    let backup = PathBuf::from(journal["backupPath"].as_str().unwrap());
    drop(transaction);
    fs::remove_file(&backup).unwrap();
    fs::write(&backup, b"attacker-replacement").unwrap();

    assert!(recover_stale_environment(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(fixture.target()).unwrap(), MANAGED_TOML.as_bytes());
    assert_eq!(fs::read(backup).unwrap(), b"attacker-replacement");
    assert!(fixture.journal().exists());
}

#[test]
fn recovery_restores_the_exact_backup_from_the_install_rename_window() {
    let fixture = Fixture::new();
    let original = b"rename-window-original\r\n\0";
    fs::write(fixture.target(), original).unwrap();
    let result = AdminEnvironmentTransaction::install_with_test_hook(
        &fixture.codex_home,
        &fixture.state_dir,
        &AdminEnvironmentSpec {
            shim_path: Path::new(r"C:\shim.exe"),
            pipe_name: "pipe",
            session_id: "session",
            proof_path: Path::new(r"C:\proof"),
        },
        |_, _| -> anyhow::Result<()> { anyhow::bail!("simulated crash") },
    );
    assert!(result.is_err());
    assert!(!fixture.target().exists());

    assert_eq!(
        recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap(),
        EnvironmentRestoreOutcome::Restored
    );
    assert_eq!(fs::read(fixture.target()).unwrap(), original);
}

#[test]
fn restore_removes_managed_file_when_no_original_existed() {
    let fixture = Fixture::new();
    let transaction = fixture.install();

    let outcome = transaction.restore().unwrap();

    assert_eq!(outcome, EnvironmentRestoreOutcome::Restored);
    assert!(!fixture.target().exists());
    assert!(!fixture.journal().exists());
}

#[test]
fn stale_journal_recovers_original_bytes() {
    let fixture = Fixture::new();
    let original = b"user-owned = true\n";
    fs::write(fixture.target(), original).unwrap();
    let transaction = fixture.install();
    drop(transaction);

    let outcome = recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap();

    assert_eq!(outcome, EnvironmentRestoreOutcome::Restored);
    assert_eq!(fs::read(fixture.target()).unwrap(), original);
    assert!(!fixture.journal().exists());
}

#[test]
fn restore_preserves_external_edit_and_returns_conflict_copies() {
    let fixture = Fixture::new();
    let original = b"original-user-bytes\r\n";
    let external = b"external-edit-must-survive\n";
    fs::write(fixture.target(), original).unwrap();
    let transaction = fixture.install();
    drop(transaction);
    fs::write(fixture.target(), external).unwrap();

    let outcome = recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap();

    let EnvironmentRestoreOutcome::Conflict {
        original_path,
        managed_path,
        conflicting_paths,
    } = outcome
    else {
        panic!("expected conflict outcome");
    };
    assert_eq!(fs::read(fixture.target()).unwrap(), external);
    assert_eq!(fs::read(original_path).unwrap(), original);
    assert_eq!(fs::read(managed_path).unwrap(), MANAGED_TOML.as_bytes());
    assert_eq!(conflicting_paths.len(), 1);
    assert_eq!(fs::read(&conflicting_paths[0]).unwrap(), external);
    assert!(!fixture.journal().exists());
}

#[test]
fn restore_retains_journal_when_current_target_cannot_be_read() {
    let fixture = Fixture::new();
    let transaction = fixture.install();
    drop(transaction);
    fs::remove_file(fixture.target()).unwrap();
    fs::create_dir(fixture.target()).unwrap();

    let error = recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap_err();

    assert!(error.to_string().contains("read"));
    assert!(fixture.journal().exists());
    assert!(fixture.target().is_dir());
}

#[test]
fn install_fails_before_mutation_when_target_is_not_a_readable_file() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.target()).unwrap();

    let error = match AdminEnvironmentTransaction::install(
        &fixture.codex_home,
        &fixture.state_dir,
        &AdminEnvironmentSpec {
            shim_path: Path::new("shim.exe"),
            pipe_name: "pipe",
            session_id: "session",
            proof_path: Path::new("proof"),
        },
    ) {
        Ok(_) => panic!("expected install failure"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("read"));
    assert!(fixture.target().is_dir());
    assert!(!fixture.journal().exists());
}

#[test]
fn install_never_overwrites_an_existing_journal() {
    let fixture = Fixture::new();
    let existing = b"existing-journal-must-survive";
    fs::write(fixture.journal(), existing).unwrap();

    assert!(
        AdminEnvironmentTransaction::install(
            &fixture.codex_home,
            &fixture.state_dir,
            &AdminEnvironmentSpec {
                shim_path: Path::new("shim.exe"),
                pipe_name: "pipe",
                session_id: "session",
                proof_path: Path::new("proof"),
            },
        )
        .is_err()
    );

    assert_eq!(fs::read(fixture.journal()).unwrap(), existing);
    assert!(!fixture.target().exists());
}

#[test]
fn generated_toml_safely_escapes_quotes_backslashes_and_unicode() {
    let fixture = Fixture::new();
    let transaction = AdminEnvironmentTransaction::install(
        &fixture.codex_home,
        &fixture.state_dir,
        &AdminEnvironmentSpec {
            shim_path: Path::new("C:\\工具\\shim \\\"admin\\\".exe"),
            pipe_name: "pipe-\\\"quoted\\\"-路径",
            session_id: "session-\\\\-雪",
            proof_path: Path::new("C:\\用户\\proof \\\"file\\\".proof"),
        },
    )
    .unwrap();

    let bytes = fs::read(fixture.target()).unwrap();
    let parsed: toml::Value = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
    let environment = &parsed["environments"][0];
    assert_eq!(
        environment["program"].as_str().unwrap(),
        "C:\\工具\\shim \\\"admin\\\".exe"
    );
    assert_eq!(
        environment["args"][2].as_str().unwrap(),
        "pipe-\\\"quoted\\\"-路径"
    );
    assert_eq!(environment["args"][4].as_str().unwrap(), "session-\\\\-雪");
    assert_eq!(
        environment["args"][6].as_str().unwrap(),
        "C:\\用户\\proof \\\"file\\\".proof"
    );
    transaction.restore().unwrap();
}

#[test]
fn sequential_conflicts_use_unique_files_without_overwriting_prior_copies() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"original-one").unwrap();
    let first = fixture.install();
    drop(first);
    fs::write(fixture.target(), b"external-one").unwrap();
    let EnvironmentRestoreOutcome::Conflict {
        original_path: first_original,
        managed_path: first_managed,
        ..
    } = recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap()
    else {
        panic!("expected first conflict");
    };
    let first_original_bytes = fs::read(&first_original).unwrap();
    let first_managed_bytes = fs::read(&first_managed).unwrap();

    let second = fixture.install();
    drop(second);
    fs::write(fixture.target(), b"external-two").unwrap();
    let EnvironmentRestoreOutcome::Conflict {
        original_path: second_original,
        managed_path: second_managed,
        ..
    } = recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap()
    else {
        panic!("expected second conflict");
    };

    assert_ne!(first_original, second_original);
    assert_ne!(first_managed, second_managed);
    assert_eq!(fs::read(first_original).unwrap(), first_original_bytes);
    assert_eq!(fs::read(first_managed).unwrap(), first_managed_bytes);
    assert_eq!(fs::read(second_original).unwrap(), b"external-one");
    assert_eq!(fs::read(second_managed).unwrap(), MANAGED_TOML.as_bytes());
    assert_eq!(fs::read(fixture.target()).unwrap(), b"external-two");
}

#[test]
fn forged_journal_auxiliary_paths_fail_closed_without_touching_external_files() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"original-user-file").unwrap();
    let transaction = fixture.install();
    drop(transaction);
    let external_path = fixture._temp.path().join("auth.json");
    let external_bytes = b"external-auth-must-not-be-read-or-deleted";
    fs::write(&external_path, external_bytes).unwrap();
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    journal["backupPath"] = serde_json::Value::String(external_path.to_string_lossy().into_owned());
    fs::write(
        fixture.journal(),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();

    assert!(recover_stale_environment(&fixture.codex_home, &fixture.state_dir).is_err());

    assert_eq!(fs::read(&external_path).unwrap(), external_bytes);
    assert_eq!(fs::read(fixture.target()).unwrap(), MANAGED_TOML.as_bytes());
    assert!(fixture.journal().exists());
}

#[test]
fn incomplete_schema_v1_journal_fails_closed() {
    let fixture = Fixture::new();
    let target_bytes = b"user-file-must-survive";
    fs::write(fixture.target(), target_bytes).unwrap();
    let incomplete = serde_json::json!({
        "schemaVersion": 1,
        "targetPath": fixture.target(),
        "originalExisted": true,
        "originalBytes": "",
        "managedSha256": "00"
    });
    fs::write(
        fixture.journal(),
        serde_json::to_vec_pretty(&incomplete).unwrap(),
    )
    .unwrap();

    assert!(recover_stale_environment(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(fixture.target()).unwrap(), target_bytes);
    assert!(fixture.journal().exists());
}

#[test]
fn complete_legacy_schema_v1_journal_fails_closed_without_trusting_its_backup() {
    let fixture = Fixture::new();
    let original = b"legacy-original-must-not-be-guessed";
    fs::write(fixture.target(), original).unwrap();
    let transaction = fixture.install();
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    drop(transaction);
    journal["schemaVersion"] = 1.into();
    journal.as_object_mut().unwrap().remove("originalSha256");
    fs::write(
        fixture.journal(),
        serde_json::to_vec_pretty(&journal).unwrap(),
    )
    .unwrap();

    assert!(recover_stale_environment(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(fixture.target()).unwrap(), MANAGED_TOML.as_bytes());
    assert!(fixture.journal().exists());
}

#[test]
fn concurrent_installs_publish_exactly_one_journal_owner() {
    let fixture = Fixture::new();
    let original = b"concurrent-original-bytes";
    fs::write(fixture.target(), original).unwrap();
    let codex_home = Arc::new(fixture.codex_home.clone());
    let state_dir = Arc::new(fixture.state_dir.clone());
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for suffix in ["one", "two"] {
        let codex_home = Arc::clone(&codex_home);
        let state_dir = Arc::clone(&state_dir);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            AdminEnvironmentTransaction::install(
                &codex_home,
                &state_dir,
                &AdminEnvironmentSpec {
                    shim_path: Path::new(r"C:\Program Files\CodexPlusPlus\shim.exe"),
                    pipe_name: suffix,
                    session_id: suffix,
                    proof_path: Path::new(r"C:\proof"),
                },
            )
        }));
    }
    barrier.wait();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let transaction = results.into_iter().find_map(Result::ok).unwrap();

    transaction.restore().unwrap();

    assert_eq!(fs::read(fixture.target()).unwrap(), original);
    assert!(!fixture.journal().exists());
}

#[cfg(windows)]
#[test]
fn journal_delete_failure_keeps_recovery_artifacts_and_retry_completes() {
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::new();
    let original = b"retryable-original";
    fs::write(fixture.target(), original).unwrap();
    let transaction = fixture.install();
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    let backup_path = PathBuf::from(journal["backupPath"].as_str().unwrap());
    let snapshot_path = PathBuf::from(journal["managedSnapshotPath"].as_str().unwrap());
    drop(transaction);
    let journal_lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001 | 0x0000_0002)
        .open(fixture.journal())
        .unwrap();

    assert!(recover_stale_environment(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(fixture.target()).unwrap(), MANAGED_TOML.as_bytes());
    assert!(fixture.journal().exists());
    assert!(backup_path.exists());
    assert!(snapshot_path.exists());

    drop(journal_lock);
    assert_eq!(
        recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap(),
        EnvironmentRestoreOutcome::Restored
    );
    assert_eq!(fs::read(fixture.target()).unwrap(), original);
    assert!(!fixture.journal().exists());
    assert!(!backup_path.exists());
    assert!(!snapshot_path.exists());
}

#[test]
fn active_transaction_guard_rejects_install_and_recovery_without_changing_owner() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"guarded-original").unwrap();
    let transaction = fixture.install();
    let owned_journal = fs::read(fixture.journal()).unwrap();

    assert!(
        AdminEnvironmentTransaction::install(
            &fixture.codex_home,
            &fixture.state_dir,
            &AdminEnvironmentSpec {
                shim_path: Path::new("other-shim.exe"),
                pipe_name: "other-pipe",
                session_id: "other-session",
                proof_path: Path::new("other-proof"),
            },
        )
        .is_err()
    );
    assert!(recover_stale_environment(&fixture.codex_home, &fixture.state_dir).is_err());
    assert_eq!(fs::read(fixture.journal()).unwrap(), owned_journal);

    transaction.restore().unwrap();
    let next = fixture.install();
    next.restore().unwrap();
}

#[test]
fn recovery_preserves_and_reports_two_intervening_external_edits() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"original").unwrap();
    let transaction = fixture.install();
    drop(transaction);
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    let transaction_id = journal["transactionId"].as_str().unwrap();
    let restoring = fixture.codex_home.join(format!(
        ".administrator-mode-environment.{transaction_id}.restoring"
    ));
    fs::write(fixture.target(), b"first-external-edit").unwrap();
    fs::rename(fixture.target(), &restoring).unwrap();
    fs::write(fixture.target(), b"second-external-edit").unwrap();

    let EnvironmentRestoreOutcome::Conflict {
        conflicting_paths, ..
    } = recover_stale_environment(&fixture.codex_home, &fixture.state_dir).unwrap()
    else {
        panic!("expected conflict");
    };

    assert_eq!(fs::read(fixture.target()).unwrap(), b"second-external-edit");
    assert_eq!(conflicting_paths.len(), 2);
    let copies = conflicting_paths
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
    assert!(copies.contains(&b"first-external-edit".to_vec()));
    assert!(copies.contains(&b"second-external-edit".to_vec()));
}

#[test]
fn cleanup_failure_after_journal_completion_does_not_fail_restore() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"cleanup-original").unwrap();
    let transaction = fixture.install();
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    let managed_stage = PathBuf::from(journal["managedStagePath"].as_str().unwrap());
    fs::create_dir(&managed_stage).unwrap();

    assert_eq!(
        transaction.restore().unwrap(),
        EnvironmentRestoreOutcome::Restored
    );
    assert_eq!(fs::read(fixture.target()).unwrap(), b"cleanup-original");
    assert!(!fixture.journal().exists());
    assert!(managed_stage.is_dir());
}

#[cfg(windows)]
#[test]
fn install_restore_failure_error_reports_preserved_intervening_path() {
    let fixture = Fixture::new();
    fs::write(fixture.target(), b"original-before-install").unwrap();
    let leaked_link = fixture._temp.path().join("linked-backup");

    let error = match AdminEnvironmentTransaction::install_with_test_hook(
        &fixture.codex_home,
        &fixture.state_dir,
        &AdminEnvironmentSpec {
            shim_path: Path::new("shim.exe"),
            pipe_name: "pipe",
            session_id: "session",
            proof_path: Path::new("proof"),
        },
        |target, backup| {
            fs::write(target, b"intervening-install-edit").unwrap();
            fs::hard_link(backup, &leaked_link).unwrap();
            Ok(())
        },
    ) {
        Ok(_) => panic!("expected install failure"),
        Err(error) => error,
    };
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.journal()).unwrap()).unwrap();
    let transaction_id = journal["transactionId"].as_str().unwrap();
    let intervening = fixture.codex_home.join(format!(
        ".administrator-mode-environment.{transaction_id}.intervening"
    ));

    assert!(
        error
            .to_string()
            .contains(&intervening.display().to_string())
    );
    assert_eq!(fs::read(intervening).unwrap(), b"intervening-install-edit");
    assert_eq!(fs::read(leaked_link).unwrap(), b"original-before-install");
    assert!(fixture.journal().exists());
}
