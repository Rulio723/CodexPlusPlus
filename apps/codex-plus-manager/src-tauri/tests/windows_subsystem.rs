#[cfg(windows)]
#[test]
fn manager_binary_uses_windows_gui_subsystem_in_debug_and_release() {
    let main_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read manager main.rs");

    assert!(
        main_rs.contains("#![cfg_attr(windows, windows_subsystem = \"windows\")]"),
        "manager binary should not allocate a console window on Windows"
    );
}

#[test]
fn manager_release_binary_uses_embedded_frontend_assets() {
    let cargo_toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read manager Cargo.toml");

    assert!(
        cargo_toml.contains("custom-protocol"),
        "release manager binary should use Tauri custom protocol instead of devUrl localhost"
    );
}

#[test]
fn manager_uses_single_instance_guard_before_starting_tauri() {
    let lib_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read manager lib.rs");

    assert!(lib_rs.contains("acquire_single_instance_guard()"));
    assert!(lib_rs.contains("manager_guard_port"));
    assert!(lib_rs.contains("manager.already_running"));
}

#[test]
fn manager_repeated_launch_activates_existing_window() {
    let lib_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read manager lib.rs");

    assert!(lib_rs.contains("focus_existing_manager_window();"));
    assert!(lib_rs.contains("windows_activate_process_window"));
}

#[test]
fn manager_main_window_uses_default_window_icon_explicitly() {
    let lib_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read manager lib.rs");

    assert!(lib_rs.contains("main_window_builder"));
    assert!(lib_rs.contains("app.default_window_icon().cloned()"));
    assert!(lib_rs.contains("main_window_builder = main_window_builder.icon(icon)?"));
}

#[test]
fn manager_close_minimizes_to_tray_without_confirmation() {
    let lib_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("read manager lib.rs");
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");

    assert!(!lib_rs.contains("MessageDialogButtons"));
    assert!(!lib_rs.contains(".dialog()"));
    assert!(!lib_rs.contains("manager://close-requested"));
    assert!(lib_rs.contains("let _ = close_event_window.hide();"));
    assert!(lib_rs.contains("startup_is_transient()"));
    assert!(lib_rs.contains("arg == \"--transient\""));
    assert!(!app_tsx.contains("CloseConfirmDialog"));
    assert!(app_tsx.contains("manager_exit_app"));
    assert!(app_tsx.contains("manager_hide_to_tray"));
}

#[test]
fn manager_queues_codexplusplus_provider_urls_for_confirmation_on_startup() {
    let main_rs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read manager main.rs");

    assert!(main_rs.contains("codexplusplus://"));
    assert!(main_rs.contains("provider_import::save_pending_provider_import_from_url"));
    assert!(!main_rs.contains("provider_import::import_provider_from_url"));
    assert!(main_rs.contains("manager.provider_import_url.pending"));
}

#[test]
fn launcher_binary_embeds_codex_icon_resource() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let launcher_build = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("codex-plus-launcher/build.rs");
    let build_rs = std::fs::read_to_string(&launcher_build).expect("read launcher build.rs");

    assert!(build_rs.contains("WindowsResource"));
    assert!(build_rs.contains("icons/icon.ico"));
}

#[test]
fn all_entrypoints_use_the_custom_codexplusplus_icon() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap();
    let icon_dir = manifest_dir.join("icons");
    let asset_dir = root.join("assets/images");

    assert_eq!(
        std::fs::read(icon_dir.join("icon.ico")).expect("read manager ico"),
        std::fs::read(asset_dir.join("codex-plus-plus.ico")).expect("read custom ico")
    );
    assert_eq!(
        std::fs::read(icon_dir.join("icon.png")).expect("read manager png"),
        std::fs::read(asset_dir.join("codex-plus-plus.png")).expect("read custom png")
    );

    let shim_build = std::fs::read_to_string(root.join("apps/codex-plus-admin-shim/build.rs"))
        .expect("read administrator shim build.rs");
    assert!(shim_build.contains("codex-plus-manager/src-tauri/icons/icon.ico"));
}

#[test]
fn launcher_recovers_administrator_state_before_single_instance_activation() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let launcher_main = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("codex-plus-launcher/src/main.rs");
    let source = std::fs::read_to_string(launcher_main).expect("read launcher main.rs");
    let recovery = source
        .find("prepare_administrator_mode_startup(&options).await?")
        .unwrap();
    let guard = source
        .find("acquire_single_instance_guard(options.debug_port)?")
        .unwrap();
    assert!(recovery < guard);
    let preparation = source
        .split("async fn prepare_administrator_mode_startup")
        .nth(1)
        .unwrap()
        .split("fn acquire_single_instance_guard")
        .next()
        .unwrap();
    assert!(preparation.contains("if !settings.administrator_mode_enabled"));
    assert!(preparation.contains("recover_stale_admin_mode"));
    assert!(preparation.contains("codex-plus-admin-shim.exe"));
    assert!(preparation.contains("administrator shim is missing"));
}

#[test]
fn administrator_second_invocation_only_activates_existing_session() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let launcher_main = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("codex-plus-launcher/src/main.rs");
    let source = std::fs::read_to_string(launcher_main).expect("read launcher main.rs");
    assert!(source.contains("if settings.administrator_mode_enabled"));
    assert!(source.contains("return activate_existing_administrator_session(options, &app_dir)"));
    let admin_activation = source
        .split("fn activate_existing_administrator_session")
        .nth(1)
        .unwrap()
        .split("fn log_launcher_already_running")
        .next()
        .unwrap();
    for forbidden in [
        ".launch_codex(",
        "start_helper(",
        "ensure_injection(",
        "start_bridge_watchdog(",
        "start_administrator_mode(",
    ] {
        assert!(
            !admin_activation.contains(forbidden),
            "forbidden call: {forbidden}"
        );
    }
    assert!(admin_activation.contains("find_codex_processes"));
    assert!(admin_activation.contains("windows_activate_process_window"));
    assert!(admin_activation.contains("activate_existing_administrator_session_with"));
    assert!(admin_activation.contains("101"));
    assert!(admin_activation.contains("Duration::from_millis(100)"));
    assert!(admin_activation.contains(".await?"));
    assert!(admin_activation.contains("\"activated\": true"));

    let core_launcher = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("../crates/codex-plus-core/src/launcher.rs");
    let core_source = std::fs::read_to_string(core_launcher).expect("read core launcher.rs");
    let polling = core_source
        .split("pub async fn activate_existing_administrator_session_with")
        .nth(1)
        .unwrap()
        .split("pub async fn launch_and_inject_with_hooks")
        .next()
        .unwrap();
    assert!(polling.contains("for attempt in 0..max_attempts"));
    assert!(polling.contains("if activate_window(process_id)"));
    assert!(polling.contains("wait().await"));
    assert!(polling.contains("before deadline"));
}

#[test]
fn windows_binaries_request_administrator_privileges() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manager_build =
        std::fs::read_to_string(manifest_dir.join("build.rs")).expect("read manager build.rs");
    let windows_manifest = std::fs::read_to_string(manifest_dir.join("windows-app-manifest.xml"))
        .expect("read windows app manifest");
    let launcher_build = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("codex-plus-launcher/build.rs");
    let launcher_build = std::fs::read_to_string(&launcher_build).expect("read launcher build.rs");
    let windows_installer = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("scripts/installer/windows/CodexPlusPlus.nsi");
    let windows_installer =
        std::fs::read_to_string(&windows_installer).expect("read windows installer");

    assert!(manager_build.contains("windows-app-manifest.xml"));
    assert!(launcher_build.contains("windows-app-manifest.xml"));
    assert!(windows_manifest.contains("requireAdministrator"));
    assert!(windows_manifest.contains("Microsoft.Windows.Common-Controls"));
    assert!(windows_installer.contains("RequestExecutionLevel admin"));
}

#[test]
fn administrator_shim_manifest_stays_non_elevated() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let shim_manifest = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("codex-plus-admin-shim/windows-app-manifest.xml");
    let shim_manifest =
        std::fs::read_to_string(shim_manifest).expect("read administrator shim manifest");

    assert!(
        shim_manifest.contains(r#"<requestedExecutionLevel level="asInvoker" uiAccess="false" />"#)
    );
    assert!(!shim_manifest.contains("requireAdministrator"));
}

#[test]
fn administrator_runtime_is_staged_with_fixed_executable_roles() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap();
    let workspace = std::fs::read_to_string(root.join("Cargo.toml")).expect("read workspace");
    let release = std::fs::read_to_string(root.join(".github/workflows/release-assets.yml"))
        .expect("read release workflow");
    let pr = std::fs::read_to_string(root.join(".github/workflows/pr-build.yml"))
        .expect("read PR workflow");
    let installer =
        std::fs::read_to_string(root.join("scripts/installer/windows/CodexPlusPlus.nsi"))
            .expect("read NSIS installer");
    let launcher_build = std::fs::read_to_string(root.join("apps/codex-plus-launcher/build.rs"))
        .expect("read launcher build script");
    let launcher_manifest =
        std::fs::read_to_string(root.join("apps/codex-plus-launcher/windows-app-manifest.xml"))
            .expect("read launcher manifest");
    let shim_build = std::fs::read_to_string(root.join("apps/codex-plus-admin-shim/build.rs"))
        .expect("read shim build script");
    let manager_manifest = std::fs::read_to_string(
        root.join("apps/codex-plus-manager/src-tauri/windows-app-manifest.xml"),
    )
    .expect("read manager manifest");
    let shim_manifest =
        std::fs::read_to_string(root.join("apps/codex-plus-admin-shim/windows-app-manifest.xml"))
            .expect("read shim manifest");
    let launcher = std::fs::read_to_string(root.join("apps/codex-plus-launcher/src/main.rs"))
        .expect("read launcher source");

    assert!(workspace.contains("apps/codex-plus-admin-shim"));
    for workflow in [&release, &pr] {
        assert!(workflow.contains("cargo build --release"));
        assert!(
            workflow
                .contains("Copy-Item target/release/codex-plus-admin-shim.exe dist/windows/app/")
        );
        assert!(workflow.contains("Remove-Item dist/windows/app -Recurse -Force"));
        for binary in [
            "codex-plus-plus.exe",
            "codex-plus-plus-manager.exe",
            "codex-plus-admin-shim.exe",
        ] {
            assert_eq!(
                workflow
                    .lines()
                    .filter(
                        |line| line.trim_start().starts_with("Copy-Item target/release/")
                            && line.contains(binary)
                    )
                    .count(),
                1,
                "Windows staging must copy {binary} exactly once"
            );
        }
    }

    assert!(launcher_build.contains("windows-app-manifest.xml"));
    assert!(!launcher_build.contains("codex-plus-manager/src-tauri/windows-app-manifest.xml"));
    assert!(!launcher_build.contains("codex-plus-admin-shim"));
    assert!(shim_build.contains("windows-app-manifest.xml"));
    assert!(!shim_build.contains("codex-plus-launcher"));
    assert!(!shim_build.contains("codex-plus-manager/src-tauri/windows-app-manifest.xml"));
    assert!(launcher_manifest.contains("requireAdministrator"));
    assert!(manager_manifest.contains("requireAdministrator"));
    assert!(shim_manifest.contains("asInvoker"));
    assert!(!shim_manifest.contains("requireAdministrator"));
    assert!(launcher.contains("current_exe"));
    assert!(launcher.contains(".join(\"codex-plus-admin-shim.exe\")"));

    assert!(installer.contains("File \"${ROOT}\\dist\\windows\\app\\codex-plus-admin-shim.exe\""));
    assert!(installer.contains("Delete \"$INSTDIR\\codex-plus-admin-shim.exe\""));
    assert_eq!(
        installer
            .matches("taskkill.exe\" /IM codex-plus-admin-shim.exe /T /F")
            .count(),
        4
    );
    let installer_executables = installer
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("File ") && line.ends_with(".exe\""))
        .collect::<Vec<_>>();
    assert_eq!(
        installer_executables,
        [
            "File /oname=codex-plus-recovery.exe \"${ROOT}\\dist\\windows\\app\\codex-plus-plus.exe\"",
            "File \"${ROOT}\\dist\\windows\\app\\codex-plus-plus.exe\"",
            "File \"${ROOT}\\dist\\windows\\app\\codex-plus-plus-manager.exe\"",
            "File \"${ROOT}\\dist\\windows\\app\\codex-plus-admin-shim.exe\"",
            "File /oname=pwsh.exe \"${ROOT}\\dist\\windows\\app\\admin-terminal\\pwsh.exe\"",
            "File /oname=codex-plus-recovery.exe \"${ROOT}\\dist\\windows\\app\\codex-plus-plus.exe\"",
        ]
    );
    assert!(!installer.contains("auth.json"));
    assert!(!installer.contains("environments.toml"));
}

#[test]
fn windows_entrypoints_register_codexplusplus_url_protocol() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let windows_install = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("crates/codex-plus-core/src/install/windows.rs");
    let windows_install =
        std::fs::read_to_string(&windows_install).expect("read windows install source");

    assert!(windows_install.contains("Software\\Classes\\codexplusplus"));
    assert!(windows_install.contains("URL Protocol"));
    assert!(windows_install.contains("%1"));
}

#[test]
fn manager_launch_button_spawns_silent_launcher_binary() {
    let commands_rs =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/commands.rs"))
            .expect("read manager commands.rs");

    assert!(commands_rs.contains("SILENT_BINARY"));
    assert!(commands_rs.contains("std::process::Command::new"));
    assert!(!commands_rs.contains("launch_and_inject_with_hooks(options"));
}

#[test]
fn macos_packager_hides_silent_launcher_but_not_manager() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let packager = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("scripts/installer/macos/package-dmg.sh");
    let script = std::fs::read_to_string(&packager).expect("read macOS packager");

    assert!(script.contains("<key>LSUIElement</key>"));
    assert!(script.contains("ARCH=\"${2:-$(uname -m)}\""));
    assert!(script.contains("BINARY_DIR=\"${BINARY_DIR:-$ROOT/target/release}\""));
    assert!(script.contains("CodexPlusPlus-${VERSION}-macos-${ARCH}.dmg"));
    assert!(script.contains(
        "create_app \"Codex++\" \"CodexPlusPlus\" \"$BINARY_DIR/codex-plus-plus\" \"com.bigpizzav3.codexplusplus\" \"true\""
    ));
    assert!(script.contains(
        "create_app \"Codex++ 管理工具\" \"CodexPlusPlusManager\" \"$BINARY_DIR/codex-plus-plus-manager\" \"com.bigpizzav3.codexplusplus.manager\" \"false\""
    ));
}

#[test]
fn github_release_workflow_builds_separate_macos_x64_and_arm64_dmgs() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap()
        .join(".github/workflows/release-assets.yml");
    let workflow = std::fs::read_to_string(&workflow).expect("read release assets workflow");

    assert!(workflow.contains("macos-15-intel"));
    assert!(workflow.contains("x86_64-apple-darwin"));
    assert!(workflow.contains("macos-14"));
    assert!(workflow.contains("aarch64-apple-darwin"));
    assert!(workflow.contains("package-dmg.sh \"$VERSION\" \"${{ matrix.arch }}\""));
    assert!(workflow.contains("target/${{ matrix.target }}/release"));
}

#[test]
fn github_release_workflow_uploads_static_latest_json() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .unwrap()
        .join(".github/workflows/release-assets.yml");
    let workflow = std::fs::read_to_string(&workflow).expect("read release assets workflow");

    assert!(workflow.contains("latest-json:"));
    assert!(workflow.contains("latest.json"));
    assert!(workflow.contains("gh release upload \"$TAG\" latest.json --clobber"));
}

#[test]
fn relay_settings_keeps_profile_config_and_auth_files_isolated() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");
    let commands_rs = manifest_dir.join("src/commands.rs");
    let commands_rs = std::fs::read_to_string(&commands_rs).expect("read manager commands.rs");

    assert!(app_tsx.contains("snapshotActiveRelayFilesBeforeSwitch"));
    assert!(app_tsx.contains("backfill_relay_profile_from_live"));
    assert!(app_tsx.contains("relayProfileSwitchValidation(selectedBeforeSave, switchSettings)"));
    assert!(app_tsx.contains("缺少独立 config.toml"));
    assert!(app_tsx.contains("const command = relayProfileSwitchCommand(selectedAfterSave)"));
    assert!(app_tsx.contains("function relayProfileSwitchCommand"));
    assert!(app_tsx.contains("return \"apply_pure_api_injection\""));
    assert!(app_tsx.contains("return \"apply_relay_injection\""));
    assert!(app_tsx.contains("const createNewAggregateProfile = () =>"));
    assert!(app_tsx.contains("onClick={createNewAggregateProfile}"));
    assert!(app_tsx.contains("已打开聚合供应商详情"));
    assert!(!commands_rs.contains("缺少独立 auth.json"));
    assert!(commands_rs.contains("backfill_relay_profile_from_live"));
    assert!(commands_rs.contains("apply_relay_profile_to_home_with_switch_rules"));
}

#[test]
fn relay_context_management_is_global_not_supplier_scoped() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");
    let styles = manifest_dir.parent().unwrap().join("src/styles.css");
    let styles = std::fs::read_to_string(&styles).expect("read manager styles.css");

    assert!(app_tsx.contains("管理 MCP、真实 SKILL.md Skills 和 Plugins"));
    assert!(
        app_tsx.contains("label: t(\"工具与插件\")") || app_tsx.contains("label: \"工具与插件\"")
    );
    assert!(
        app_tsx.contains("title={t(\"Codex 工具与插件\")}")
            || app_tsx.contains("title=\"Codex 工具与插件\"")
    );
    assert!(!app_tsx.contains("label: \"上下文配置\""));
    assert!(!app_tsx.contains("title=\"上下文配置\""));
    assert!(!app_tsx.contains("<strong>Codex 上下文</strong>"));
    assert!(app_tsx.contains("id: \"context\""));
    assert!(app_tsx.contains("function ContextScreen"));
    assert!(app_tsx.contains("route === \"context\""));
    assert!(app_tsx.contains("if (next === \"context\")"));
    assert!(app_tsx.contains("selectedContextConfigToml(entries)"));
    assert!(app_tsx.contains("toggleContextEntryEnabled"));
    assert!(app_tsx.contains("relayFiles={relayFiles}"));
    assert!(app_tsx.contains("read_live_context_entries"));
    assert!(app_tsx.contains("sync_live_context_entries"));
    assert!(app_tsx.contains("refreshLiveContextEntries"));
    assert!(app_tsx.contains("syncLiveContextEntries(next, true)"));
    assert!(app_tsx.contains("const syncLiveContextEntries = async (next: BackendSettings"));
    assert!(app_tsx.contains("actions.syncLiveContextEntries(next, true)"));
    assert!(app_tsx.contains("function contextEntriesWithLiveEntries"));
    assert!(app_tsx.contains("liveByKind"));
    assert!(app_tsx.contains("mergeLiveContextEntries"));
    assert!(app_tsx.contains("withLiveEntryState"));
    assert!(app_tsx.contains("contextEnabledSwitch"));
    assert!(!app_tsx.contains("entry.enabled ? \"已启用\" : \"已禁用\""));
    assert!(!app_tsx.contains("空配置体"));
    assert!(app_tsx.contains("relay-context-delete"));
    assert!(!app_tsx.contains("切换供应商时只合并勾选项"));
    assert!(!app_tsx.contains("未勾选的条目不会写入"));
    assert!(!app_tsx.contains("className=\"context-switch\""));
    assert!(!styles.contains(".context-switch {"));
    assert!(styles.contains(".context-enabled-switch"));
    assert!(styles.contains(".context-switch-track"));
    assert!(styles.contains(".context-switch-thumb"));
    assert!(!styles.contains(".relay-context-row code"));
    assert!(styles.contains(".relay-context-delete"));
}

#[test]
fn manager_window_and_relay_detail_header_stay_usable() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");
    let styles = manifest_dir.parent().unwrap().join("src/styles.css");
    let styles = std::fs::read_to_string(&styles).expect("read manager styles.css");
    let lib_rs =
        std::fs::read_to_string(manifest_dir.join("src/lib.rs")).expect("read manager lib.rs");
    let tauri_conf =
        std::fs::read_to_string(manifest_dir.join("tauri.conf.json")).expect("read tauri config");

    assert!(app_tsx.contains("relay-detail-sticky"));
    assert!(!app_tsx.contains("CardHead title=\"供应商详情\""));
    assert!(styles.contains(".relay-detail-sticky"));
    assert!(styles.contains("position: sticky"));
    assert!(styles.contains("top: 0"));
    assert!(styles.contains("margin: 0"));
    assert!(lib_rs.contains(".inner_size(1180.0, 820.0)"));
    assert!(lib_rs.contains(".min_inner_size(960.0, 720.0)"));
    assert!(tauri_conf.contains("\"width\": 1180"));
    assert!(tauri_conf.contains("\"height\": 820"));
    assert!(tauri_conf.contains("\"minWidth\": 960"));
    assert!(tauri_conf.contains("\"minHeight\": 720"));
}

#[test]
fn relay_preview_deduplicates_root_keys_when_merging_common_config() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");

    assert!(app_tsx.contains("dedupeTomlRootLines"));
    assert!(app_tsx.contains("rootSeen.add(key)"));
    assert!(app_tsx.contains("joinTomlSectionsRootFirst"));
}

#[test]
fn provider_presets_include_runapi() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let presets = manifest_dir.parent().unwrap().join("src/presets.ts");
    let presets = std::fs::read_to_string(&presets).expect("read manager presets.ts");

    assert!(presets.contains("id: \"runapi\""));
    assert!(presets.contains("name: \"RunAPI\""));
    assert!(presets.contains("category: \"aggregator\""));
    assert!(presets.contains("baseUrl: \"https://runapi.co/v1\""));
}

#[test]
fn manager_no_longer_exposes_mobile_control() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");

    assert!(!app_tsx.contains("mobileControl"));
    assert!(!app_tsx.contains("手机控制"));
    assert!(!app_tsx.contains("mobileRelayServers"));
    assert!(!app_tsx.contains("MobileControlScreen"));
}

#[test]
fn manager_ui_no_longer_exposes_command_wrapper_or_startup_marketplace_prompt() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");

    assert!(!app_tsx.contains("启用 Codex 命令包装器"));
    assert!(!app_tsx.contains("修复后端"));
    assert!(!app_tsx.contains("repairBackend"));
    assert!(!app_tsx.contains("await checkPluginMarketplacePrompt()"));
}

#[test]
fn manager_update_install_keeps_visible_progress_bar() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let app_tsx = manifest_dir.parent().unwrap().join("src/App.tsx");
    let app_tsx = std::fs::read_to_string(&app_tsx).expect("read manager App.tsx");

    assert!(app_tsx.contains("下载并运行安装包"));
    assert!(app_tsx.contains("updateInstallProgress"));
    assert!(app_tsx.contains("安装包更新进度"));
    assert!(app_tsx.contains("completedTitle={t(\"上次更新结果\")}"));
    assert!(app_tsx.contains("progress={updateInstallProgress}"));
    assert!(app_tsx.contains("current.percent + 0.2"));
    assert!(app_tsx.contains("下载或启动耗时较长"));
}
