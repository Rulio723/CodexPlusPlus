#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

use anyhow::{Context, ensure};
use sha2::{Digest, Sha256};
use toml_edit::{Array, DocumentMut, Item, Table};

const BUNDLED_MARKETPLACE: &str = "openai-bundled";
const BUNDLED_MARKETPLACE_PLUGINS: &[&str] = &["browser", "chrome", "computer-use", "latex"];
const COMPUTER_USE_PLUGINS: &[&str] = &[
    "browser@openai-bundled",
    "chrome@openai-bundled",
    "computer-use@openai-bundled",
];
const COMPUTER_USE_EXE: &str = "codex-computer-use.exe";
const COMPUTER_USE_CLIENT_SCRIPT: &str = "computer-use-client.mjs";
const SKY_HELPER_EXE_RELATIVE_PATH: &str = "bin/windows/codex-computer-use.exe";
const SKY_HELPER_TRANSPORT_RELATIVE_PATH: &str =
    "dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js";
const SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT: &str =
    "./dist/project/cua/sky_js/src/targets/windows/internal/computer_use_client_base.js";
const SKY_INTERNAL_COMPUTER_USE_CLIENT_IMPORT: &str =
    "@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/computer_use_client_base.js";
const SKY_PACKAGE_EXPORTS_BACKUP: &str = "package.json.bak-codexpp-runtime-exports";
const ADMIN_HELPER_HOOK_BEGIN: &str = "/* codex-plus-admin-computer-use:begin */";
const ADMIN_HELPER_HOOK_END: &str = "/* codex-plus-admin-computer-use:end */";
#[cfg(test)]
const SUPPORTED_SKY_VERSION: &str = "0.4.20";
const LEGACY_HELPER_TRANSPORT_LAUNCH_TEMPLATE: &str = "const i=s(e(this,w,\"f\"),e(this,v,\"f\"),{env:null==e(this,y,\"f\")?void 0:Object.assign(Object.assign({},P().env),e(this,y,\"f\")),stdio:[\"pipe\",\"pipe\",\"pipe\"],windowsHide:!0});";
const LEGACY_HELPER_TRANSPORT_LAUNCH_SHA256: &str =
    "f983f085eea6e1d6a976e11ce67c19db6aabb6e6c7ba50e1276127db7945390e";
const LEGACY_HELPER_TRANSPORT_SHA256: &str =
    "b0a6ef3de918b83798bad9459824723f12796249e0e8620c8b66af2ccca1969e";
const SKY_062_HELPER_TRANSPORT_LAUNCH_TEMPLATE: &str = "const i=s(e(this,w,\"f\"),e(this,v,\"f\"),{env:null==e(this,y,\"f\")?void 0:Object.assign(Object.assign({},H().env),e(this,y,\"f\")),stdio:[\"pipe\",\"pipe\",\"pipe\"],windowsHide:!0});";
const SKY_062_HELPER_TRANSPORT_LAUNCH_SHA256: &str =
    "87319217fea133ef77d2a544434e063120caac4375c4e7fadfbffdbd3fd6869a";
const SKY_062_HELPER_TRANSPORT_SHA256: &str =
    "6423ba834f18139d55cdac2290c91cd9b24b568332b07cddd2a7eda043702b7c";
// 同一 @oai/sky 版本可能随官方 Codex 发布包携带不同的签名 helper 构建；两枚都必须显式列入 allowlist。
const SKY_062_HELPER_SHA256: &[&str] = &[
    "627b317ccfd3c7386a2d5bc4fb4e97ff30e30425945a7a5370006ad89cf3605a",
    "463d54ddb8a351cb206cb4bebf4943f63e1bc8087d310d102c5fae417b255eb4",
];
const SKY_066_HELPER_SHA256: &[&str] =
    &["be488e66c38e12fa46850ee48c1f5e44ecdb0a3a64042e064e3a1a1da286ac42"];
const SKY_066_HELPER_TRANSPORT_SHA256: &str =
    "7bc54c5bb7f49661fb1f501c6832f5490620501464d3f1593a361a85c7f66b39";
const SKY_0611_HELPER_TRANSPORT_LAUNCH_TEMPLATE: &str = "const i=s(e(this,w,\"f\"),e(this,v,\"f\"),{env:null==e(this,y,\"f\")?void 0:Object.assign(Object.assign({},J().env),e(this,y,\"f\")),stdio:[\"pipe\",\"pipe\",\"pipe\"],windowsHide:!0});";
const SKY_0611_HELPER_TRANSPORT_LAUNCH_SHA256: &str =
    "4ce69185bcd92f0fc88ac4a98c55bcaeb2a5dc6631b0cfbd34ed0f82ac3585b7";
const SKY_0611_HELPER_SHA256: &[&str] =
    &["7a95d14ebf992955d8ab8e6c57a75545ed7d18e864b0f5c1b9fe7f47685bd897"];
const SKY_0611_HELPER_TRANSPORT_SHA256: &str =
    "56ac031983d85e4718f10c5a814923afe2cb4ead649466eef02b1b4d4cf63e40";

#[cfg(test)]
const HELPER_TRANSPORT_LAUNCH_TEMPLATE: &str = LEGACY_HELPER_TRANSPORT_LAUNCH_TEMPLATE;
#[cfg(test)]
const SUPPORTED_HELPER_TRANSPORT_SHA256: &str = LEGACY_HELPER_TRANSPORT_SHA256;

#[derive(Debug, Clone, Copy)]
struct SupportedComputerUseContract {
    sky_version: &'static str,
    helper_sha256: &'static [&'static str],
    transport_sha256: &'static str,
    launch_template: &'static str,
    launch_template_sha256: &'static str,
    process_expression: &'static str,
}

const SUPPORTED_COMPUTER_USE_CONTRACTS: &[SupportedComputerUseContract] = &[
    SupportedComputerUseContract {
        sky_version: "0.4.20",
        helper_sha256: &["f2b2f56fcd1699b0fa32dec3214a56a1d36b937a2ecf58cc822ab4a904551e03"],
        transport_sha256: LEGACY_HELPER_TRANSPORT_SHA256,
        launch_template: LEGACY_HELPER_TRANSPORT_LAUNCH_TEMPLATE,
        launch_template_sha256: LEGACY_HELPER_TRANSPORT_LAUNCH_SHA256,
        process_expression: "P()",
    },
    SupportedComputerUseContract {
        sky_version: "0.5.2",
        helper_sha256: &["2c4cac168200520c2752058177ea9fe7d1ccf9a26b7287dddff669d41ca9af16"],
        transport_sha256: LEGACY_HELPER_TRANSPORT_SHA256,
        launch_template: LEGACY_HELPER_TRANSPORT_LAUNCH_TEMPLATE,
        launch_template_sha256: LEGACY_HELPER_TRANSPORT_LAUNCH_SHA256,
        process_expression: "P()",
    },
    SupportedComputerUseContract {
        sky_version: "0.6.2",
        helper_sha256: SKY_062_HELPER_SHA256,
        transport_sha256: SKY_062_HELPER_TRANSPORT_SHA256,
        launch_template: SKY_062_HELPER_TRANSPORT_LAUNCH_TEMPLATE,
        launch_template_sha256: SKY_062_HELPER_TRANSPORT_LAUNCH_SHA256,
        process_expression: "H()",
    },
    SupportedComputerUseContract {
        sky_version: "0.6.6",
        helper_sha256: SKY_066_HELPER_SHA256,
        transport_sha256: SKY_066_HELPER_TRANSPORT_SHA256,
        launch_template: LEGACY_HELPER_TRANSPORT_LAUNCH_TEMPLATE,
        launch_template_sha256: LEGACY_HELPER_TRANSPORT_LAUNCH_SHA256,
        process_expression: "P()",
    },
    SupportedComputerUseContract {
        sky_version: "0.6.11",
        helper_sha256: SKY_0611_HELPER_SHA256,
        transport_sha256: SKY_0611_HELPER_TRANSPORT_SHA256,
        launch_template: SKY_0611_HELPER_TRANSPORT_LAUNCH_TEMPLATE,
        launch_template_sha256: SKY_0611_HELPER_TRANSPORT_LAUNCH_SHA256,
        process_expression: "J()",
    },
];
const ADMIN_HELPER_TRANSPORT_BACKUP: &str = "helper_transport.js.bak-codex-plus-admin";

fn supported_computer_use_contract(
    sky_version: &str,
) -> Option<&'static SupportedComputerUseContract> {
    SUPPORTED_COMPUTER_USE_CONTRACTS
        .iter()
        .find(|contract| contract.sky_version == sky_version)
}

pub(crate) fn supported_helper_sha256s(sky_version: &str) -> Option<&'static [&'static str]> {
    supported_computer_use_contract(sky_version).map(|contract| contract.helper_sha256)
}

fn supported_transport_sha256(sky_version: &str) -> Option<&'static str> {
    supported_computer_use_contract(sky_version).map(|contract| contract.transport_sha256)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminComputerUseArtifacts {
    pub helper_exe: PathBuf,
    pub helper_transport: PathBuf,
    pub sky_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstalledAdminComputerUseHook {
    pub transport_path: PathBuf,
    pub backup_path: PathBuf,
    pub original_hash: String,
    pub patched_hash: String,
}

#[derive(Debug)]
pub(crate) struct OwnedAdminComputerUseHook {
    installed: InstalledAdminComputerUseHook,
    transport: Option<crate::admin_secure_io::SecureFileLease>,
    backup: Option<crate::admin_secure_io::SecureFileLease>,
}

impl OwnedAdminComputerUseHook {
    pub(crate) fn installed(&self) -> &InstalledAdminComputerUseHook {
        &self.installed
    }

    pub(crate) fn restore(mut self) -> anyhow::Result<()> {
        let mut transport = self
            .transport
            .take()
            .context("computer_use_contract_incompatible")?;
        let mut backup = self
            .backup
            .take()
            .context("computer_use_contract_incompatible")?;
        let backup_bytes = backup.read_all()?;
        ensure!(
            format!("{:x}", Sha256::digest(&backup_bytes)) == self.installed.original_hash,
            "computer_use_contract_incompatible"
        );
        let patched_bytes = transport.read_all()?;
        ensure!(
            format!("{:x}", Sha256::digest(&patched_bytes)) == self.installed.patched_hash,
            "computer_use_contract_incompatible"
        );
        let patched =
            String::from_utf8(patched_bytes).context("computer_use_contract_incompatible")?;
        ensure!(
            remove_admin_helper_transport_patch(&patched)?.as_bytes() == backup_bytes,
            "computer_use_contract_incompatible"
        );
        transport.replace_contents(&backup_bytes)?;
        backup.delete()?;
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct AdminComputerUseHookRollback(Option<OwnedAdminComputerUseHook>);

impl AdminComputerUseHookRollback {
    pub(crate) fn installed(&self) -> &InstalledAdminComputerUseHook {
        self.0.as_ref().expect("hook rollback disarmed").installed()
    }

    pub(crate) fn commit(mut self) -> OwnedAdminComputerUseHook {
        self.0.take().expect("hook rollback disarmed")
    }
}

impl Drop for AdminComputerUseHookRollback {
    fn drop(&mut self) {
        if let Some(owned) = self.0.take() {
            let _ = owned.restore();
        }
    }
}

use crate::admin_mode::computer_use::ComputerUseHookOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardResult {
    pub changed: bool,
    pub notify_exe: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardArtifacts {
    pub notify_exe: Option<PathBuf>,
    pub marketplace_path: Option<PathBuf>,
    pub sky_package_json: Option<PathBuf>,
    pub runtime_exports_needed: bool,
}

fn patch_admin_helper_transport(
    contents: &str,
    sky_version: &str,
    descriptor_path: &Path,
) -> anyhow::Result<String> {
    if contents.contains(ADMIN_HELPER_HOOK_BEGIN) {
        ensure!(
            contents.matches(ADMIN_HELPER_HOOK_BEGIN).count() == 1
                && contents.matches(ADMIN_HELPER_HOOK_END).count() == 1,
            "computer_use_contract_incompatible"
        );
        let restored = remove_admin_helper_transport_patch(contents)?;
        let expected = patch_admin_helper_transport(&restored, sky_version, descriptor_path)?;
        ensure!(expected == contents, "computer_use_contract_incompatible");
        return Ok(expected);
    }
    let contract = supported_computer_use_contract(sky_version)
        .context("computer_use_contract_incompatible")?;
    let launch_count = contents.matches(contract.launch_template).count();
    ensure!(launch_count == 1, "computer_use_contract_incompatible");
    let digest = format!("{:x}", Sha256::digest(contract.launch_template.as_bytes()));
    ensure!(
        digest == contract.launch_template_sha256,
        "computer_use_contract_incompatible"
    );
    let descriptor = serde_json::to_string(&descriptor_path.to_string_lossy().as_ref())?;
    let process_expression = contract.process_expression;
    let replacement = format!(
        "{ADMIN_HELPER_HOOK_BEGIN}const originalCommand=e(this,w,\"f\"),originalArgs=e(this,v,\"f\");let adminDescriptor;try{{adminDescriptor=JSON.parse({process_expression}.getBuiltinModule(\"node:fs\").readFileSync({descriptor},\"utf8\"))}}catch{{throw new Error(\"administrator Computer Use unavailable\")}}if(!adminDescriptor||typeof adminDescriptor.shimPath!==\"string\"||!adminDescriptor.shimPath||typeof adminDescriptor.pipeName!==\"string\"||!adminDescriptor.pipeName||typeof adminDescriptor.sessionId!==\"string\"||!adminDescriptor.sessionId||typeof adminDescriptor.proofPath!==\"string\"||!adminDescriptor.proofPath||typeof originalCommand!==\"string\"||!originalCommand||!Array.isArray(originalArgs)||!originalArgs.every(value=>typeof value===\"string\"))throw new Error(\"administrator Computer Use unavailable\");const adminCommand=adminDescriptor.shimPath,adminArgs=[\"computer-use-client\",\"--pipe\",adminDescriptor.pipeName,\"--session\",adminDescriptor.sessionId,\"--proof-file\",adminDescriptor.proofPath,\"--\",originalCommand,...originalArgs];const i=s(adminCommand,adminArgs,{{env:null==e(this,y,\"f\")?void 0:Object.assign(Object.assign({{}},{process_expression}.env),e(this,y,\"f\")),stdio:[\"pipe\",\"pipe\",\"pipe\"],windowsHide:!0}});{ADMIN_HELPER_HOOK_END}"
    );
    Ok(contents.replacen(contract.launch_template, &replacement, 1))
}

fn remove_admin_helper_transport_patch(contents: &str) -> anyhow::Result<String> {
    let Some(begin) = contents.find(ADMIN_HELPER_HOOK_BEGIN) else {
        ensure!(
            !contents.contains(ADMIN_HELPER_HOOK_END),
            "computer_use_contract_incompatible"
        );
        return Ok(contents.to_owned());
    };
    let end_start = contents[begin..]
        .find(ADMIN_HELPER_HOOK_END)
        .map(|offset| begin + offset)
        .context("computer_use_contract_incompatible")?;
    ensure!(
        contents[begin + ADMIN_HELPER_HOOK_BEGIN.len()..end_start]
            .find(ADMIN_HELPER_HOOK_BEGIN)
            .is_none()
            && contents[end_start + ADMIN_HELPER_HOOK_END.len()..]
                .find(ADMIN_HELPER_HOOK_END)
                .is_none(),
        "computer_use_contract_incompatible"
    );
    let hook_body = &contents[begin + ADMIN_HELPER_HOOK_BEGIN.len()..end_start];
    let contract = SUPPORTED_COMPUTER_USE_CONTRACTS
        .iter()
        .find(|contract| {
            hook_body.contains(&format!("{}.getBuiltinModule", contract.process_expression))
        })
        .context("computer_use_contract_incompatible")?;
    let mut restored = String::with_capacity(contents.len());
    restored.push_str(&contents[..begin]);
    restored.push_str(contract.launch_template);
    restored.push_str(&contents[end_start + ADMIN_HELPER_HOOK_END.len()..]);
    Ok(restored)
}

fn expected_patched_admin_helper_transport(
    original: &str,
    descriptor_path: &Path,
) -> Option<String> {
    SUPPORTED_COMPUTER_USE_CONTRACTS
        .iter()
        .find_map(|contract| {
            patch_admin_helper_transport(original, contract.sky_version, descriptor_path).ok()
        })
}

fn sky_root_from_artifact_path(path: &Path) -> anyhow::Result<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("sky"))
                && ancestor
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("@oai"))
        })
        .map(Path::to_path_buf)
        .context("computer_use_contract_incompatible")
}

pub(crate) fn resolve_admin_computer_use_artifacts(
    home: &Path,
) -> anyhow::Result<AdminComputerUseArtifacts> {
    let mut candidates = Vec::new();
    if let Some(configured) = configured_computer_use_notify_exe(home) {
        candidates.push(configured);
    }
    #[cfg(windows)]
    candidates.extend(computer_use_notify_exe_candidates_windows(home));
    #[cfg(not(windows))]
    if let Some(discovered) = find_computer_use_notify_exe(home) {
        candidates.push(discovered);
    }
    let mut seen = std::collections::HashSet::new();
    let mut last_error = None;
    for helper_exe in candidates {
        let key = helper_exe.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        match admin_computer_use_artifacts_from_helper(helper_exe) {
            Ok(artifacts) => return Ok(artifacts),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("computer_use_contract_incompatible")))
}

fn admin_computer_use_artifacts_from_helper(
    helper_exe: PathBuf,
) -> anyhow::Result<AdminComputerUseArtifacts> {
    let sky_root = sky_root_from_artifact_path(&helper_exe)?;
    let helper_transport = sky_root.join(SKY_HELPER_TRANSPORT_RELATIVE_PATH);
    let package: serde_json::Value = serde_json::from_slice(
        &std::fs::read(sky_root.join("package.json"))
            .context("computer_use_contract_incompatible")?,
    )?;
    let sky_version = package["version"]
        .as_str()
        .context("computer_use_contract_incompatible")?
        .to_owned();
    validate_admin_computer_use_artifacts(&helper_exe, &helper_transport, &sky_version)?;
    Ok(AdminComputerUseArtifacts {
        helper_exe,
        helper_transport,
        sky_version,
    })
}

fn trusted_admin_computer_use_runtime_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        home.join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use"),
    ];
    #[cfg(windows)]
    if recovery_home_matches_default(home)
        && let Some(local_app_data) = std::env::var_os("LOCALAPPDATA")
    {
        roots.push(
            PathBuf::from(local_app_data)
                .join("OpenAI")
                .join("Codex")
                .join("runtimes")
                .join("cua_node"),
        );
    }
    roots
}

fn trusted_admin_computer_use_runtime_root(root: &Path) -> anyhow::Result<Option<PathBuf>> {
    #[cfg(windows)]
    {
        trusted_recovery_root(root)
    }
    #[cfg(not(windows))]
    {
        let metadata = match std::fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "computer_use_contract_incompatible"
        );
        Ok(Some(std::fs::canonicalize(root)?))
    }
}

fn path_is_within_trusted_admin_runtime(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        path_is_within(path, root)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

fn ensure_trusted_admin_runtime_path(root: &Path, descendant: &Path) -> anyhow::Result<()> {
    let mut reached_root = false;
    for ancestor in descendant.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)?;
        #[cfg(windows)]
        ensure!(
            !metadata_is_reparse(&metadata),
            "computer_use_contract_incompatible"
        );
        #[cfg(not(windows))]
        ensure!(
            !metadata.file_type().is_symlink(),
            "computer_use_contract_incompatible"
        );
        if paths_equal(ancestor, root) {
            reached_root = true;
            break;
        }
    }
    ensure!(reached_root, "computer_use_contract_incompatible");
    Ok(())
}

pub(crate) fn resolve_admin_computer_use_artifacts_for_transport(
    home: &Path,
    transport_path: &Path,
    original_hash: &str,
) -> anyhow::Result<AdminComputerUseArtifacts> {
    let sky_root = sky_root_from_artifact_path(transport_path)?;
    let expected_transport = sky_root.join(SKY_HELPER_TRANSPORT_RELATIVE_PATH);
    ensure!(
        paths_equal(transport_path, &expected_transport),
        "computer_use_contract_incompatible"
    );
    let canonical_sky_root =
        std::fs::canonicalize(&sky_root).context("computer_use_contract_incompatible")?;
    let mut trusted_root = None;
    for root in trusted_admin_computer_use_runtime_roots(home) {
        let Some(canonical_root) = trusted_admin_computer_use_runtime_root(&root)? else {
            continue;
        };
        if path_is_within_trusted_admin_runtime(&canonical_sky_root, &canonical_root) {
            ensure_trusted_admin_runtime_path(&root, &sky_root)?;
            trusted_root = Some(canonical_root);
            break;
        }
    }
    let _trusted_root = trusted_root.context("computer_use_contract_incompatible")?;
    let package: serde_json::Value = serde_json::from_slice(
        &std::fs::read(sky_root.join("package.json"))
            .context("computer_use_contract_incompatible")?,
    )?;
    let sky_version = package["version"]
        .as_str()
        .context("computer_use_contract_incompatible")?
        .to_owned();
    let expected_original_hash =
        supported_transport_sha256(&sky_version).context("computer_use_contract_incompatible")?;
    ensure!(
        original_hash.eq_ignore_ascii_case(expected_original_hash),
        "computer_use_contract_incompatible"
    );
    Ok(AdminComputerUseArtifacts {
        helper_exe: sky_root.join(SKY_HELPER_EXE_RELATIVE_PATH),
        helper_transport: expected_transport,
        sky_version,
    })
}

pub(crate) fn validate_admin_computer_use_artifacts(
    helper_exe: &Path,
    helper_transport: &Path,
    sky_version: &str,
) -> anyhow::Result<()> {
    let expected_helper_sha256s =
        supported_helper_sha256s(sky_version).context("computer_use_contract_incompatible")?;
    let expected_transport_sha256 =
        supported_transport_sha256(sky_version).context("computer_use_contract_incompatible")?;
    let helper = std::fs::canonicalize(helper_exe).context("computer_use_contract_incompatible")?;
    let transport =
        canonical_owned_path(helper_transport).context("computer_use_contract_incompatible")?;
    let sky_root = helper
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .context("computer_use_contract_incompatible")?;
    ensure!(
        helper
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(COMPUTER_USE_EXE))
            && transport == sky_root.join(SKY_HELPER_TRANSPORT_RELATIVE_PATH),
        "computer_use_contract_incompatible"
    );
    let transport_bytes = if transport.exists() {
        std::fs::read(&transport)?
    } else {
        std::fs::read(transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP))?
    };
    let transport_hash = format!("{:x}", Sha256::digest(&transport_bytes));
    let transport_supported = if transport_hash == expected_transport_sha256 {
        true
    } else {
        let patched =
            String::from_utf8(transport_bytes).context("computer_use_contract_incompatible")?;
        let restored = remove_admin_helper_transport_patch(&patched)?;
        format!("{:x}", Sha256::digest(restored.as_bytes())) == expected_transport_sha256
    };
    let helper_hash = sha256_file(&helper)?;
    ensure!(
        expected_helper_sha256s
            .iter()
            .any(|expected| helper_hash.eq_ignore_ascii_case(expected))
            && transport_supported,
        "computer_use_contract_incompatible"
    );
    Ok(())
}

#[cfg(windows)]
const PACKAGED_COMPUTER_USE_RUNTIME_IDENTITY_FILES: &[&str] =
    &["manifest.json", "bin/node.exe", "bin/node_repl.exe"];

#[cfg(windows)]
const PACKAGED_COMPUTER_USE_RUNTIME_CRITICAL_FILES: &[&str] = &[
    "bin/node_modules/@oai/sky/bin/windows/codex-computer-use.exe",
    "bin/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js",
];

#[cfg(windows)]
fn packaged_computer_use_runtime_fingerprint(source: &Path) -> anyhow::Result<String> {
    let mut fingerprint = Sha256::new();
    for relative in PACKAGED_COMPUTER_USE_RUNTIME_IDENTITY_FILES {
        let path = source.join(relative);
        let digest = sha256_file(&path)
            .with_context(|| format!("hash packaged Computer Use runtime file {relative}"))?;
        fingerprint.update(relative.as_bytes());
        fingerprint.update([0]);
        fingerprint.update(digest.as_bytes());
        fingerprint.update([0]);
    }
    Ok(format!("{:x}", fingerprint.finalize())[..16].to_owned())
}

#[cfg(windows)]
fn packaged_computer_use_runtime_matches(source: &Path, destination: &Path) -> bool {
    if !destination.join("bin/node_modules").is_dir() {
        return false;
    }
    PACKAGED_COMPUTER_USE_RUNTIME_IDENTITY_FILES
        .iter()
        .chain(PACKAGED_COMPUTER_USE_RUNTIME_CRITICAL_FILES)
        .all(|relative| {
            let source_file = source.join(relative);
            if !source_file.is_file() {
                return !PACKAGED_COMPUTER_USE_RUNTIME_IDENTITY_FILES.contains(relative);
            }
            let destination_file = destination.join(relative);
            let (Ok(source_metadata), Ok(destination_metadata)) = (
                std::fs::metadata(&source_file),
                std::fs::metadata(&destination_file),
            ) else {
                return false;
            };
            if !destination_metadata.is_file()
                || source_metadata.len() != destination_metadata.len()
            {
                return false;
            }
            matches!(
                (sha256_file(&source_file), sha256_file(&destination_file)),
                (Ok(source_hash), Ok(destination_hash)) if source_hash == destination_hash
            )
        })
}

#[cfg(windows)]
fn ensure_runtime_tree_has_no_reparse_points(root: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(root)
        .with_context(|| format!("inspect Computer Use runtime path {}", root.display()))?;
    ensure!(
        !metadata_is_reparse(&metadata),
        "computer_use_contract_incompatible"
    );
    if metadata.is_dir() {
        for entry in std::fs::read_dir(root)? {
            ensure_runtime_tree_has_no_reparse_points(&entry?.path())?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn copy_packaged_computer_use_runtime_tree(
    source: &Path,
    destination: &Path,
) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("inspect packaged Computer Use path {}", source.display()))?;
    ensure!(
        !metadata_is_reparse(&metadata),
        "computer_use_contract_incompatible"
    );
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_packaged_computer_use_runtime_tree(
                &entry.path(),
                &destination.join(entry.file_name()),
            )?;
        }
    } else if metadata.is_file() {
        let copied = std::fs::copy(source, destination)?;
        ensure!(
            copied == metadata.len(),
            "computer_use_contract_incompatible"
        );
    } else {
        anyhow::bail!("computer_use_contract_incompatible");
    }
    Ok(())
}

#[cfg(windows)]
fn remove_invalid_packaged_computer_use_runtime(
    destination: &Path,
    destination_root: &Path,
) -> anyhow::Result<()> {
    ensure!(
        destination.parent() == Some(destination_root),
        "computer_use_contract_incompatible"
    );
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .context("computer_use_contract_incompatible")?;
    ensure!(
        name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "computer_use_contract_incompatible"
    );
    ensure_runtime_tree_has_no_reparse_points(destination)?;
    if destination.is_dir() {
        std::fs::remove_dir_all(destination)?;
    } else {
        std::fs::remove_file(destination)?;
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_packaged_computer_use_runtime_copy(
    source: &Path,
    destination_root: &Path,
) -> anyhow::Result<PathBuf> {
    ensure_runtime_tree_has_no_reparse_points(source)?;
    let fingerprint = packaged_computer_use_runtime_fingerprint(source)?;
    std::fs::create_dir_all(destination_root)?;
    ensure_runtime_tree_has_no_reparse_points(destination_root)?;
    let destination = destination_root.join(&fingerprint);
    if destination.exists() {
        if packaged_computer_use_runtime_matches(source, &destination) {
            return Ok(destination);
        }
        remove_invalid_packaged_computer_use_runtime(&destination, destination_root)?;
    }

    let staging = destination_root.join(format!(
        ".codex-plus-staging-{fingerprint}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let copied = copy_packaged_computer_use_runtime_tree(source, &staging).and_then(|_| {
        ensure!(
            packaged_computer_use_runtime_matches(source, &staging),
            "computer_use_contract_incompatible"
        );
        std::fs::rename(&staging, &destination)?;
        Ok(())
    });
    if let Err(error) = copied {
        if staging.exists() {
            let _ = ensure_runtime_tree_has_no_reparse_points(&staging)
                .and_then(|_| std::fs::remove_dir_all(&staging).map_err(Into::into));
        }
        if packaged_computer_use_runtime_matches(source, &destination) {
            return Ok(destination);
        }
        return Err(error);
    }
    Ok(destination)
}

#[cfg(windows)]
pub(crate) fn ensure_packaged_admin_computer_use_artifacts(
    app_dir: &Path,
) -> anyhow::Result<AdminComputerUseArtifacts> {
    let source = app_dir.join("resources").join("cua_node");
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("computer_use_contract_incompatible")?;
    let destination_root = local_app_data
        .join("OpenAI")
        .join("Codex")
        .join("runtimes")
        .join("cua_node");
    let runtime = ensure_packaged_computer_use_runtime_copy(&source, &destination_root)?;
    let sky_root = runtime
        .join("bin")
        .join("node_modules")
        .join("@oai")
        .join("sky");
    let helper_exe = sky_root.join("bin/windows/codex-computer-use.exe");
    let helper_transport =
        sky_root.join("dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sky_root.join("package.json"))?)?;
    let sky_version = package["version"]
        .as_str()
        .context("computer_use_contract_incompatible")?
        .to_owned();
    validate_admin_computer_use_artifacts(&helper_exe, &helper_transport, &sky_version)?;
    Ok(AdminComputerUseArtifacts {
        helper_exe,
        helper_transport,
        sky_version,
    })
}

pub(crate) fn install_admin_computer_use_hook(
    home: &Path,
    descriptor_path: &Path,
) -> anyhow::Result<ComputerUseHookOutcome> {
    let artifacts = resolve_admin_computer_use_artifacts(home)?;
    preflight_admin_computer_use_artifacts(&artifacts)?;
    install_admin_computer_use_hook_with_artifacts(&artifacts, descriptor_path)
}

#[cfg(test)]
pub(crate) fn install_admin_computer_use_hook_transaction_with_artifacts(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
) -> anyhow::Result<AdminComputerUseHookRollback> {
    let expected_hash = if artifacts.helper_transport.exists() {
        sha256_file(&artifacts.helper_transport)?
    } else {
        sha256_file(
            &artifacts
                .helper_transport
                .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP),
        )?
    };
    install_admin_computer_use_hook_transaction_with_expected_hash(
        artifacts,
        descriptor_path,
        &expected_hash,
    )
}

pub(crate) fn install_admin_computer_use_hook_transaction_with_resolved_artifacts(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
) -> anyhow::Result<AdminComputerUseHookRollback> {
    install_admin_computer_use_hook_transaction_with_expected_hash(
        artifacts,
        descriptor_path,
        supported_transport_sha256(&artifacts.sky_version)
            .context("computer_use_contract_incompatible")?,
    )
}

fn install_admin_computer_use_hook_transaction_with_expected_hash(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
    expected_original_hash: &str,
) -> anyhow::Result<AdminComputerUseHookRollback> {
    install_admin_computer_use_hook_transaction_impl(
        artifacts,
        descriptor_path,
        expected_original_hash,
        || Ok(()),
    )
}

fn install_admin_computer_use_hook_transaction_impl(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
    expected_original_hash: &str,
    before_publish: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<AdminComputerUseHookRollback> {
    recover_interrupted_admin_computer_use_hook(
        &artifacts.helper_transport,
        expected_original_hash,
    )?;
    let mut transport =
        crate::admin_secure_io::SecureFileLease::open(&artifacts.helper_transport, true)?;
    let transport_path = transport.final_path()?;
    let captured = transport.read_all()?;
    let existing =
        String::from_utf8(captured.clone()).context("computer_use_contract_incompatible")?;
    let backup_path = artifacts
        .helper_transport
        .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    if existing.contains(ADMIN_HELPER_HOOK_BEGIN) {
        let mut backup = crate::admin_secure_io::SecureFileLease::open(&backup_path, true)?;
        let backup_bytes = backup.read_all()?;
        let original_hash = sha256_bytes(&backup_bytes);
        let patched_hash = sha256_bytes(&captured);
        let backup_text =
            std::str::from_utf8(&backup_bytes).context("computer_use_contract_incompatible")?;
        let expected_patched =
            patch_admin_helper_transport(backup_text, &artifacts.sky_version, descriptor_path)?;
        ensure!(
            original_hash == expected_original_hash
                && remove_admin_helper_transport_patch(&existing)?.as_bytes() == backup_bytes
                && existing == expected_patched,
            "computer_use_contract_incompatible"
        );
        let rollback = AdminComputerUseHookRollback(Some(OwnedAdminComputerUseHook {
            installed: InstalledAdminComputerUseHook {
                transport_path,
                backup_path: backup.final_path()?,
                original_hash,
                patched_hash,
            },
            transport: Some(transport),
            backup: Some(backup),
        }));
        return Ok(rollback);
    }
    ensure!(
        open_optional_secure_file(&backup_path, false)?.is_none(),
        "computer_use_contract_incompatible"
    );
    if format!("{:x}", Sha256::digest(&captured)) != expected_original_hash {
        anyhow::bail!("computer_use_contract_incompatible");
    }
    let original = match String::from_utf8(captured) {
        Ok(original) => original,
        Err(error) => {
            return Err(error).context("computer_use_contract_incompatible");
        }
    };
    let patched =
        match patch_admin_helper_transport(&original, &artifacts.sky_version, descriptor_path) {
            Ok(patched) => patched,
            Err(error) => {
                return Err(error);
            }
        };
    let publish = (|| -> anyhow::Result<crate::admin_secure_io::SecureFileLease> {
        let mut backup = crate::admin_secure_io::SecureFileLease::create(&backup_path)?;
        if let Err(error) = backup.replace_contents(original.as_bytes()) {
            let _ = backup.delete();
            return Err(error);
        }
        before_publish()?;
        transport.replace_contents(patched.as_bytes())?;
        Ok(backup)
    })();
    let backup = publish?;
    let backup_path = backup.final_path()?;
    let rollback = AdminComputerUseHookRollback(Some(OwnedAdminComputerUseHook {
        installed: InstalledAdminComputerUseHook {
            transport_path,
            backup_path,
            original_hash: expected_original_hash.to_owned(),
            patched_hash: format!("{:x}", Sha256::digest(patched.as_bytes())),
        },
        transport: Some(transport),
        backup: Some(backup),
    }));
    Ok(rollback)
}

fn recover_interrupted_admin_computer_use_hook(
    transport: &Path,
    expected_original_hash: &str,
) -> anyhow::Result<()> {
    let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    let transport_file = open_optional_secure_file(transport, true)?;
    let backup_file = open_optional_secure_file(&backup, true)?;
    match (transport_file, backup_file) {
        (None, None) => anyhow::bail!("computer_use_contract_incompatible"),
        (None, Some(mut backup_file)) => {
            ensure!(
                sha256_lease(&mut backup_file)? == expected_original_hash,
                "computer_use_contract_incompatible"
            );
            backup_file.rename_to(transport)?;
        }
        (Some(mut transport_file), Some(mut backup_file)) => {
            let transport_bytes = transport_file.read_all()?;
            let transport_hash = sha256_bytes(&transport_bytes);
            let backup_hash = sha256_lease(&mut backup_file)?;
            ensure!(
                backup_hash == expected_original_hash,
                "computer_use_contract_incompatible"
            );
            if transport_hash == expected_original_hash {
                backup_file.delete()?;
            } else {
                let contents = String::from_utf8(transport_bytes)
                    .context("computer_use_contract_incompatible")?;
                ensure!(
                    contents.contains(ADMIN_HELPER_HOOK_BEGIN),
                    "computer_use_contract_incompatible"
                );
            }
        }
        (Some(_), None) => {}
    }
    Ok(())
}

pub(crate) fn preflight_admin_computer_use_artifacts(
    artifacts: &AdminComputerUseArtifacts,
) -> anyhow::Result<()> {
    recover_interrupted_admin_computer_use_hook(
        &artifacts.helper_transport,
        supported_transport_sha256(&artifacts.sky_version)
            .context("computer_use_contract_incompatible")?,
    )
}

#[cfg(test)]
fn recover_descriptorless_admin_computer_use_rename_window(
    transport: &Path,
    expected_original_hash: &str,
) -> anyhow::Result<()> {
    let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    let transport_file = open_optional_secure_file(transport, true)?;
    let backup_file = open_optional_secure_file(&backup, true)?;
    match (transport_file, backup_file) {
        (None, Some(mut backup_file)) => {
            ensure!(
                sha256_lease(&mut backup_file)? == expected_original_hash,
                "computer_use_contract_incompatible"
            );
            backup_file.rename_to(transport)?;
            Ok(())
        }
        (Some(mut transport_file), Some(mut backup_file)) => {
            let backup_bytes = backup_file.read_all()?;
            ensure!(
                format!("{:x}", Sha256::digest(&backup_bytes)) == expected_original_hash,
                "computer_use_contract_incompatible"
            );
            let transport_bytes = transport_file.read_all()?;
            if transport_bytes == backup_bytes {
                backup_file.delete()?;
                return Ok(());
            }
            let patched =
                String::from_utf8(transport_bytes).context("computer_use_contract_incompatible")?;
            ensure!(
                patched.contains(ADMIN_HELPER_HOOK_BEGIN)
                    && patched.contains(ADMIN_HELPER_HOOK_END),
                "computer_use_contract_incompatible"
            );
            let restored = remove_admin_helper_transport_patch(&patched)?;
            ensure!(
                restored.as_bytes() == backup_bytes,
                "computer_use_contract_incompatible"
            );
            transport_file.replace_contents(&backup_bytes)?;
            backup_file.delete()?;
            Ok(())
        }
        _ => anyhow::bail!("computer_use_contract_incompatible"),
    }
}

pub(crate) fn recover_descriptorless_admin_computer_use(
    home: &Path,
    inspect_marked_transports: bool,
    descriptor_path: Option<&Path>,
) -> anyhow::Result<bool> {
    let mut roots = vec![
        home.join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use"),
    ];
    #[cfg(windows)]
    if recovery_home_matches_default(home)
        && let Some(local_app_data) = std::env::var_os("LOCALAPPDATA")
    {
        roots.push(
            PathBuf::from(local_app_data)
                .join("OpenAI")
                .join("Codex")
                .join("runtimes")
                .join("cua_node"),
        );
    }
    recover_descriptorless_admin_computer_use_at_roots_with_options(
        &roots,
        inspect_marked_transports,
        descriptor_path,
        || Ok(()),
    )
}

#[cfg(windows)]
fn recovery_home_matches_default(home: &Path) -> bool {
    let default_home = crate::codex_home::default_codex_home_dir();
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let default_home =
        std::fs::canonicalize(&default_home).unwrap_or_else(|_| default_home.to_path_buf());
    home.to_string_lossy()
        .eq_ignore_ascii_case(&default_home.to_string_lossy())
}

#[cfg(test)]
fn recover_descriptorless_admin_computer_use_at_roots(roots: &[PathBuf]) -> anyhow::Result<bool> {
    recover_descriptorless_admin_computer_use_at_roots_with_options(roots, false, None, || Ok(()))
}

#[cfg(test)]
fn recover_descriptorless_admin_computer_use_at_roots_with_hook(
    roots: &[PathBuf],
    before_mutation: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    recover_descriptorless_admin_computer_use_at_roots_with_options(
        roots,
        false,
        None,
        before_mutation,
    )
}

fn recover_descriptorless_admin_computer_use_at_roots_with_options(
    roots: &[PathBuf],
    inspect_marked_transports: bool,
    descriptor_path: Option<&Path>,
    before_mutation: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    #[cfg(not(windows))]
    {
        let _ = (roots, before_mutation);
        return Ok(false);
    }
    #[cfg(windows)]
    {
        let mut candidates = Vec::new();
        for root in roots {
            let Some(canonical_root) = trusted_recovery_root(root)? else {
                continue;
            };
            collect_admin_recovery_candidates(
                root,
                &canonical_root,
                inspect_marked_transports,
                16,
                &mut candidates,
            )?;
        }
        candidates.sort_by(|left, right| left.transport.cmp(&right.transport));
        candidates.dedup_by(|left, right| left.transport == right.transport);
        let mut leases = candidates
            .into_iter()
            .map(AdminRecoveryCandidateLease::open)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut recovery_required = Vec::with_capacity(leases.len());
        for lease in &mut leases {
            recovery_required.push(lease.needs_recovery()?);
        }
        if !recovery_required.iter().any(|required| *required) {
            return Ok(false);
        }
        before_mutation()?;
        for (lease, required) in leases.iter_mut().zip(recovery_required) {
            if required {
                lease.recover(descriptor_path)?;
            }
        }
        Ok(true)
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct AdminRecoveryCandidate {
    root: PathBuf,
    transport: PathBuf,
}

#[cfg(windows)]
fn trusted_recovery_root(root: &Path) -> anyhow::Result<Option<PathBuf>> {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        !metadata_is_reparse(&metadata),
        "computer_use_contract_incompatible"
    );
    ensure!(metadata.is_dir(), "computer_use_contract_incompatible");
    for ancestor in root.ancestors() {
        ensure!(
            !metadata_is_reparse(&std::fs::symlink_metadata(ancestor)?),
            "computer_use_contract_incompatible"
        );
    }
    Ok(Some(std::fs::canonicalize(root)?))
}

#[cfg(windows)]
fn collect_admin_recovery_candidates(
    directory: &Path,
    canonical_root: &Path,
    inspect_marked_transports: bool,
    depth: usize,
    output: &mut Vec<AdminRecoveryCandidate>,
) -> anyhow::Result<()> {
    if depth == 0 {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(
            !metadata_is_reparse(&metadata),
            "computer_use_contract_incompatible"
        );
        if metadata.is_dir() {
            collect_admin_recovery_candidates(
                &path,
                canonical_root,
                inspect_marked_transports,
                depth - 1,
                output,
            )?;
            continue;
        }
        let name = path.file_name().and_then(|value| value.to_str());
        let transport = match name {
            Some(name) if name.eq_ignore_ascii_case(ADMIN_HELPER_TRANSPORT_BACKUP) => {
                path.with_file_name("helper_transport.js")
            }
            Some(name)
                if inspect_marked_transports
                    && name.eq_ignore_ascii_case("helper_transport.js") =>
            {
                path
            }
            _ => continue,
        };
        if is_owned_admin_transport_path(&transport) {
            output.push(AdminRecoveryCandidate {
                root: canonical_root.to_owned(),
                transport,
            });
        }
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(windows)]
fn is_owned_admin_transport_path(path: &Path) -> bool {
    path.ends_with(Path::new(
        "dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js",
    )) && path.ancestors().any(|ancestor| {
        ancestor
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("sky"))
            && ancestor
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("@oai"))
    })
}

#[cfg(windows)]
struct AdminRecoveryFileLease {
    file: crate::admin_secure_io::SecureFileLease,
}

#[cfg(windows)]
impl AdminRecoveryFileLease {
    fn open(path: &Path, root: &Path, writable: bool) -> anyhow::Result<Self> {
        ensure!(
            path_is_within(path, root),
            "computer_use_contract_incompatible"
        );
        let file = if writable {
            crate::admin_secure_io::SecureFileLease::open(path, true)?
        } else {
            crate::admin_secure_io::SecureFileLease::open_for_delete(path)?
        };
        ensure!(
            path_is_within(&file.final_path()?, root),
            "computer_use_contract_incompatible"
        );
        Ok(Self { file })
    }

    fn create(path: &Path, root: &Path) -> anyhow::Result<Self> {
        ensure!(
            path_is_within(path, root),
            "computer_use_contract_incompatible"
        );
        let file = crate::admin_secure_io::SecureFileLease::create(path)?;
        ensure!(
            path_is_within(&file.final_path()?, root),
            "computer_use_contract_incompatible"
        );
        Ok(Self { file })
    }

    fn read_all(&mut self) -> anyhow::Result<Vec<u8>> {
        self.file.read_all()
    }

    fn replace_contents(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.file.replace_contents(bytes)
    }

    fn delete(self) -> anyhow::Result<()> {
        self.file.delete()
    }
}

#[cfg(windows)]
fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('/', "\\");
    let root = root.to_string_lossy().replace('/', "\\");
    let path = path.trim_start_matches("\\\\?\\");
    let root = root.trim_start_matches("\\\\?\\").trim_end_matches('\\');
    path.get(..root.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(root))
        && (path.len() == root.len() || path.as_bytes().get(root.len()) == Some(&b'\\'))
}

#[cfg(windows)]
struct AdminRecoveryCandidateLease {
    root: PathBuf,
    transport_path: PathBuf,
    backup: Option<AdminRecoveryFileLease>,
    transport: Option<AdminRecoveryFileLease>,
}

#[cfg(windows)]
impl AdminRecoveryCandidateLease {
    fn open(candidate: AdminRecoveryCandidate) -> anyhow::Result<Self> {
        let backup_path = candidate
            .transport
            .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let backup = match AdminRecoveryFileLease::open(&backup_path, &candidate.root, false) {
            Ok(lease) => Some(lease),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|value| value.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let transport =
            match AdminRecoveryFileLease::open(&candidate.transport, &candidate.root, true) {
                Ok(lease) => Some(lease),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|value| value.kind() == std::io::ErrorKind::NotFound) =>
                {
                    None
                }
                Err(error) => return Err(error),
            };
        Ok(Self {
            root: candidate.root,
            transport_path: candidate.transport,
            backup,
            transport,
        })
    }

    fn needs_recovery(&mut self) -> anyhow::Result<bool> {
        if self.backup.is_some() {
            return Ok(true);
        }
        let Some(transport) = self.transport.as_mut() else {
            return Ok(false);
        };
        let bytes = transport.read_all()?;
        let contents = String::from_utf8_lossy(&bytes);
        ensure!(
            !contents.contains(ADMIN_HELPER_HOOK_BEGIN)
                && !contents.contains(ADMIN_HELPER_HOOK_END),
            "computer_use_contract_incompatible"
        );
        Ok(false)
    }

    fn recover(&mut self, descriptor_path: Option<&Path>) -> anyhow::Result<()> {
        let mut backup = self
            .backup
            .take()
            .context("computer_use_contract_incompatible")?;
        let backup_bytes = backup.read_all()?;
        if let Some(transport) = self.transport.as_mut() {
            let transport_bytes = transport.read_all()?;
            if transport_bytes != backup_bytes {
                let valid_partial = descriptor_path.is_some_and(|descriptor_path| {
                    String::from_utf8(backup_bytes.clone())
                        .ok()
                        .and_then(|original| {
                            expected_patched_admin_helper_transport(&original, descriptor_path)
                        })
                        .is_some_and(|expected| expected.as_bytes().starts_with(&transport_bytes))
                });
                if !valid_partial {
                    let patched = String::from_utf8(transport_bytes)
                        .context("computer_use_contract_incompatible")?;
                    ensure!(
                        patched.contains(ADMIN_HELPER_HOOK_BEGIN)
                            && patched.contains(ADMIN_HELPER_HOOK_END),
                        "computer_use_contract_incompatible"
                    );
                    let restored = remove_admin_helper_transport_patch(&patched)?;
                    ensure!(
                        restored.as_bytes() == backup_bytes,
                        "computer_use_contract_incompatible"
                    );
                }
                transport.replace_contents(&backup_bytes)?;
            }
        } else {
            let mut transport = AdminRecoveryFileLease::create(&self.transport_path, &self.root)?;
            transport.replace_contents(&backup_bytes)?;
            self.transport = Some(transport);
        }
        backup.delete()?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn recover_descriptorless_admin_computer_use_artifacts_for_test(
    artifacts: &AdminComputerUseArtifacts,
) -> anyhow::Result<()> {
    let backup = artifacts
        .helper_transport
        .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    let expected_original = if backup.exists() {
        &backup
    } else {
        &artifacts.helper_transport
    };
    recover_descriptorless_admin_computer_use_rename_window(
        &artifacts.helper_transport,
        &sha256_file(expected_original)?,
    )
}

pub(crate) fn install_admin_computer_use_hook_with_artifacts(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
) -> anyhow::Result<ComputerUseHookOutcome> {
    let existing = std::fs::read_to_string(&artifacts.helper_transport)
        .context("computer_use_contract_incompatible")?;
    let patched = patch_admin_helper_transport(&existing, &artifacts.sky_version, descriptor_path)?;
    if patched == existing {
        return Ok(ComputerUseHookOutcome::AlreadyInstalled);
    }
    let backup = artifacts
        .helper_transport
        .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    if backup.exists() {
        ensure!(
            std::fs::read(&backup)? == existing.as_bytes(),
            "computer_use_contract_incompatible"
        );
    } else {
        crate::admin_secure_io::create_new(&backup, existing.as_bytes())?;
    }
    atomic_write_runtime_file(&artifacts.helper_transport, patched.as_bytes())?;
    Ok(ComputerUseHookOutcome::Installed)
}

pub(crate) fn remove_admin_computer_use_hook(
    home: &Path,
) -> anyhow::Result<ComputerUseHookOutcome> {
    let artifacts = resolve_admin_computer_use_artifacts(home)?;
    preflight_admin_computer_use_artifacts(&artifacts)?;
    remove_admin_computer_use_hook_with_artifacts(&artifacts)
}

fn remove_admin_computer_use_hook_with_artifacts(
    artifacts: &AdminComputerUseArtifacts,
) -> anyhow::Result<ComputerUseHookOutcome> {
    let existing = std::fs::read_to_string(&artifacts.helper_transport)?;
    let backup = artifacts
        .helper_transport
        .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    if !existing.contains(ADMIN_HELPER_HOOK_BEGIN) {
        if backup.exists() && std::fs::read(&backup)? == existing.as_bytes() {
            std::fs::remove_file(backup)?;
        }
        return Ok(ComputerUseHookOutcome::NotInstalled);
    }
    let restored = remove_admin_helper_transport_patch(&existing)?;
    let backup_bytes = std::fs::read(&backup).context("computer_use_contract_incompatible")?;
    ensure!(
        restored.as_bytes() == backup_bytes,
        "computer_use_contract_incompatible"
    );
    atomic_write_runtime_file(&artifacts.helper_transport, &backup_bytes)?;
    std::fs::remove_file(backup)?;
    Ok(ComputerUseHookOutcome::Removed)
}

fn configured_computer_use_notify_exe(home: &Path) -> Option<PathBuf> {
    let config = std::fs::read_to_string(home.join("config.toml")).ok()?;
    let doc = parse_toml_document(config.trim_start_matches('\u{feff}')).ok()?;
    let notify = doc.get("notify")?.as_array()?;
    let path = notify.get(0)?.as_str()?;
    let path = PathBuf::from(path);
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(COMPUTER_USE_EXE))
        .then_some(path)
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_lease(file: &mut crate::admin_secure_io::SecureFileLease) -> anyhow::Result<String> {
    Ok(sha256_bytes(&file.read_all()?))
}

fn open_optional_secure_file(
    path: &Path,
    writable: bool,
) -> anyhow::Result<Option<crate::admin_secure_io::SecureFileLease>> {
    match crate::admin_secure_io::SecureFileLease::open(path, writable) {
        Ok(file) => Ok(Some(file)),
        Err(error)
            if error
                .chain()
                .filter_map(|source| source.downcast_ref::<std::io::Error>())
                .any(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn restore_stale_admin_computer_use_hook(
    home: &Path,
    descriptor_path: &Path,
    transport_path: &Path,
    backup_path: &Path,
    original_hash: &str,
    patched_hash: &str,
) -> anyhow::Result<()> {
    let artifacts =
        resolve_admin_computer_use_artifacts_for_transport(home, transport_path, original_hash)?;
    restore_stale_admin_computer_use_hook_with_artifacts(
        &artifacts,
        descriptor_path,
        transport_path,
        backup_path,
        original_hash,
        patched_hash,
    )
}

pub(crate) fn verify_stale_admin_computer_use_hook(
    home: &Path,
    descriptor_path: &Path,
    transport_path: &Path,
    backup_path: &Path,
    original_hash: &str,
    patched_hash: &str,
) -> anyhow::Result<()> {
    let artifacts =
        resolve_admin_computer_use_artifacts_for_transport(home, transport_path, original_hash)?;
    verify_stale_admin_computer_use_hook_with_artifacts(
        &artifacts,
        descriptor_path,
        transport_path,
        backup_path,
        original_hash,
        patched_hash,
    )
}

pub(crate) fn restore_stale_admin_computer_use_hook_with_artifacts(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
    transport_path: &Path,
    backup_path: &Path,
    original_hash: &str,
    patched_hash: &str,
) -> anyhow::Result<()> {
    let mut transport =
        crate::admin_secure_io::SecureFileLease::open(&artifacts.helper_transport, true)?;
    ensure!(
        paths_equal(&transport.final_path()?, transport_path),
        "computer_use_contract_incompatible"
    );
    let expected_backup = artifacts
        .helper_transport
        .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
    ensure!(
        paths_equal(
            backup_path,
            &transport_path.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP),
        ),
        "computer_use_contract_incompatible"
    );
    let transport_bytes = transport.read_all()?;
    let transport_hash = format!("{:x}", Sha256::digest(&transport_bytes));
    let mut backup =
        match crate::admin_secure_io::SecureFileLease::open_for_delete(&expected_backup) {
            Ok(backup) => Some(backup),
            Err(error)
                if error
                    .chain()
                    .filter_map(|source| source.downcast_ref::<std::io::Error>())
                    .any(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                None
            }
            Err(error) => return Err(error),
        };
    let Some(mut backup) = backup.take() else {
        ensure!(
            transport_hash == original_hash,
            "computer_use_contract_incompatible"
        );
        return Ok(());
    };
    ensure!(
        paths_equal(&backup.final_path()?, backup_path),
        "computer_use_contract_incompatible"
    );
    let backup_bytes = backup.read_all()?;
    ensure!(
        format!("{:x}", Sha256::digest(&backup_bytes)) == original_hash,
        "computer_use_contract_incompatible"
    );
    if transport_hash == original_hash {
        backup.delete()?;
        return Ok(());
    }
    if transport_hash == patched_hash {
        let patched =
            String::from_utf8(transport_bytes).context("computer_use_contract_incompatible")?;
        ensure!(
            remove_admin_helper_transport_patch(&patched)?.as_bytes() == backup_bytes,
            "computer_use_contract_incompatible"
        );
        transport.replace_contents(&backup_bytes)?;
    } else {
        let original =
            std::str::from_utf8(&backup_bytes).context("computer_use_contract_incompatible")?;
        let expected_patched =
            patch_admin_helper_transport(original, &artifacts.sky_version, descriptor_path)?;
        ensure!(
            (transport_bytes.len() < backup_bytes.len()
                && backup_bytes.starts_with(&transport_bytes))
                || (transport_bytes.len() < expected_patched.len()
                    && expected_patched.as_bytes().starts_with(&transport_bytes)),
            "computer_use_contract_incompatible"
        );
        transport.replace_contents(&backup_bytes)?;
    }
    backup.delete()?;
    Ok(())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        let left = left.to_string_lossy().replace('/', "\\");
        let right = right.to_string_lossy().replace('/', "\\");
        return left
            .trim_start_matches(r"\\?\")
            .eq_ignore_ascii_case(right.trim_start_matches(r"\\?\"));
    }
    #[cfg(not(windows))]
    left.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .eq_ignore_ascii_case(right.to_string_lossy().trim_start_matches(r"\\?\"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifiedHookState {
    Patched,
    RestoredWithBackup,
    RestoredWithoutBackup,
}

pub(crate) fn verify_stale_admin_computer_use_hook_with_artifacts(
    artifacts: &AdminComputerUseArtifacts,
    descriptor_path: &Path,
    transport_path: &Path,
    backup_path: &Path,
    original_hash: &str,
    patched_hash: &str,
) -> anyhow::Result<()> {
    verify_stale_admin_computer_use_hook_state_with_artifacts(
        artifacts,
        descriptor_path,
        transport_path,
        backup_path,
        original_hash,
        patched_hash,
    )
    .map(|_| ())
}

fn verify_stale_admin_computer_use_hook_state_with_artifacts(
    artifacts: &AdminComputerUseArtifacts,
    _descriptor_path: &Path,
    transport_path: &Path,
    backup_path: &Path,
    original_hash: &str,
    patched_hash: &str,
) -> anyhow::Result<VerifiedHookState> {
    let expected_transport = canonical_owned_path(&artifacts.helper_transport)?;
    let expected_backup = canonical_owned_path(
        &artifacts
            .helper_transport
            .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP),
    )?;
    ensure!(
        canonical_owned_path(transport_path)? == expected_transport
            && canonical_owned_path(backup_path)? == expected_backup,
        "computer_use_contract_incompatible"
    );
    let transport_hash = sha256_file(&expected_transport)?;
    if !expected_backup.exists() {
        ensure!(
            transport_hash == original_hash,
            "computer_use_contract_incompatible"
        );
        return Ok(VerifiedHookState::RestoredWithoutBackup);
    }
    ensure!(
        sha256_file(&expected_backup)? == original_hash,
        "computer_use_contract_incompatible"
    );
    if transport_hash == original_hash {
        return Ok(VerifiedHookState::RestoredWithBackup);
    }
    ensure!(
        transport_hash == patched_hash,
        "computer_use_contract_incompatible"
    );
    let backup_bytes = std::fs::read(&expected_backup)?;
    let patched = String::from_utf8(std::fs::read(&expected_transport)?)
        .context("computer_use_contract_incompatible")?;
    ensure!(
        remove_admin_helper_transport_patch(&patched)?.as_bytes() == backup_bytes
            && format!("{:x}", Sha256::digest(backup_bytes.as_slice())) == original_hash,
        "computer_use_contract_incompatible"
    );
    Ok(VerifiedHookState::Patched)
}

fn canonical_owned_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return Ok(std::fs::canonicalize(path)?);
    }
    let parent = path
        .parent()
        .context("computer_use_contract_incompatible")?;
    let parent = std::fs::canonicalize(parent)?;
    let name = path
        .file_name()
        .context("computer_use_contract_incompatible")?;
    Ok(parent.join(name))
}

pub(crate) fn resolve_computer_use_guard_artifacts(home: &Path) -> anyhow::Result<GuardArtifacts> {
    #[cfg(windows)]
    {
        let notify_exe = find_computer_use_notify_exe(home);
        let runtime_exports_needed = computer_use_client_needs_sky_internal_export(home)?;
        Ok(GuardArtifacts {
            sky_package_json: find_sky_package_json_for_notify_exe(notify_exe.as_deref())
                .or_else(find_latest_sky_package_json),
            notify_exe,
            marketplace_path: ensure_openai_bundled_marketplace(home)?,
            runtime_exports_needed,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = home;
        Ok(GuardArtifacts {
            notify_exe: None,
            marketplace_path: None,
            sky_package_json: None,
            runtime_exports_needed: false,
        })
    }
}

pub(crate) fn ensure_computer_use_config_with_artifacts(
    home: &Path,
    artifacts: &GuardArtifacts,
) -> anyhow::Result<GuardResult> {
    #[cfg(windows)]
    {
        ensure_computer_use_config_with_artifacts_windows(home, artifacts)
    }
    #[cfg(not(windows))]
    {
        let _ = (home, artifacts);
        Ok(GuardResult {
            changed: false,
            notify_exe: None,
        })
    }
}

#[cfg(windows)]
pub(crate) fn ensure_admin_computer_use_config_for_artifacts(
    home: &Path,
    app_dir: &Path,
    artifacts: &AdminComputerUseArtifacts,
) -> anyhow::Result<GuardResult> {
    let packaged_marketplace = app_dir
        .join("resources")
        .join("plugins")
        .join(BUNDLED_MARKETPLACE);
    let marketplace_path = if is_complete_openai_bundled_marketplace(&packaged_marketplace) {
        Some(packaged_marketplace)
    } else {
        ensure_openai_bundled_marketplace(home)?
    };
    let sky_package_json = artifacts
        .helper_exe
        .ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("sky"))
        })
        .map(|sky_root| sky_root.join("package.json"));
    ensure_computer_use_config_with_artifacts_windows(
        home,
        &GuardArtifacts {
            notify_exe: Some(artifacts.helper_exe.clone()),
            marketplace_path,
            sky_package_json,
            runtime_exports_needed: computer_use_client_needs_sky_internal_export(home)?,
        },
    )
}

#[cfg(windows)]
fn ensure_computer_use_config_with_artifacts_windows(
    home: &Path,
    artifacts: &GuardArtifacts,
) -> anyhow::Result<GuardResult> {
    let config_path = home.join("config.toml");
    let existing = match std::fs::read(&config_path) {
        Ok(bytes) => String::from_utf8(bytes)
            .with_context(|| format!("failed to read UTF-8 {}", config_path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let updated = if let Some(marketplace_path) = artifacts.marketplace_path.as_deref() {
        guard_config_text_with_marketplace(
            &existing,
            artifacts.notify_exe.as_deref(),
            Some(marketplace_path),
        )?
    } else {
        guard_config_text(&existing, artifacts.notify_exe.as_deref())?
    };
    let changed = updated.as_bytes() != existing.as_bytes();
    if changed {
        crate::settings::atomic_write(&config_path, updated.as_bytes())?;
    }
    let runtime_compat = ensure_computer_use_runtime_exports_compat_windows(
        home,
        artifacts.sky_package_json.as_deref(),
    )?;
    Ok(GuardResult {
        changed: changed || runtime_compat.changed,
        notify_exe: artifacts.notify_exe.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeCompatResult {
    pub changed: bool,
    pub package_json: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
}

#[cfg(not(windows))]
pub(crate) fn ensure_computer_use_runtime_exports_compat(
    home: &Path,
) -> anyhow::Result<RuntimeCompatResult> {
    let _ = home;
    Ok(RuntimeCompatResult {
        changed: false,
        package_json: None,
        backup_path: None,
    })
}

#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn ensure_computer_use_runtime_exports_compat(
    home: &Path,
) -> anyhow::Result<RuntimeCompatResult> {
    ensure_computer_use_runtime_exports_compat_windows(
        home,
        find_latest_sky_package_json().as_deref(),
    )
}

#[cfg(windows)]
fn ensure_computer_use_runtime_exports_compat_windows(
    home: &Path,
    sky_package_json: Option<&Path>,
) -> anyhow::Result<RuntimeCompatResult> {
    if !computer_use_client_needs_sky_internal_export(home)? {
        return Ok(RuntimeCompatResult {
            changed: false,
            package_json: sky_package_json.map(Path::to_path_buf),
            backup_path: None,
        });
    }
    let Some(package_json) = sky_package_json else {
        return Ok(RuntimeCompatResult {
            changed: false,
            package_json: None,
            backup_path: None,
        });
    };
    if !sky_internal_computer_use_client_file_exists(package_json) {
        return Ok(RuntimeCompatResult {
            changed: false,
            package_json: Some(package_json.to_path_buf()),
            backup_path: None,
        });
    }

    let existing = std::fs::read_to_string(package_json)
        .with_context(|| format!("failed to read {}", package_json.display()))?;
    let Some(updated) = add_sky_internal_computer_use_export(&existing)? else {
        return Ok(RuntimeCompatResult {
            changed: false,
            package_json: Some(package_json.to_path_buf()),
            backup_path: None,
        });
    };

    let backup_path = package_json
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid @oai/sky package.json path"))?
        .join(SKY_PACKAGE_EXPORTS_BACKUP);
    if !backup_path.exists() {
        std::fs::copy(package_json, &backup_path).with_context(|| {
            format!(
                "failed to back up {} to {}",
                package_json.display(),
                backup_path.display()
            )
        })?;
    }
    atomic_write_runtime_file(package_json, updated.as_bytes())?;
    Ok(RuntimeCompatResult {
        changed: true,
        package_json: Some(package_json.to_path_buf()),
        backup_path: Some(backup_path),
    })
}

pub(crate) fn guard_config_text(
    config_text: &str,
    notify_exe: Option<&Path>,
) -> anyhow::Result<String> {
    guard_config_text_with_marketplace(config_text, notify_exe, None)
}

pub(crate) fn guard_config_text_with_marketplace(
    config_text: &str,
    notify_exe: Option<&Path>,
    marketplace_path: Option<&Path>,
) -> anyhow::Result<String> {
    let without_bom = config_text.trim_start_matches('\u{feff}');
    let mut doc = parse_toml_document(without_bom)?;

    let features = table_mut_or_insert(&mut doc, "features")?;
    features["js_repl"] = toml_edit::value(true);

    for plugin_id in COMPUTER_USE_PLUGINS {
        ensure_plugin_enabled(&mut doc, plugin_id)?;
    }

    if let Some(notify_exe) = notify_exe {
        let mut notify = Array::default();
        notify.push(notify_exe.to_string_lossy().as_ref());
        notify.push("turn-ended");
        doc["notify"] = toml_edit::value(notify);
    }

    if let Some(marketplace_path) = marketplace_path {
        ensure_openai_bundled_marketplace_config(&mut doc, marketplace_path)?;
    }

    Ok(ensure_trailing_newline(doc.to_string()))
}

pub(crate) fn find_computer_use_notify_exe(home: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        find_computer_use_notify_exe_windows(home)
    }
    #[cfg(not(windows))]
    {
        let _ = home;
        None
    }
}

#[cfg(windows)]
fn find_computer_use_notify_exe_windows(home: &Path) -> Option<PathBuf> {
    computer_use_notify_exe_candidates_windows(home)
        .into_iter()
        .find(|candidate| admin_computer_use_artifacts_from_helper(candidate.clone()).is_ok())
}

#[cfg(windows)]
fn computer_use_notify_exe_candidates_windows(home: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        collect_named_files(
            &PathBuf::from(local_app_data)
                .join("OpenAI")
                .join("Codex")
                .join("runtimes")
                .join("cua_node"),
            COMPUTER_USE_EXE,
            12,
            &mut candidates,
        );
    }
    collect_named_files(
        &home
            .join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use"),
        COMPUTER_USE_EXE,
        12,
        &mut candidates,
    );
    candidates.sort_by(|left, right| {
        modified_millis(right)
            .cmp(&modified_millis(left))
            .then_with(|| left.cmp(right))
    });
    candidates
}

#[cfg(windows)]
fn collect_named_files(root: &Path, file_name: &str, depth: usize, output: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(file_name))
            {
                output.push(path);
            }
        } else if path.is_dir() {
            collect_named_files(&path, file_name, depth - 1, output);
        }
    }
}

#[cfg(windows)]
fn modified_millis(path: &Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(windows)]
fn computer_use_client_needs_sky_internal_export(home: &Path) -> anyhow::Result<bool> {
    let mut candidates = Vec::new();
    collect_named_files(
        &home
            .join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use"),
        COMPUTER_USE_CLIENT_SCRIPT,
        8,
        &mut candidates,
    );
    candidates.sort_by(|left, right| {
        modified_millis(right)
            .cmp(&modified_millis(left))
            .then_with(|| left.cmp(right))
    });
    for candidate in candidates {
        let contents = std::fs::read_to_string(&candidate)
            .with_context(|| format!("failed to read {}", candidate.display()))?;
        if contents.contains(SKY_INTERNAL_COMPUTER_USE_CLIENT_IMPORT) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn find_sky_package_json_for_notify_exe(notify_exe: Option<&Path>) -> Option<PathBuf> {
    let notify_exe = notify_exe?;
    for ancestor in notify_exe.ancestors() {
        if ancestor
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("sky"))
            && ancestor
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("@oai"))
        {
            let package_json = ancestor.join("package.json");
            if package_json.is_file() {
                return Some(package_json);
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_latest_sky_package_json() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    let runtimes = PathBuf::from(local_app_data)
        .join("OpenAI")
        .join("Codex")
        .join("runtimes")
        .join("cua_node");
    let Ok(entries) = std::fs::read_dir(runtimes) else {
        return None;
    };
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| {
            entry
                .path()
                .join("bin")
                .join("node_modules")
                .join("@oai")
                .join("sky")
                .join("package.json")
        })
        .filter(|path| path.is_file())
        .collect();
    candidates.sort_by(|left, right| {
        modified_millis(right)
            .cmp(&modified_millis(left))
            .then_with(|| left.cmp(right))
    });
    candidates.into_iter().next()
}

#[cfg(windows)]
fn sky_internal_computer_use_client_file_exists(package_json: &Path) -> bool {
    let Some(package_root) = package_json.parent() else {
        return false;
    };
    package_root
        .join(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT.trim_start_matches("./"))
        .is_file()
}

fn add_sky_internal_computer_use_export(contents: &str) -> anyhow::Result<Option<String>> {
    let mut package: serde_json::Value =
        serde_json::from_str(contents).with_context(|| "@oai/sky package.json parse failed")?;
    let Some(exports) = package
        .get_mut("exports")
        .and_then(|value| value.as_object_mut())
    else {
        return Ok(None);
    };
    if exports.contains_key(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT) {
        return Ok(None);
    }
    exports.insert(
        SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT.to_string(),
        serde_json::Value::String(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT.to_string()),
    );
    let mut updated = serde_json::to_string_pretty(&package)?;
    updated.push('\n');
    Ok(Some(updated))
}

#[cfg(windows)]
fn atomic_write_runtime_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = crate::admin_secure_io::SecureFileLease::open(path, true)?;
    file.replace_contents(bytes)
        .with_context(|| format!("failed to replace {}", path.display()))
}

#[cfg(windows)]
pub(crate) fn ensure_openai_bundled_marketplace(home: &Path) -> anyhow::Result<Option<PathBuf>> {
    let active = home
        .join(".tmp")
        .join("bundled-marketplaces")
        .join(BUNDLED_MARKETPLACE);
    if is_complete_openai_bundled_marketplace(&active) {
        return Ok(Some(active));
    }
    if let Some(configured) = configured_openai_bundled_marketplace(home) {
        if is_complete_openai_bundled_marketplace(&configured) {
            return Ok(Some(configured));
        }
    }

    let parent = active
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid bundled marketplace path"))?;
    std::fs::create_dir_all(parent)?;

    let staging = parent.join(format!(
        "{BUNDLED_MARKETPLACE}.guard-staging-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }

    if let Some(source) = find_complete_openai_bundled_marketplace(parent, &active) {
        copy_dir_recursive(&source, &staging)?;
    } else if can_build_marketplace_from_cache(home) {
        build_marketplace_from_cache(home, &staging)?;
    } else {
        return Ok(None);
    }

    match replace_active_marketplace(&active, &staging) {
        Ok(()) => Ok(Some(active)),
        Err(_) if is_complete_openai_bundled_marketplace(&staging) => {
            // Windows can keep the active marketplace directory pinned while
            // Codex extension hosts are still alive. Pointing config at the
            // complete staging marketplace still restores plugin discovery.
            Ok(Some(staging))
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to replace active bundled marketplace at {}",
                active.display()
            )
        }),
    }
}

#[cfg(windows)]
fn configured_openai_bundled_marketplace(home: &Path) -> Option<PathBuf> {
    let config = std::fs::read_to_string(home.join("config.toml")).ok()?;
    let without_bom = config.trim_start_matches('\u{feff}');
    let doc = parse_toml_document(without_bom).ok()?;
    let source = doc
        .get("marketplaces")?
        .as_table()?
        .get(BUNDLED_MARKETPLACE)?
        .as_table()?
        .get("source")?
        .as_str()?;
    Some(path_from_configured_marketplace_source(source))
}

#[cfg(windows)]
fn path_from_configured_marketplace_source(source: &str) -> PathBuf {
    source
        .strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(source))
}

#[cfg(windows)]
fn is_complete_openai_bundled_marketplace(path: &Path) -> bool {
    if !path
        .join(".agents")
        .join("plugins")
        .join("marketplace.json")
        .is_file()
    {
        return false;
    }
    BUNDLED_MARKETPLACE_PLUGINS.iter().all(|plugin| {
        path.join("plugins")
            .join(plugin)
            .join(".codex-plugin")
            .join("plugin.json")
            .is_file()
    })
}

#[cfg(windows)]
fn find_complete_openai_bundled_marketplace(parent: &Path, active: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == active || !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with(BUNDLED_MARKETPLACE) && is_complete_openai_bundled_marketplace(&path) {
            candidates.push(path);
        }
    }
    candidates.sort_by(|left, right| {
        modified_millis(right)
            .cmp(&modified_millis(left))
            .then_with(|| left.cmp(right))
    });
    candidates.into_iter().next()
}

#[cfg(windows)]
fn cache_plugin_root(home: &Path, plugin: &str) -> PathBuf {
    home.join("plugins")
        .join("cache")
        .join(BUNDLED_MARKETPLACE)
        .join(plugin)
}

#[cfg(windows)]
fn can_build_marketplace_from_cache(home: &Path) -> bool {
    BUNDLED_MARKETPLACE_PLUGINS
        .iter()
        .all(|plugin| latest_cache_plugin_version(home, plugin).is_some())
}

#[cfg(windows)]
fn latest_cache_plugin_version(home: &Path, plugin: &str) -> Option<PathBuf> {
    let root = cache_plugin_root(home, plugin);
    let mut candidates = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join(".codex-plugin").join("plugin.json").is_file() {
            candidates.push(path);
        }
    }
    candidates.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let right_name = right
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        right_name
            .cmp(left_name)
            .then_with(|| modified_millis(right).cmp(&modified_millis(left)))
    });
    candidates.into_iter().next()
}

#[cfg(windows)]
fn build_marketplace_from_cache(home: &Path, staging: &Path) -> anyhow::Result<()> {
    let plugins_dir = staging.join("plugins");
    std::fs::create_dir_all(staging.join(".agents").join("plugins"))?;
    std::fs::create_dir_all(&plugins_dir)?;
    std::fs::write(
        staging
            .join(".agents")
            .join("plugins")
            .join("marketplace.json"),
        bundled_marketplace_json().as_bytes(),
    )?;
    for plugin in BUNDLED_MARKETPLACE_PLUGINS {
        let Some(source) = latest_cache_plugin_version(home, plugin) else {
            anyhow::bail!("missing cached {plugin} plugin for openai-bundled marketplace");
        };
        copy_dir_recursive(&source, &plugins_dir.join(plugin))?;
    }
    Ok(())
}

#[cfg(windows)]
fn bundled_marketplace_json() -> String {
    let plugins = [
        ("browser", "Engineering"),
        ("chrome", "Productivity"),
        ("computer-use", "Productivity"),
        ("latex", "Research"),
    ]
    .into_iter()
    .map(|(name, category)| {
        serde_json::json!({
            "name": name,
            "source": {
                "source": "local",
                "path": format!("./plugins/{name}")
            },
            "policy": {
                "installation": "AVAILABLE",
                "authentication": "ON_INSTALL"
            },
            "category": category
        })
    })
    .collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({
        "name": BUNDLED_MARKETPLACE,
        "interface": {
            "displayName": "OpenAI Bundled"
        },
        "plugins": plugins
    }))
    .expect("bundled marketplace JSON should serialize")
}

#[cfg(windows)]
fn replace_active_marketplace(active: &Path, staging: &Path) -> anyhow::Result<()> {
    if active.exists() {
        let backup = active.with_file_name(format!(
            "{BUNDLED_MARKETPLACE}.bak-guard-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        std::fs::rename(active, backup)?;
    }
    std::fs::rename(staging, active)?;
    Ok(())
}

#[cfg(windows)]
fn copy_dir_recursive(source: &Path, destination: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn ensure_openai_bundled_marketplace_config(
    doc: &mut DocumentMut,
    marketplace_path: &Path,
) -> anyhow::Result<()> {
    let marketplaces = table_mut_or_insert(doc, "marketplaces")?;
    if marketplaces
        .get(BUNDLED_MARKETPLACE)
        .and_then(Item::as_table)
        .is_none()
    {
        marketplaces[BUNDLED_MARKETPLACE] = toml_edit::table();
    }
    marketplaces[BUNDLED_MARKETPLACE]["source_type"] = toml_edit::value("local");
    marketplaces[BUNDLED_MARKETPLACE]["source"] =
        toml_edit::value(windows_extended_path(marketplace_path));
    Ok(())
}

fn windows_extended_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value.starts_with(r"\\?\") {
        value.into_owned()
    } else {
        format!(r"\\?\{value}")
    }
}

fn parse_toml_document(contents: &str) -> anyhow::Result<DocumentMut> {
    if contents.trim().is_empty() {
        Ok(DocumentMut::new())
    } else {
        contents
            .parse::<DocumentMut>()
            .with_context(|| "config.toml TOML parse failed")
    }
}

fn table_mut_or_insert<'a>(doc: &'a mut DocumentMut, key: &str) -> anyhow::Result<&'a mut Table> {
    if !doc.as_table().contains_key(key) {
        doc[key] = toml_edit::table();
    }
    if doc.get(key).and_then(Item::as_table).is_none() {
        doc[key] = toml_edit::table();
    }
    doc.get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("{key} must be a TOML table"))
}

fn ensure_plugin_enabled(doc: &mut DocumentMut, plugin_id: &str) -> anyhow::Result<()> {
    let plugins = table_mut_or_insert(doc, "plugins")?;
    if !plugins.contains_key(plugin_id) {
        plugins[plugin_id] = toml_edit::table();
    }
    if plugins.get(plugin_id).and_then(Item::as_table).is_none() {
        plugins[plugin_id] = toml_edit::table();
    }
    plugins[plugin_id]["enabled"] = toml_edit::value(true);
    Ok(())
}

fn ensure_trailing_newline(mut contents: String) -> String {
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER_TRANSPORT_FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const P=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},P().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;
    const HELPER_TRANSPORT_062_FIXTURE: &str = r#"import{spawn as s}from"node:child_process";const H=()=>globalThis.process;const e=()=>{};const w=0,v=0,y=0;function launch(){const i=s(e(this,w,"f"),e(this,v,"f"),{env:null==e(this,y,"f")?void 0:Object.assign(Object.assign({},H().env),e(this,y,"f")),stdio:["pipe","pipe","pipe"],windowsHide:!0});return i}
"#;

    #[cfg(windows)]
    #[test]
    fn paths_equal_accepts_windows_separator_variants() {
        assert!(paths_equal(
            Path::new(r"C:\Codex\runtime\helper_transport.js"),
            Path::new("C:/Codex/runtime/helper_transport.js"),
        ));
    }

    #[test]
    fn supported_computer_use_contracts_bind_each_sky_version_to_its_helper_hash() {
        assert_eq!(
            supported_helper_sha256s("0.4.20").and_then(|hashes| hashes.first().copied()),
            Some("f2b2f56fcd1699b0fa32dec3214a56a1d36b937a2ecf58cc822ab4a904551e03")
        );
        assert_eq!(
            supported_helper_sha256s("0.5.2").and_then(|hashes| hashes.first().copied()),
            Some("2c4cac168200520c2752058177ea9fe7d1ccf9a26b7287dddff669d41ca9af16")
        );
        assert_eq!(
            supported_helper_sha256s("0.6.2").and_then(|hashes| hashes.first().copied()),
            Some("627b317ccfd3c7386a2d5bc4fb4e97ff30e30425945a7a5370006ad89cf3605a")
        );
        assert!(supported_helper_sha256s("0.6.2").unwrap().iter().any(
            |hash| *hash == "463d54ddb8a351cb206cb4bebf4943f63e1bc8087d310d102c5fae417b255eb4"
        ));
        assert_eq!(
            supported_transport_sha256("0.6.2"),
            Some("6423ba834f18139d55cdac2290c91cd9b24b568332b07cddd2a7eda043702b7c")
        );
        assert_eq!(
            supported_helper_sha256s("0.6.6").and_then(|hashes| hashes.first().copied()),
            Some("be488e66c38e12fa46850ee48c1f5e44ecdb0a3a64042e064e3a1a1da286ac42")
        );
        assert_eq!(
            supported_transport_sha256("0.6.6"),
            Some("7bc54c5bb7f49661fb1f501c6832f5490620501464d3f1593a361a85c7f66b39")
        );
        assert_eq!(
            supported_helper_sha256s("0.6.11").and_then(|hashes| hashes.first().copied()),
            Some("7a95d14ebf992955d8ab8e6c57a75545ed7d18e864b0f5c1b9fe7f47685bd897")
        );
        assert_eq!(
            supported_transport_sha256("0.6.11"),
            Some("56ac031983d85e4718f10c5a814923afe2cb4ead649466eef02b1b4d4cf63e40")
        );
        assert_eq!(supported_helper_sha256s("0.5.3"), None);
        assert_eq!(supported_transport_sha256("0.5.3"), None);
    }

    #[cfg(windows)]
    #[test]
    fn descriptorless_recovery_scans_global_runtime_only_for_the_active_home() {
        let default_home = crate::codex_home::default_codex_home_dir();
        assert!(recovery_home_matches_default(&default_home));

        let temp = tempfile::tempdir().unwrap();
        assert!(!recovery_home_matches_default(temp.path()));
    }

    #[cfg(windows)]
    #[test]
    fn packaged_computer_use_runtime_is_materialized_before_first_codex_launch() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("package").join("cua_node");
        std::fs::create_dir_all(source.join("bin/node_modules/@oai/sky")).unwrap();
        std::fs::write(source.join("manifest.json"), b"manifest").unwrap();
        std::fs::write(source.join("bin/node.exe"), b"node").unwrap();
        std::fs::write(source.join("bin/node_repl.exe"), b"repl").unwrap();
        std::fs::write(
            source.join("bin/node_modules/@oai/sky/package.json"),
            br#"{"version":"0.4.20"}"#,
        )
        .unwrap();
        let destination_root = temp.path().join("local-runtimes");

        let installed = ensure_packaged_computer_use_runtime_copy(&source, &destination_root)
            .expect("materialize bundled Computer Use runtime");

        assert_eq!(installed, destination_root.join("b5d5954590634650"));
        assert_eq!(
            std::fs::read(installed.join("manifest.json")).unwrap(),
            b"manifest"
        );
        assert_eq!(
            std::fs::read(installed.join("bin/node.exe")).unwrap(),
            b"node"
        );
        assert_eq!(
            std::fs::read(installed.join("bin/node_repl.exe")).unwrap(),
            b"repl"
        );
        assert!(
            installed
                .join("bin/node_modules/@oai/sky/package.json")
                .is_file()
        );

        std::fs::write(installed.join("bin/node.exe"), b"tampered").unwrap();
        let repaired = ensure_packaged_computer_use_runtime_copy(&source, &destination_root)
            .expect("replace an invalid pre-existing runtime from the packaged source");
        assert_eq!(repaired, installed);
        assert_eq!(
            std::fs::read(repaired.join("bin/node.exe")).unwrap(),
            b"node"
        );
    }

    #[cfg(windows)]
    #[test]
    fn stale_pure_api_notify_path_falls_back_to_a_valid_official_runtime() {
        let Some(app_dir) = crate::app_paths::find_latest_codex_app_dir_default() else {
            eprintln!("SKIP: official Codex package is not installed");
            return;
        };
        let source_sky = app_dir.join("resources/cua_node/bin/node_modules/@oai/sky");
        if !source_sky.is_dir() {
            eprintln!("SKIP: official packaged Computer Use runtime is unavailable");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let sky =
            home.join("plugins/cache/openai-bundled/computer-use/fixture/node_modules/@oai/sky");
        for relative in [
            "package.json",
            "bin/windows/codex-computer-use.exe",
            "dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js",
        ] {
            let destination = sky.join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::copy(source_sky.join(relative), destination).unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            r#"model_provider = "custom"
notify = ["C:\\missing-runtime\\codex-computer-use.exe", "turn-ended"]
"#,
        )
        .unwrap();

        let resolved = resolve_admin_computer_use_artifacts(&home)
            .expect("ignore stale pure-API notify path and resolve a supported runtime");

        assert!(resolved.helper_exe.is_file());
        assert_ne!(
            resolved.helper_exe,
            PathBuf::from(r"C:\missing-runtime\codex-computer-use.exe")
        );
        validate_admin_computer_use_artifacts(
            &resolved.helper_exe,
            &resolved.helper_transport,
            &resolved.sky_version,
        )
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn descriptor_bound_recovery_uses_old_runtime_when_helper_executable_is_missing() {
        let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
            eprintln!("SKIP: LOCALAPPDATA is unavailable");
            return;
        };
        let runtimes = PathBuf::from(local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("runtimes")
            .join("cua_node");
        let mut source = None;
        if let Ok(entries) = std::fs::read_dir(runtimes) {
            for entry in entries.flatten() {
                let sky = entry.path().join("bin/node_modules/@oai/sky");
                let package_path = sky.join("package.json");
                let transport_path = sky.join(SKY_HELPER_TRANSPORT_RELATIVE_PATH);
                let Ok(package) = std::fs::read(&package_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .ok_or(())
                else {
                    continue;
                };
                let Some(version) = package["version"].as_str() else {
                    continue;
                };
                let Some(expected_hash) = supported_transport_sha256(version) else {
                    continue;
                };
                if transport_path.is_file()
                    && sha256_file(&transport_path).is_ok_and(|hash| hash == expected_hash)
                {
                    source = Some((sky, version.to_owned()));
                    break;
                }
            }
        }
        let Some((source_sky, sky_version)) = source else {
            eprintln!("SKIP: no supported installed Computer Use transport is available");
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let sky = home
            .join("plugins/cache/openai-bundled/computer-use/old-runtime/node_modules/@oai/sky");
        let transport = sky.join(SKY_HELPER_TRANSPORT_RELATIVE_PATH);
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        std::fs::copy(source_sky.join("package.json"), sky.join("package.json")).unwrap();

        let descriptor = temp.path().join("descriptor.json");
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let source_transport = source_sky.join(SKY_HELPER_TRANSPORT_RELATIVE_PATH);
        std::fs::copy(&source_transport, &backup).unwrap();
        let original_hash = sha256_file(&backup).unwrap();
        let original = std::fs::read_to_string(&backup).unwrap();
        let patched = patch_admin_helper_transport(&original, &sky_version, &descriptor).unwrap();
        std::fs::write(&transport, patched.as_bytes()).unwrap();
        let transport_path = std::fs::canonicalize(&transport).unwrap();
        let backup_path = std::fs::canonicalize(&backup).unwrap();
        let patched_hash = sha256_file(&transport).unwrap();

        let resolved = resolve_admin_computer_use_artifacts_for_transport(
            &home,
            &transport_path,
            &original_hash,
        )
        .expect("resolve the runtime bound by the stale descriptor");
        assert_eq!(resolved.sky_version, sky_version);
        assert_eq!(
            std::fs::canonicalize(&resolved.helper_transport).unwrap(),
            transport_path
        );
        assert!(!resolved.helper_exe.exists());

        verify_stale_admin_computer_use_hook(
            &home,
            &descriptor,
            &transport_path,
            &backup_path,
            &original_hash,
            &patched_hash,
        )
        .unwrap();
        restore_stale_admin_computer_use_hook(
            &home,
            &descriptor,
            &transport_path,
            &backup_path,
            &original_hash,
            &patched_hash,
        )
        .unwrap();

        assert_eq!(std::fs::read(&transport).unwrap(), original.as_bytes());
        assert!(!backup.exists());
    }

    #[test]
    fn descriptorless_recovery_without_owned_evidence_ignores_missing_runtime() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            !recover_descriptorless_admin_computer_use_at_roots(&[temp
                .path()
                .join("missing-runtime")])
            .unwrap()
        );
    }

    #[test]
    fn descriptorless_recovery_without_owned_evidence_ignores_unsupported_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp
            .path()
            .join("runtime/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        std::fs::write(&transport, b"future unsupported transport").unwrap();

        assert!(
            !recover_descriptorless_admin_computer_use_at_roots(&[temp.path().to_path_buf()])
                .unwrap()
        );
        assert_eq!(
            std::fs::read(&transport).unwrap(),
            b"future unsupported transport"
        );
    }

    #[test]
    fn descriptorless_recovery_checks_marker_only_when_state_evidence_requires_it() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp
            .path()
            .join("runtime/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        let marked = format!("{ADMIN_HELPER_HOOK_BEGIN}broken{ADMIN_HELPER_HOOK_END}");
        std::fs::write(&transport, &marked).unwrap();
        let roots = [temp.path().to_path_buf()];

        assert!(!recover_descriptorless_admin_computer_use_at_roots(&roots).unwrap());
        assert!(
            recover_descriptorless_admin_computer_use_at_roots_with_options(
                &roots,
                true,
                None,
                || Ok(())
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&transport).unwrap(), marked);
    }

    #[test]
    fn descriptorless_recovery_restores_owned_unknown_version_rename_window() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp
            .path()
            .join("runtime/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        let original = b"future transport bytes";
        std::fs::write(&transport, original).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        std::fs::rename(&transport, &backup).unwrap();

        assert!(
            recover_descriptorless_admin_computer_use_at_roots(&[temp.path().to_path_buf()])
                .unwrap()
        );
        assert_eq!(std::fs::read(&transport).unwrap(), original);
        assert!(!backup.exists());
    }

    #[cfg(windows)]
    #[test]
    fn descriptorless_recovery_rejects_junction_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("trusted");
        let outside = temp.path().join("outside");
        let outside_transport = outside
            .join("node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(outside_transport.parent().unwrap()).unwrap();
        let backup = outside_transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        std::fs::write(&backup, b"outside bytes").unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let junction = root.join("runtime");
        let status = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());

        assert!(recover_descriptorless_admin_computer_use_at_roots(&[root]).is_err());
        assert_eq!(std::fs::read(&backup).unwrap(), b"outside bytes");
        assert!(!outside_transport.exists());
    }

    #[cfg(windows)]
    #[test]
    fn descriptorless_recovery_rejects_reparse_root_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let transport = outside
            .join("runtime/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        std::fs::write(&backup, b"outside bytes").unwrap();
        let junction = temp.path().join("trusted-link");
        let status = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());

        assert!(
            recover_descriptorless_admin_computer_use_at_roots(&[junction.join("runtime")])
                .is_err()
        );
        assert_eq!(std::fs::read(&backup).unwrap(), b"outside bytes");
        assert!(!transport.exists());
    }

    #[cfg(windows)]
    #[test]
    fn descriptorless_recovery_holds_candidate_lease_during_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("trusted");
        let transport = root
            .join("runtime/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        std::fs::write(&backup, b"owned original").unwrap();
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"attacker replacement").unwrap();

        assert!(
            recover_descriptorless_admin_computer_use_at_roots_with_hook(&[root], || {
                assert!(std::fs::remove_file(&backup).is_err());
                assert!(std::fs::rename(&replacement, &backup).is_err());
                Ok(())
            })
            .unwrap()
        );
        assert_eq!(std::fs::read(&transport).unwrap(), b"owned original");
        assert!(!backup.exists());
        assert_eq!(
            std::fs::read(&replacement).unwrap(),
            b"attacker replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn descriptorless_recovery_rejects_multiply_linked_transport() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("trusted");
        let transport = root
            .join("runtime/node_modules/@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/helper_transport.js");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        let descriptor = temp.path().join("descriptor.json");
        let patched = patch_admin_helper_transport(
            HELPER_TRANSPORT_FIXTURE,
            SUPPORTED_SKY_VERSION,
            &descriptor,
        )
        .unwrap();
        let outside = temp.path().join("outside.js");
        std::fs::write(&outside, patched.as_bytes()).unwrap();
        std::fs::hard_link(&outside, &transport).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        std::fs::write(&backup, HELPER_TRANSPORT_FIXTURE).unwrap();

        assert!(recover_descriptorless_admin_computer_use_at_roots(&[root]).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), patched);
        assert_eq!(std::fs::read_to_string(&transport).unwrap(), patched);
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
    }

    #[test]
    fn computer_use_guard_admin_hook_is_marked_idempotent_and_reversible() {
        let descriptor = Path::new(r"C:\Users\me\.codex\admin-computer-use.json");
        let patched = patch_admin_helper_transport(HELPER_TRANSPORT_FIXTURE, "0.4.20", descriptor)
            .expect("supported transport must patch");
        assert_eq!(patched.matches(ADMIN_HELPER_HOOK_BEGIN).count(), 1);
        assert!(patched.contains("computer-use-client"));
        assert!(patched.contains("--proof-file"));
        assert!(patched.contains("originalCommand"));
        assert!(patched.contains("...originalArgs"));

        let repeated = patch_admin_helper_transport(&patched, "0.4.20", descriptor)
            .expect("repeated patch must be idempotent");
        assert_eq!(repeated, patched);
        assert_eq!(
            remove_admin_helper_transport_patch(&patched).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
    }

    #[test]
    fn computer_use_guard_admin_hook_supports_sky_062_process_accessor() {
        let descriptor = Path::new(r"C:\Users\me\.codex\admin-computer-use.json");
        let patched =
            patch_admin_helper_transport(HELPER_TRANSPORT_062_FIXTURE, "0.6.2", descriptor)
                .expect("Sky 0.6.2 transport must patch");
        assert!(patched.contains("H().getBuiltinModule"));
        assert!(patched.contains("Object.assign({},H().env)"));
        assert!(!patched.contains("P().getBuiltinModule"));
        assert_eq!(
            patch_admin_helper_transport(&patched, "0.6.2", descriptor).unwrap(),
            patched
        );
        assert_eq!(
            remove_admin_helper_transport_patch(&patched).unwrap(),
            HELPER_TRANSPORT_062_FIXTURE
        );

        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("helper_transport-0.6.2.mjs");
        std::fs::write(&script, patched).unwrap();
        let status = std::process::Command::new("node")
            .arg("--check")
            .arg(&script)
            .status()
            .expect("Node.js is required for helper hook syntax verification");
        assert!(status.success());
    }

    #[test]
    fn computer_use_guard_admin_hook_supports_sky_066_process_accessor() {
        let descriptor = Path::new(r"C:\Users\me\.codex\admin-computer-use.json");
        let patched = patch_admin_helper_transport(HELPER_TRANSPORT_FIXTURE, "0.6.6", descriptor)
            .expect("Sky 0.6.6 transport must patch");
        assert!(patched.contains("P().getBuiltinModule"));
        assert!(patched.contains("Object.assign({},P().env)"));
        assert_eq!(
            patch_admin_helper_transport(&patched, "0.6.6", descriptor).unwrap(),
            patched
        );
        assert_eq!(
            remove_admin_helper_transport_patch(&patched).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
    }

    #[test]
    fn computer_use_guard_admin_hook_fails_closed_on_version_or_template_mismatch() {
        assert!(
            patch_admin_helper_transport(
                HELPER_TRANSPORT_FIXTURE,
                "0.4.21",
                Path::new("descriptor.json")
            )
            .is_err()
        );
        let changed = HELPER_TRANSPORT_FIXTURE.replace("windowsHide:!0", "windowsHide:!1");
        assert!(
            patch_admin_helper_transport(&changed, "0.4.20", Path::new("descriptor.json")).is_err()
        );
        assert!(!changed.contains(ADMIN_HELPER_HOOK_BEGIN));
    }

    #[test]
    fn computer_use_guard_admin_hook_rejects_tampered_marker_body_or_descriptor_change() {
        let descriptor = Path::new(r"C:\state\descriptor.json");
        let patched =
            patch_admin_helper_transport(HELPER_TRANSPORT_FIXTURE, "0.4.20", descriptor).unwrap();
        let tampered = patched.replace("computer-use-client", "computer-use-client-tampered");
        assert!(patch_admin_helper_transport(&tampered, "0.4.20", descriptor).is_err());
        assert!(
            patch_admin_helper_transport(&patched, "0.4.20", Path::new(r"C:\state\different.json"))
                .is_err()
        );
    }

    #[test]
    fn computer_use_guard_admin_hook_is_valid_node_syntax() {
        let patched = patch_admin_helper_transport(
            HELPER_TRANSPORT_FIXTURE,
            "0.4.20",
            Path::new(r"C:\state\descriptor.json"),
        )
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("helper_transport.mjs");
        std::fs::write(&script, patched).unwrap();
        let status = std::process::Command::new("node")
            .arg("--check")
            .arg(&script)
            .status()
            .expect("Node.js is required for helper hook syntax verification");
        assert!(status.success());
    }

    #[test]
    fn computer_use_guard_admin_hook_never_spawns_original_helper_when_descriptor_is_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("original-helper-spawned");
        let original = temp.path().join("original-helper.cjs");
        std::fs::write(
            &original,
            format!(
                "require('node:fs').writeFileSync({}, 'spawned')",
                serde_json::to_string(&marker.to_string_lossy().as_ref()).unwrap()
            ),
        )
        .unwrap();
        let fixture = format!(
            "import{{spawn as s}}from\"node:child_process\";const P=()=>globalThis.process;let calls=0;const e=()=>++calls===1?process.execPath:calls===2?[{}]:undefined;const w=0,v=0,y=0;function launch(){{{HELPER_TRANSPORT_LAUNCH_TEMPLATE}}}launch();",
            serde_json::to_string(&original.to_string_lossy().as_ref()).unwrap()
        );

        for (name, descriptor_setup) in [
            ("missing", None),
            ("malformed", Some(b"{".as_slice())),
            ("tampered", Some(br#"{"shimPath":"","pipeName":"pipe","sessionId":"session","proofPath":"proof"}"#.as_slice())),
        ] {
            let descriptor = temp.path().join(format!("{name}.json"));
            if let Some(bytes) = descriptor_setup {
                std::fs::write(&descriptor, bytes).unwrap();
            }
            let patched = patch_admin_helper_transport(&fixture, "0.4.20", &descriptor).unwrap();
            let script = temp.path().join(format!("{name}.mjs"));
            std::fs::write(&script, patched).unwrap();
            let output = std::process::Command::new("node")
                .arg(&script)
                .output()
                .expect("Node.js is required for fail-closed hook verification");
            assert!(!output.status.success(), "{name} descriptor must fail closed");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("administrator Computer Use unavailable"));
            assert!(!stderr.contains("proof-token-secret"));
            assert!(!marker.exists(), "{name} descriptor spawned original helper");
        }

        let unreadable = temp.path().join("unreadable.json");
        std::fs::create_dir(&unreadable).unwrap();
        let patched = patch_admin_helper_transport(&fixture, "0.4.20", &unreadable).unwrap();
        let script = temp.path().join("unreadable.mjs");
        std::fs::write(&script, patched).unwrap();
        let output = std::process::Command::new("node")
            .arg(&script)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("administrator Computer Use unavailable"));
        assert!(!stderr.contains("proof-token-secret"));
        assert!(
            !marker.exists(),
            "unreadable descriptor spawned original helper"
        );
    }

    #[test]
    fn computer_use_guard_admin_hook_install_remove_restores_full_file_and_backup() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let descriptor = temp.path().join("descriptor.json");
        assert_eq!(
            install_admin_computer_use_hook_with_artifacts(&artifacts, &descriptor).unwrap(),
            ComputerUseHookOutcome::Installed
        );
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert_eq!(
            install_admin_computer_use_hook_with_artifacts(&artifacts, &descriptor).unwrap(),
            ComputerUseHookOutcome::AlreadyInstalled
        );
        assert_eq!(
            remove_admin_computer_use_hook_with_artifacts(&artifacts).unwrap(),
            ComputerUseHookOutcome::Removed
        );
        assert_eq!(
            std::fs::read_to_string(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert!(!backup.exists());
    }

    #[test]
    fn startup_rollback_restores_hook_when_later_shim_lookup_fails() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let descriptor = temp.path().join("descriptor.json");

        let result = (|| -> anyhow::Result<()> {
            let _rollback = install_admin_computer_use_hook_transaction_with_artifacts(
                &artifacts,
                &descriptor,
            )?;
            assert!(
                std::fs::read_to_string(&transport)?.contains(ADMIN_HELPER_HOOK_BEGIN),
                "the later failure must happen after hook installation"
            );
            std::fs::canonicalize(temp.path().join("missing-shim.exe"))?;
            Ok(())
        })();

        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert!(
            !transport
                .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP)
                .exists()
        );
    }

    #[test]
    fn transaction_rejects_transport_replaced_after_resolution_without_patching_it() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let expected_hash = sha256_file(&transport).unwrap();
        let replacement = HELPER_TRANSPORT_FIXTURE.replacen(
            "function launch()",
            "const replaced=1;function launch()",
            1,
        );
        std::fs::write(&transport, &replacement).unwrap();

        assert!(
            install_admin_computer_use_hook_transaction_with_expected_hash(
                &artifacts,
                &temp.path().join("descriptor.json"),
                &expected_hash,
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&transport).unwrap(), replacement);
        assert!(
            !std::fs::read_to_string(&transport)
                .unwrap()
                .contains(ADMIN_HELPER_HOOK_BEGIN)
        );
        assert!(
            !transport
                .with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP)
                .exists()
        );
    }

    #[test]
    fn transaction_pins_transport_against_intervening_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let expected_hash = sha256_file(&transport).unwrap();

        assert!(
            install_admin_computer_use_hook_transaction_impl(
                &artifacts,
                &temp.path().join("descriptor.json"),
                &expected_hash,
                || std::fs::write(&transport, b"intervening target").map_err(Into::into),
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE.as_bytes()
        );
        assert_eq!(
            std::fs::read(transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP)).unwrap(),
            HELPER_TRANSPORT_FIXTURE.as_bytes()
        );
    }

    #[cfg(windows)]
    #[test]
    fn transaction_rejects_a_transport_reparse_point_without_writing_its_target() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::create_dir(&outside).unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&transport)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport,
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };

        assert!(
            install_admin_computer_use_hook_transaction_with_expected_hash(
                &artifacts,
                &temp.path().join("descriptor.json"),
                SUPPORTED_HELPER_TRANSPORT_SHA256,
            )
            .is_err()
        );
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[test]
    fn stale_restore_recovers_a_partial_transport_write_from_the_verified_backup() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let descriptor = temp.path().join("descriptor.json");
        install_admin_computer_use_hook_with_artifacts(&artifacts, &descriptor).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let original_hash = sha256_file(&backup).unwrap();
        let patched = std::fs::read(&transport).unwrap();
        let patched_hash = format!("{:x}", Sha256::digest(&patched));
        std::fs::write(&transport, &patched[..patched.len() / 2]).unwrap();

        restore_stale_admin_computer_use_hook_with_artifacts(
            &artifacts,
            &descriptor,
            &std::fs::canonicalize(&transport).unwrap(),
            &std::fs::canonicalize(&backup).unwrap(),
            &original_hash,
            &patched_hash,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert!(!backup.exists());
    }

    #[test]
    fn descriptorless_recovery_accepts_only_the_expected_partial_patched_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let descriptor = temp.path().join("descriptor.json");
        std::fs::write(&backup, HELPER_TRANSPORT_FIXTURE).unwrap();
        let expected = patch_admin_helper_transport(
            HELPER_TRANSPORT_FIXTURE,
            SUPPORTED_SKY_VERSION,
            &descriptor,
        )
        .unwrap();
        std::fs::write(&transport, &expected.as_bytes()[..expected.len() / 2]).unwrap();
        let candidate = AdminRecoveryCandidate {
            root: std::fs::canonicalize(temp.path()).unwrap(),
            transport: transport.clone(),
        };
        let mut lease = AdminRecoveryCandidateLease::open(candidate).unwrap();
        assert!(lease.needs_recovery().unwrap());
        lease.recover(Some(&descriptor)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert!(!backup.exists());
    }

    #[test]
    fn preflight_restores_missing_transport_from_owned_official_backup() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        let expected_hash = sha256_file(&transport).unwrap();
        std::fs::rename(&transport, &backup).unwrap();

        recover_interrupted_admin_computer_use_hook(&transport, &expected_hash).unwrap();

        assert_eq!(
            std::fs::read_to_string(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert!(!backup.exists());
    }

    #[cfg(windows)]
    #[test]
    fn preflight_rejects_a_hardlinked_backup_without_publishing_it() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let outside = temp.path().join("outside-helper-transport.js");
        std::fs::write(&outside, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::hard_link(&outside, &backup).unwrap();
        let expected_hash = sha256_file(&outside).unwrap();

        assert!(recover_interrupted_admin_computer_use_hook(&transport, &expected_hash).is_err());
        assert!(!transport.exists());
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
    }

    #[test]
    fn transaction_rejects_existing_hook_bound_to_different_descriptor() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let descriptor_a = temp.path().join("descriptor-a.json");
        install_admin_computer_use_hook_with_artifacts(&artifacts, &descriptor_a).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let expected_hash = sha256_file(&backup).unwrap();
        let before = [
            std::fs::read(&transport).unwrap(),
            std::fs::read(&backup).unwrap(),
        ];

        assert!(
            install_admin_computer_use_hook_transaction_with_expected_hash(
                &artifacts,
                &temp.path().join("descriptor-b.json"),
                &expected_hash,
            )
            .is_err()
        );
        assert_eq!(before[0], std::fs::read(&transport).unwrap());
        assert_eq!(before[1], std::fs::read(&backup).unwrap());
    }

    #[test]
    fn stale_dead_broker_restores_exact_original_and_removes_owned_backup() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let descriptor = temp.path().join("descriptor.json");
        install_admin_computer_use_hook_with_artifacts(&artifacts, &descriptor).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let original_hash = sha256_file(&backup).unwrap();
        let patched_hash = sha256_file(&transport).unwrap();

        restore_stale_admin_computer_use_hook_with_artifacts(
            &artifacts,
            &descriptor,
            &transport,
            &backup,
            &original_hash,
            &patched_hash,
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&transport).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
        assert!(!backup.exists());
    }

    #[test]
    fn stale_forged_path_hash_or_original_bytes_fail_closed_without_touching_files() {
        let temp = tempfile::tempdir().unwrap();
        let transport = temp.path().join("helper_transport.js");
        let helper = temp.path().join(COMPUTER_USE_EXE);
        std::fs::write(&transport, HELPER_TRANSPORT_FIXTURE).unwrap();
        std::fs::write(&helper, b"fixture helper").unwrap();
        let artifacts = AdminComputerUseArtifacts {
            helper_exe: helper,
            helper_transport: transport.clone(),
            sky_version: SUPPORTED_SKY_VERSION.to_owned(),
        };
        let descriptor = temp.path().join("descriptor.json");
        install_admin_computer_use_hook_with_artifacts(&artifacts, &descriptor).unwrap();
        let backup = transport.with_file_name(ADMIN_HELPER_TRANSPORT_BACKUP);
        let original_hash = sha256_file(&backup).unwrap();
        let patched = std::fs::read_to_string(&transport).unwrap();
        let patched_hash = sha256_file(&transport).unwrap();
        let forged = temp.path().join("forged.js");
        std::fs::write(&forged, b"do not touch").unwrap();

        assert!(
            restore_stale_admin_computer_use_hook_with_artifacts(
                &artifacts,
                &descriptor,
                &forged,
                &backup,
                &original_hash,
                &patched_hash
            )
            .is_err()
        );
        assert!(
            restore_stale_admin_computer_use_hook_with_artifacts(
                &artifacts,
                &descriptor,
                &transport,
                &backup,
                &original_hash,
                "unknown"
            )
            .is_err()
        );
        let tampered = patched.replace("return i}", "return null}");
        std::fs::write(&transport, &tampered).unwrap();
        let tampered_hash = sha256_file(&transport).unwrap();
        assert!(
            restore_stale_admin_computer_use_hook_with_artifacts(
                &artifacts,
                &descriptor,
                &transport,
                &backup,
                &original_hash,
                &tampered_hash
            )
            .is_err()
        );

        assert_eq!(std::fs::read(&forged).unwrap(), b"do not touch");
        assert_eq!(std::fs::read_to_string(&transport).unwrap(), tampered);
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            HELPER_TRANSPORT_FIXTURE
        );
    }

    #[test]
    fn guard_config_text_repairs_computer_use_settings() {
        let updated = guard_config_text(
            "\u{feff}notify = [\"old.exe\", \"turn-ended\"]\n\n[features]\njs_repl = false\n\n[plugins.\"computer-use@openai-bundled\"]\nenabled = false\n",
            Some(Path::new(r"C:\tools\codex-computer-use.exe")),
        )
        .unwrap();

        assert!(!updated.as_bytes().starts_with(&[0xef, 0xbb, 0xbf]));
        assert!(updated.contains("js_repl = true"));
        assert!(updated.contains("[plugins.\"browser@openai-bundled\"]"));
        assert!(updated.contains("[plugins.\"chrome@openai-bundled\"]"));
        assert!(updated.contains("[plugins.\"computer-use@openai-bundled\"]"));
        assert!(updated.contains("enabled = true"));
        let parsed = updated.parse::<DocumentMut>().unwrap();
        let notify = parsed["notify"].as_array().unwrap();
        assert_eq!(
            notify.get(0).and_then(|value| value.as_str()),
            Some(r"C:\tools\codex-computer-use.exe")
        );
        assert_eq!(
            notify.get(1).and_then(|value| value.as_str()),
            Some("turn-ended")
        );
        assert!(!updated.contains("old.exe"));
    }

    #[test]
    fn guard_config_text_creates_missing_sections() {
        let updated = guard_config_text("model = \"gpt-5\"\n", None).unwrap();

        assert!(updated.contains("[features]"));
        assert!(updated.contains("js_repl = true"));
        for plugin_id in COMPUTER_USE_PLUGINS {
            assert!(updated.contains(&format!("[plugins.\"{plugin_id}\"]")));
        }
        assert!(!updated.contains("notify ="));
    }

    #[test]
    fn guard_config_text_writes_openai_bundled_marketplace_source() {
        let updated = guard_config_text_with_marketplace(
            "model = \"gpt-5\"\n\n[marketplaces.openai-bundled]\nsource_type = \"remote\"\nsource = \"old\"\n",
            None,
            Some(Path::new(r"C:\Users\me\.codex\.tmp\bundled-marketplaces\openai-bundled")),
        )
        .unwrap();
        let parsed = updated.parse::<DocumentMut>().unwrap();
        assert_eq!(
            parsed["marketplaces"]["openai-bundled"]["source_type"].as_str(),
            Some("local")
        );
        assert_eq!(
            parsed["marketplaces"]["openai-bundled"]["source"].as_str(),
            Some(r"\\?\C:\Users\me\.codex\.tmp\bundled-marketplaces\openai-bundled")
        );
    }

    #[test]
    fn add_sky_internal_computer_use_export_adds_exact_subpath() {
        let updated = add_sky_internal_computer_use_export(
            r#"{
  "name": "@oai/sky",
  "exports": {
    ".": "./dist/project/cua/sky_js/src/index.js"
  }
}"#,
        )
        .unwrap()
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&updated).unwrap();

        assert_eq!(
            parsed["exports"][SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT].as_str(),
            Some(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT)
        );
        assert!(updated.ends_with('\n'));
    }

    #[test]
    fn add_sky_internal_computer_use_export_is_idempotent() {
        let updated = add_sky_internal_computer_use_export(&format!(
            r#"{{
  "name": "@oai/sky",
  "exports": {{
    ".": "./dist/project/cua/sky_js/src/index.js",
    "{SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT}": "{SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT}"
  }}
}}"#
        ))
        .unwrap();

        assert!(updated.is_none());
    }

    #[test]
    fn add_sky_internal_computer_use_export_ignores_non_object_exports() {
        let updated =
            add_sky_internal_computer_use_export(r#"{ "name": "@oai/sky", "exports": "." }"#)
                .unwrap();

        assert!(updated.is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn runtime_exports_compat_is_noop_off_windows() {
        let temp = tempfile::tempdir().unwrap();
        let result = ensure_computer_use_runtime_exports_compat(temp.path()).unwrap();

        assert!(!result.changed);
        assert!(result.package_json.is_none());
        assert!(result.backup_path.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn runtime_exports_compat_adds_missing_exact_export() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".codex");
        let script = home
            .join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use")
            .join("26.608.12217")
            .join("scripts")
            .join(COMPUTER_USE_CLIENT_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(
            &script,
            format!("import {{ x }} from \"{SKY_INTERNAL_COMPUTER_USE_CLIENT_IMPORT}\";\n"),
        )
        .unwrap();

        let package_json = temp.path().join("@oai").join("sky").join("package.json");
        let internal_file = package_json
            .parent()
            .unwrap()
            .join(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT.trim_start_matches("./"));
        std::fs::create_dir_all(internal_file.parent().unwrap()).unwrap();
        std::fs::write(
            &internal_file,
            "export class WindowsComputerUseClientBase {}\n",
        )
        .unwrap();
        std::fs::write(
            &package_json,
            r#"{
  "name": "@oai/sky",
  "exports": {
    ".": "./dist/project/cua/sky_js/src/index.js"
  }
}
"#,
        )
        .unwrap();

        let result =
            ensure_computer_use_runtime_exports_compat_windows(&home, Some(&package_json)).unwrap();

        assert!(result.changed);
        assert_eq!(result.package_json.as_deref(), Some(package_json.as_path()));
        assert!(result.backup_path.as_deref().unwrap().is_file());
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&package_json).unwrap()).unwrap();
        assert_eq!(
            parsed["exports"][SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT].as_str(),
            Some(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT)
        );
    }

    #[cfg(windows)]
    #[test]
    fn runtime_exports_compat_skips_when_internal_file_missing() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".codex");
        let script = home
            .join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use")
            .join("26.608.12217")
            .join("scripts")
            .join(COMPUTER_USE_CLIENT_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(
            &script,
            format!("import {{ x }} from \"{SKY_INTERNAL_COMPUTER_USE_CLIENT_IMPORT}\";\n"),
        )
        .unwrap();
        let package_json = temp.path().join("@oai").join("sky").join("package.json");
        std::fs::create_dir_all(package_json.parent().unwrap()).unwrap();
        std::fs::write(
            &package_json,
            r#"{ "name": "@oai/sky", "exports": { ".": "./index.js" } }"#,
        )
        .unwrap();

        let result =
            ensure_computer_use_runtime_exports_compat_windows(&home, Some(&package_json)).unwrap();

        assert!(!result.changed);
        assert!(
            !package_json
                .parent()
                .unwrap()
                .join(SKY_PACKAGE_EXPORTS_BACKUP)
                .exists()
        );
    }

    #[cfg(windows)]
    #[test]
    fn runtime_exports_compat_skips_when_plugin_script_no_longer_needs_patch() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".codex");
        let script = home
            .join("plugins")
            .join("cache")
            .join("openai-bundled")
            .join("computer-use")
            .join("26.608.12217")
            .join("scripts")
            .join(COMPUTER_USE_CLIENT_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "import { sky } from \"@oai/sky\";\n").unwrap();
        let package_json = temp.path().join("@oai").join("sky").join("package.json");
        let internal_file = package_json
            .parent()
            .unwrap()
            .join(SKY_INTERNAL_COMPUTER_USE_CLIENT_EXPORT.trim_start_matches("./"));
        std::fs::create_dir_all(internal_file.parent().unwrap()).unwrap();
        std::fs::write(
            &internal_file,
            "export class WindowsComputerUseClientBase {}\n",
        )
        .unwrap();
        std::fs::write(
            &package_json,
            r#"{ "name": "@oai/sky", "exports": { ".": "./index.js" } }"#,
        )
        .unwrap();

        let result =
            ensure_computer_use_runtime_exports_compat_windows(&home, Some(&package_json)).unwrap();

        assert!(!result.changed);
    }

    #[cfg(windows)]
    #[test]
    fn ensure_openai_bundled_marketplace_rebuilds_damaged_active_from_cache() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let active = home
            .join(".tmp")
            .join("bundled-marketplaces")
            .join(BUNDLED_MARKETPLACE);
        std::fs::create_dir_all(active.join("plugins").join("chrome").join(".codex-plugin"))
            .unwrap();
        std::fs::write(
            active
                .join("plugins")
                .join("chrome")
                .join(".codex-plugin")
                .join("plugin.json"),
            "{}",
        )
        .unwrap();

        for plugin in BUNDLED_MARKETPLACE_PLUGINS {
            let root = home
                .join("plugins")
                .join("cache")
                .join(BUNDLED_MARKETPLACE)
                .join(plugin)
                .join("26.608.12217");
            std::fs::create_dir_all(root.join(".codex-plugin")).unwrap();
            std::fs::write(root.join(".codex-plugin").join("plugin.json"), "{}").unwrap();
            std::fs::write(root.join("payload.txt"), plugin).unwrap();
        }

        let repaired = ensure_openai_bundled_marketplace(home).unwrap().unwrap();
        assert_eq!(repaired, active);
        assert!(
            active
                .join(".agents")
                .join("plugins")
                .join("marketplace.json")
                .is_file()
        );
        let marketplace = std::fs::read_to_string(
            active
                .join(".agents")
                .join("plugins")
                .join("marketplace.json"),
        )
        .unwrap();
        assert!(marketplace.contains("\"computer-use\""));
        for plugin in BUNDLED_MARKETPLACE_PLUGINS {
            assert!(
                active
                    .join("plugins")
                    .join(plugin)
                    .join(".codex-plugin")
                    .join("plugin.json")
                    .is_file()
            );
            assert_eq!(
                std::fs::read_to_string(active.join("plugins").join(plugin).join("payload.txt"))
                    .unwrap(),
                *plugin
            );
        }
        let backup_count = std::fs::read_dir(active.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("openai-bundled.bak-guard-")
            })
            .count();
        assert_eq!(backup_count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn ensure_openai_bundled_marketplace_reuses_configured_complete_staging() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let parent = home.join(".tmp").join("bundled-marketplaces");
        let active = parent.join(BUNDLED_MARKETPLACE);
        let configured = parent.join("openai-bundled.guard-staging-existing");
        std::fs::create_dir_all(active.join("plugins")).unwrap();
        std::fs::create_dir_all(configured.join(".agents").join("plugins")).unwrap();
        std::fs::write(
            configured
                .join(".agents")
                .join("plugins")
                .join("marketplace.json"),
            "{}",
        )
        .unwrap();
        for plugin in BUNDLED_MARKETPLACE_PLUGINS {
            let plugin_root = configured
                .join("plugins")
                .join(plugin)
                .join(".codex-plugin");
            std::fs::create_dir_all(&plugin_root).unwrap();
            std::fs::write(plugin_root.join("plugin.json"), "{}").unwrap();
        }
        let source = format!(r"\\?\{}", configured.display());
        std::fs::write(
            home.join("config.toml"),
            format!(
                "[marketplaces.openai-bundled]\nsource_type = \"local\"\nsource = '{}'\n",
                source
            ),
        )
        .unwrap();

        let repaired = ensure_openai_bundled_marketplace(home).unwrap().unwrap();
        assert_eq!(repaired, configured);
        let guard_staging_count = std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("openai-bundled.guard-staging-")
            })
            .count();
        assert_eq!(guard_staging_count, 1);
    }
}

/// Kill orphaned SkyComputerUseClient processes on macOS.
///
/// On macOS, Codex spawns a `SkyComputerUseClient` subprocess for each
/// Computer Use session via the bundled openai-bundled computer-use plugin.
/// Codex does not reliably clean these up when conversations end — they
/// accumulate and consume significant memory (~20MB RSS each), eventually
/// causing swap pressure and UI freezes.
///
/// This function kills all `SkyComputerUseClient` processes it can find.
/// Codex re-spawns them lazily on the next Computer Use session, so killing
/// them is safe and does not affect active conversations.
///
/// We intentionally leave `node_repl` processes alone — they are lightweight
/// (~1MB RSS) and killing them could disrupt in-flight code execution.
#[cfg(target_os = "macos")]
pub fn kill_orphaned_computer_use_processes() {
    let _ = std::process::Command::new("pkill")
        .arg("-f")
        .arg("SkyComputerUseClient")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
