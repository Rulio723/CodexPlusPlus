use aes_gcm::aead::{Aead, OsRng, rand_core::RngCore};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const VAULT_VERSION: u32 = 1;
const VAULT_FILE: &str = "official-accounts.v1.json";
const KEY_FILE: &str = "official-accounts.key";
const GUARD_FILE: &str = "official-accounts.guard";
const MAX_LABEL_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountSummary {
    pub id: String,
    pub label: String,
    pub account_hint: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub active: bool,
    pub pending_login: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountInventory {
    pub accounts: Vec<OfficialAccountSummary>,
    pub current_account_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialAccountSwitchResult {
    pub inventory: OfficialAccountInventory,
    pub selected: OfficialAccountSummary,
    pub restart_required: bool,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingOfficialAccountResult {
    pub inventory: OfficialAccountInventory,
    pub pending: OfficialAccountSummary,
    pub restart_required: bool,
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    #[serde(default)]
    accounts: Vec<VaultEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultEntry {
    id: String,
    label: String,
    account_hint: Option<String>,
    created_at: u64,
    updated_at: u64,
    #[serde(default)]
    pending_login: bool,
    #[serde(default)]
    nonce: String,
    #[serde(default)]
    ciphertext: String,
}

struct VaultGuard {
    file: File,
}

impl Drop for VaultGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn list_official_accounts(
    home: &Path,
    data_dir: &Path,
) -> anyhow::Result<OfficialAccountInventory> {
    let _guard = acquire_vault_guard(data_dir)?;
    let vault = load_vault(data_dir)?;
    Ok(build_inventory(home, &vault))
}

pub fn mark_official_accounts_unused_after_provider_switch(
    home: &Path,
    data_dir: &Path,
) -> anyhow::Result<OfficialAccountInventory> {
    let _guard = acquire_vault_guard(data_dir)?;
    let vault = load_vault(data_dir)?;
    Ok(build_inventory(home, &vault))
}

pub fn save_current_official_account(
    home: &Path,
    data_dir: &Path,
    requested_label: Option<&str>,
) -> anyhow::Result<OfficialAccountInventory> {
    let auth_path = home.join("auth.json");
    let auth_bytes =
        fs::read(&auth_path).map_err(|error| anyhow::anyhow!("读取当前官方登录失败：{error}"))?;
    let auth: Value = serde_json::from_slice(&auth_bytes)
        .map_err(|_| anyhow::anyhow!("当前 auth.json 不是有效 JSON。"))?;
    let (sanitized_auth, account_hint) = sanitize_official_auth(auth)?;

    let _guard = acquire_vault_guard(data_dir)?;
    let key = load_or_create_key(data_dir)?;
    let mut vault = load_vault(data_dir)?;
    let now = now_secs();
    let requested_label = requested_label.map(normalize_label).transpose()?;
    let existing_index = account_hint.as_ref().and_then(|hint| {
        vault.accounts.iter().position(|entry| {
            !entry.pending_login
                && entry
                    .account_hint
                    .as_deref()
                    .is_some_and(|saved| saved.eq_ignore_ascii_case(hint))
        })
    });
    let encrypted = encrypt_auth(&key, &sanitized_auth)?;

    if let Some(index) = existing_index {
        let entry = &mut vault.accounts[index];
        if let Some(label) = requested_label {
            entry.label = label;
        }
        entry.account_hint = account_hint;
        entry.updated_at = now;
        entry.pending_login = false;
        entry.nonce = encrypted.0;
        entry.ciphertext = encrypted.1;
    } else {
        let label = requested_label
            .or_else(|| account_hint.clone())
            .unwrap_or_else(|| format!("官方账号 {}", vault.accounts.len() + 1));
        vault.accounts.push(VaultEntry {
            id: Uuid::new_v4().to_string(),
            label,
            account_hint,
            created_at: now,
            updated_at: now,
            pending_login: false,
            nonce: encrypted.0,
            ciphertext: encrypted.1,
        });
    }

    save_vault(data_dir, &vault)?;
    Ok(build_inventory(home, &vault))
}

pub fn create_pending_official_account(
    home: &Path,
    data_dir: &Path,
    requested_label: Option<&str>,
) -> anyhow::Result<PendingOfficialAccountResult> {
    let auth_path = home.join("auth.json");
    let live_auth = match fs::read(&auth_path) {
        Ok(bytes) => {
            let auth: Value = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("当前 auth.json 不是有效 JSON，未创建待登录账号。"))?;
            let preserved_api_key = auth.get("OPENAI_API_KEY").cloned();
            let (sanitized_auth, account_hint) = sanitize_official_auth(auth).map_err(|_| {
                anyhow::anyhow!("当前不是有效的 ChatGPT 官方登录，未创建待登录账号。")
            })?;
            Some((sanitized_auth, account_hint, preserved_api_key))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let requested_label = requested_label.map(normalize_label).transpose()?;

    let _guard = acquire_vault_guard(data_dir)?;
    let mut vault = load_vault(data_dir)?;
    if vault.accounts.iter().any(|entry| entry.pending_login) {
        anyhow::bail!("已有待登录账号，请先完成登录保存或删除该占位项。");
    }

    let now = now_secs();
    let mut pending_context = None;
    if let Some((sanitized_auth, account_hint, preserved_api_key)) = live_auth {
        let key = load_or_create_key(data_dir)?;
        let existing_index = account_hint.as_ref().and_then(|hint| {
            vault.accounts.iter().position(|entry| {
                !entry.pending_login
                    && entry
                        .account_hint
                        .as_deref()
                        .is_some_and(|saved| saved.eq_ignore_ascii_case(hint))
            })
        });
        let encrypted = encrypt_auth(&key, &sanitized_auth)?;
        if let Some(api_key) = preserved_api_key {
            pending_context = Some(encrypt_auth(
                &key,
                &serde_json::json!({ "OPENAI_API_KEY": api_key }),
            )?);
        }
        if let Some(index) = existing_index {
            let entry = &mut vault.accounts[index];
            entry.account_hint = account_hint;
            entry.updated_at = now;
            entry.pending_login = false;
            entry.nonce = encrypted.0;
            entry.ciphertext = encrypted.1;
        } else {
            let label = account_hint
                .clone()
                .unwrap_or_else(|| format!("官方账号 {}", vault.accounts.len() + 1));
            vault.accounts.push(VaultEntry {
                id: Uuid::new_v4().to_string(),
                label,
                account_hint,
                created_at: now,
                updated_at: now,
                pending_login: false,
                nonce: encrypted.0,
                ciphertext: encrypted.1,
            });
        }
    }

    let pending_id = Uuid::new_v4().to_string();
    let pending_label =
        requested_label.unwrap_or_else(|| format!("待登录账号 {}", vault.accounts.len() + 1));
    let (pending_nonce, pending_ciphertext) = pending_context.unwrap_or_default();
    vault.accounts.push(VaultEntry {
        id: pending_id.clone(),
        label: pending_label,
        account_hint: None,
        created_at: now,
        updated_at: now,
        pending_login: true,
        nonce: pending_nonce,
        ciphertext: pending_ciphertext,
    });

    let backup_path = detach_live_auth_for_new_account(&auth_path)?;
    if let Err(error) = save_vault(data_dir, &vault) {
        if let Some(backup) = backup_path.as_deref() {
            let _ = fs::rename(backup, &auth_path);
        }
        return Err(error);
    }

    let inventory = build_inventory(home, &vault);
    let pending = inventory
        .accounts
        .iter()
        .find(|account| account.id == pending_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("待登录账号已创建，但无法读取账号摘要。"))?;
    Ok(PendingOfficialAccountResult {
        inventory,
        pending,
        restart_required: true,
        backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
    })
}

pub fn capture_pending_official_account(
    home: &Path,
    data_dir: &Path,
    account_id: &str,
) -> anyhow::Result<OfficialAccountInventory> {
    let auth_bytes = fs::read(home.join("auth.json"))
        .map_err(|error| anyhow::anyhow!("尚未检测到新的官方登录：{error}"))?;
    let mut auth: Value = serde_json::from_slice(&auth_bytes)
        .map_err(|_| anyhow::anyhow!("当前 auth.json 不是有效 JSON。"))?;
    let (sanitized_auth, account_hint) = sanitize_official_auth(auth.clone())?;

    let _guard = acquire_vault_guard(data_dir)?;
    let key = load_or_create_key(data_dir)?;
    let mut vault = load_vault(data_dir)?;
    let pending_index = vault
        .accounts
        .iter()
        .position(|entry| entry.id == account_id && entry.pending_login)
        .ok_or_else(|| anyhow::anyhow!("未找到指定的待登录账号。"))?;
    let preserved_api_key = if vault.accounts[pending_index].nonce.is_empty()
        && vault.accounts[pending_index].ciphertext.is_empty()
    {
        None
    } else {
        decrypt_auth(&key, &vault.accounts[pending_index])?
            .get("OPENAI_API_KEY")
            .cloned()
    };
    let duplicate_index = account_hint.as_ref().and_then(|hint| {
        vault.accounts.iter().position(|entry| {
            !entry.pending_login
                && entry.id != account_id
                && entry
                    .account_hint
                    .as_deref()
                    .is_some_and(|saved| saved.eq_ignore_ascii_case(hint))
        })
    });
    let encrypted = encrypt_auth(&key, &sanitized_auth)?;
    let now = now_secs();

    if let Some(index) = duplicate_index {
        let entry = &mut vault.accounts[index];
        entry.account_hint = account_hint;
        entry.updated_at = now;
        entry.pending_login = false;
        entry.nonce = encrypted.0;
        entry.ciphertext = encrypted.1;
        vault.accounts.remove(pending_index);
    } else {
        let entry = &mut vault.accounts[pending_index];
        if entry.label.starts_with("待登录账号 ") {
            if let Some(hint) = account_hint.as_ref() {
                entry.label = hint.clone();
            }
        }
        entry.account_hint = account_hint;
        entry.updated_at = now;
        entry.pending_login = false;
        entry.nonce = encrypted.0;
        entry.ciphertext = encrypted.1;
    }

    if let Some(api_key) = preserved_api_key {
        let object = auth
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("当前 auth.json 根节点必须是对象。"))?;
        object.insert("OPENAI_API_KEY".to_string(), api_key);
        atomic_replace(&home.join("auth.json"), &serde_json::to_vec_pretty(&auth)?)?;
    }
    save_vault(data_dir, &vault)?;
    Ok(build_inventory(home, &vault))
}

pub fn rename_official_account(
    home: &Path,
    data_dir: &Path,
    account_id: &str,
    label: &str,
) -> anyhow::Result<OfficialAccountInventory> {
    let label = normalize_label(label)?;
    let _guard = acquire_vault_guard(data_dir)?;
    let mut vault = load_vault(data_dir)?;
    let entry = vault
        .accounts
        .iter_mut()
        .find(|entry| entry.id == account_id)
        .ok_or_else(|| anyhow::anyhow!("未找到要重命名的官方账号。"))?;
    entry.label = label;
    entry.updated_at = now_secs();
    save_vault(data_dir, &vault)?;
    Ok(build_inventory(home, &vault))
}

pub fn delete_official_account(
    home: &Path,
    data_dir: &Path,
    account_id: &str,
) -> anyhow::Result<OfficialAccountInventory> {
    let _guard = acquire_vault_guard(data_dir)?;
    let mut vault = load_vault(data_dir)?;
    let previous_len = vault.accounts.len();
    vault.accounts.retain(|entry| entry.id != account_id);
    if vault.accounts.len() == previous_len {
        anyhow::bail!("未找到要删除的官方账号。");
    }
    save_vault(data_dir, &vault)?;
    Ok(build_inventory(home, &vault))
}

pub fn switch_official_account(
    home: &Path,
    data_dir: &Path,
    account_id: &str,
) -> anyhow::Result<OfficialAccountSwitchResult> {
    let _guard = acquire_vault_guard(data_dir)?;
    let vault = load_vault(data_dir)?;
    let entry = vault
        .accounts
        .iter()
        .find(|entry| entry.id == account_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("未找到要切换的官方账号。"))?;
    if entry.pending_login {
        anyhow::bail!("该账号仍在等待登录，请先登录 Codex 并保存登录凭据。");
    }
    let key = load_existing_key(data_dir)?;
    let mut target_auth = decrypt_auth(&key, &entry)?;
    chatgpt_account_label_from_auth(&target_auth)
        .ok_or_else(|| anyhow::anyhow!("保存的官方账号凭据已失效或损坏。"))?;

    let auth_path = home.join("auth.json");
    let live_bytes = match fs::read(&auth_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(bytes) = live_bytes.as_deref() {
        let live: Value = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("当前 auth.json 已损坏，未执行账号切换。"))?;
        if let Some(api_key) = live.get("OPENAI_API_KEY").cloned() {
            let object = target_auth
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("保存的官方账号根节点无效。"))?;
            object.insert("OPENAI_API_KEY".to_string(), api_key);
        }
    }

    chatgpt_account_label_from_auth(&target_auth)
        .ok_or_else(|| anyhow::anyhow!("目标官方账号凭据校验失败。"))?;
    let target_bytes = serde_json::to_vec_pretty(&target_auth)?;
    let backup_path = replace_live_auth_with_backup(&auth_path, &target_bytes)?;
    let inventory = build_inventory(home, &vault);
    let selected = inventory
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("账号已切换，但无法读取账号摘要。"))?;
    Ok(OfficialAccountSwitchResult {
        inventory,
        selected,
        restart_required: true,
        backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
    })
}

pub(crate) fn chatgpt_account_label_from_auth(value: &Value) -> Option<Option<String>> {
    let is_chatgpt = value
        .get("auth_mode")
        .and_then(Value::as_str)
        .map(|mode| mode.eq_ignore_ascii_case("chatgpt"))
        .unwrap_or(false);
    let tokens = value.get("tokens")?;
    if !is_chatgpt || !tokens_have_login_secret(tokens) {
        return None;
    }
    Some(account_label_from_tokens(tokens))
}

fn sanitize_official_auth(mut value: Value) -> anyhow::Result<(Value, Option<String>)> {
    let account_hint = chatgpt_account_label_from_auth(&value)
        .ok_or_else(|| anyhow::anyhow!("当前 auth.json 不包含有效的 ChatGPT 官方登录凭据。"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("当前 auth.json 根节点必须是对象。"))?;
    object.remove("OPENAI_API_KEY");
    Ok((value, account_hint))
}

fn tokens_have_login_secret(tokens: &Value) -> bool {
    ["access_token", "id_token", "refresh_token"]
        .iter()
        .any(|key| {
            tokens
                .get(*key)
                .and_then(Value::as_str)
                .map(|token| !token.trim().is_empty())
                .unwrap_or(false)
        })
}

fn account_label_from_tokens(tokens: &Value) -> Option<String> {
    ["id_token", "access_token"].iter().find_map(|key| {
        tokens
            .get(*key)
            .and_then(Value::as_str)
            .and_then(account_label_from_jwt)
    })
}

fn account_label_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE
                .decode(payload.as_bytes())
                .ok()
        })?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("https://api.openai.com/profile")
                .and_then(|profile| profile.get("email"))
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToString::to_string)
}

fn build_inventory(home: &Path, vault: &VaultFile) -> OfficialAccountInventory {
    // 官方账号面板描述的是实时 auth.json 登录，而不是当前供应商选择。
    // 供应商切换可能在写入有效官方 auth.json 后创建 marker；marker 不能覆盖真实登录状态。
    let current_account_label = read_live_account_label(home);
    let accounts = vault
        .accounts
        .iter()
        .map(|entry| OfficialAccountSummary {
            id: entry.id.clone(),
            label: entry.label.clone(),
            account_hint: entry.account_hint.clone(),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            active: !entry.pending_login
                && current_account_label.as_ref().is_some_and(|current| {
                    entry
                        .account_hint
                        .as_ref()
                        .is_some_and(|hint| hint.eq_ignore_ascii_case(current))
                }),
            pending_login: entry.pending_login,
        })
        .collect();
    OfficialAccountInventory {
        accounts,
        current_account_label,
    }
}

fn read_live_account_label(home: &Path) -> Option<String> {
    let value = fs::read(home.join("auth.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())?;
    chatgpt_account_label_from_auth(&value).flatten()
}

fn acquire_vault_guard(data_dir: &Path) -> anyhow::Result<VaultGuard> {
    fs::create_dir_all(data_dir)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(data_dir.join(GUARD_FILE))?;
    file.try_lock_exclusive()
        .map_err(|error| anyhow::anyhow!("官方账号保险库正在被其他操作使用：{error}"))?;
    Ok(VaultGuard { file })
}

fn load_or_create_key(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join(KEY_FILE);
    match fs::read(&path) {
        Ok(bytes) => return key_from_bytes(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let mut key = [0_u8; 32];
    OsRng.fill_bytes(&mut key);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(&key)?;
            file.sync_all()?;
            Ok(key)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => key_from_bytes(fs::read(path)?),
        Err(error) => Err(error.into()),
    }
}

fn load_existing_key(data_dir: &Path) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join(KEY_FILE);
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            anyhow::anyhow!("官方账号保险库密钥不存在。")
        } else {
            error.into()
        }
    })?;
    key_from_bytes(bytes)
}

fn key_from_bytes(bytes: Vec<u8>) -> anyhow::Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("官方账号保险库密钥长度无效。"))
}

fn load_vault(data_dir: &Path) -> anyhow::Result<VaultFile> {
    let path = data_dir.join(VAULT_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(VaultFile {
                version: VAULT_VERSION,
                accounts: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    let vault: VaultFile = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("官方账号保险库文件已损坏。"))?;
    if vault.version != VAULT_VERSION {
        anyhow::bail!("不支持的官方账号保险库版本。");
    }
    Ok(vault)
}

fn save_vault(data_dir: &Path, vault: &VaultFile) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(vault)?;
    atomic_replace(&data_dir.join(VAULT_FILE), &bytes)
}

fn encrypt_auth(key: &[u8; 32], auth: &Value) -> anyhow::Result<(String, String)> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("初始化官方账号加密失败。"))?;
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let plaintext = serde_json::to_vec(auth)?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .map_err(|_| anyhow::anyhow!("加密官方账号失败。"))?;
    Ok((
        base64::engine::general_purpose::STANDARD.encode(nonce_bytes),
        base64::engine::general_purpose::STANDARD.encode(ciphertext),
    ))
}

fn decrypt_auth(key: &[u8; 32], entry: &VaultEntry) -> anyhow::Result<Value> {
    let nonce_bytes = base64::engine::general_purpose::STANDARD
        .decode(entry.nonce.as_bytes())
        .map_err(|_| anyhow::anyhow!("官方账号保险库 nonce 已损坏。"))?;
    if nonce_bytes.len() != 12 {
        anyhow::bail!("官方账号保险库 nonce 长度无效。");
    }
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(entry.ciphertext.as_bytes())
        .map_err(|_| anyhow::anyhow!("官方账号保险库密文已损坏。"))?;
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("初始化官方账号解密失败。"))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("官方账号保险库解密失败。"))?;
    serde_json::from_slice(&plaintext).map_err(|_| anyhow::anyhow!("解密后的官方账号数据无效。"))
}

fn replace_live_auth_with_backup(path: &Path, bytes: &[u8]) -> anyhow::Result<Option<PathBuf>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".auth.json.{}.account-switch.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    if !path.exists() {
        fs::rename(&temp, path)?;
        return Ok(None);
    }

    let backup = parent.join(format!(
        "auth.json.codex-plus-account-switch.{}.{}.bak",
        now_secs(),
        Uuid::new_v4()
    ));
    fs::rename(path, &backup)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(Some(backup)),
        Err(error) => {
            let _ = fs::rename(&backup, path);
            let _ = fs::remove_file(temp);
            Err(error.into())
        }
    }
}

fn detach_live_auth_for_new_account(path: &Path) -> anyhow::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup = parent.join(format!(
        "auth.json.codex-plus-new-account.{}.{}.bak",
        now_secs(),
        Uuid::new_v4()
    ));
    fs::rename(path, &backup)?;
    Ok(Some(backup))
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("official-accounts");
    let temp = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    if !path.exists() {
        fs::rename(&temp, path)?;
        return Ok(());
    }

    let backup = parent.join(format!(".{file_name}.{}.replace-backup", Uuid::new_v4()));
    fs::rename(path, &backup)?;
    match fs::rename(&temp, path) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&backup, path);
            let _ = fs::remove_file(temp);
            Err(error.into())
        }
    }
}

fn normalize_label(label: &str) -> anyhow::Result<String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        anyhow::bail!("官方账号名称不能为空。");
    }
    if trimmed.chars().count() > MAX_LABEL_CHARS {
        anyhow::bail!("官方账号名称不能超过 {MAX_LABEL_CHARS} 个字符。");
    }
    Ok(trimmed.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
