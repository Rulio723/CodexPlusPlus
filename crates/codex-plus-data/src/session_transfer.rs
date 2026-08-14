use anyhow::{Context, bail};
use chrono::Utc;
use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
use rusqlite::{Connection, OpenFlags, ToSql};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const ARCHIVE_FORMAT: &str = "codex-plus-session-archive";
const ARCHIVE_FORMAT_VERSION: u32 = 1;
const MANIFEST_ENTRY: &str = "manifest.json";

const RELATED_TABLES: &[&str] = &[
    "thread_dynamic_tools",
    "thread_goals",
    "thread_spawn_edges",
    "stage1_outputs",
    "agent_jobs",
    "agent_job_items",
    "automation_runs",
    "inbox_items",
];

const IMPORT_TABLE_ORDER: &[&str] = &[
    "threads",
    "automation_runs",
    "agent_jobs",
    "thread_dynamic_tools",
    "thread_goals",
    "thread_spawn_edges",
    "stage1_outputs",
    "agent_job_items",
    "inbox_items",
];

const PROJECT_STATE_ARRAY_KEYS: &[&str] = &["electron-saved-workspace-roots", "project-order"];

const THREAD_STATE_ARRAY_KEYS: &[&str] = &["projectless-thread-ids", "pinned-thread-ids"];

const PROJECT_STATE_OBJECT_KEYS: &[&str] = &["electron-workspace-root-labels"];

const THREAD_STATE_OBJECT_KEYS: &[&str] = &[
    "thread-project-assignments",
    "thread-projectless-output-directories",
    "thread-workspace-root-hints",
    "thread-writable-roots",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPathMapping {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchiveImportOptions {
    pub conflict_policy: String,
    #[serde(default)]
    pub path_mappings: Vec<SessionPathMapping>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchiveExportResult {
    pub archive_path: String,
    pub session_count: usize,
    pub requested_count: usize,
    pub related_session_count: usize,
    pub rollout_bytes: u64,
    pub asset_count: usize,
    pub asset_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchivePreview {
    pub archive_path: String,
    pub format_version: u32,
    pub exported_at: String,
    pub source_version: String,
    pub session_count: usize,
    pub asset_count: usize,
    pub conflict_count: usize,
    pub missing_project_roots: Vec<String>,
    pub sessions: Vec<SessionArchivePreviewItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchivePreviewItem {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub archived: bool,
    pub conflict: bool,
    pub has_rollout: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchiveImportResult {
    pub archive_path: String,
    pub imported_count: usize,
    pub skipped_count: usize,
    pub overwritten_count: usize,
    pub restored_rollout_count: usize,
    pub restored_asset_count: usize,
    pub missing_tables: Vec<String>,
    pub backup_path: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionArchiveManifest {
    format: String,
    format_version: u32,
    exported_at: String,
    source_version: String,
    sessions: Vec<ManifestSession>,
    #[serde(default)]
    assets: Vec<ManifestAsset>,
    tables: BTreeMap<String, Vec<Value>>,
    project_state: Value,
    session_index: Vec<Value>,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestAsset {
    archive_entry: String,
    relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestSession {
    id: String,
    title: String,
    cwd: String,
    model_provider: String,
    archived: bool,
    updated_at_ms: Option<i64>,
    rollout_entry: Option<String>,
    rollout_relative_path: Option<String>,
}

#[derive(Debug, Clone)]
struct ThreadCandidate {
    row: Value,
    score: i64,
}

#[derive(Debug, Clone)]
struct OwnedSqlValue(SqlValue);

impl ToSql for OwnedSqlValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(self.0.clone()))
    }
}

pub fn export_session_archive(
    codex_home: &Path,
    output_path: &Path,
    requested_session_ids: &[String],
) -> anyhow::Result<SessionArchiveExportResult> {
    let output_path = normalized_archive_output_path(output_path);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let db_paths = discover_database_paths(codex_home);
    let (thread_candidates, mut table_rows) = collect_database_rows(&db_paths)?;
    let automation_ids = table_rows
        .get("automation_runs")
        .into_iter()
        .flatten()
        .filter_map(|row| string_field(row, "thread_id"))
        .collect::<BTreeSet<_>>();
    let all_ids = thread_candidates
        .keys()
        .cloned()
        .chain(automation_ids)
        .collect::<BTreeSet<_>>();

    let requested = requested_session_ids
        .iter()
        .map(|id| normalize_thread_id(id))
        .filter(|id| all_ids.contains(id))
        .collect::<BTreeSet<_>>();
    let requested = if requested_session_ids.is_empty() {
        all_ids.clone()
    } else {
        requested
    };
    if requested.is_empty() {
        bail!("没有找到可导出的本地会话");
    }

    let session_ids = expand_related_session_ids(&requested, &table_rows, &all_ids);
    let agent_job_ids = agent_job_ids_for_sessions(&table_rows, &session_ids);
    table_rows.insert(
        "threads".to_string(),
        session_ids
            .iter()
            .filter_map(|id| {
                thread_candidates
                    .get(id)
                    .map(|candidate| candidate.row.clone())
            })
            .collect(),
    );
    for table in RELATED_TABLES {
        if let Some(rows) = table_rows.get_mut(*table) {
            rows.retain(|row| row_belongs_to_transfer(table, row, &session_ids, &agent_job_ids));
        }
    }

    let project_paths = session_ids
        .iter()
        .filter_map(|id| {
            thread_candidates
                .get(id)
                .and_then(|candidate| string_field(&candidate.row, "cwd"))
                .or_else(|| {
                    table_rows
                        .get("automation_runs")
                        .into_iter()
                        .flatten()
                        .find(|row| string_field(row, "thread_id").as_deref() == Some(id.as_str()))
                        .and_then(|row| string_field(row, "source_cwd"))
                })
        })
        .filter(|path| !path.trim().is_empty())
        .collect::<BTreeSet<_>>();
    let project_state = export_project_state(codex_home, &session_ids, &project_paths)?;
    let session_index = export_session_index(codex_home, &session_ids)?;
    let mut sessions = Vec::new();
    let mut rollout_sources = HashMap::new();
    let mut warnings = Vec::new();
    for id in &session_ids {
        let thread_row = thread_candidates.get(id).map(|candidate| &candidate.row);
        let automation_row = table_rows
            .get("automation_runs")
            .into_iter()
            .flatten()
            .find(|row| string_field(row, "thread_id").as_deref() == Some(id.as_str()));
        let title = thread_row
            .and_then(|row| string_field(row, "title"))
            .or_else(|| automation_row.and_then(|row| string_field(row, "thread_title")))
            .unwrap_or_else(|| id.clone());
        let cwd = thread_row
            .and_then(|row| string_field(row, "cwd"))
            .or_else(|| automation_row.and_then(|row| string_field(row, "source_cwd")))
            .unwrap_or_default();
        let model_provider = thread_row
            .and_then(|row| string_field(row, "model_provider"))
            .unwrap_or_default();
        let archived = thread_row
            .and_then(|row| boolish_field(row, "archived"))
            .unwrap_or(false)
            || automation_row
                .and_then(|row| string_field(row, "status"))
                .is_some_and(|status| status.eq_ignore_ascii_case("archived"));
        let updated_at_ms = thread_row.and_then(row_timestamp_option);
        let rollout_path = thread_row
            .and_then(|row| string_field(row, "rollout_path"))
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from);
        let (rollout_entry, rollout_relative_path) = match rollout_path {
            Some(path) => {
                let source = if path.is_absolute() {
                    path
                } else {
                    codex_home.join(path)
                };
                if !source.is_file() {
                    warnings.push(format!(
                        "会话 {id} 的 rollout 文件不存在：{}",
                        source.display()
                    ));
                    (None, None)
                } else {
                    let relative = source
                        .strip_prefix(codex_home)
                        .ok()
                        .and_then(safe_relative_path)
                        .unwrap_or_else(|| {
                            PathBuf::from("sessions")
                                .join("imported")
                                .join(format!("rollout-{id}.jsonl"))
                        });
                    rollout_sources.insert(id.clone(), source);
                    (
                        Some(format!("sessions/{}/rollout.jsonl", safe_archive_id(id))),
                        Some(path_to_archive_string(&relative)),
                    )
                }
            }
            None => (None, None),
        };
        sessions.push(ManifestSession {
            id: id.clone(),
            title,
            cwd,
            model_provider,
            archived,
            updated_at_ms,
            rollout_entry,
            rollout_relative_path,
        });
    }
    sessions.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });

    let assets = collect_referenced_assets(codex_home, &sessions, &rollout_sources);
    let mut manifest = SessionArchiveManifest {
        format: ARCHIVE_FORMAT.to_string(),
        format_version: ARCHIVE_FORMAT_VERSION,
        exported_at: Utc::now().to_rfc3339(),
        source_version: env!("CARGO_PKG_VERSION").to_string(),
        sessions,
        assets,
        tables: table_rows,
        project_state,
        session_index,
        warnings: warnings.clone(),
    };

    let file = File::create(&output_path)?;
    let mut writer = ZipWriter::new(BufWriter::new(file));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut rollout_bytes = 0_u64;
    let mut asset_bytes = 0_u64;
    for index in 0..manifest.sessions.len() {
        let session = manifest.sessions[index].clone();
        let (Some(entry_name), Some(relative_path)) = (
            session.rollout_entry.as_deref(),
            session.rollout_relative_path.as_deref(),
        ) else {
            continue;
        };
        let Some(relative_path) = safe_relative_path(Path::new(relative_path)) else {
            manifest.sessions[index].rollout_entry = None;
            manifest.sessions[index].rollout_relative_path = None;
            warnings.push(format!(
                "会话 {} 的 rollout 路径不在 Codex home 内，已跳过。",
                session.id
            ));
            continue;
        };
        let source = rollout_sources
            .get(&session.id)
            .cloned()
            .unwrap_or_else(|| codex_home.join(relative_path));
        if !source.is_file() {
            manifest.sessions[index].rollout_entry = None;
            manifest.sessions[index].rollout_relative_path = None;
            warnings.push(format!(
                "会话 {} 的 rollout 文件在打包时不存在，已跳过。",
                session.id
            ));
            continue;
        }
        let mut input = match File::open(&source) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                manifest.sessions[index].rollout_entry = None;
                manifest.sessions[index].rollout_relative_path = None;
                warnings.push(format!(
                    "会话 {} 的 rollout 文件在打包时不存在，已跳过。",
                    session.id
                ));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        writer.start_file(entry_name, options)?;
        rollout_bytes += std::io::copy(&mut input, &mut writer)?;
    }
    let mut included_assets = Vec::with_capacity(manifest.assets.len());
    for asset in manifest.assets.clone() {
        let source = codex_home.join(
            safe_relative_path(Path::new(&asset.relative_path))
                .ok_or_else(|| anyhow::anyhow!("附件路径不安全：{}", asset.relative_path))?,
        );
        if !source.is_file() {
            warnings.push(format!(
                "附件 {} 在打包时不存在，已跳过。",
                asset.relative_path
            ));
            continue;
        }
        let mut input = match File::open(&source) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warnings.push(format!(
                    "附件 {} 在打包时不存在，已跳过。",
                    asset.relative_path
                ));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        writer.start_file(validate_asset_entry_name(&asset.archive_entry)?, options)?;
        asset_bytes += std::io::copy(&mut input, &mut writer)?;
        included_assets.push(asset);
    }
    manifest.assets = included_assets;
    manifest.warnings = warnings.clone();
    writer.start_file(MANIFEST_ENTRY, options)?;
    serde_json::to_writer_pretty(&mut writer, &manifest)?;
    writer.finish()?.flush()?;

    Ok(SessionArchiveExportResult {
        archive_path: output_path.to_string_lossy().to_string(),
        session_count: manifest.sessions.len(),
        requested_count: requested.len(),
        related_session_count: manifest.sessions.len().saturating_sub(requested.len()),
        rollout_bytes,
        asset_count: manifest.assets.len(),
        asset_bytes,
        warnings,
    })
}

pub fn inspect_session_archive(
    codex_home: &Path,
    archive_path: &Path,
) -> anyhow::Result<SessionArchivePreview> {
    let manifest = read_manifest(archive_path)?;
    validate_manifest(&manifest)?;
    let archive_entries = archive_entry_names(archive_path)?;
    let existing_ids = existing_session_ids(codex_home)?;
    let missing_project_roots = project_roots(&manifest.project_state)
        .into_iter()
        .filter(|path| {
            looks_like_local_windows_path(path) && !normalized_windows_path(path).exists()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut warnings = manifest.warnings.clone();
    let sessions = manifest
        .sessions
        .iter()
        .map(|session| {
            let has_rollout = session_rollout_entry_is_available(session, &archive_entries);
            if session.rollout_entry.is_some() && !has_rollout {
                warnings.push(format!(
                    "会话 {} 的 rollout 文件不在迁移包中，导入时将跳过。",
                    session.id
                ));
            }
            SessionArchivePreviewItem {
                id: session.id.clone(),
                title: session.title.clone(),
                cwd: session.cwd.clone(),
                archived: session.archived,
                conflict: existing_ids.contains(&session.id),
                has_rollout,
            }
        })
        .collect::<Vec<_>>();
    let asset_count = manifest
        .assets
        .iter()
        .filter(|asset| {
            archive_entries.contains(&normalized_archive_entry_name(&asset.archive_entry))
        })
        .count();
    for asset in &manifest.assets {
        if !archive_entries.contains(&normalized_archive_entry_name(&asset.archive_entry)) {
            warnings.push(format!(
                "附件 {} 不在迁移包中，导入时将跳过。",
                asset.relative_path
            ));
        }
    }
    let conflict_count = sessions.iter().filter(|session| session.conflict).count();
    Ok(SessionArchivePreview {
        archive_path: archive_path.to_string_lossy().to_string(),
        format_version: manifest.format_version,
        exported_at: manifest.exported_at,
        source_version: manifest.source_version,
        session_count: sessions.len(),
        asset_count,
        conflict_count,
        missing_project_roots,
        sessions,
        warnings,
    })
}

pub fn import_session_archive(
    codex_home: &Path,
    archive_path: &Path,
    options: &SessionArchiveImportOptions,
) -> anyhow::Result<SessionArchiveImportResult> {
    let manifest = read_manifest(archive_path)?;
    validate_manifest(&manifest)?;
    let archive_entries = archive_entry_names(archive_path)?;
    let overwrite = match options.conflict_policy.trim().to_ascii_lowercase().as_str() {
        "skip" | "" => false,
        "overwrite" => true,
        other => bail!("不支持的会话冲突策略：{other}"),
    };
    fs::create_dir_all(codex_home)?;
    let destination_provider =
        crate::provider_sync::read_current_provider(&codex_home.join("config.toml"));
    let existing_ids = existing_session_ids(codex_home)?;
    let regular_thread_ids = manifest
        .tables
        .get("threads")
        .into_iter()
        .flatten()
        .filter_map(|row| string_field(row, "id"))
        .collect::<BTreeSet<_>>();
    let missing_rollout_thread_ids = manifest
        .sessions
        .iter()
        .filter(|session| regular_thread_ids.contains(&session.id))
        .filter(|session| {
            session.rollout_entry.as_deref().is_none_or(str::is_empty)
                || session
                    .rollout_relative_path
                    .as_deref()
                    .is_none_or(str::is_empty)
                || session.rollout_entry.as_deref().is_none_or(|entry| {
                    !archive_entries.contains(&normalized_archive_entry_name(entry))
                })
        })
        .map(|session| session.id.clone())
        .collect::<BTreeSet<_>>();
    let metadata_ids = manifest
        .sessions
        .iter()
        .filter(|session| !missing_rollout_thread_ids.contains(&session.id))
        .map(|session| session.id.clone())
        .collect::<BTreeSet<_>>();
    let imported_ids = metadata_ids
        .iter()
        .filter(|session| overwrite || !existing_ids.contains(*session))
        .cloned()
        .collect::<BTreeSet<_>>();
    let skipped_count = manifest.sessions.len().saturating_sub(imported_ids.len());
    let missing_rollout_warnings = missing_rollout_thread_ids
        .iter()
        .map(|id| format!("已跳过会话 {id}：迁移包中没有可恢复的 rollout 文件。"))
        .collect::<Vec<_>>();
    if imported_ids.is_empty() {
        let mut warnings = missing_rollout_warnings;
        let backup_path = if metadata_ids.is_empty() {
            PathBuf::new()
        } else {
            let backup_path =
                create_import_safety_backup(codex_home, &existing_ids, &metadata_ids)?;
            let repair_result = (|| -> anyhow::Result<Vec<String>> {
                let warnings = merge_project_state_and_index(
                    codex_home,
                    &manifest,
                    &metadata_ids,
                    &options.path_mappings,
                )?;
                rebind_existing_session_providers(
                    codex_home,
                    &metadata_ids,
                    &destination_provider,
                )?;
                Ok(warnings)
            })();
            match repair_result {
                Ok(repair_warnings) => warnings.extend(repair_warnings),
                Err(error) => {
                    let rollback =
                        rollback_failed_import(codex_home, &backup_path, &metadata_ids, &[], &[]);
                    return Err(import_error_with_rollback_context(
                        error,
                        rollback,
                        &backup_path,
                    ));
                }
            }
            warnings.push(
                "归档中的其余会话均已存在，未覆盖数据库内容；已重新合并项目归属和会话索引。"
                    .to_string(),
            );
            backup_path
        };
        if metadata_ids.is_empty() && manifest.sessions.len() > missing_rollout_thread_ids.len() {
            warnings.push("归档中的其余会话均已存在，未写入任何内容。".to_string());
        }
        warnings.extend(manifest.warnings);
        return Ok(SessionArchiveImportResult {
            archive_path: archive_path.to_string_lossy().to_string(),
            skipped_count,
            backup_path: backup_path.to_string_lossy().to_string(),
            warnings,
            ..SessionArchiveImportResult::default()
        });
    }

    let destination_databases = discover_database_paths(codex_home);
    let imported_thread_ids = manifest
        .tables
        .get("threads")
        .into_iter()
        .flatten()
        .filter_map(|row| string_field(row, "id"))
        .filter(|id| imported_ids.contains(id))
        .collect::<BTreeSet<_>>();
    if !imported_thread_ids.is_empty()
        && select_target_database(&destination_databases, "threads")?.is_none()
    {
        bail!("目标 Codex 会话数据库尚未初始化。请先启动一次新安装的 Codex，完全退出后再导入。");
    }
    let automation_only = imported_ids
        .difference(&imported_thread_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !automation_only.is_empty()
        && select_target_database(&destination_databases, "automation_runs")?.is_none()
    {
        bail!("目标 Codex 自动化数据库尚未初始化。请先启动一次新安装的 Codex，完全退出后再导入。");
    }

    let backup_path = create_import_safety_backup(codex_home, &existing_ids, &metadata_ids)?;
    let mut created_rollouts = Vec::new();
    let mut created_assets = Vec::new();
    let import_result = (|| -> anyhow::Result<(usize, usize, Vec<String>, Vec<String>)> {
        if overwrite {
            delete_existing_sessions(codex_home, &imported_ids, &backup_path)?;
        }

        let mut archive = ZipArchive::new(File::open(archive_path)?)?;
        let mut rollout_destinations = HashMap::new();
        let mut restored_rollout_count = 0usize;
        let mut restored_asset_count = 0usize;
        let mut warnings = Vec::new();
        for session in &manifest.sessions {
            if !imported_ids.contains(&session.id) {
                continue;
            }
            let (Some(entry_name), Some(relative_path)) = (
                session.rollout_entry.as_deref(),
                session.rollout_relative_path.as_deref(),
            ) else {
                continue;
            };
            let entry_name = validate_archive_entry_name(entry_name)?;
            let requested_relative =
                safe_relative_path(Path::new(relative_path)).unwrap_or_else(|| {
                    PathBuf::from("sessions")
                        .join("imported")
                        .join(format!("rollout-{}.jsonl", session.id))
                });
            let mut destination = codex_home.join(requested_relative);
            if destination.exists() && !overwrite {
                destination = codex_home.join("sessions").join("imported").join(format!(
                    "rollout-{}-{}.jsonl",
                    session.id,
                    Uuid::new_v4().simple()
                ));
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let temp_path =
                destination.with_extension(format!("import-{}.tmp", Uuid::new_v4().simple()));
            let Some(entry_index) = find_archive_entry_index(&mut archive, &entry_name) else {
                warnings.push(format!(
                    "已跳过会话 {}：迁移包缺少 rollout 文件 {}。",
                    session.id, entry_name
                ));
                continue;
            };
            let entry = archive.by_index(entry_index)?;
            write_mapped_rollout(
                entry,
                &temp_path,
                &options.path_mappings,
                &destination_provider,
            )?;
            if destination.exists() {
                fs::remove_file(&destination)?;
            }
            fs::rename(&temp_path, &destination)?;
            created_rollouts.push(destination.clone());
            rollout_destinations.insert(
                session.id.clone(),
                destination.to_string_lossy().to_string(),
            );
            restored_rollout_count += 1;
        }
        for asset in &manifest.assets {
            let entry_name = validate_asset_entry_name(&asset.archive_entry)?;
            let relative_path = safe_relative_path(Path::new(&asset.relative_path))
                .ok_or_else(|| anyhow::anyhow!("附件目标路径不安全：{}", asset.relative_path))?;
            let destination = codex_home.join(relative_path);
            if destination.exists() {
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let Some(entry_index) = find_archive_entry_index(&mut archive, &entry_name) else {
                warnings.push(format!(
                    "已跳过附件 {}：迁移包缺少文件 {}。",
                    asset.relative_path, entry_name
                ));
                continue;
            };
            let mut entry = archive.by_index(entry_index)?;
            let mut output = BufWriter::new(File::create(&destination)?);
            std::io::copy(&mut entry, &mut output)?;
            output.flush()?;
            created_assets.push(destination);
            restored_asset_count += 1;
        }

        let mut missing_tables = BTreeSet::new();
        let db_paths = discover_database_paths(codex_home);
        let agent_job_ids = agent_job_ids_for_sessions(&manifest.tables, &imported_ids);
        let mut table_names = IMPORT_TABLE_ORDER
            .iter()
            .copied()
            .filter(|table| manifest.tables.contains_key(*table))
            .collect::<Vec<_>>();
        table_names.extend(
            manifest
                .tables
                .keys()
                .map(String::as_str)
                .filter(|table| !IMPORT_TABLE_ORDER.contains(table)),
        );
        for table in table_names {
            let Some(rows) = manifest.tables.get(table) else {
                continue;
            };
            let rows = rows
                .iter()
                .filter(|row| row_belongs_to_transfer(table, row, &imported_ids, &agent_job_ids))
                .cloned()
                .collect::<Vec<_>>();
            if rows.is_empty() {
                continue;
            }
            let Some(db_path) = select_target_database(&db_paths, table)? else {
                missing_tables.insert(table.to_string());
                continue;
            };
            let mut db = Connection::open(&db_path)?;
            let tx = db.transaction()?;
            for mut row in rows {
                map_json_paths(&mut row, &options.path_mappings);
                if table == "threads" {
                    if let Some(id) = string_field(&row, "id") {
                        if let Some(path) = rollout_destinations.get(&id) {
                            if let Some(object) = row.as_object_mut() {
                                object.insert("rollout_path".to_string(), json!(path));
                            }
                        }
                    }
                    if let Some(object) = row.as_object_mut() {
                        object.insert("model_provider".to_string(), json!(destination_provider));
                    }
                }
                insert_row_adaptive(&tx, table, &row)?;
            }
            tx.commit()?;
        }
        warnings.extend(merge_project_state_and_index(
            codex_home,
            &manifest,
            &metadata_ids,
            &options.path_mappings,
        )?);
        let preexisting_import_ids = metadata_ids
            .intersection(&existing_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        rebind_existing_session_providers(
            codex_home,
            &preexisting_import_ids,
            &destination_provider,
        )?;
        Ok((
            restored_rollout_count,
            restored_asset_count,
            missing_tables.into_iter().collect(),
            warnings,
        ))
    })();
    let (restored_rollout_count, restored_asset_count, missing_tables, mut warnings) =
        match import_result {
            Ok(result) => result,
            Err(error) => {
                let rollback = rollback_failed_import(
                    codex_home,
                    &backup_path,
                    &metadata_ids,
                    &created_rollouts,
                    &created_assets,
                );
                return Err(import_error_with_rollback_context(
                    error,
                    rollback,
                    &backup_path,
                ));
            }
        };
    warnings.extend(missing_rollout_warnings);
    warnings.extend(manifest.warnings);
    Ok(SessionArchiveImportResult {
        archive_path: archive_path.to_string_lossy().to_string(),
        imported_count: imported_ids.len(),
        skipped_count,
        overwritten_count: if overwrite {
            imported_ids.intersection(&existing_ids).count()
        } else {
            0
        },
        restored_rollout_count,
        restored_asset_count,
        missing_tables,
        backup_path: backup_path.to_string_lossy().to_string(),
        warnings,
    })
}

fn normalized_archive_output_path(path: &Path) -> PathBuf {
    if path.extension().and_then(|value| value.to_str()) == Some("codexbackup") {
        path.to_path_buf()
    } else {
        PathBuf::from(format!("{}.codexbackup", path.to_string_lossy()))
    }
}

fn collect_database_rows(
    db_paths: &[PathBuf],
) -> anyhow::Result<(
    BTreeMap<String, ThreadCandidate>,
    BTreeMap<String, Vec<Value>>,
)> {
    let mut threads = BTreeMap::new();
    let mut tables = RELATED_TABLES
        .iter()
        .map(|table| ((*table).to_string(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for db_path in db_paths {
        let Ok(db) = open_read_only(db_path) else {
            continue;
        };
        if has_table(&db, "threads")? {
            for row in select_all_rows(&db, "threads")? {
                let Some(id) = string_field(&row, "id") else {
                    continue;
                };
                let score = row_timestamp(&row);
                if threads
                    .get(&id)
                    .is_none_or(|candidate: &ThreadCandidate| score >= candidate.score)
                {
                    threads.insert(id, ThreadCandidate { row, score });
                }
            }
        }
        for table in RELATED_TABLES {
            if has_table(&db, table)? {
                let target = tables.entry((*table).to_string()).or_default();
                for row in select_all_rows(&db, table)? {
                    if !target.contains(&row) {
                        target.push(row);
                    }
                }
            }
        }
    }
    Ok((threads, tables))
}

fn expand_related_session_ids(
    requested: &BTreeSet<String>,
    tables: &BTreeMap<String, Vec<Value>>,
    all_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    let edges = tables
        .get("thread_spawn_edges")
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                string_field(row, "parent_thread_id")?,
                string_field(row, "child_thread_id")?,
            ))
        })
        .collect::<Vec<_>>();
    let mut result = requested.clone();
    let mut queue = requested.iter().cloned().collect::<VecDeque<_>>();
    while let Some(parent) = queue.pop_front() {
        for (_, child) in edges.iter().filter(|(candidate, _)| candidate == &parent) {
            if all_ids.contains(child) && result.insert(child.clone()) {
                queue.push_back(child.clone());
            }
        }
    }
    result
}

fn agent_job_ids_for_sessions(
    tables: &BTreeMap<String, Vec<Value>>,
    session_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    tables
        .get("agent_job_items")
        .into_iter()
        .flatten()
        .filter(|row| {
            string_field(row, "assigned_thread_id").is_some_and(|id| session_ids.contains(&id))
        })
        .filter_map(|row| string_field(row, "job_id"))
        .collect()
}

fn row_belongs_to_transfer(
    table: &str,
    row: &Value,
    session_ids: &BTreeSet<String>,
    agent_job_ids: &BTreeSet<String>,
) -> bool {
    match table {
        "threads" => string_field(row, "id").is_some_and(|id| session_ids.contains(&id)),
        "agent_jobs" => string_field(row, "id").is_some_and(|id| agent_job_ids.contains(&id)),
        "thread_spawn_edges" => {
            let parent = string_field(row, "parent_thread_id");
            let child = string_field(row, "child_thread_id");
            parent.is_some_and(|id| session_ids.contains(&id))
                && child.is_some_and(|id| session_ids.contains(&id))
        }
        "agent_job_items" => {
            string_field(row, "assigned_thread_id").is_some_and(|id| session_ids.contains(&id))
        }
        _ => string_field(row, "thread_id").is_some_and(|id| session_ids.contains(&id)),
    }
}

fn export_project_state(
    codex_home: &Path,
    session_ids: &BTreeSet<String>,
    project_paths: &BTreeSet<String>,
) -> anyhow::Result<Value> {
    let path = codex_home.join(".codex-global-state.json");
    let source = read_json_object(&path)?;
    let mut exported = Map::new();
    let mut relevant_projects = project_paths.clone();
    for key in THREAD_STATE_ARRAY_KEYS {
        if let Some(values) = source.get(*key).and_then(Value::as_array) {
            exported.insert(
                (*key).to_string(),
                Value::Array(
                    values
                        .iter()
                        .filter(|value| value.as_str().is_some_and(|id| session_ids.contains(id)))
                        .cloned()
                        .collect(),
                ),
            );
        }
    }
    for key in THREAD_STATE_OBJECT_KEYS {
        if let Some(values) = source.get(*key).and_then(Value::as_object) {
            let filtered = values
                .iter()
                .filter(|(id, _)| session_ids.contains(*id))
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect::<Map<_, _>>();
            for value in filtered.values() {
                collect_project_identifiers(value, &mut relevant_projects);
            }
            exported.insert((*key).to_string(), Value::Object(filtered));
        }
    }
    for key in PROJECT_STATE_ARRAY_KEYS {
        if let Some(values) = source.get(*key).and_then(Value::as_array) {
            exported.insert(
                (*key).to_string(),
                Value::Array(
                    values
                        .iter()
                        .filter(|value| {
                            value.as_str().is_some_and(|candidate| {
                                project_identifier_relevant(candidate, &relevant_projects)
                            })
                        })
                        .cloned()
                        .collect(),
                ),
            );
        }
    }
    for key in PROJECT_STATE_OBJECT_KEYS {
        if let Some(values) = source.get(*key).and_then(Value::as_object) {
            exported.insert(
                (*key).to_string(),
                Value::Object(
                    values
                        .iter()
                        .filter(|(candidate, _)| {
                            project_identifier_relevant(candidate, &relevant_projects)
                        })
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
            );
        }
    }
    Ok(Value::Object(exported))
}

fn collect_project_identifiers(value: &Value, identifiers: &mut BTreeSet<String>) {
    match value {
        Value::String(value) if !value.trim().is_empty() => {
            identifiers.insert(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_project_identifiers(value, identifiers);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_project_identifiers(value, identifiers);
            }
        }
        _ => {}
    }
}

fn project_identifier_relevant(candidate: &str, relevant: &BTreeSet<String>) -> bool {
    let candidate = strip_windows_extended_prefix(candidate)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    relevant.iter().any(|value| {
        let value = strip_windows_extended_prefix(value)
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase();
        candidate == value
            || candidate.starts_with(&format!("{value}/"))
            || value.starts_with(&format!("{candidate}/"))
    })
}

fn export_session_index(
    codex_home: &Path,
    session_ids: &BTreeSet<String>,
) -> anyhow::Result<Vec<Value>> {
    let path = codex_home.join("session_index.jsonl");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let reader = BufReader::new(File::open(path)?);
    Ok(reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|row| string_field(row, "id").is_some_and(|id| session_ids.contains(&id)))
        .collect())
}

fn read_manifest(archive_path: &Path) -> anyhow::Result<SessionArchiveManifest> {
    let file = File::open(archive_path)
        .with_context(|| format!("无法打开会话归档：{}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file).context("会话归档不是有效的 ZIP 文件")?;
    let entry = archive
        .by_name(MANIFEST_ENTRY)
        .context("会话归档缺少 manifest.json")?;
    Ok(serde_json::from_reader(entry).context("无法读取会话归档清单")?)
}

fn archive_entry_names(archive_path: &Path) -> anyhow::Result<HashSet<String>> {
    let file = File::open(archive_path)
        .with_context(|| format!("无法打开会话归档：{}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file).context("会话归档不是有效的 ZIP 文件")?;
    let mut names = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        names.insert(normalized_archive_entry_name(entry.name()));
    }
    Ok(names)
}

fn find_archive_entry_index<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    expected_name: &str,
) -> Option<usize> {
    let expected_name = normalized_archive_entry_name(expected_name);
    (0..archive.len()).find(|index| {
        archive
            .by_index(*index)
            .ok()
            .is_some_and(|entry| normalized_archive_entry_name(entry.name()) == expected_name)
    })
}

fn normalized_archive_entry_name(name: &str) -> String {
    name.replace('\\', "/")
}

fn session_rollout_entry_is_available(
    session: &ManifestSession,
    archive_entries: &HashSet<String>,
) -> bool {
    session
        .rollout_entry
        .as_deref()
        .is_some_and(|entry| archive_entries.contains(&normalized_archive_entry_name(entry)))
        && session
            .rollout_relative_path
            .as_deref()
            .is_some_and(|path| !path.trim().is_empty())
}

fn validate_manifest(manifest: &SessionArchiveManifest) -> anyhow::Result<()> {
    if manifest.format != ARCHIVE_FORMAT {
        bail!("不是 Codex++ 会话迁移包");
    }
    if manifest.format_version != ARCHIVE_FORMAT_VERSION {
        bail!("不支持的会话迁移包版本：{}", manifest.format_version);
    }
    let mut ids = HashSet::new();
    for session in &manifest.sessions {
        if session.id.trim().is_empty() || !ids.insert(session.id.clone()) {
            bail!("会话迁移包包含空或重复的会话 ID");
        }
        if let Some(entry) = &session.rollout_entry {
            validate_archive_entry_name(entry)?;
        }
    }
    for asset in &manifest.assets {
        validate_asset_entry_name(&asset.archive_entry)?;
        safe_relative_path(Path::new(&asset.relative_path))
            .ok_or_else(|| anyhow::anyhow!("附件目标路径不安全：{}", asset.relative_path))?;
    }
    Ok(())
}

fn collect_referenced_assets(
    codex_home: &Path,
    sessions: &[ManifestSession],
    rollout_sources: &HashMap<String, PathBuf>,
) -> Vec<ManifestAsset> {
    let allowed_roots = [
        "attachments",
        "codex-remote-attachments",
        "generated_images",
    ]
    .into_iter()
    .map(|name| codex_home.join(name))
    .collect::<Vec<_>>();
    let mut paths = BTreeSet::new();
    for session in sessions {
        let rollout = rollout_sources.get(&session.id).cloned().or_else(|| {
            session
                .rollout_relative_path
                .as_deref()
                .and_then(|path| safe_relative_path(Path::new(path)))
                .map(|relative| codex_home.join(relative))
        });
        let Some(rollout) = rollout else { continue };
        let Ok(file) = File::open(rollout) else {
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            collect_asset_strings(&value, &allowed_roots, &mut paths);
        }
    }
    paths
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(codex_home).ok()?.to_path_buf();
            let relative = safe_relative_path(&relative)?;
            let relative_string = path_to_archive_string(&relative);
            Some(ManifestAsset {
                archive_entry: format!("assets/{relative_string}"),
                relative_path: relative_string,
            })
        })
        .collect()
}

fn collect_asset_strings(value: &Value, allowed_roots: &[PathBuf], paths: &mut BTreeSet<PathBuf>) {
    match value {
        Value::String(value) => {
            let value = value.strip_prefix("file:///").unwrap_or(value);
            let path = PathBuf::from(value.replace('/', "\\"));
            if path.is_file() && allowed_roots.iter().any(|root| path.starts_with(root)) {
                paths.insert(path);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_asset_strings(value, allowed_roots, paths);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_asset_strings(value, allowed_roots, paths);
            }
        }
        _ => {}
    }
}

fn existing_session_ids(codex_home: &Path) -> anyhow::Result<BTreeSet<String>> {
    let db_paths = discover_database_paths(codex_home);
    let (threads, tables) = collect_database_rows(&db_paths)?;
    let mut ids = threads.keys().cloned().collect::<BTreeSet<_>>();
    ids.extend(
        tables
            .get("automation_runs")
            .into_iter()
            .flatten()
            .filter_map(|row| string_field(row, "thread_id")),
    );
    Ok(ids)
}

fn create_import_safety_backup(
    codex_home: &Path,
    existing_ids: &BTreeSet<String>,
    imported_ids: &BTreeSet<String>,
) -> anyhow::Result<PathBuf> {
    let root = codex_home.join("backups").join(format!(
        "session-import-{}-{}",
        Utc::now().format("%Y%m%d-%H%M%S"),
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root)?;
    for db_path in discover_database_paths(codex_home) {
        for source in [
            db_path.clone(),
            PathBuf::from(format!("{}-wal", db_path.to_string_lossy())),
            PathBuf::from(format!("{}-shm", db_path.to_string_lossy())),
        ] {
            if source.is_file() {
                let relative = source.strip_prefix(codex_home).unwrap_or(&source);
                let destination = root.join(relative);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&source, destination)?;
            }
        }
    }
    for name in [".codex-global-state.json", "session_index.jsonl"] {
        let source = codex_home.join(name);
        if source.is_file() {
            fs::copy(&source, root.join(name))?;
        }
    }
    let conflicts = existing_ids
        .intersection(imported_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !conflicts.is_empty() {
        let (threads, _) = collect_database_rows(&discover_database_paths(codex_home))?;
        for id in conflicts {
            let Some(path) = threads
                .get(&id)
                .and_then(|candidate| string_field(&candidate.row, "rollout_path"))
                .map(PathBuf::from)
            else {
                continue;
            };
            if path.is_file() {
                let destination = root
                    .join("rollouts")
                    .join(format!("{}.jsonl", safe_archive_id(&id)));
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(path, destination)?;
            }
        }
    }
    Ok(root)
}

fn rollback_failed_import(
    codex_home: &Path,
    backup_path: &Path,
    session_ids: &BTreeSet<String>,
    created_rollouts: &[PathBuf],
    created_assets: &[PathBuf],
) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for path in created_rollouts
        .iter()
        .rev()
        .chain(created_assets.iter().rev())
    {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!(
                    "删除本次创建的文件 {} 失败：{error}",
                    path.display()
                ));
            }
        }
    }
    if let Err(error) = restore_import_safety_backup(codex_home, backup_path, session_ids) {
        errors.push(format!("恢复安全备份失败：{error:#}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(errors.join("；"))
    }
}

fn import_error_with_rollback_context(
    error: anyhow::Error,
    rollback: anyhow::Result<()>,
    backup_path: &Path,
) -> anyhow::Error {
    match rollback {
        Ok(()) => error.context(format!(
            "会话导入未完成，已恢复导入前状态；安全备份位于 {}",
            backup_path.display()
        )),
        Err(rollback_error) => anyhow::anyhow!(
            "会话导入未完成：{error:#}；自动回滚未完整完成：{rollback_error:#}；请从安全备份恢复：{}",
            backup_path.display()
        ),
    }
}

fn restore_import_safety_backup(
    codex_home: &Path,
    backup_path: &Path,
    session_ids: &BTreeSet<String>,
) -> anyhow::Result<()> {
    for backup_database in discover_database_paths(backup_path) {
        let relative = backup_database.strip_prefix(backup_path)?;
        let destination = codex_home.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        for suffix in ["-wal", "-shm"] {
            let current_sidecar =
                PathBuf::from(format!("{}{}", destination.to_string_lossy(), suffix));
            match fs::remove_file(&current_sidecar) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("清理数据库 sidecar 失败：{}", current_sidecar.display())
                    });
                }
            }
        }
        fs::copy(&backup_database, &destination).with_context(|| {
            format!(
                "恢复会话数据库 {} 到 {} 失败",
                backup_database.display(),
                destination.display()
            )
        })?;
        for suffix in ["-wal", "-shm"] {
            let backup_sidecar =
                PathBuf::from(format!("{}{}", backup_database.to_string_lossy(), suffix));
            if backup_sidecar.is_file() {
                let destination_sidecar =
                    PathBuf::from(format!("{}{}", destination.to_string_lossy(), suffix));
                fs::copy(&backup_sidecar, &destination_sidecar).with_context(|| {
                    format!("恢复数据库 sidecar 失败：{}", destination_sidecar.display())
                })?;
            }
        }
    }

    for name in [".codex-global-state.json", "session_index.jsonl"] {
        let source = backup_path.join(name);
        let destination = codex_home.join(name);
        if source.is_file() {
            fs::copy(&source, &destination)
                .with_context(|| format!("恢复会话元数据失败：{}", destination.display()))?;
        } else {
            match fs::remove_file(&destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("移除导入新建的会话元数据失败：{}", destination.display())
                    });
                }
            }
        }
    }

    let (threads, _) = collect_database_rows(&discover_database_paths(codex_home))?;
    for id in session_ids {
        let backup_rollout = backup_path
            .join("rollouts")
            .join(format!("{}.jsonl", safe_archive_id(id)));
        if !backup_rollout.is_file() {
            continue;
        }
        let Some(raw_path) = threads
            .get(id)
            .and_then(|candidate| string_field(&candidate.row, "rollout_path"))
        else {
            continue;
        };
        let destination = resolve_trusted_rollout_restore_destination(codex_home, &raw_path)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&backup_rollout, &destination)
            .with_context(|| format!("恢复会话 rollout 失败：{}", destination.display()))?;
    }
    Ok(())
}

fn resolve_trusted_rollout_restore_destination(
    codex_home: &Path,
    raw_path: &str,
) -> anyhow::Result<PathBuf> {
    let normalized = PathBuf::from(strip_windows_extended_prefix(raw_path.trim()));
    let candidate = if normalized.is_absolute() {
        normalized
    } else {
        let relative = safe_relative_path(&normalized)
            .ok_or_else(|| anyhow::anyhow!("会话 rollout 相对路径不安全：{raw_path}"))?;
        codex_home.join(relative)
    };
    let canonical_home = codex_home.canonicalize()?;
    let parent = candidate
        .parent()
        .ok_or_else(|| anyhow::anyhow!("会话 rollout 没有父目录：{}", candidate.display()))?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("会话 rollout 父目录不存在：{}", parent.display()))?;
    let relative_parent = canonical_parent
        .strip_prefix(&canonical_home)
        .map_err(|_| {
            anyhow::anyhow!(
                "拒绝恢复 Codex home 之外的会话 rollout：{}",
                candidate.display()
            )
        })?;
    let trusted_directory = relative_parent
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .is_some_and(|directory| matches!(directory, "sessions" | "archived_sessions"));
    if !trusted_directory {
        bail!("拒绝恢复非会话 rollout 路径：{}", candidate.display());
    }
    Ok(candidate)
}

fn delete_existing_sessions(
    codex_home: &Path,
    session_ids: &BTreeSet<String>,
    _backup_path: &Path,
) -> anyhow::Result<()> {
    let db_paths = discover_database_paths(codex_home);
    let (threads, _) = collect_database_rows(&db_paths)?;
    let rollout_paths = session_ids
        .iter()
        .filter_map(|id| threads.get(id))
        .filter_map(|candidate| string_field(&candidate.row, "rollout_path"))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    for db_path in db_paths {
        let mut db = Connection::open(&db_path)?;
        let tx = db.transaction()?;
        for id in session_ids {
            delete_rows_for_session(&tx, "thread_dynamic_tools", "thread_id", id)?;
            delete_rows_for_session(&tx, "thread_goals", "thread_id", id)?;
            delete_rows_for_session(&tx, "stage1_outputs", "thread_id", id)?;
            delete_rows_for_session(&tx, "automation_runs", "thread_id", id)?;
            delete_rows_for_session(&tx, "inbox_items", "thread_id", id)?;
            if has_table(&tx, "thread_spawn_edges")? {
                tx.execute(
                    "DELETE FROM thread_spawn_edges WHERE parent_thread_id = ?1 OR child_thread_id = ?1",
                    [id],
                )?;
            }
            if has_table(&tx, "agent_job_items")?
                && table_columns(&tx, "agent_job_items")?
                    .contains(&"assigned_thread_id".to_string())
            {
                tx.execute(
                    "UPDATE agent_job_items SET assigned_thread_id = NULL WHERE assigned_thread_id = ?1",
                    [id],
                )?;
            }
            delete_rows_for_session(&tx, "threads", "id", id)?;
        }
        tx.commit()?;
    }
    for path in rollout_paths {
        if path.starts_with(codex_home) && path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn delete_rows_for_session(
    db: &Connection,
    table: &str,
    column: &str,
    id: &str,
) -> anyhow::Result<()> {
    if has_table(db, table)? && table_columns(db, table)?.contains(&column.to_string()) {
        db.execute(
            &format!(
                "DELETE FROM \"{}\" WHERE \"{}\" = ?1",
                table.replace('"', "\"\""),
                column.replace('"', "\"\"")
            ),
            [id],
        )?;
    }
    Ok(())
}

fn select_target_database(db_paths: &[PathBuf], table: &str) -> anyhow::Result<Option<PathBuf>> {
    let mut candidates = Vec::new();
    for path in db_paths {
        let Ok(db) = open_read_only(path) else {
            continue;
        };
        if has_table(&db, table)? {
            let modified = path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_millis())
                .unwrap_or_default();
            let rows = db
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{}\"", table.replace('"', "\"\"")),
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or_default();
            candidates.push((modified, rows, path.clone()));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(candidates.pop().map(|(_, _, path)| path))
}

fn insert_row_adaptive(db: &Connection, table: &str, row: &Value) -> anyhow::Result<()> {
    let Some(row) = row.as_object() else {
        return Ok(());
    };
    let available = table_columns(db, table)?
        .into_iter()
        .collect::<HashSet<_>>();
    let columns = row
        .keys()
        .filter(|column| available.contains(*column))
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Ok(());
    }
    let quoted = columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = (0..columns.len())
        .map(|index| format!("?{}", index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .map(|column| OwnedSqlValue(json_to_sql_value(&row[*column])))
        .collect::<Vec<_>>();
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    db.execute(
        &format!("INSERT OR IGNORE INTO \"{table}\" ({quoted}) VALUES ({marks})"),
        refs.as_slice(),
    )?;
    Ok(())
}

fn merge_project_state_and_index(
    codex_home: &Path,
    manifest: &SessionArchiveManifest,
    imported_ids: &BTreeSet<String>,
    mappings: &[SessionPathMapping],
) -> anyhow::Result<Vec<String>> {
    let mut warnings = Vec::new();
    let state_path = codex_home.join(".codex-global-state.json");
    let mut current = Value::Object(read_json_object(&state_path)?);
    let mut imported = manifest.project_state.clone();
    retain_project_state_sessions(&mut imported, imported_ids);
    map_json_paths(&mut imported, mappings);
    map_project_state_path_keys(&mut imported, mappings);
    ensure_imported_project_assignments(&mut imported, manifest, imported_ids, mappings);
    merge_json(&mut current, imported);
    fs::write(&state_path, serde_json::to_vec_pretty(&current)?)?;

    let index_path = codex_home.join("session_index.jsonl");
    let mut rows = BTreeMap::<String, Value>::new();
    let mut invalid_lines = Vec::new();
    if index_path.is_file() {
        for line in BufReader::new(File::open(&index_path)?)
            .lines()
            .map_while(Result::ok)
        {
            match serde_json::from_str::<Value>(&line) {
                Ok(row) => {
                    if let Some(id) = string_field(&row, "id") {
                        rows.insert(id, row);
                    }
                }
                Err(_) if !line.trim().is_empty() => invalid_lines.push(line),
                Err(_) => {}
            }
        }
    }
    for row in &manifest.session_index {
        if let Some(id) = string_field(row, "id") {
            if imported_ids.contains(&id) {
                rows.insert(id, row.clone());
            }
        }
    }
    let index_count_before_rebuild = rows.len();
    for session in &manifest.sessions {
        if imported_ids.contains(&session.id) {
            rows.entry(session.id.clone())
                .or_insert_with(|| session_index_row_from_manifest(session));
        }
    }
    let rebuilt_index_count = rows.len().saturating_sub(index_count_before_rebuild);
    let mut output = BufWriter::new(File::create(&index_path)?);
    for line in invalid_lines {
        writeln!(output, "{line}")?;
    }
    for row in rows.values() {
        writeln!(output, "{}", serde_json::to_string(row)?)?;
    }
    output.flush()?;
    if manifest.session_index.is_empty() {
        warnings.push("归档未包含 session_index.jsonl 条目，已根据会话清单重建索引。".to_string());
    }
    if rebuilt_index_count > 0 {
        warnings.push(format!(
            "已为 {rebuilt_index_count} 条缺少 session_index.jsonl 记录的导入会话重建索引。"
        ));
    }
    Ok(warnings)
}

fn session_index_row_from_manifest(session: &ManifestSession) -> Value {
    let title = if session.title.trim().is_empty() {
        session.id.clone()
    } else {
        session.title.clone()
    };
    let updated_at = session
        .updated_at_ms
        .and_then(chrono::DateTime::<Utc>::from_timestamp_millis)
        .map(|timestamp| timestamp.to_rfc3339())
        .unwrap_or_default();
    json!({
        "id": session.id,
        "thread_name": title,
        "updated_at": updated_at,
    })
}

fn retain_project_state_sessions(state: &mut Value, session_ids: &BTreeSet<String>) {
    let Some(state) = state.as_object_mut() else {
        return;
    };
    for key in THREAD_STATE_ARRAY_KEYS {
        if let Some(values) = state.get_mut(*key).and_then(Value::as_array_mut) {
            values.retain(|value| value.as_str().is_some_and(|id| session_ids.contains(id)));
        }
    }
    for key in THREAD_STATE_OBJECT_KEYS {
        if let Some(values) = state.get_mut(*key).and_then(Value::as_object_mut) {
            values.retain(|id, _| session_ids.contains(id));
        }
    }
}

fn map_project_state_path_keys(state: &mut Value, mappings: &[SessionPathMapping]) {
    let Some(labels) = state
        .get_mut("electron-workspace-root-labels")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let previous = std::mem::take(labels);
    for (path, label) in previous {
        labels.insert(mapped_path(&path, mappings), label);
    }
}

fn ensure_imported_project_assignments(
    state: &mut Value,
    manifest: &SessionArchiveManifest,
    imported_ids: &BTreeSet<String>,
    mappings: &[SessionPathMapping],
) {
    let roots = project_roots(state);
    if roots.is_empty() {
        return;
    }
    let projectless_ids = state
        .get("projectless-thread-ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let existing_assignments = state
        .get("thread-project-assignments")
        .and_then(Value::as_object)
        .map(|assignments| assignments.keys().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    let inferred = manifest
        .tables
        .get("threads")
        .into_iter()
        .flatten()
        .filter_map(|thread| {
            let id = string_field(thread, "id")?;
            if !imported_ids.contains(&id)
                || projectless_ids.contains(id.as_str())
                || existing_assignments.contains(&id)
            {
                return None;
            }
            let cwd = mapped_path(&string_field(thread, "cwd")?, mappings);
            let root = roots
                .iter()
                .filter(|root| project_contains_path(root, &cwd))
                .max_by_key(|root| normalized_project_key(root).len())?
                .clone();
            Some((id, root))
        })
        .collect::<Vec<_>>();
    if inferred.is_empty() {
        return;
    }
    let Some(state) = state.as_object_mut() else {
        return;
    };
    let assignments = state
        .entry("thread-project-assignments")
        .or_insert_with(|| json!({}));
    let Some(assignments) = assignments.as_object_mut() else {
        return;
    };
    for (id, root) in inferred {
        assignments.insert(
            id,
            json!({
                "projectKind": "local",
                "projectId": root,
                "path": root,
                "cwd": root,
                "pendingCoreUpdate": false,
            }),
        );
    }
}

fn project_contains_path(project_root: &str, candidate: &str) -> bool {
    let project_root = normalized_project_key(project_root);
    let candidate = normalized_project_key(candidate);
    candidate == project_root || candidate.starts_with(&format!("{project_root}\\"))
}

fn normalized_project_key(path: &str) -> String {
    strip_windows_extended_prefix(path)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn merge_json(target: &mut Value, incoming: Value) {
    match (target, incoming) {
        (Value::Object(target), Value::Object(incoming)) => {
            for (key, value) in incoming {
                match target.get_mut(&key) {
                    Some(current) => merge_json(current, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (Value::Array(target), Value::Array(incoming)) => {
            for value in incoming {
                if !target.contains(&value) {
                    target.push(value);
                }
            }
        }
        (target, incoming) => *target = incoming,
    }
}

fn write_mapped_rollout<R: Read>(
    mut input: R,
    destination: &Path,
    mappings: &[SessionPathMapping],
    destination_provider: &str,
) -> anyhow::Result<()> {
    let mut original = String::new();
    input.read_to_string(&mut original)?;
    let rewritten = rewrite_rollout_text(&original, mappings, destination_provider)?;
    let mut output = BufWriter::new(File::create(destination)?);
    output.write_all(rewritten.as_bytes())?;
    output.flush()?;
    Ok(())
}

fn rewrite_rollout_text(
    original: &str,
    mappings: &[SessionPathMapping],
    provider: &str,
) -> anyhow::Result<String> {
    let mut rewritten = String::with_capacity(original.len());
    for segment in original.split_inclusive('\n') {
        let (line, newline) = split_jsonl_line_ending(segment);
        match serde_json::from_str::<Value>(line) {
            Ok(mut value) => {
                let before = value.clone();
                map_json_paths(&mut value, mappings);
                set_rollout_model_provider(&mut value, provider);
                if value == before {
                    rewritten.push_str(line);
                } else {
                    rewritten.push_str(&serde_json::to_string(&value)?);
                }
            }
            Err(_) => rewritten.push_str(line),
        }
        rewritten.push_str(newline);
    }
    Ok(rewritten)
}

fn split_jsonl_line_ending(segment: &str) -> (&str, &str) {
    if let Some(line) = segment.strip_suffix("\r\n") {
        (line, "\r\n")
    } else if let Some(line) = segment.strip_suffix('\n') {
        (line, "\n")
    } else {
        (segment, "")
    }
}

fn set_rollout_model_provider(value: &mut Value, provider: &str) {
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return;
    }
    let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) else {
        return;
    };
    payload.insert("model_provider".to_string(), json!(provider));
}

fn rebind_existing_session_providers(
    codex_home: &Path,
    session_ids: &BTreeSet<String>,
    provider: &str,
) -> anyhow::Result<()> {
    if session_ids.is_empty() {
        return Ok(());
    }
    let mut database_plans = Vec::new();
    let mut rollout_paths = BTreeSet::new();
    for db_path in discover_database_paths(codex_home) {
        let db = Connection::open(&db_path)?;
        if !has_table(&db, "threads")? {
            continue;
        }
        let columns = table_columns(&db, "threads")?;
        if !columns.iter().any(|column| column == "id")
            || !columns.iter().any(|column| column == "model_provider")
        {
            continue;
        }
        let has_rollout_path = columns.iter().any(|column| column == "rollout_path");
        let mut rows = Vec::new();
        for id in session_ids {
            let row = if has_rollout_path {
                db.query_row(
                    "SELECT model_provider, rollout_path FROM threads WHERE id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
            } else {
                db.query_row(
                    "SELECT model_provider, NULL FROM threads WHERE id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
            };
            let (old_provider, rollout_path) = match row {
                Ok(row) => row,
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(error) => return Err(error.into()),
            };
            if let Some(path) = rollout_path.filter(|path| !path.trim().is_empty()) {
                rollout_paths.insert(resolve_trusted_rollout_path(codex_home, &path)?);
            }
            rows.push((id.clone(), old_provider));
        }
        if !rows.is_empty() {
            database_plans.push(SessionProviderDatabasePlan {
                path: db_path,
                rows,
            });
        }
    }

    let mut rollout_plans = Vec::new();
    for path in rollout_paths {
        let original = fs::read(&path)?;
        let text = std::str::from_utf8(&original)
            .with_context(|| format!("会话 rollout 不是 UTF-8：{}", path.display()))?;
        let rewritten = rewrite_rollout_text(text, &[], provider)?.into_bytes();
        if rewritten != original {
            rollout_plans.push(SessionProviderRolloutPlan {
                path,
                original,
                rewritten,
            });
        }
    }

    apply_session_provider_rebind_plan(&database_plans, &rollout_plans, provider)
}

struct SessionProviderDatabasePlan {
    path: PathBuf,
    rows: Vec<(String, Option<String>)>,
}

struct SessionProviderRolloutPlan {
    path: PathBuf,
    original: Vec<u8>,
    rewritten: Vec<u8>,
}

fn resolve_trusted_rollout_path(codex_home: &Path, raw_path: &str) -> anyhow::Result<PathBuf> {
    let normalized = PathBuf::from(strip_windows_extended_prefix(raw_path.trim()));
    let candidate = if normalized.is_absolute() {
        normalized
    } else {
        let relative = safe_relative_path(&normalized)
            .ok_or_else(|| anyhow::anyhow!("会话 rollout 相对路径不安全：{raw_path}"))?;
        codex_home.join(relative)
    };
    let canonical_home = codex_home.canonicalize()?;
    let canonical_path = candidate
        .canonicalize()
        .with_context(|| format!("会话 rollout 不存在或不可访问：{}", candidate.display()))?;
    let relative = canonical_path.strip_prefix(&canonical_home).map_err(|_| {
        anyhow::anyhow!(
            "拒绝修改 Codex home 之外的会话 rollout：{}",
            canonical_path.display()
        )
    })?;
    let trusted_directory = relative
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .is_some_and(|directory| matches!(directory, "sessions" | "archived_sessions"));
    if !trusted_directory || !canonical_path.is_file() {
        bail!("拒绝修改非会话 rollout 路径：{}", canonical_path.display());
    }
    Ok(canonical_path)
}

fn apply_session_provider_rebind_plan(
    database_plans: &[SessionProviderDatabasePlan],
    rollout_plans: &[SessionProviderRolloutPlan],
    provider: &str,
) -> anyhow::Result<()> {
    let mut active_databases = Vec::new();
    for (plan_index, plan) in database_plans.iter().enumerate() {
        let db = Connection::open(&plan.path)?;
        if let Err(error) = db.execute_batch("BEGIN IMMEDIATE") {
            rollback_open_provider_transactions(&active_databases);
            return Err(error.into());
        }
        let update_result = (|| -> rusqlite::Result<()> {
            for (id, _) in &plan.rows {
                db.execute(
                    "UPDATE threads SET model_provider = ?1 WHERE id = ?2 AND COALESCE(model_provider, '') <> ?1",
                    (provider, id),
                )?;
            }
            Ok(())
        })();
        if let Err(error) = update_result {
            let _ = db.execute_batch("ROLLBACK");
            rollback_open_provider_transactions(&active_databases);
            return Err(error.into());
        }
        active_databases.push((db, plan_index));
    }

    let mut written_rollouts = Vec::new();
    for (index, plan) in rollout_plans.iter().enumerate() {
        if let Err(error) = codex_plus_core::settings::atomic_write(&plan.path, &plan.rewritten) {
            let restore_error = restore_provider_rollouts(rollout_plans, &written_rollouts).err();
            rollback_open_provider_transactions(&active_databases);
            let error = anyhow::Error::from(error).context("写入会话 rollout provider 失败");
            return Err(with_provider_compensation_errors(
                error,
                restore_error.into_iter().collect(),
            ));
        }
        written_rollouts.push(index);
    }

    let mut committed_plan_indices = Vec::new();
    for position in 0..active_databases.len() {
        let (db, plan_index) = &active_databases[position];
        if let Err(error) = db.execute_batch("COMMIT") {
            let _ = db.execute_batch("ROLLBACK");
            rollback_open_provider_transactions(&active_databases[position + 1..]);
            let mut compensation_errors = Vec::new();
            if let Err(error) =
                compensate_committed_provider_databases(database_plans, &committed_plan_indices)
            {
                compensation_errors.push(error);
            }
            if let Err(error) = restore_provider_rollouts(rollout_plans, &written_rollouts) {
                compensation_errors.push(error);
            }
            return Err(with_provider_compensation_errors(
                error.into(),
                compensation_errors,
            ));
        }
        committed_plan_indices.push(*plan_index);
    }
    Ok(())
}

fn rollback_open_provider_transactions(databases: &[(Connection, usize)]) {
    for (db, _) in databases {
        let _ = db.execute_batch("ROLLBACK");
    }
}

fn restore_provider_rollouts(
    plans: &[SessionProviderRolloutPlan],
    written: &[usize],
) -> anyhow::Result<()> {
    for index in written.iter().rev() {
        let plan = &plans[*index];
        codex_plus_core::settings::atomic_write(&plan.path, &plan.original)
            .with_context(|| format!("恢复会话 rollout 失败：{}", plan.path.display()))?;
    }
    Ok(())
}

fn compensate_committed_provider_databases(
    plans: &[SessionProviderDatabasePlan],
    committed_plan_indices: &[usize],
) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for index in committed_plan_indices.iter().rev() {
        let plan = &plans[*index];
        let result = (|| -> anyhow::Result<()> {
            let mut db = Connection::open(&plan.path)?;
            let tx = db.transaction()?;
            for (id, old_provider) in &plan.rows {
                tx.execute(
                    "UPDATE threads SET model_provider = ?1 WHERE id = ?2",
                    (old_provider, id),
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!(
                "恢复数据库 {} 的原供应商失败：{error:#}",
                plan.path.display()
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(errors.join("；"))
    }
}

fn with_provider_compensation_errors(
    error: anyhow::Error,
    compensation_errors: Vec<anyhow::Error>,
) -> anyhow::Error {
    if compensation_errors.is_empty() {
        return error;
    }
    let details = compensation_errors
        .iter()
        .map(|error| format!("{error:#}"))
        .collect::<Vec<_>>()
        .join("；");
    anyhow::anyhow!("{error:#}；自动补偿未完整完成：{details}")
}

fn map_json_paths(value: &mut Value, mappings: &[SessionPathMapping]) {
    match value {
        Value::String(text) => {
            *text = mapped_path(text, mappings);
        }
        Value::Array(values) => {
            for value in values {
                map_json_paths(value, mappings);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                map_json_paths(value, mappings);
            }
        }
        _ => {}
    }
}

fn mapped_path(value: &str, mappings: &[SessionPathMapping]) -> String {
    let normalized_value = strip_windows_extended_prefix(value);
    for mapping in mappings {
        let from = strip_windows_extended_prefix(mapping.from.trim()).trim_end_matches(['\\', '/']);
        let to = strip_windows_extended_prefix(mapping.to.trim()).trim_end_matches(['\\', '/']);
        if from.is_empty() || to.is_empty() {
            continue;
        }
        if normalized_value.eq_ignore_ascii_case(from) {
            return to.to_string();
        }
        if normalized_value.len() > from.len()
            && let (Some(prefix), Some(suffix)) = (
                normalized_value.get(..from.len()),
                normalized_value.get(from.len()..),
            )
            && prefix.eq_ignore_ascii_case(from)
            && suffix.starts_with(['\\', '/'])
        {
            return format!("{to}{suffix}");
        }
    }
    value.to_string()
}

fn project_roots(state: &Value) -> Vec<String> {
    PROJECT_STATE_ARRAY_KEYS
        .iter()
        .flat_map(|key| {
            state
                .get(*key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn read_json_object(path: &Path) -> anyhow::Result<Map<String, Value>> {
    if !path.is_file() {
        return Ok(Map::new());
    }
    Ok(serde_json::from_slice::<Value>(&fs::read(path)?)?
        .as_object()
        .cloned()
        .unwrap_or_default())
}

fn discover_database_paths(codex_home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in [codex_home.to_path_buf(), codex_home.join("sqlite")] {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("db") | Some("sqlite") | Some("sqlite3")
                )
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn open_read_only(path: &Path) -> anyhow::Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

fn has_table(db: &Connection, table: &str) -> anyhow::Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
            [table],
            |_| Ok(()),
        )
        .is_ok())
}

fn table_columns(db: &Connection, table: &str) -> anyhow::Result<Vec<String>> {
    let mut statement = db.prepare(&format!(
        "PRAGMA table_info(\"{}\")",
        table.replace('"', "\"\"")
    ))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn select_all_rows(db: &Connection, table: &str) -> anyhow::Result<Vec<Value>> {
    let mut statement = db.prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))?;
    let columns = statement
        .column_names()
        .iter()
        .map(|column| column.to_string())
        .collect::<Vec<_>>();
    let rows = statement.query_map([], |row| {
        let mut value = Map::new();
        for (index, column) in columns.iter().enumerate() {
            value.insert(column.clone(), sql_value_to_json(row.get_ref(index)?));
        }
        Ok(Value::Object(value))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn sql_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => json!(String::from_utf8_lossy(value).to_string()),
        ValueRef::Blob(value) => json!({
            "__codexPlusBlobBase64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                value
            )
        }),
    }
}

fn json_to_sql_value(value: &Value) -> SqlValue {
    if let Some(encoded) = value
        .as_object()
        .and_then(|value| value.get("__codexPlusBlobBase64"))
        .and_then(Value::as_str)
    {
        return base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .map(SqlValue::Blob)
            .unwrap_or(SqlValue::Null);
    }
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
        Value::Number(number) => number
            .as_i64()
            .map(SqlValue::Integer)
            .or_else(|| number.as_f64().map(SqlValue::Real))
            .unwrap_or_else(|| SqlValue::Text(number.to_string())),
        Value::String(value) => SqlValue::Text(value.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}

fn row_timestamp(row: &Value) -> i64 {
    row_timestamp_option(row).unwrap_or_default()
}

fn row_timestamp_option(row: &Value) -> Option<i64> {
    ["updated_at_ms", "recency_at_ms", "created_at_ms"]
        .iter()
        .find_map(|key| row.get(*key).and_then(Value::as_i64))
        .or_else(|| {
            ["updated_at", "recency_at", "created_at"]
                .iter()
                .find_map(|key| row.get(*key).and_then(Value::as_i64))
                .map(|value| value.saturating_mul(1000))
        })
}

fn string_field(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn boolish_field(row: &Value, key: &str) -> Option<bool> {
    row.get(key).and_then(|value| {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
    })
}

fn normalize_thread_id(id: &str) -> String {
    id.trim()
        .strip_prefix("local:")
        .unwrap_or(id.trim())
        .to_string()
}

fn safe_archive_id(id: &str) -> String {
    let safe = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        Uuid::new_v4().simple().to_string()
    } else {
        safe
    }
}

fn safe_relative_path(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => safe.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!safe.as_os_str().is_empty()).then_some(safe)
}

fn validate_archive_entry_name(name: &str) -> anyhow::Result<String> {
    let normalized = name.replace('\\', "/");
    let path = safe_relative_path(Path::new(&normalized))
        .ok_or_else(|| anyhow::anyhow!("归档包含不安全的文件路径：{name}"))?;
    let normalized = path_to_archive_string(&path);
    if !normalized.starts_with("sessions/") {
        bail!("归档包含不允许的文件路径：{name}");
    }
    Ok(normalized)
}

fn validate_asset_entry_name(name: &str) -> anyhow::Result<String> {
    let normalized = name.replace('\\', "/");
    let path = safe_relative_path(Path::new(&normalized))
        .ok_or_else(|| anyhow::anyhow!("归档包含不安全的附件路径：{name}"))?;
    let normalized = path_to_archive_string(&path);
    if !normalized.starts_with("assets/") {
        bail!("归档包含不允许的附件路径：{name}");
    }
    Ok(normalized)
}

fn path_to_archive_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn strip_windows_extended_prefix(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

fn normalized_windows_path(path: &str) -> PathBuf {
    PathBuf::from(strip_windows_extended_prefix(path))
}

fn looks_like_local_windows_path(path: &str) -> bool {
    let path = strip_windows_extended_prefix(path);
    (path.len() >= 3 && path.as_bytes()[1] == b':' && matches!(path.as_bytes()[2], b'\\' | b'/'))
        || path.starts_with(r"\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_path_ignores_nonmatching_unicode_text_without_panicking() {
        let mappings = vec![SessionPathMapping {
            from: "abcdefghijklm".to_string(),
            to: r"C:\mapped".to_string(),
        }];

        assert_eq!(
            mapped_path("012345678901什么", &mappings),
            "012345678901什么"
        );
    }

    #[test]
    fn mapped_path_rewrites_matching_unicode_project_path() {
        let mappings = vec![SessionPathMapping {
            from: r"C:\旧项目".to_string(),
            to: r"D:\新项目".to_string(),
        }];

        assert_eq!(
            mapped_path(r"C:\旧项目\资料\会话.jsonl", &mappings),
            r"D:\新项目\资料\会话.jsonl"
        );
    }
}
