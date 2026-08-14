use codex_plus_data::{
    SessionArchiveImportOptions, SessionPathMapping, export_session_archive,
    import_session_archive, inspect_session_archive,
};
use rusqlite::Connection;
use serde_json::json;
use std::fs::{self, File};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[test]
fn exports_and_imports_sessions_projects_and_related_databases() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);

    let archive = temp.path().join("sessions.codexbackup");
    let exported = export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    assert_eq!(exported.requested_count, 1);
    assert_eq!(exported.session_count, 2);
    assert_eq!(exported.related_session_count, 1);
    assert!(exported.rollout_bytes > 0);
    assert_eq!(exported.asset_count, 1);
    assert!(exported.asset_bytes > 0);

    let preview = inspect_session_archive(&destination, &archive).unwrap();
    assert_eq!(preview.session_count, 2);
    assert_eq!(preview.conflict_count, 0);
    assert_eq!(preview.asset_count, 1);
    assert!(preview.sessions.iter().all(|session| session.has_rollout));

    let imported = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: vec![SessionPathMapping {
                from: r"C:\old-project".to_string(),
                to: r"D:\new-project".to_string(),
            }],
        },
    )
    .unwrap();
    assert_eq!(imported.imported_count, 2);
    assert_eq!(imported.skipped_count, 0);
    assert_eq!(imported.restored_rollout_count, 2);
    assert_eq!(imported.restored_asset_count, 1);
    assert!(Path::new(&imported.backup_path).is_dir());

    let state_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let threads: i64 = state_db
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(threads, 2);
    let parent_cwd: String = state_db
        .query_row("SELECT cwd FROM threads WHERE id = 'parent'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(parent_cwd, r"D:\new-project");
    let rollout_path: String = state_db
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(Path::new(&rollout_path).is_file());
    let rollout = fs::read_to_string(rollout_path).unwrap();
    assert!(rollout.contains(r"D:\\new-project"));

    let goals_db = Connection::open(destination.join("goals_1.sqlite")).unwrap();
    let goals: i64 = goals_db
        .query_row("SELECT COUNT(*) FROM thread_goals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(goals, 2);

    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join(".codex-global-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        state["electron-saved-workspace-roots"],
        json!([r"D:\new-project"])
    );
    assert_eq!(
        state["thread-workspace-root-hints"]["parent"],
        json!(r"D:\new-project")
    );
    assert_eq!(
        state["thread-project-assignments"]["parent"]["projectId"],
        json!(r"D:\new-project")
    );
    assert_eq!(
        state["thread-project-assignments"]["child"]["projectId"],
        json!(r"D:\new-project")
    );
    assert_eq!(
        state["electron-workspace-root-labels"][r"D:\new-project"],
        json!("Old Project")
    );
    assert!(
        state["electron-workspace-root-labels"]
            .get(r"C:\old-project")
            .is_none()
    );

    let index = fs::read_to_string(destination.join("session_index.jsonl")).unwrap();
    assert!(index.contains("parent"));
    assert!(index.contains("child"));
    assert_eq!(
        fs::read(destination.join("attachments/parent-image.png")).unwrap(),
        b"image-bytes"
    );

    let mut state_without_assignments = state;
    state_without_assignments
        .as_object_mut()
        .unwrap()
        .remove("thread-project-assignments");
    fs::write(
        destination.join(".codex-global-state.json"),
        serde_json::to_vec_pretty(&state_without_assignments).unwrap(),
    )
    .unwrap();
    let second = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: vec![SessionPathMapping {
                from: r"C:\old-project".to_string(),
                to: r"D:\new-project".to_string(),
            }],
        },
    )
    .unwrap();
    assert_eq!(second.imported_count, 0);
    assert_eq!(second.skipped_count, 2);
    assert!(!second.backup_path.is_empty());
    let repaired_state: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join(".codex-global-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        repaired_state["thread-project-assignments"]["parent"]["projectId"],
        json!(r"D:\new-project")
    );

    state_db
        .execute(
            "UPDATE threads SET title = 'Changed' WHERE id = 'parent'",
            [],
        )
        .unwrap();
    let overwritten = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "overwrite".to_string(),
            path_mappings: vec![SessionPathMapping {
                from: r"C:\old-project".to_string(),
                to: r"D:\new-project".to_string(),
            }],
        },
    )
    .unwrap();
    assert_eq!(overwritten.imported_count, 2);
    assert_eq!(overwritten.overwritten_count, 2);
    let title: String = state_db
        .query_row("SELECT title FROM threads WHERE id = 'parent'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(title, "Parent");
}

#[test]
fn import_requires_codex_to_initialize_its_database_first() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("empty-destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    let archive = temp.path().join("sessions.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();

    let error = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("启动一次"));
}

#[test]
fn import_rebinds_threads_and_rollouts_to_the_destination_provider() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    fs::write(
        destination.join("config.toml"),
        r#"model_provider = "custom"

[model_providers.custom]
name = "custom"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
    )
    .unwrap();

    let archive = temp.path().join("provider-rebind.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    let db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let providers = db
        .prepare("SELECT DISTINCT model_provider FROM threads ORDER BY model_provider")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(providers, vec!["custom"]);

    let rollout_path: String = db
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let first_line = fs::read_to_string(&rollout_path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let first: serde_json::Value = serde_json::from_str(&first_line).unwrap();
    assert_eq!(first["payload"]["model_provider"], "custom");

    db.execute("UPDATE threads SET model_provider = 'openai'", [])
        .unwrap();
    let relative_rollout = Path::new(&rollout_path)
        .strip_prefix(&destination)
        .unwrap()
        .to_string_lossy()
        .to_string();
    db.execute("UPDATE threads SET rollout_path = ?1", [&relative_rollout])
        .unwrap();
    let stale_first_line = fs::read_to_string(&rollout_path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .replace(
            r#""model_provider":"custom""#,
            r#""model_provider":"openai""#,
        );
    let stale_rollout = format!("{stale_first_line}\r\nnot-json-tail");
    fs::write(&rollout_path, stale_rollout).unwrap();
    let repaired = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(repaired.imported_count, 0);
    let repaired_provider: String = db
        .query_row(
            "SELECT model_provider FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repaired_provider, "custom");
    let repaired_rollout = fs::read_to_string(rollout_path).unwrap();
    assert!(repaired_rollout.contains("\r\nnot-json-tail"));
    assert!(repaired_rollout.ends_with("not-json-tail"));
    let repaired_first_line = repaired_rollout.lines().next().unwrap().to_string();
    let repaired_first: serde_json::Value = serde_json::from_str(&repaired_first_line).unwrap();
    assert_eq!(repaired_first["payload"]["model_provider"], "custom");
}

#[test]
fn import_rejects_provider_repair_for_rollouts_outside_codex_home() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    fs::write(
        destination.join("config.toml"),
        "model_provider = \"custom\"\n",
    )
    .unwrap();
    let archive = temp.path().join("outside-rollout.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    let outside = temp.path().join("outside-rollout.jsonl");
    let outside_contents = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"parent\",\"model_provider\":\"openai\"}}\n";
    fs::write(&outside, outside_contents).unwrap();
    let db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    db.execute(
        "UPDATE threads SET model_provider = 'openai', rollout_path = ?1",
        [outside.to_string_lossy().to_string()],
    )
    .unwrap();

    let error = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap_err();

    assert!(format!("{error:#}").contains("Codex home 之外"));
    let provider: String = db
        .query_row(
            "SELECT model_provider FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provider, "openai");
    assert_eq!(fs::read_to_string(outside).unwrap(), outside_contents);
}

#[test]
fn mixed_import_failure_restores_new_rows_project_state_index_rollouts_and_assets() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    fs::write(
        destination.join("config.toml"),
        "model_provider = \"custom\"\n",
    )
    .unwrap();

    let missing_parent_rollout = destination
        .join("sessions/2026/07/14/missing-parent.jsonl")
        .to_string_lossy()
        .to_string();
    let destination_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    destination_db
        .execute(
            "INSERT INTO threads VALUES ('parent', ?1, 'Existing parent', 'C:\\existing', 'openai', 0, 1)",
            [missing_parent_rollout],
        )
        .unwrap();
    drop(destination_db);
    let original_state = fs::read(destination.join(".codex-global-state.json")).unwrap();
    assert!(!destination.join("session_index.jsonl").exists());

    let archive = temp.path().join("mixed-rollback.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    let error = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap_err();

    assert!(format!("{error:#}").contains("已恢复导入前状态"));
    let restored_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let rows: i64 = restored_db
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    let provider: String = restored_db
        .query_row(
            "SELECT model_provider FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provider, "openai");
    assert_eq!(
        fs::read(destination.join(".codex-global-state.json")).unwrap(),
        original_state
    );
    assert!(!destination.join("session_index.jsonl").exists());
    assert!(
        !destination
            .join("sessions/2026/07/14/rollout-child.jsonl")
            .exists()
    );
    assert!(!destination.join("attachments/parent-image.png").exists());
}

#[test]
fn import_rolls_back_earlier_database_provider_updates_when_a_later_database_fails() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    fs::write(
        destination.join("config.toml"),
        "model_provider = \"custom\"\n",
    )
    .unwrap();
    let archive = temp.path().join("transaction-rollback.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    let primary = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let rollout_path: String = primary
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    primary
        .execute("UPDATE threads SET model_provider = 'openai'", [])
        .unwrap();
    let stale_rollout = fs::read_to_string(&rollout_path).unwrap().replace(
        r#""model_provider":"custom""#,
        r#""model_provider":"openai""#,
    );
    fs::write(&rollout_path, &stale_rollout).unwrap();

    let failing = Connection::open(destination.join("z-failing.sqlite")).unwrap();
    failing
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT,
                rollout_path TEXT
            );
            INSERT INTO threads VALUES ('parent', 'openai', NULL);
            CREATE TRIGGER reject_provider_update
            BEFORE UPDATE OF model_provider ON threads
            BEGIN
                SELECT RAISE(ABORT, 'fixture provider update failure');
            END;",
        )
        .unwrap();
    drop(failing);

    let error = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("fixture provider update failure"));

    let provider: String = primary
        .query_row(
            "SELECT model_provider FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provider, "openai");
    assert_eq!(fs::read_to_string(rollout_path).unwrap(), stale_rollout);
}

#[test]
fn import_does_not_assign_explicit_projectless_threads_to_projects() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    let state_path = source.join(".codex-global-state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["projectless-thread-ids"] = json!(["child"]);
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

    let archive = temp.path().join("projectless.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: vec![SessionPathMapping {
                from: r"C:\old-project".to_string(),
                to: r"D:\new-project".to_string(),
            }],
        },
    )
    .unwrap();

    let imported: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join(".codex-global-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        imported["thread-project-assignments"]["parent"]["projectId"],
        json!(r"D:\new-project")
    );
    assert!(
        imported["thread-project-assignments"]
            .get("child")
            .is_none()
    );
    assert!(
        imported["projectless-thread-ids"]
            .as_array()
            .unwrap()
            .contains(&json!("child"))
    );
}

#[test]
fn import_skips_regular_threads_without_rollout_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    fs::remove_file(source.join("sessions/2026/07/14/rollout-child.jsonl")).unwrap();

    let archive = temp.path().join("missing-rollout.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    let preview = inspect_session_archive(&destination, &archive).unwrap();
    assert_eq!(
        preview
            .sessions
            .iter()
            .filter(|session| !session.has_rollout)
            .count(),
        1
    );

    let imported = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    assert_eq!(imported.imported_count, 1);
    assert_eq!(imported.skipped_count, 1);
    assert_eq!(imported.restored_rollout_count, 1);
    assert!(
        imported
            .warnings
            .iter()
            .any(|warning| warning.contains("child") && warning.contains("rollout"))
    );
    let state_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let thread_count: i64 = state_db
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(thread_count, 1);
}

#[test]
fn export_includes_rollouts_stored_outside_codex_home() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);

    let external_rollout = temp.path().join("external-parent-rollout.jsonl");
    let external_contents = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"parent\",\"cwd\":\"C:\\\\old-project\"}}\n";
    fs::write(&external_rollout, external_contents).unwrap();
    let source_db = Connection::open(source.join("state_5.sqlite")).unwrap();
    source_db
        .execute(
            "UPDATE threads SET rollout_path = ?1 WHERE id = 'parent'",
            [external_rollout.to_string_lossy().to_string()],
        )
        .unwrap();
    drop(source_db);

    let archive = temp.path().join("external-rollout.codexbackup");
    let exported = export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    assert_eq!(exported.session_count, 2);

    let preview = inspect_session_archive(&destination, &archive).unwrap();
    assert!(
        preview
            .sessions
            .iter()
            .find(|session| session.id == "parent")
            .unwrap()
            .has_rollout
    );

    let imported = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(imported.imported_count, 2);
    assert_eq!(imported.restored_rollout_count, 2);

    let destination_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let restored_path: String = destination_db
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = 'parent'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(Path::new(&restored_path).starts_with(&destination));
    let restored = fs::read_to_string(restored_path).unwrap();
    assert!(restored.contains("\"id\":\"parent\""));
    assert!(restored.contains("\"model_provider\":\"openai\""));
}

#[test]
fn import_rebuilds_missing_session_index_rows_for_project_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);

    // 即使 Codex 尚未写出项目会话的 session_index 条目，线程数据库和项目状态仍是权威来源。
    fs::write(
        source.join("session_index.jsonl"),
        "{\"id\":\"parent\",\"thread_name\":\"Parent\",\"updated_at\":200}\n",
    )
    .unwrap();

    let archive = temp.path().join("missing-project-index.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    let imported = import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    assert_eq!(imported.imported_count, 2);
    let index = fs::read_to_string(destination.join("session_index.jsonl")).unwrap();
    assert!(index.contains("\"id\":\"parent\""));
    assert!(index.contains("\"id\":\"child\""));
    let child = index
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|row| row["id"] == "child")
        .unwrap();
    assert_eq!(child["thread_name"], "Child");
    assert!(child["updated_at"].as_str().is_some());
}

#[test]
fn import_skips_rollout_entries_missing_from_archive() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);

    let exported_archive = temp.path().join("complete.codexbackup");
    export_session_archive(&source, &exported_archive, &["parent".to_string()]).unwrap();
    let damaged_archive = temp.path().join("missing-entry.codexbackup");
    copy_archive_without_entries(
        &exported_archive,
        &damaged_archive,
        &["sessions/parent/rollout.jsonl"],
    );

    let preview = inspect_session_archive(&destination, &damaged_archive).unwrap();
    assert_eq!(
        preview
            .sessions
            .iter()
            .filter(|session| !session.has_rollout)
            .count(),
        1
    );
    assert!(
        preview.warnings.iter().any(|warning| {
            warning.contains("parent") && warning.contains("不在迁移包中")
        })
    );

    let imported = import_session_archive(
        &destination,
        &damaged_archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    assert_eq!(imported.imported_count, 1);
    assert_eq!(imported.skipped_count, 1);
    assert_eq!(imported.restored_rollout_count, 1);
    assert!(
        imported
            .warnings
            .iter()
            .any(|warning| { warning.contains("parent") && warning.contains("rollout") })
    );
    let state_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let thread_count: i64 = state_db
        .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(thread_count, 1);
}

#[test]
fn import_preserves_agent_job_parents_before_foreign_key_children() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    create_codex_home(&source, true);
    create_codex_home(&destination, false);
    for home in [&source, &destination] {
        let db = Connection::open(home.join("state_5.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE agent_jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE agent_job_items (
                job_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                assigned_thread_id TEXT,
                PRIMARY KEY (job_id, item_id),
                FOREIGN KEY (job_id) REFERENCES agent_jobs(id)
            );",
        )
        .unwrap();
    }
    let source_db = Connection::open(source.join("state_5.sqlite")).unwrap();
    source_db
        .execute(
            "INSERT INTO agent_jobs VALUES ('job-parent', 'Parent job')",
            [],
        )
        .unwrap();
    source_db
        .execute(
            "INSERT INTO agent_job_items VALUES ('job-parent', 'item-1', 'parent')",
            [],
        )
        .unwrap();

    let archive = temp.path().join("agent-job.codexbackup");
    export_session_archive(&source, &archive, &["parent".to_string()]).unwrap();
    import_session_archive(
        &destination,
        &archive,
        &SessionArchiveImportOptions {
            conflict_policy: "skip".to_string(),
            path_mappings: Vec::new(),
        },
    )
    .unwrap();

    let destination_db = Connection::open(destination.join("state_5.sqlite")).unwrap();
    let job_count: i64 = destination_db
        .query_row("SELECT COUNT(*) FROM agent_jobs", [], |row| row.get(0))
        .unwrap();
    let item_count: i64 = destination_db
        .query_row("SELECT COUNT(*) FROM agent_job_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(job_count, 1);
    assert_eq!(item_count, 1);
}

fn create_codex_home(home: &Path, with_data: bool) {
    fs::create_dir_all(home.join("sessions/2026/07/14")).unwrap();
    let state_db = Connection::open(home.join("state_5.sqlite")).unwrap();
    state_db
        .execute_batch(
            "
            CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                title TEXT,
                cwd TEXT,
                model_provider TEXT,
                archived INTEGER,
                updated_at_ms INTEGER
            );
            CREATE TABLE thread_dynamic_tools (
                thread_id TEXT,
                position INTEGER,
                name TEXT,
                PRIMARY KEY (thread_id, name)
            );
            CREATE TABLE thread_spawn_edges (
                parent_thread_id TEXT,
                child_thread_id TEXT,
                status TEXT,
                PRIMARY KEY (parent_thread_id, child_thread_id)
            );
            ",
        )
        .unwrap();
    let goals_db = Connection::open(home.join("goals_1.sqlite")).unwrap();
    goals_db
        .execute_batch(
            "CREATE TABLE thread_goals (
                thread_id TEXT,
                goal_id TEXT,
                objective TEXT,
                PRIMARY KEY (thread_id, goal_id)
            );",
        )
        .unwrap();

    if !with_data {
        fs::write(home.join(".codex-global-state.json"), "{}").unwrap();
        return;
    }

    fs::create_dir_all(home.join("attachments")).unwrap();
    let attachment = home.join("attachments/parent-image.png");
    fs::write(&attachment, b"image-bytes").unwrap();

    for (id, title, updated_at) in [("parent", "Parent", 200_i64), ("child", "Child", 100_i64)] {
        let rollout = home
            .join("sessions/2026/07/14")
            .join(format!("rollout-{id}.jsonl"));
        let attachment_line = if id == "parent" {
            format!(
                "{{\"type\":\"response_item\",\"payload\":{{\"path\":{}}}}}\n",
                serde_json::to_string(&attachment.to_string_lossy()).unwrap()
            )
        } else {
            String::new()
        };
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"C:\\\\old-project\"}}}}\n{attachment_line}"
            ),
        )
        .unwrap();
        state_db
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, 'openai', 0, ?5)",
                (
                    id,
                    rollout.to_string_lossy().to_string(),
                    title,
                    r"C:\old-project",
                    updated_at,
                ),
            )
            .unwrap();
        state_db
            .execute(
                "INSERT INTO thread_dynamic_tools VALUES (?1, 0, 'computer')",
                [id],
            )
            .unwrap();
        goals_db
            .execute(
                "INSERT INTO thread_goals VALUES (?1, ?2, 'Finish')",
                (id, format!("goal-{id}")),
            )
            .unwrap();
    }
    state_db
        .execute(
            "INSERT INTO thread_spawn_edges VALUES ('parent', 'child', 'completed')",
            [],
        )
        .unwrap();
    fs::write(
        home.join(".codex-global-state.json"),
        serde_json::to_vec_pretty(&json!({
            "electron-saved-workspace-roots": [r"C:\old-project"],
            "project-order": [r"C:\old-project"],
            "electron-workspace-root-labels": {r"C:\old-project": "Old Project"},
            "thread-workspace-root-hints": {
                "parent": r"C:\old-project",
                "child": r"C:\old-project"
            },
            "projectless-thread-ids": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        home.join("session_index.jsonl"),
        "{\"id\":\"parent\",\"thread_name\":\"Parent\",\"updated_at\":200}\n{\"id\":\"child\",\"thread_name\":\"Child\",\"updated_at\":100}\n",
    )
    .unwrap();
}

fn copy_archive_without_entries(source: &Path, destination: &Path, excluded: &[&str]) {
    let source_file = File::open(source).unwrap();
    let mut archive = ZipArchive::new(source_file).unwrap();
    let output_file = File::create(destination).unwrap();
    let mut writer = ZipWriter::new(output_file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        if excluded.iter().any(|excluded_name| *excluded_name == name) {
            continue;
        }
        writer.start_file(name, options).unwrap();
        std::io::copy(&mut entry, &mut writer).unwrap();
    }
    writer.finish().unwrap();
}
