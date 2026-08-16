use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use serde::Serialize;
use toml_edit::Item;
use uuid::Uuid;

const SKILLS_DIR: &str = "skills";
const DISABLED_SKILLS_DIR: &str = "skills-disabled";
const SKILL_BACKUPS_DIR: &str = "skill-backups";
const SKILL_INSTALL_STAGING_DIR: &str = ".skill-install-staging";
const SKILL_FILE_NAME: &str = "SKILL.md";
const MAX_ARCHIVE_ENTRIES: usize = 4096;
const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillInventory {
    pub codex_home: String,
    pub user_skills_dir: String,
    pub disabled_skills_dir: String,
    pub skills: Vec<InstalledSkill>,
}

impl SkillInventory {
    pub fn empty(home: &Path) -> Self {
        Self {
            codex_home: home.to_string_lossy().to_string(),
            user_skills_dir: home.join(SKILLS_DIR).to_string_lossy().to_string(),
            disabled_skills_dir: home.join(DISABLED_SKILLS_DIR).to_string_lossy().to_string(),
            skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: String,
    pub enabled: bool,
    pub read_only: bool,
    pub valid: bool,
    pub error: Option<String>,
    pub plugin_id: Option<String>,
    pub invocation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillMetadata {
    name: String,
    description: String,
}

pub fn list_skills(home: &Path) -> anyhow::Result<SkillInventory> {
    let mut skills = Vec::new();
    let user_root = home.join(SKILLS_DIR);
    let disabled_root = home.join(DISABLED_SKILLS_DIR);

    scan_direct_skill_dirs(
        &user_root,
        "user",
        true,
        false,
        Some(".system"),
        None,
        &mut skills,
    )?;
    scan_direct_skill_dirs(
        &disabled_root,
        "user",
        false,
        false,
        None,
        None,
        &mut skills,
    )?;
    scan_direct_skill_dirs(
        &user_root.join(".system"),
        "system",
        true,
        true,
        None,
        None,
        &mut skills,
    )?;
    scan_enabled_plugin_skills(home, &mut skills)?;

    skills.sort_by(|left, right| {
        skill_source_rank(&left.source)
            .cmp(&skill_source_rank(&right.source))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(SkillInventory {
        codex_home: home.to_string_lossy().to_string(),
        user_skills_dir: user_root.to_string_lossy().to_string(),
        disabled_skills_dir: disabled_root.to_string_lossy().to_string(),
        skills,
    })
}

pub fn import_skill(home: &Path, source_path: &Path) -> anyhow::Result<SkillInventory> {
    let source_path = source_path
        .canonicalize()
        .with_context(|| format!("无法读取 Skill 来源：{}", source_path.display()))?;
    let staging_parent = home.join(SKILL_INSTALL_STAGING_DIR);
    fs::create_dir_all(&staging_parent)?;
    let staging_root = staging_parent.join(Uuid::new_v4().to_string());
    fs::create_dir_all(&staging_root)?;
    let cleanup = StagingCleanup(staging_root.clone());

    let skill_source = prepare_skill_source(&source_path, &staging_root)?;
    let metadata = read_skill_metadata(&skill_source.join(SKILL_FILE_NAME))
        .with_context(|| format!("Skill 元数据无效：{}", skill_source.display()))?;
    validate_skill_name(&metadata.name)?;

    let target_root = home.join(SKILLS_DIR);
    fs::create_dir_all(&target_root)?;
    let target = target_root.join(&metadata.name);
    if target.exists() {
        bail!(
            "Skill“{}”已存在：{}。请先卸载或改名后再导入。",
            metadata.name,
            target.display()
        );
    }

    let prepared = staging_root.join("prepared").join(&metadata.name);
    copy_skill_tree(&skill_source, &prepared)?;
    read_skill_metadata(&prepared.join(SKILL_FILE_NAME)).context("复制后的 SKILL.md 校验失败")?;
    fs::rename(&prepared, &target)
        .with_context(|| format!("安装 Skill 到 {} 失败", target.display()))?;

    drop(cleanup);
    list_skills(home)
}

pub fn set_skill_enabled(
    home: &Path,
    skill_id: &str,
    enabled: bool,
) -> anyhow::Result<SkillInventory> {
    let (current_enabled, folder_name) = parse_user_skill_id(skill_id)?;
    if current_enabled == enabled {
        return list_skills(home);
    }

    let source_root = if current_enabled {
        home.join(SKILLS_DIR)
    } else {
        home.join(DISABLED_SKILLS_DIR)
    };
    let target_root = if enabled {
        home.join(SKILLS_DIR)
    } else {
        home.join(DISABLED_SKILLS_DIR)
    };
    let source = checked_child_path(&source_root, folder_name)?;
    ensure_managed_skill_dir(&source, &source_root)?;
    fs::create_dir_all(&target_root)?;
    let target = checked_child_path(&target_root, folder_name)?;
    if target.exists() {
        bail!("目标位置已存在同名 Skill：{}", target.display());
    }

    fs::rename(&source, &target).with_context(|| {
        format!(
            "{} Skill 失败：{}",
            if enabled { "启用" } else { "禁用" },
            folder_name
        )
    })?;
    list_skills(home)
}

pub fn uninstall_skill(home: &Path, skill_id: &str) -> anyhow::Result<(SkillInventory, PathBuf)> {
    let (enabled, folder_name) = parse_user_skill_id(skill_id)?;
    let source_root = if enabled {
        home.join(SKILLS_DIR)
    } else {
        home.join(DISABLED_SKILLS_DIR)
    };
    let source = checked_child_path(&source_root, folder_name)?;
    ensure_managed_skill_dir(&source, &source_root)?;

    let backup_root = home.join(SKILL_BACKUPS_DIR);
    fs::create_dir_all(&backup_root)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup = backup_root.join(format!(
        "{}-{}-{}",
        folder_name,
        timestamp,
        Uuid::new_v4().simple()
    ));
    fs::rename(&source, &backup)
        .with_context(|| format!("卸载 Skill“{}”失败，无法移动到安全备份目录", folder_name))?;

    Ok((list_skills(home)?, backup))
}

fn scan_direct_skill_dirs(
    root: &Path,
    source: &str,
    enabled: bool,
    read_only: bool,
    skip_name: Option<&str>,
    plugin_id: Option<&str>,
    output: &mut Vec<InstalledSkill>,
) -> anyhow::Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let folder_name = entry.file_name().to_string_lossy().to_string();
        if skip_name == Some(folder_name.as_str()) {
            continue;
        }
        let skill_file = path.join(SKILL_FILE_NAME);
        if !skill_file.is_file() {
            continue;
        }
        output.push(installed_skill_from_path(
            &path, source, enabled, read_only, plugin_id,
        ));
    }
    Ok(())
}

fn installed_skill_from_path(
    path: &Path,
    source: &str,
    enabled: bool,
    read_only: bool,
    plugin_id: Option<&str>,
) -> InstalledSkill {
    let folder_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();
    let metadata = read_skill_metadata(&path.join(SKILL_FILE_NAME));
    let (name, description, valid, error) = match metadata {
        Ok(metadata) => (metadata.name, metadata.description, true, None),
        Err(error) => (
            folder_name.clone(),
            String::new(),
            false,
            Some(error.to_string()),
        ),
    };
    let id = match source {
        "user" => format!(
            "{}:{}",
            if enabled { "user" } else { "disabled" },
            folder_name
        ),
        "system" => format!("system:{folder_name}"),
        "plugin" => format!("plugin:{}:{folder_name}", plugin_id.unwrap_or("unknown")),
        _ => format!("{source}:{folder_name}"),
    };
    InstalledSkill {
        id,
        invocation: format!("${name}"),
        name,
        description,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        enabled,
        read_only,
        valid,
        error,
        plugin_id: plugin_id.map(str::to_string),
    }
}

fn scan_enabled_plugin_skills(home: &Path, output: &mut Vec<InstalledSkill>) -> anyhow::Result<()> {
    for plugin_id in enabled_plugin_ids(home) {
        let Some((plugin_name, marketplace)) = plugin_id.split_once('@') else {
            continue;
        };
        let plugin_versions_root = home
            .join("plugins")
            .join("cache")
            .join(marketplace)
            .join(plugin_name);
        let Some(plugin_root) = newest_plugin_cache_dir(&plugin_versions_root)? else {
            continue;
        };
        scan_direct_skill_dirs(
            &plugin_root.join(SKILLS_DIR),
            "plugin",
            true,
            true,
            None,
            Some(&plugin_id),
            output,
        )?;
    }
    Ok(())
}

fn enabled_plugin_ids(home: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return BTreeSet::new();
    };
    let Some(plugins) = doc.get("plugins").and_then(Item::as_table) else {
        return BTreeSet::new();
    };
    plugins
        .iter()
        .filter_map(|(id, item)| {
            item.get("enabled")
                .and_then(Item::as_bool)
                .unwrap_or(false)
                .then(|| id.to_string())
        })
        .collect()
}

fn newest_plugin_cache_dir(root: &Path) -> anyhow::Result<Option<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            candidates.push((modified, entry.path()));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    Ok(candidates
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| path.join(SKILLS_DIR).is_dir()))
}

fn prepare_skill_source(source: &Path, staging_root: &Path) -> anyhow::Result<PathBuf> {
    if source.is_dir() {
        return find_single_skill_root(source, "所选目录");
    }

    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if file_name.eq_ignore_ascii_case(SKILL_FILE_NAME) {
        let single_file_root = staging_root.join("single-file");
        fs::create_dir_all(&single_file_root)?;
        fs::copy(source, single_file_root.join(SKILL_FILE_NAME))?;
        return Ok(single_file_root);
    }
    if source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        let extracted_root = staging_root.join("archive");
        extract_skill_archive(source, &extracted_root)?;
        return find_single_skill_root(&extracted_root, "ZIP Skill 包");
    }

    bail!("请选择包含 SKILL.md 的目录、SKILL.md 文件或 .zip Skill 包");
}

fn extract_skill_archive(archive_path: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    let file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).context("无法读取 ZIP Skill 包")?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        bail!("ZIP Skill 包文件数量过多");
    }
    let mut total_uncompressed = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        total_uncompressed = total_uncompressed.saturating_add(entry.size());
        if total_uncompressed > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
            bail!("ZIP Skill 包解压后超过 128 MiB 限制");
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            bail!("ZIP Skill 包不能包含符号链接");
        }
        let Some(relative_path) = entry.enclosed_name() else {
            bail!("ZIP Skill 包包含不安全路径");
        };
        let output_path = destination.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&output_path)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(&output_path)?;
        std::io::copy(&mut entry, &mut output)?;
        output.flush()?;
    }
    Ok(())
}

fn find_single_skill_root(root: &Path, source_label: &str) -> anyhow::Result<PathBuf> {
    let mut matches = Vec::new();
    collect_skill_roots(root, 0, &mut matches)?;
    match matches.len() {
        0 => bail!("{source_label}中没有找到 SKILL.md"),
        1 => Ok(matches.remove(0)),
        count => bail!("{source_label}包含 {count} 个 Skill，请分别导入"),
    }
}

fn collect_skill_roots(
    root: &Path,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if depth > 4 || matches.len() > 1 {
        return Ok(());
    }
    if root.join(SKILL_FILE_NAME).is_file() {
        matches.push(root.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_skill_roots(&entry.path(), depth + 1, matches)?;
        }
    }
    Ok(())
}

fn copy_skill_tree(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!("Skill 目录不能包含符号链接：{}", source_path.display());
        }
        if file_type.is_dir() {
            copy_skill_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "复制 Skill 文件失败：{} -> {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn read_skill_metadata(path: &Path) -> anyhow::Result<SkillMetadata> {
    let mut file =
        fs::File::open(path).with_context(|| format!("缺少 {}", path.to_string_lossy()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("SKILL.md 不是有效 UTF-8：{}", path.display()))?;
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        bail!("SKILL.md 缺少 YAML frontmatter");
    }

    let mut name = String::new();
    let mut description = String::new();
    let mut collecting_description = false;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if collecting_description && indented {
            let value = line.trim();
            if !value.is_empty() {
                if !description.is_empty() {
                    description.push(' ');
                }
                description.push_str(value);
            }
            continue;
        }
        collecting_description = false;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = unquote_yaml_scalar(value.trim());
        match key {
            "name" => name = value,
            "description" => {
                description = if value == "|" || value == ">" {
                    String::new()
                } else {
                    value
                };
                collecting_description = true;
            }
            _ => {}
        }
    }

    if name.trim().is_empty() {
        bail!("SKILL.md frontmatter 缺少 name");
    }
    if description.trim().is_empty() {
        bail!("SKILL.md frontmatter 缺少 description");
    }
    Ok(SkillMetadata {
        name: name.trim().to_string(),
        description: description.trim().to_string(),
    })
}

fn unquote_yaml_scalar(value: &str) -> String {
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    if name.len() > 80 {
        bail!("Skill name 不能超过 80 个字符");
    }
    if name == ".system"
        || name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        bail!("Skill name 只能包含字母、数字、点、下划线和连字符");
    }
    Ok(())
}

fn parse_user_skill_id(skill_id: &str) -> anyhow::Result<(bool, &str)> {
    let (prefix, folder_name) = skill_id
        .split_once(':')
        .ok_or_else(|| anyhow!("无效 Skill ID"))?;
    validate_path_component(folder_name)?;
    match prefix {
        "user" => Ok((true, folder_name)),
        "disabled" => Ok((false, folder_name)),
        _ => bail!("只能管理用户安装的 Skill"),
    }
}

fn validate_path_component(value: &str) -> anyhow::Result<()> {
    let mut components = Path::new(value).components();
    if value.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        bail!("无效 Skill 路径");
    }
    Ok(())
}

fn checked_child_path(root: &Path, child: &str) -> anyhow::Result<PathBuf> {
    validate_path_component(child)?;
    Ok(root.join(child))
}

fn ensure_managed_skill_dir(path: &Path, expected_root: &Path) -> anyhow::Result<()> {
    if !path.join(SKILL_FILE_NAME).is_file() {
        bail!("Skill 不存在或缺少 SKILL.md：{}", path.display());
    }
    let canonical_path = path.canonicalize()?;
    let canonical_root = expected_root.canonicalize()?;
    if canonical_path.parent() != Some(canonical_root.as_path()) {
        bail!("拒绝管理 Skill 根目录之外的路径");
    }
    Ok(())
}

fn skill_source_rank(source: &str) -> usize {
    match source {
        "user" => 0,
        "system" => 1,
        "plugin" => 2,
        _ => 3,
    }
}

struct StagingCleanup(PathBuf);

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, folder: &str, name: &str, description: &str) -> PathBuf {
        let path = root.join(folder);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join(SKILL_FILE_NAME),
            format!("---\nname: \"{name}\"\ndescription: \"{description}\"\n---\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn lists_user_system_disabled_and_enabled_plugin_skills() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write_skill(&home.join(SKILLS_DIR), "custom", "custom", "Custom skill");
        write_skill(
            &home.join(SKILLS_DIR).join(".system"),
            "builtin",
            "builtin",
            "Built-in skill",
        );
        write_skill(
            &home.join(DISABLED_SKILLS_DIR),
            "paused",
            "paused",
            "Paused skill",
        );
        write_skill(
            &home
                .join("plugins/cache/market/demo/1.0.0")
                .join(SKILLS_DIR),
            "plugin-skill",
            "plugin-skill",
            "Plugin skill",
        );
        fs::write(
            home.join("config.toml"),
            "[plugins.\"demo@market\"]\nenabled = true\n",
        )
        .unwrap();

        let inventory = list_skills(home).unwrap();

        assert_eq!(inventory.skills.len(), 4);
        assert!(
            inventory
                .skills
                .iter()
                .any(|skill| skill.id == "user:custom" && skill.enabled && !skill.read_only)
        );
        assert!(
            inventory
                .skills
                .iter()
                .any(|skill| skill.id == "disabled:paused" && !skill.enabled)
        );
        assert!(
            inventory
                .skills
                .iter()
                .any(|skill| skill.id == "system:builtin" && skill.read_only)
        );
        assert!(inventory.skills.iter().any(|skill| {
            skill.source == "plugin" && skill.plugin_id.as_deref() == Some("demo@market")
        }));
    }

    #[test]
    fn imports_skill_directory_with_supporting_files() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        write_skill(temp.path(), "source", "imported-skill", "Imported skill");
        fs::create_dir_all(source.join("scripts")).unwrap();
        fs::write(source.join("scripts/run.js"), "console.log('ok');").unwrap();
        let home = temp.path().join("home");

        let inventory = import_skill(&home, &source).unwrap();

        assert!(
            home.join("skills/imported-skill/SKILL.md").is_file(),
            "SKILL.md should be installed"
        );
        assert!(
            home.join("skills/imported-skill/scripts/run.js").is_file(),
            "supporting files should be copied"
        );
        assert_eq!(inventory.skills[0].id, "user:imported-skill");
    }

    #[test]
    fn imports_single_skill_md() {
        let temp = tempfile::tempdir().unwrap();
        let source = write_skill(temp.path(), "source", "single-skill", "Single file skill")
            .join(SKILL_FILE_NAME);
        let home = temp.path().join("home");

        import_skill(&home, &source).unwrap();

        assert!(home.join("skills/single-skill/SKILL.md").is_file());
    }

    #[test]
    fn imports_folder_with_one_nested_skill() {
        let temp = tempfile::tempdir().unwrap();
        let repository_root = temp.path().join("repository");
        write_skill(
            &repository_root.join("skills"),
            "nested",
            "nested-skill",
            "Nested skill",
        );
        let home = temp.path().join("home");

        import_skill(&home, &repository_root).unwrap();

        assert!(home.join("skills/nested-skill/SKILL.md").is_file());
    }

    #[test]
    fn disables_and_reenables_user_skill_by_moving_it_out_of_discovery_root() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write_skill(&home.join(SKILLS_DIR), "custom", "custom", "Custom skill");

        let disabled = set_skill_enabled(home, "user:custom", false).unwrap();
        assert!(home.join("skills-disabled/custom/SKILL.md").is_file());
        assert_eq!(disabled.skills[0].id, "disabled:custom");
        assert!(!disabled.skills[0].enabled);

        let enabled = set_skill_enabled(home, "disabled:custom", true).unwrap();
        assert!(home.join("skills/custom/SKILL.md").is_file());
        assert_eq!(enabled.skills[0].id, "user:custom");
        assert!(enabled.skills[0].enabled);
    }

    #[test]
    fn uninstall_moves_user_skill_to_backup() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        write_skill(&home.join(SKILLS_DIR), "custom", "custom", "Custom skill");

        let (inventory, backup) = uninstall_skill(home, "user:custom").unwrap();

        assert!(inventory.skills.is_empty());
        assert!(backup.join(SKILL_FILE_NAME).is_file());
        assert!(!home.join("skills/custom").exists());
    }

    #[test]
    fn rejects_archive_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("bad.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("../SKILL.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(b"---\nname: bad\ndescription: bad\n---\n")
            .unwrap();
        archive.finish().unwrap();

        let error = import_skill(&temp.path().join("home"), &archive_path).unwrap_err();

        assert!(error.to_string().contains("不安全路径"));
    }
}
