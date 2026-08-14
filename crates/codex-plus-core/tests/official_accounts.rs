use base64::Engine;
use codex_plus_core::official_accounts::{
    capture_pending_official_account, create_pending_official_account, delete_official_account,
    list_official_accounts, mark_official_accounts_unused_after_provider_switch,
    rename_official_account, save_current_official_account, switch_official_account,
};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn jwt(email: &str) -> String {
    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&json!({"email": email})).unwrap());
    format!("{header}.{payload}.signature")
}

fn official_auth(email: &str, access_token: &str, refresh_token: &str) -> Value {
    json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": jwt(email),
            "access_token": access_token,
            "refresh_token": refresh_token
        },
        "last_refresh": "2026-07-10T12:00:00Z",
        "OPENAI_API_KEY": "sk-mixed-live"
    })
}

#[test]
fn official_account_data_dir_uses_app_state_directory() {
    let data_dir = codex_plus_core::paths::default_official_accounts_data_dir();

    assert_eq!(data_dir, codex_plus_core::paths::default_app_state_dir());
}

#[test]
fn save_current_account_encrypts_tokens_and_returns_secret_free_summary() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    let auth = official_auth("alice@example.com", "access-alice", "refresh-alice");
    let auth_bytes = serde_json::to_vec_pretty(&auth).unwrap();
    fs::write(home.join("auth.json"), &auth_bytes).unwrap();

    let inventory = save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();

    assert_eq!(inventory.accounts.len(), 1);
    assert_eq!(inventory.accounts[0].label, "Alice");
    assert_eq!(
        inventory.accounts[0].account_hint.as_deref(),
        Some("alice@example.com")
    );
    assert!(inventory.accounts[0].active);
    assert_eq!(
        inventory.current_account_label.as_deref(),
        Some("alice@example.com")
    );
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);

    let vault = fs::read(data_dir.join("official-accounts.v1.json")).unwrap();
    let vault_text = String::from_utf8_lossy(&vault);
    for secret in [
        "access-alice",
        "refresh-alice",
        "sk-mixed-live",
        "id_token",
        "access_token",
        "refresh_token",
    ] {
        assert!(
            !vault_text.contains(secret),
            "vault unexpectedly contains {secret}"
        );
    }
    assert_eq!(
        fs::read(data_dir.join("official-accounts.key"))
            .unwrap()
            .len(),
        32
    );

    let serialized = serde_json::to_string(&inventory).unwrap();
    assert!(!serialized.contains("access-alice"));
    assert!(!serialized.contains("refresh-alice"));
    assert!(!serialized.contains("ciphertext"));
    assert!(!serialized.contains("nonce"));
}

#[test]
fn save_current_account_updates_matching_account_hint() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-old",
            "refresh-old",
        ))
        .unwrap(),
    )
    .unwrap();
    let first = save_current_official_account(&home, &data_dir, None).unwrap();

    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-new",
            "refresh-new",
        ))
        .unwrap(),
    )
    .unwrap();
    let second = save_current_official_account(&home, &data_dir, Some("Alice updated")).unwrap();

    assert_eq!(second.accounts.len(), 1);
    assert_eq!(second.accounts[0].id, first.accounts[0].id);
    assert_eq!(second.accounts[0].label, "Alice updated");
}

#[test]
fn pending_account_safely_preserves_current_login_and_prepares_a_fresh_sign_in() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    let auth = official_auth("alice@example.com", "access-alice", "refresh-alice");
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&auth).unwrap(),
    )
    .unwrap();
    let config_bytes = b"model_provider = \"custom\"\n";
    fs::write(home.join("config.toml"), config_bytes).unwrap();

    let result = create_pending_official_account(&home, &data_dir, Some("新工作账号")).unwrap();

    assert!(result.restart_required);
    assert!(
        result
            .backup_path
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file())
    );
    assert!(!home.join("auth.json").exists());
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), config_bytes);
    assert_eq!(result.inventory.accounts.len(), 2);
    assert_eq!(
        result
            .inventory
            .accounts
            .iter()
            .filter(|account| account.pending_login)
            .count(),
        1
    );
    assert!(result.pending.pending_login);
    assert_eq!(result.pending.label, "新工作账号");
    assert_eq!(result.pending.account_hint, None);
    assert!(!result.pending.active);
    assert_eq!(result.inventory.current_account_label, None);

    let vault_text =
        String::from_utf8(fs::read(data_dir.join("official-accounts.v1.json")).unwrap()).unwrap();
    assert!(!vault_text.contains("access-alice"));
    assert!(!vault_text.contains("refresh-alice"));
    assert!(!vault_text.contains("sk-mixed-live"));
}

#[test]
fn pending_account_captures_the_new_live_login_without_exposing_credentials() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-alice",
            "refresh-alice",
        ))
        .unwrap(),
    )
    .unwrap();
    let pending = create_pending_official_account(&home, &data_dir, Some("团队账号")).unwrap();
    let pending_id = pending.pending.id.clone();
    let mut new_auth = official_auth("bob@example.com", "access-bob", "refresh-bob");
    new_auth.as_object_mut().unwrap().remove("OPENAI_API_KEY");
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&new_auth).unwrap(),
    )
    .unwrap();

    let inventory = capture_pending_official_account(&home, &data_dir, &pending_id).unwrap();

    let captured = inventory
        .accounts
        .iter()
        .find(|account| account.id == pending_id)
        .unwrap();
    assert!(!captured.pending_login);
    assert_eq!(captured.label, "团队账号");
    assert_eq!(captured.account_hint.as_deref(), Some("bob@example.com"));
    assert!(captured.active);
    assert_eq!(
        inventory.current_account_label.as_deref(),
        Some("bob@example.com")
    );
    let live: Value = serde_json::from_slice(&fs::read(home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(live["OPENAI_API_KEY"], "sk-mixed-live");
    let serialized = serde_json::to_string(&inventory).unwrap();
    assert!(!serialized.contains("access-bob"));
    assert!(!serialized.contains("refresh-bob"));
    let vault_text =
        String::from_utf8(fs::read(data_dir.join("official-accounts.v1.json")).unwrap()).unwrap();
    assert!(!vault_text.contains("access-bob"));
    assert!(!vault_text.contains("refresh-bob"));
}

#[test]
fn pending_account_cannot_be_switched_or_duplicated_before_login_is_captured() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-alice",
            "refresh-alice",
        ))
        .unwrap(),
    )
    .unwrap();
    let created = create_pending_official_account(&home, &data_dir, None).unwrap();
    let pending_id = created.pending.id;

    assert!(switch_official_account(&home, &data_dir, &pending_id).is_err());
    assert!(create_pending_official_account(&home, &data_dir, None).is_err());
    assert!(capture_pending_official_account(&home, &data_dir, &pending_id).is_err());
    let inventory = list_official_accounts(&home, &data_dir).unwrap();
    assert_eq!(
        inventory
            .accounts
            .iter()
            .filter(|account| account.pending_login)
            .count(),
        1
    );
}

#[test]
fn existing_v1_vault_entries_without_pending_field_remain_usable() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-alice",
            "refresh-alice",
        ))
        .unwrap(),
    )
    .unwrap();
    save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();
    let vault_path = data_dir.join("official-accounts.v1.json");
    let mut vault: Value = serde_json::from_slice(&fs::read(&vault_path).unwrap()).unwrap();
    vault["accounts"][0]
        .as_object_mut()
        .unwrap()
        .remove("pending_login");
    fs::write(&vault_path, serde_json::to_vec_pretty(&vault).unwrap()).unwrap();

    let inventory = list_official_accounts(&home, &data_dir).unwrap();

    assert_eq!(inventory.accounts.len(), 1);
    assert!(!inventory.accounts[0].pending_login);
    assert!(inventory.accounts[0].active);
}

#[test]
fn save_current_account_rejects_non_chatgpt_or_missing_tokens_without_mutating_vault() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();

    for auth in [
        json!({"auth_mode": "apikey", "tokens": {"access_token": "secret"}}),
        json!({"auth_mode": "chatgpt", "tokens": {}}),
        json!({"auth_mode": "chatgpt"}),
    ] {
        fs::write(
            home.join("auth.json"),
            serde_json::to_vec_pretty(&auth).unwrap(),
        )
        .unwrap();
        assert!(save_current_official_account(&home, &data_dir, None).is_err());
        assert!(!data_dir.join("official-accounts.v1.json").exists());
    }

    fs::write(home.join("auth.json"), b"{not-json").unwrap();
    assert!(save_current_official_account(&home, &data_dir, None).is_err());
    assert!(!data_dir.join("official-accounts.v1.json").exists());
}

#[test]
fn rename_and_delete_change_only_vault_metadata() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    let auth_bytes =
        serde_json::to_vec_pretty(&official_auth("alice@example.com", "access", "refresh"))
            .unwrap();
    fs::write(home.join("auth.json"), &auth_bytes).unwrap();
    let saved = save_current_official_account(&home, &data_dir, None).unwrap();
    let account_id = saved.accounts[0].id.clone();

    let renamed = rename_official_account(&home, &data_dir, &account_id, "Work").unwrap();

    assert_eq!(renamed.accounts[0].label, "Work");
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);

    let deleted = delete_official_account(&home, &data_dir, &account_id).unwrap();

    assert!(deleted.accounts.is_empty());
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);
    assert!(
        list_official_accounts(&home, &data_dir)
            .unwrap()
            .accounts
            .is_empty()
    );
}

#[test]
fn switch_account_restores_tokens_preserves_mixed_api_key_and_leaves_config_unchanged() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();

    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-alice",
            "refresh-alice",
        ))
        .unwrap(),
    )
    .unwrap();
    save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();

    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "bob@example.com",
            "access-bob",
            "refresh-bob",
        ))
        .unwrap(),
    )
    .unwrap();
    let inventory = save_current_official_account(&home, &data_dir, Some("Bob")).unwrap();
    let bob_id = inventory
        .accounts
        .iter()
        .find(|account| account.account_hint.as_deref() == Some("bob@example.com"))
        .unwrap()
        .id
        .clone();

    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-alice-live",
            "refresh-alice-live",
        ))
        .unwrap(),
    )
    .unwrap();
    let config_bytes =
        b"model_provider = \"custom\"\nexperimental_bearer_token = \"sk-provider\"\n";
    fs::write(home.join("config.toml"), config_bytes).unwrap();

    let result = switch_official_account(&home, &data_dir, &bob_id).unwrap();

    let live: Value = serde_json::from_slice(&fs::read(home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(live["tokens"]["access_token"], "access-bob");
    assert_eq!(live["tokens"]["refresh_token"], "refresh-bob");
    assert_eq!(live["OPENAI_API_KEY"], "sk-mixed-live");
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), config_bytes);
    assert!(result.restart_required);
    assert_eq!(result.selected.id, bob_id);
    assert!(result.selected.active);
    assert!(
        result
            .backup_path
            .as_deref()
            .is_some_and(|path| Path::new(path).exists())
    );
}

#[test]
fn provider_switch_keeps_a_live_official_account_visible_without_touching_auth_or_mixed_api_key() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    let auth_bytes = serde_json::to_vec_pretty(&official_auth(
        "alice@example.com",
        "access-alice",
        "refresh-alice",
    ))
    .unwrap();
    fs::write(home.join("auth.json"), &auth_bytes).unwrap();
    let config_bytes = b"model_provider = \"custom\"\n";
    fs::write(home.join("config.toml"), config_bytes).unwrap();
    let saved = save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();
    let account_id = saved.accounts[0].id.clone();
    assert!(saved.accounts[0].active);

    let inventory = mark_official_accounts_unused_after_provider_switch(&home, &data_dir).unwrap();

    assert!(inventory.accounts[0].active);
    assert_eq!(
        inventory.current_account_label.as_deref(),
        Some("alice@example.com")
    );
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), config_bytes);
    let live: Value = serde_json::from_slice(&auth_bytes).unwrap();
    assert_eq!(live["OPENAI_API_KEY"], "sk-mixed-live");
    assert!(list_official_accounts(&home, &data_dir).unwrap().accounts[0].active);

    let switched = switch_official_account(&home, &data_dir, &account_id).unwrap();
    assert!(switched.selected.active);
}

#[test]
fn provider_switch_hides_saved_accounts_when_live_auth_is_not_official() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("auth.json"),
        serde_json::to_vec_pretty(&official_auth(
            "alice@example.com",
            "access-alice",
            "refresh-alice",
        ))
        .unwrap(),
    )
    .unwrap();
    save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();
    fs::write(home.join("auth.json"), br#"{"OPENAI_API_KEY":"sk-api"}"#).unwrap();

    let inventory = mark_official_accounts_unused_after_provider_switch(&home, &data_dir).unwrap();

    assert_eq!(inventory.current_account_label, None);
    assert!(inventory.accounts.iter().all(|account| !account.active));
}

#[test]
fn switch_failures_preserve_live_auth_bytes() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    let auth_bytes = serde_json::to_vec_pretty(&official_auth(
        "alice@example.com",
        "access-alice",
        "refresh-alice",
    ))
    .unwrap();
    fs::write(home.join("auth.json"), &auth_bytes).unwrap();
    let saved = save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();
    let account_id = saved.accounts[0].id.clone();

    assert!(switch_official_account(&home, &data_dir, "missing-account").is_err());
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);

    fs::write(data_dir.join("official-accounts.key"), b"bad-key").unwrap();
    assert!(switch_official_account(&home, &data_dir, &account_id).is_err());
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);
}

#[test]
fn corrupt_ciphertext_does_not_replace_live_auth() {
    let temp = tempdir().unwrap();
    let home = temp.path().join(".codex");
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&home).unwrap();
    let auth_bytes = serde_json::to_vec_pretty(&official_auth(
        "alice@example.com",
        "access-alice",
        "refresh-alice",
    ))
    .unwrap();
    fs::write(home.join("auth.json"), &auth_bytes).unwrap();
    let saved = save_current_official_account(&home, &data_dir, Some("Alice")).unwrap();
    let account_id = saved.accounts[0].id.clone();
    let vault_path = data_dir.join("official-accounts.v1.json");
    let mut vault: Value = serde_json::from_slice(&fs::read(&vault_path).unwrap()).unwrap();
    vault["accounts"][0]["ciphertext"] = Value::String("not-valid-base64".to_string());
    fs::write(&vault_path, serde_json::to_vec_pretty(&vault).unwrap()).unwrap();

    assert!(switch_official_account(&home, &data_dir, &account_id).is_err());
    assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth_bytes);
}
