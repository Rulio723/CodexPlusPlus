use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use codex_plus_core::admin_mode::environment::{AdminEnvironmentSpec, AdminEnvironmentTransaction};
use codex_plus_core::app_paths::{
    build_codex_executable, codex_app_version, find_bundled_codex_cli, find_latest_codex_app_dir,
    find_latest_codex_app_dir_from_roots, find_macos_codex_app, normalize_codex_app_path,
    packaged_app_user_model_id, resolve_codex_app_dir_with_saved, user_data_candidates_from,
};
use codex_plus_core::launcher::{
    AdminModeLease, CodexLaunch, DefaultLaunchHooks, LaunchHooks, LaunchOptions,
    MacosCleanupPolicy, activate_existing_administrator_session_with, browser_identity_changed,
    build_codex_arguments, build_codex_arguments_for_settings,
    build_codex_arguments_with_main_inspector, build_codex_arguments_with_native_menu_inspector,
    build_codex_command, build_codex_command_with_native_menu_inspector,
    build_macos_cleanup_command, build_macos_open_command,
    build_macos_open_command_with_native_menu_inspector, build_packaged_activation,
    build_packaged_activation_with_main_inspector,
    build_packaged_activation_with_native_menu_inspector, launch_and_inject_with_hooks,
};
#[cfg(target_os = "macos")]
use codex_plus_core::launcher::{MacosDebugLaunchAction, select_macos_debug_launch_action};
#[cfg(windows)]
use codex_plus_core::launcher::{WindowsProcessControlStrategy, windows_process_control_strategy};
use codex_plus_core::ports::{
    select_packaged_codex_debug_port_with, select_platform_loopback_port_with,
};
use codex_plus_core::settings::{
    BackendSettings, RelayMode, RelayModelRoute, RelayProfile, RelayProtocol,
};
use codex_plus_core::status::StatusStore;
use tokio::sync::Notify;

#[test]
fn browser_identity_change_requires_two_distinct_observations() {
    assert!(!browser_identity_changed(None, "browser-a"));
    assert!(!browser_identity_changed(Some("browser-a"), "browser-a"));
    assert!(browser_identity_changed(Some("browser-a"), "browser-b"));
}

#[test]
fn app_paths_find_latest_windows_package_prefers_highest_version_app_dir() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("OpenAI.Codex_1.2.3.0_x64__abc/app")).unwrap();
    std::fs::create_dir_all(temp.path().join("OpenAI.Codex_26.429.8261.0_x64__abc/app")).unwrap();
    std::fs::create_dir_all(temp.path().join("OpenAI.Codex_not-a-version_x64__abc")).unwrap();

    let latest = find_latest_codex_app_dir(temp.path()).unwrap();

    assert_eq!(
        latest,
        temp.path().join("OpenAI.Codex_26.429.8261.0_x64__abc/app")
    );
}

#[test]
fn app_paths_find_latest_windows_package_ignores_chatgpt_desktop_package() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("OpenAI.Codex_26.707.3748.0_x64__abc/app")).unwrap();
    std::fs::create_dir_all(
        temp.path()
            .join("OpenAI.ChatGPT-Desktop_1.2026.133.0_x64__abc/app"),
    )
    .unwrap();
    std::fs::create_dir_all(
        temp.path()
            .join("OpenAI.ChatGPT-Desktop_2026.514.421.0_neutral_~_abc"),
    )
    .unwrap();

    let latest = find_latest_codex_app_dir(temp.path()).unwrap();

    assert_eq!(
        latest,
        temp.path().join("OpenAI.Codex_26.707.3748.0_x64__abc/app")
    );
    assert_eq!(codex_app_version(&latest).as_deref(), Some("26.707.3748.0"));
    assert_eq!(
        packaged_app_user_model_id(&latest).as_deref(),
        Some("OpenAI.Codex_abc!App")
    );
}

#[test]
fn app_paths_find_latest_windows_package_detects_beta_package() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(
        temp.path()
            .join("OpenAI.CodexBeta_26.527.7698.0_x64__2p2nqsd0c76g0/app"),
    )
    .unwrap();

    let latest = find_latest_codex_app_dir(temp.path()).unwrap();

    assert_eq!(
        latest,
        temp.path()
            .join("OpenAI.CodexBeta_26.527.7698.0_x64__2p2nqsd0c76g0/app")
    );
    assert_eq!(codex_app_version(&latest).as_deref(), Some("26.527.7698.0"));
    assert_eq!(
        packaged_app_user_model_id(&latest).as_deref(),
        Some("OpenAI.CodexBeta_2p2nqsd0c76g0!App")
    );
}

#[test]
fn app_paths_find_latest_windows_package_returns_package_when_app_dir_missing() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("OpenAI.Codex_26.429.8261.0_x64__abc");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("ChatGPT.exe"), "").unwrap();

    assert_eq!(find_latest_codex_app_dir(temp.path()).unwrap(), package);
}

#[test]
fn app_paths_find_latest_windows_package_checks_roots_before_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("WindowsApps");
    std::fs::create_dir_all(root.join("OpenAI.Codex_1.0.0.0_x64__abc/app")).unwrap();
    std::fs::create_dir_all(root.join("OpenAI.Codex_26.513.3673.0_x64__abc/app")).unwrap();

    let latest = find_latest_codex_app_dir_from_roots(&[root]).unwrap();

    assert!(latest.ends_with("OpenAI.Codex_26.513.3673.0_x64__abc/app"));
}

#[test]
fn app_paths_default_windows_discovery_checks_registered_package_before_stale_roots() {
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app_paths.rs")).unwrap();
    let body = source
        .split("pub fn find_latest_codex_app_dir_default")
        .nth(1)
        .unwrap()
        .split("fn find_latest_codex_app_dir_from_appx_package")
        .next()
        .unwrap();

    let registered = body
        .find("find_latest_codex_app_dir_from_appx_package()")
        .unwrap();
    let roots = body
        .find("find_latest_codex_app_dir_from_roots(&windows_app_package_roots())")
        .unwrap();
    assert!(registered < roots);
}

#[test]
fn app_paths_find_latest_windows_package_ignores_chatgpt_across_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root_a = temp.path().join("WindowsAppsA");
    let root_b = temp.path().join("WindowsAppsB");
    std::fs::create_dir_all(root_a.join("OpenAI.Codex_26.999.0.0_x64__abc/app")).unwrap();
    std::fs::create_dir_all(root_b.join("OpenAI.ChatGPT-Desktop_1.2026.133.0_x64__abc/app"))
        .unwrap();

    let latest = find_latest_codex_app_dir_from_roots(&[root_a, root_b]).unwrap();

    assert!(latest.ends_with("OpenAI.Codex_26.999.0.0_x64__abc/app"));
}

#[test]
fn app_paths_extracts_codex_version_from_windows_package_app_dir() {
    let app_dir =
        PathBuf::from(r"C:\Program Files\WindowsApps\OpenAI.Codex_26.513.3673.0_x64__abc\app");

    assert_eq!(
        codex_app_version(&app_dir).as_deref(),
        Some("26.513.3673.0")
    );
}

#[test]
fn app_paths_extracts_codex_version_from_portable_version_file() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("versions").join("current");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("Codex.exe"), "").unwrap();
    std::fs::write(app_dir.join("version"), "42.1.0\n").unwrap();

    assert_eq!(codex_app_version(&app_dir).as_deref(), Some("42.1.0"));
}

#[test]
fn app_paths_prefers_portable_directory_version_over_internal_version_file() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("versions").join("26.519.2736.0");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("Codex.exe"), "").unwrap();
    std::fs::write(app_dir.join("version"), "42.1.0\n").unwrap();

    assert_eq!(
        codex_app_version(&app_dir).as_deref(),
        Some("26.519.2736.0")
    );
}

#[cfg(windows)]
#[test]
fn app_paths_resolves_portable_current_link_to_directory_version() {
    let temp = tempfile::tempdir().unwrap();
    let versions = temp.path().join("versions");
    let target = versions.join("26.519.2736.0");
    let current = versions.join("current");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("Codex.exe"), "").unwrap();
    std::fs::write(target.join("version"), "42.1.0\n").unwrap();
    if let Err(error) = std::os::windows::fs::symlink_dir(&target, &current) {
        if error.raw_os_error() == Some(1314) {
            eprintln!("SKIP: creating a directory symlink requires Windows developer mode");
            return;
        }
        panic!("failed to create portable current link: {error}");
    }

    assert_eq!(
        codex_app_version(&current).as_deref(),
        Some("26.519.2736.0")
    );
}

#[test]
fn app_paths_prefers_chatgpt_entrypoint_when_portable_bundle_contains_codex_shim() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("current");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("Codex.exe"), "").unwrap();
    std::fs::write(app.join("ChatGPT.exe"), "").unwrap();

    assert_eq!(build_codex_executable(&app), app.join("ChatGPT.exe"));
}

#[test]
fn app_paths_extracts_codex_version_from_macos_bundle_plist() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("OpenAI Codex.app");
    let contents = app.join("Contents");
    std::fs::create_dir_all(&contents).unwrap();
    std::fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleVersion</key>
  <string>26.500.0</string>
  <key>CFBundleShortVersionString</key>
  <string>26.513.3673</string>
</dict>
</plist>
"#,
    )
    .unwrap();

    assert_eq!(codex_app_version(&app).as_deref(), Some("26.513.3673"));
}

#[test]
fn app_paths_user_data_candidates_include_local_and_roaming_variants() {
    let local = PathBuf::from(r"C:\Users\me\AppData\Local");
    let roaming = PathBuf::from(r"C:\Users\me\AppData\Roaming");

    let candidates = user_data_candidates_from(Some(&local), Some(&roaming));

    assert_eq!(
        candidates,
        vec![
            local.join("OpenAI").join("ChatGPT"),
            local.join("OpenAI.ChatGPT-Desktop"),
            local.join("ChatGPT"),
            local.join("OpenAI").join("Codex"),
            local.join("OpenAI.Codex"),
            local.join("Codex"),
            roaming.join("OpenAI").join("ChatGPT"),
            roaming.join("OpenAI.ChatGPT-Desktop"),
            roaming.join("ChatGPT"),
            roaming.join("OpenAI").join("Codex"),
            roaming.join("OpenAI.Codex"),
            roaming.join("Codex"),
        ]
    );
}

#[test]
fn app_paths_find_macos_codex_app_prefers_first_search_root_and_known_names() {
    let temp = tempfile::tempdir().unwrap();
    let system_root = temp.path().join("Applications");
    let user_root = temp.path().join("Users/me/Applications");
    let system_app = system_root.join("OpenAI Codex.app");
    let user_app = user_root.join("Codex.app");
    std::fs::create_dir_all(&system_app).unwrap();
    std::fs::create_dir_all(&user_app).unwrap();

    assert_eq!(
        find_macos_codex_app(&[system_root, user_root]).unwrap(),
        system_app
    );
}

#[test]
fn app_paths_prefers_codex_app_over_chatgpt_app() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Applications");
    let codex = root.join("Codex.app");
    let chatgpt = root.join("ChatGPT.app");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::create_dir_all(&chatgpt).unwrap();

    assert_eq!(
        find_macos_codex_app(&[root]).as_deref(),
        Some(codex.as_path())
    );
}

#[test]
fn app_paths_preserves_legacy_macos_candidates_before_chatgpt_app() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Applications");
    let legacy = root.join("OpenAI Codex.app");
    let chatgpt = root.join("ChatGPT.app");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::create_dir_all(&chatgpt).unwrap();

    assert_eq!(
        find_macos_codex_app(&[root]).as_deref(),
        Some(legacy.as_path())
    );
}

#[test]
fn app_paths_build_macos_bundle_executable() {
    let app = PathBuf::from("/Applications/OpenAI Codex.app");

    assert_eq!(
        build_codex_executable(&app),
        PathBuf::from("/Applications/OpenAI Codex.app/Contents/MacOS/Codex")
    );
}

#[test]
fn app_paths_finds_macos_bundled_codex_cli() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("ChatGPT.app");
    let cli = app.join("Contents/Resources/codex");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    std::fs::write(&cli, "").unwrap();

    assert_eq!(find_bundled_codex_cli(&app).as_deref(), Some(cli.as_path()));
}

#[test]
fn app_paths_finds_windows_bundled_codex_cli() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("OpenAI.Codex_1.0.0.0_x64__abc/app");
    let cli = app.join("resources/codex.exe");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    std::fs::write(&cli, "").unwrap();

    assert_eq!(find_bundled_codex_cli(&app).as_deref(), Some(cli.as_path()));
}

#[test]
fn app_paths_returns_none_when_bundled_codex_cli_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("ChatGPT.app");
    std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();

    assert_eq!(find_bundled_codex_cli(&app), None);
}

#[test]
fn app_paths_finds_chatgpt_bundle_and_uses_its_declared_executable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Applications");
    let app = root.join("ChatGPT.app");
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(
        contents.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.openai.codex</string>
  <key>CFBundleExecutable</key>
  <string>ChatGPT</string>
</dict>
</plist>
"#,
    )
    .unwrap();
    std::fs::write(macos.join("ChatGPT"), "").unwrap();

    assert_eq!(
        find_macos_codex_app(&[root]).as_deref(),
        Some(app.as_path())
    );
    assert_eq!(build_codex_executable(&app), macos.join("ChatGPT"));
}

#[test]
fn app_paths_normalizes_executable_and_package_paths() {
    let temp = tempfile::tempdir().unwrap();
    let portable = temp.path().join("CodexPortable");
    let app = portable.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("Codex.exe"), "").unwrap();

    assert_eq!(
        normalize_codex_app_path(&app.join("Codex.exe")).as_deref(),
        Some(app.as_path())
    );
    assert_eq!(
        normalize_codex_app_path(&portable).as_deref(),
        Some(app.as_path())
    );
}

#[test]
fn app_paths_normalizes_chatgpt_desktop_executable_and_builds_it() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp
        .path()
        .join("OpenAI.Codex_1.2026.133.0_x64__abc")
        .join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(app.join("ChatGPT.exe"), "").unwrap();

    assert_eq!(
        normalize_codex_app_path(&app.join("ChatGPT.exe")).as_deref(),
        Some(app.as_path())
    );
    assert_eq!(build_codex_executable(&app), app.join("ChatGPT.exe"));
    assert_eq!(
        packaged_app_user_model_id(&app).as_deref(),
        Some("OpenAI.Codex_abc!App")
    );
}

#[test]
fn app_paths_saved_path_is_used_when_no_explicit_path_is_provided() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app).unwrap();

    assert_eq!(
        resolve_codex_app_dir_with_saved(None, Some(&app.to_string_lossy())).as_deref(),
        Some(app.as_path())
    );
}

#[test]
fn app_paths_rejects_codex_plus_plus_install_dir_as_codex_app() {
    let temp = tempfile::tempdir().unwrap();
    let manager = temp.path().join("Programs").join("Codex++");
    std::fs::create_dir_all(&manager).unwrap();
    std::fs::write(manager.join("Codex++ Manager.exe"), "").unwrap();

    assert_eq!(normalize_codex_app_path(&manager), None);
    assert_eq!(
        normalize_codex_app_path(&manager.join("Codex++ Manager.exe")),
        None
    );

    let resolved = resolve_codex_app_dir_with_saved(None, Some(&manager.to_string_lossy()));
    assert_ne!(resolved.as_deref(), Some(manager.as_path()));
}

#[test]
fn app_paths_rejects_plain_directory_without_codex_executable() {
    let temp = tempfile::tempdir().unwrap();
    let plain = temp.path().join("not-a-codex-app");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("readme.txt"), "nope").unwrap();

    assert_eq!(normalize_codex_app_path(&plain), None);
    assert_eq!(normalize_codex_app_path(&plain.join("readme.txt")), None);
}

#[test]
fn app_paths_empty_saved_path_matches_no_saved_path() {
    assert_eq!(
        resolve_codex_app_dir_with_saved(None, Some("")),
        resolve_codex_app_dir_with_saved(None, None)
    );
    assert_eq!(
        resolve_codex_app_dir_with_saved(None, Some("   ")),
        resolve_codex_app_dir_with_saved(None, None)
    );
}

#[test]
fn app_paths_invalid_saved_path_falls_back_instead_of_sticking() {
    let temp = tempfile::tempdir().unwrap();
    let junk = temp.path().join("Codex++");
    std::fs::create_dir_all(&junk).unwrap();

    // 合法独立安装：即使 saved 指向 Codex++，规范化失败后应能落到该候选
    // （通过显式 app_dir 验证回退链之外的合法路径仍可用）
    let standalone = temp.path().join("OpenAI").join("Codex").join("bin");
    std::fs::create_dir_all(&standalone).unwrap();
    std::fs::write(standalone.join("codex.exe"), "").unwrap();

    assert_eq!(normalize_codex_app_path(&junk), None);
    assert_eq!(
        normalize_codex_app_path(&standalone).as_deref(),
        Some(standalone.as_path())
    );
    assert_eq!(
        resolve_codex_app_dir_with_saved(Some(&standalone), Some(&junk.to_string_lossy()))
            .as_deref(),
        Some(standalone.as_path())
    );
}

#[test]
fn launcher_builds_debug_arguments_and_commands() {
    let app_dir = PathBuf::from(r"C:\Codex\app");

    assert_eq!(
        build_codex_arguments(9229, &[]),
        vec![
            "--remote-debugging-port=9229".to_string(),
            "--remote-allow-origins=http://127.0.0.1:9229".to_string(),
        ]
    );
    let command = build_codex_command(&app_dir, 9229, &[]);
    assert_eq!(command[1], "--remote-debugging-port=9229");
    assert_eq!(command[2], "--remote-allow-origins=http://127.0.0.1:9229");
}

#[test]
fn launcher_does_not_override_codex_app_environment() {
    let source = include_str!("../src/launcher.rs");

    assert!(!source.contains(".envs(codex_process_environment())"));
    assert!(!source.contains("activate_packaged_app_with_environment"));
    assert!(!source.contains("with_temporary_proxy_environment"));
}

#[test]
fn launcher_does_not_prepare_projectless_main_window() {
    let source = include_str!("../src/launcher.rs");

    assert!(!source.contains("prepare_projectless_main_window_nonfatal"));
    assert!(!source.contains("launcher.prelaunch"));
}

#[test]
fn launcher_windows_process_wait_uses_platform_cfg_guards() {
    let source = include_str!("../src/launcher.rs").replace("\r\n", "\n");

    assert!(source.contains(
        "#[cfg(windows)]\nasync fn wait_for_windows_process_id(process_id: u32) -> anyhow::Result<()>"
    ));
    assert!(source.contains(
        "#[cfg(not(windows))]\nasync fn wait_for_windows_process_id(process_id: u32) -> anyhow::Result<()>"
    ));
    assert!(source.contains(
        "#[cfg(windows)]\nfn wait_for_windows_process_id_blocking(process_id: u32) -> anyhow::Result<()>"
    ));
}

#[test]
fn launcher_appends_extra_codex_arguments_after_debug_arguments() {
    let app_dir = PathBuf::from(r"C:\Codex\app");
    let extra_args = vec![
        "--force_high_performance_gpu".to_string(),
        "  ".to_string(),
        "--enable-features=UseOzonePlatform".to_string(),
    ];

    assert_eq!(
        build_codex_arguments(9229, &extra_args),
        vec![
            "--remote-debugging-port=9229".to_string(),
            "--remote-allow-origins=http://127.0.0.1:9229".to_string(),
            "--force_high_performance_gpu".to_string(),
            "--enable-features=UseOzonePlatform".to_string(),
        ]
    );
    let command = build_codex_command(&app_dir, 9229, &extra_args);
    assert_eq!(command[1], "--remote-debugging-port=9229");
    assert_eq!(command[2], "--remote-allow-origins=http://127.0.0.1:9229");
    assert_eq!(command[3], "--force_high_performance_gpu");
    assert_eq!(command[4], "--enable-features=UseOzonePlatform");
}

#[test]
fn launcher_fast_startup_adds_statsig_fast_fail_argument_when_enabled() {
    let settings = BackendSettings {
        codex_app_fast_startup: true,
        ..BackendSettings::default()
    };
    let args = build_codex_arguments_for_settings(9229, &settings);

    assert!(args.iter().any(|arg| {
        arg.starts_with("--host-resolver-rules=")
            && arg.contains("MAP ab.chatgpt.com 127.0.0.1")
            && arg.contains("MAP featureassets.org 127.0.0.1")
            && arg.contains("MAP cloudflare-dns.com 127.0.0.1")
    }));

    let settings = BackendSettings {
        codex_app_fast_startup: true,
        codex_extra_args: vec!["--host-resolver-rules=MAP example.test 127.0.0.1".to_string()],
        ..BackendSettings::default()
    };
    let args = build_codex_arguments_for_settings(9229, &settings);
    assert_eq!(
        args.iter()
            .filter(|arg| arg.starts_with("--host-resolver-rules="))
            .count(),
        1
    );

    let settings = BackendSettings {
        codex_app_fast_startup: false,
        ..BackendSettings::default()
    };
    let args = build_codex_arguments_for_settings(9229, &settings);
    assert!(
        !args
            .iter()
            .any(|arg| arg.starts_with("--host-resolver-rules="))
    );
}

#[test]
fn launcher_native_menu_inspector_arguments_are_added_before_extra_args() {
    let app_dir = PathBuf::from(r"C:\Codex\app");
    let extra_args = vec!["--force_high_performance_gpu".to_string()];

    assert_eq!(
        build_codex_arguments_with_native_menu_inspector(9229, 9329, &extra_args),
        vec![
            "--remote-debugging-port=9229".to_string(),
            "--remote-allow-origins=http://127.0.0.1:9229".to_string(),
            "--inspect=127.0.0.1:9329".to_string(),
            "--force_high_performance_gpu".to_string(),
        ]
    );
    let command = build_codex_command_with_native_menu_inspector(&app_dir, 9229, 9329, &extra_args);
    assert_eq!(command[1], "--remote-debugging-port=9229");
    assert_eq!(command[2], "--remote-allow-origins=http://127.0.0.1:9229");
    assert_eq!(command[3], "--inspect=127.0.0.1:9329");
    assert_eq!(command[4], "--force_high_performance_gpu");
}

#[test]
fn launcher_administrator_app_server_does_not_pause_electron_main_process() {
    assert_eq!(
        build_codex_arguments_with_main_inspector(9229, 9329, &[]),
        vec![
            "--remote-debugging-port=9229".to_string(),
            "--remote-allow-origins=http://127.0.0.1:9229".to_string(),
            "--inspect=127.0.0.1:9329".to_string(),
        ]
    );

    let app_dir = PathBuf::from(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.506.2212.0_x64__2p2nqsd0c76g0\app",
    );
    let activation =
        build_packaged_activation_with_main_inspector(&app_dir, 9229, 9329, &[]).unwrap();
    let CodexLaunch::PackagedActivation { arguments, .. } = activation else {
        panic!("expected packaged activation")
    };
    assert!(arguments.contains("--inspect=127.0.0.1:9329"));
    assert!(!arguments.contains("--inspect-brk"));
    assert!(!arguments.contains("CODEX_PLUS_ADMIN_APP_SERVER_PIPE"));
}

#[test]
fn launcher_constructs_windows_packaged_activation_without_real_app() {
    let version = [26, 820, 9563, 0]
        .into_iter()
        .map(|part| part.to_string())
        .collect::<Vec<_>>()
        .join(".");
    let app_dir = PathBuf::from(format!(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_{version}_x64__2p2nqsd0c76g0\app"
    ));

    assert_eq!(
        packaged_app_user_model_id(&app_dir).unwrap(),
        "OpenAI.Codex_2p2nqsd0c76g0!App"
    );
    assert_eq!(
        build_packaged_activation(&app_dir, 9229, &[]).unwrap(),
        CodexLaunch::PackagedActivation {
            app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".to_string(),
            arguments: "--remote-debugging-port=9229 --remote-allow-origins=http://127.0.0.1:9229"
                .to_string(),
            process_id: None,
        }
    );
}

#[test]
fn launcher_packaged_activation_appends_extra_codex_arguments() {
    let app_dir = PathBuf::from(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.506.2212.0_x64__2p2nqsd0c76g0\app",
    );
    let extra_args = vec!["--force_high_performance_gpu".to_string()];

    assert_eq!(
        build_packaged_activation(&app_dir, 9229, &extra_args).unwrap(),
        CodexLaunch::PackagedActivation {
            app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".to_string(),
            arguments:
                "--remote-debugging-port=9229 --remote-allow-origins=http://127.0.0.1:9229 --force_high_performance_gpu"
                    .to_string(),
            process_id: None,
        }
    );
}

#[test]
fn launcher_packaged_activation_adds_native_menu_inspector_argument() {
    let app_dir = PathBuf::from(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.506.2212.0_x64__2p2nqsd0c76g0\app",
    );

    assert_eq!(
        build_packaged_activation_with_native_menu_inspector(&app_dir, 9229, 9329, &[]).unwrap(),
        CodexLaunch::PackagedActivation {
            app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".to_string(),
            arguments:
                "--remote-debugging-port=9229 --remote-allow-origins=http://127.0.0.1:9229 --inspect=127.0.0.1:9329"
                    .to_string(),
            process_id: None,
        }
    );
}

#[test]
fn launcher_packaged_activation_can_preserve_process_id() {
    let launch = CodexLaunch::PackagedActivation {
        app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".to_string(),
        arguments: "--remote-debugging-port=9229".to_string(),
        process_id: Some(4242),
    };

    assert_eq!(launch.process_id(), Some(4242));
}

#[test]
fn launcher_applies_codexplusplus_window_icon_after_packaged_activation() {
    let source = include_str!("../src/launcher.rs");

    assert!(source.contains("apply_codexplusplus_window_icon_after_launch(process_id);"));
    assert!(source.contains("windows_apply_codexplusplus_icon_to_process_window"));
}

#[test]
fn launcher_no_longer_contains_mobile_control_runtime() {
    let launcher_source = include_str!("../src/launcher.rs");
    let settings_source = include_str!("../src/settings.rs");
    let workspace_toml = include_str!("../../../Cargo.toml");

    assert!(!workspace_toml.contains("apps/codex-plus-mobile-relay"));
    assert!(!launcher_source.contains("MobileRelay"));
    assert!(!launcher_source.contains("mobile_relay"));
    assert!(!launcher_source.contains("\"/mobile\""));
    assert!(!launcher_source.contains("CODEX_PLUS_MOBILE"));
    assert!(!settings_source.contains("mobileControl"));
}

#[test]
fn launcher_plugin_marketplace_unlock_repairs_role_specific_plugins() {
    let launcher_source = include_str!("../src/launcher.rs");

    assert!(launcher_source.contains("ensure_openai_curated_marketplace_config(&home)"));
    assert!(launcher_source.contains("ensure_role_specific_plugins_marketplace_config(&home)"));
}

#[test]
fn app_paths_uses_native_windows_package_api_without_powershell() {
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app_paths.rs")).unwrap();

    assert!(source.contains("GetPackagesByPackageFamily"));
    assert!(source.contains("GetPackagePathByFullName"));
    assert!(!source.contains("Command::new(\"powershell\")"));
}

#[test]
fn launcher_packaged_activation_does_not_directly_fallback_to_windowsapps_exe() {
    let source = include_str!("../src/launcher.rs");

    assert!(!source.contains("launcher.packaged_activation_cdp_unready_direct_fallback"));
    assert!(!source.contains("terminate_windows_process_id(process_id).await"));
}

#[cfg(windows)]
#[test]
fn launcher_windows_packaged_process_management_uses_native_api() {
    assert_eq!(
        windows_process_control_strategy(),
        WindowsProcessControlStrategy::NativeWindowsApi
    );
}

#[test]
fn launcher_macos_open_command_waits_for_app_exit() {
    let command = build_macos_open_command(Path::new("/Applications/Codex.app"), 9229, &[]);

    assert_eq!(command[0], "open");
    assert!(command.contains(&"-W".to_string()));
    assert!(command.contains(&"-a".to_string()));
    assert!(command.contains(&"--args".to_string()));
    assert!(command.contains(&"--remote-debugging-port=9229".to_string()));
}

#[test]
fn launcher_macos_open_command_appends_extra_codex_arguments_after_args() {
    let extra_args = vec!["--force_high_performance_gpu".to_string()];
    let command = build_macos_open_command(Path::new("/Applications/Codex.app"), 9229, &extra_args);
    let args_index = command
        .iter()
        .position(|part| part == "--args")
        .expect("macOS command should contain --args");

    assert_eq!(
        &command[args_index + 1..],
        &[
            "--remote-debugging-port=9229".to_string(),
            "--remote-allow-origins=http://127.0.0.1:9229".to_string(),
            "--force_high_performance_gpu".to_string(),
        ]
    );
}

#[test]
fn launcher_macos_open_command_adds_native_menu_inspector_argument() {
    let command = build_macos_open_command_with_native_menu_inspector(
        Path::new("/Applications/Codex.app"),
        9229,
        9329,
        &[],
    );
    let args_index = command
        .iter()
        .position(|part| part == "--args")
        .expect("macOS command should contain --args");

    assert_eq!(
        &command[args_index + 1..],
        &[
            "--remote-debugging-port=9229".to_string(),
            "--remote-allow-origins=http://127.0.0.1:9229".to_string(),
            "--inspect=127.0.0.1:9329".to_string(),
        ]
    );
}

#[test]
fn ports_windows_falls_back_to_ephemeral_when_requested_is_busy() {
    let selected = select_platform_loopback_port_with(9229, true, |_| false, || 43001);

    assert_eq!(selected, 43001);
}

#[test]
fn ports_windows_packaged_debug_falls_back_to_ephemeral_when_requested_is_busy() {
    let selected =
        select_packaged_codex_debug_port_with(9229, true, |_| false, |_| false, || 43001);

    assert_eq!(selected, 43001);
}

#[test]
fn ports_windows_packaged_debug_keeps_requested_when_existing_cdp_is_available() {
    let selected = select_packaged_codex_debug_port_with(9229, true, |_| false, |_| true, || 43001);

    assert_eq!(selected, 9229);
}

#[test]
fn ports_non_windows_keeps_requested_even_when_busy() {
    let selected = select_platform_loopback_port_with(9229, false, |_| false, || 43001);

    assert_eq!(selected, 9229);
}

#[tokio::test]
async fn default_helper_serves_backend_status_over_http() {
    let hooks = DefaultLaunchHooks::default();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    hooks.start_helper(port).await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(format!("http://127.0.0.1:{port}/backend/status"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let payload: serde_json::Value = response.json().await.unwrap();
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["transport"], "http-helper");
    assert!(payload["hideOfficialUsageAlert"].is_boolean());

    let repair_response = client
        .post(format!("http://127.0.0.1:{port}/backend/repair"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert!(!repair_response.status().is_success());

    hooks.shutdown_helper(port).await;
}

#[tokio::test]
async fn default_helper_accepts_diagnostic_log_events_over_http() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("codex-plus.log");
    codex_plus_core::diagnostic_log::set_diagnostic_log_path_for_tests(Some(log_path.clone()));
    let hooks = DefaultLaunchHooks::default();
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    hooks.start_helper(port).await.unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{port}/diagnostics/log"))
        .json(&serde_json::json!({
            "event": "backend_check_failed",
            "message": "fetch failed",
            "helperBase": format!("http://127.0.0.1:{port}")
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let payload: serde_json::Value = response.json().await.unwrap();
    assert_eq!(payload["status"], "ok");
    hooks.shutdown_helper(port).await;

    let contents = std::fs::read_to_string(&log_path).unwrap();
    assert!(contents.contains("renderer.backend_check_failed"));
    assert!(contents.contains("fetch failed"));
    codex_plus_core::diagnostic_log::set_diagnostic_log_path_for_tests(None);
}

#[tokio::test]
async fn launch_lifecycle_runs_enabled_maintenance_without_applying_relay_profile() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone())
        .with_settings(BackendSettings {
            provider_sync_enabled: true,
            relay_profiles_enabled: true,
            codex_app_plugin_marketplace_unlock: true,
            ..BackendSettings::default()
        })
        .with_launch_result(CodexLaunch::Process {
            command: vec!["codex".to_string()],
            wait_strategy: codex_plus_core::launcher::ProcessWaitStrategy::TrackedChild,
            macos_cleanup_policy: None,
        });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir.clone()),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "provider-sync",
            "start-helper:57321",
            "launch:9229",
            "inject:9229:57321",
            "status:running",
            "wait-codex",
            "shutdown-helper:57321",
        ]
    );
    let events = events.lock().unwrap().clone();
    assert!(!events.contains(&"apply-relay".to_string()));
    assert!(events.contains(&"provider-sync".to_string()));
    assert_eq!(
        handle
            .status_store
            .load_latest()
            .unwrap()
            .unwrap()
            .codex_app
            .as_deref(),
        Some(app_dir.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn launch_lifecycle_passes_configured_extra_args_to_codex_launch() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        codex_extra_args: vec!["--force_high_performance_gpu".to_string()],
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"launch:9229:--force_high_performance_gpu".to_string())
    );
}

#[tokio::test]
async fn launch_lifecycle_passes_native_menu_localization_switch_to_codex_launch() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        codex_app_native_menu_localization: false,
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"launch:9229:native-menu-off".to_string())
    );
}

#[tokio::test]
async fn launch_lifecycle_keeps_js_injection_in_relay_mode() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        launch_mode: codex_plus_core::settings::LaunchMode::Relay,
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "start-helper:57321",
            "launch:9229",
            "inject:9229:57321",
            "status:running",
            "wait-codex",
            "shutdown-helper:57321",
        ]
    );
}

#[tokio::test]
async fn launch_lifecycle_skips_helper_and_injection_when_enhancements_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        enhancements_enabled: false,
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "launch:9229",
            "status:running",
            "wait-codex",
        ]
    );
}

#[tokio::test]
async fn official_mix_responses_profile_starts_fixed_protocol_proxy_without_enhancements() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        enhancements_enabled: false,
        relay_profiles_enabled: true,
        active_relay_id: "official-mix".to_string(),
        relay_profiles: vec![RelayProfile {
            id: "official-mix".to_string(),
            relay_mode: RelayMode::Official,
            official_mix_api_key: true,
            hide_official_usage_alert: false,
            protocol: RelayProtocol::Responses,
            ..RelayProfile::default()
        }],
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58123,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap().clone();
    assert!(!events.contains(&"remote-control-session-recovery".to_string()));
    assert!(!events.contains(&"provider-sync".to_string()));
    assert!(events.contains(&"select-helper:58123".to_string()));
    assert!(events.contains(&"start-helper:58123".to_string()));
    assert!(events.contains(&"shutdown-helper:58123".to_string()));
    assert!(!events.iter().any(|event| event.starts_with("inject:")));
}

fn official_mix_responses_settings() -> BackendSettings {
    BackendSettings {
        enhancements_enabled: false,
        relay_profiles_enabled: true,
        active_relay_id: "official-mix".to_string(),
        relay_profiles: vec![RelayProfile {
            id: "official-mix".to_string(),
            relay_mode: RelayMode::Official,
            official_mix_api_key: true,
            hide_official_usage_alert: false,
            protocol: RelayProtocol::Responses,
            ..RelayProfile::default()
        }],
        ..BackendSettings::default()
    }
}

/// issue #1933：管理器「重启」先强杀旧 launcher 再拉新的，旧 helper 交还 57321 要一小会儿。
/// 该端口写死在 config.toml 的 base_url 里换不了，所以必须等前任让位，
/// 而不是像过去那样一次 bind 失败就中止整个启动。
#[tokio::test]
async fn fixed_protocol_proxy_port_waits_for_the_previous_helper_to_release_it() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone())
        .with_settings(official_mix_responses_settings())
        .with_helper_bind_conflicts(3);

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58123,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "start-helper-busy:58123")
            .count(),
        3
    );
    assert!(events.contains(&"start-helper:58123".to_string()));
    assert!(events.contains(&"launch:9229".to_string()));
}

/// 等不到就得给出能照着做的说明，而不是裸的 bind 失败。
#[tokio::test]
async fn a_permanently_busy_protocol_proxy_port_reports_what_the_user_should_do() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone())
        .with_settings(official_mix_responses_settings())
        .with_helper_bind_conflicts(u32::MAX);

    let error = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58123,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("58123"), "unexpected message: {message}");
    assert!(
        message.contains("base_url"),
        "unexpected message: {message}"
    );
    // 端口没起来就不该继续把 Codex 拉起来，否则它会连到没人监听的地址。
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.starts_with("launch:"))
    );
}

/// 普通 helper 端口在上面已经挑过空闲的了，占用说明是别的问题，不该白等六秒。
#[tokio::test]
async fn a_busy_floating_helper_port_fails_immediately_without_waiting() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_helper_bind_conflicts(u32::MAX);

    let error = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58123,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("failed to bind helper runtime"));
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.starts_with("start-helper-busy:"))
            .count(),
        1
    );
}

#[tokio::test]
async fn pending_remote_control_recovery_runs_without_an_official_mix_profile() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_pending_remote_control_session_recoveries();

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58123,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"remote-control-session-recovery".to_string())
    );
}

#[tokio::test]
async fn official_mix_responses_profile_keeps_proxy_when_profile_switching_is_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        enhancements_enabled: false,
        relay_profiles_enabled: false,
        active_relay_id: "official-mix".to_string(),
        relay_profiles: vec![RelayProfile {
            id: "official-mix".to_string(),
            relay_mode: RelayMode::Official,
            official_mix_api_key: true,
            hide_official_usage_alert: false,
            protocol: RelayProtocol::Responses,
            ..RelayProfile::default()
        }],
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58123,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap().clone();
    assert!(events.contains(&"select-helper:58123".to_string()));
    assert!(events.contains(&"start-helper:58123".to_string()));
    assert!(events.contains(&"shutdown-helper:58123".to_string()));
    assert!(!events.iter().any(|event| event.starts_with("inject:")));
}

#[tokio::test]
async fn launch_lifecycle_does_not_apply_relay_profile_before_launching_codex() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        relay_profiles_enabled: true,
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap().clone();
    assert!(!events.contains(&"apply-relay".to_string()));
    assert!(events.contains(&"launch:9229".to_string()));
}

#[tokio::test]
async fn launch_lifecycle_skips_active_relay_profile_when_supplier_config_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        relay_profiles_enabled: false,
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap().clone();
    assert!(!events.contains(&"apply-relay".to_string()));
    assert!(events.contains(&"launch:9229".to_string()));
}

#[tokio::test]
async fn launch_lifecycle_tolerates_duplicate_context_parent_tables_without_applying_relay() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_settings(BackendSettings {
        relay_common_config_contents: "[mcp_servers]\n".to_string(),
        relay_context_config_contents: "[mcp_servers]\n\n[mcp_servers.ida]\ncommand = \"python\"\n"
            .to_string(),
        relay_profiles: vec![RelayProfile {
            id: "relay-a".to_string(),
            name: "Relay A".to_string(),
            relay_mode: codex_plus_core::settings::RelayMode::PureApi,
            config_contents: r#"model = "gpt-5.5"
model_provider = "custom"

[model_providers.custom]
name = "custom"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://relay.example/v1"
experimental_bearer_token = "sk-test"
"#
            .to_string(),
            auth_contents: r#"{"OPENAI_API_KEY":"sk-test"}"#.to_string(),
            ..RelayProfile::default()
        }],
        active_relay_id: "relay-a".to_string(),
        ..BackendSettings::default()
    });

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap().clone();
    assert!(!events.contains(&"apply-relay".to_string()));
    assert!(events.contains(&"launch:9229".to_string()));
}

#[tokio::test]
async fn launch_lifecycle_enters_degraded_mode_and_retries_when_injection_fails() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_inject_error("inject failed");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "start-helper:57321",
            "launch:9229",
            "inject:9229:57321",
            "status:running_degraded",
        ]
    );
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "running_degraded");
    assert!(status.message.contains("Codex launched"));

    handle.wait_for_codex_exit().await.unwrap();
    let events = events.lock().unwrap().clone();
    assert!(events.contains(&"wait-codex".to_string()));
    assert!(events.contains(&"shutdown-helper:57321".to_string()));
    assert!(!events.contains(&"terminate-codex".to_string()));
}

#[tokio::test]
async fn launch_lifecycle_cleans_helper_when_launch_fails_after_helper_started() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone()).with_launch_error("launch failed");

    let error = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("launch failed"));
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "start-helper:57321",
            "launch:9229",
            "shutdown-helper:57321",
            "status:failed",
        ]
    );
}

#[tokio::test]
async fn launch_starts_helper_when_chat_protocol_proxy_is_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let settings = BackendSettings {
        enhancements_enabled: false,
        relay_profiles: vec![RelayProfile {
            id: "relay-chat".to_string(),
            name: "Chat".to_string(),
            model: String::new(),
            base_url: "https://chat-only.example.test/v1".to_string(),
            upstream_base_url: "https://chat-only.example.test/v1".to_string(),
            api_key: "sk-test".to_string(),
            protocol: RelayProtocol::ChatCompletions,
            relay_mode: codex_plus_core::settings::RelayMode::MixedApi,
            official_mix_api_key: false,
            no_auth: false,
            hide_official_usage_alert: false,
            test_model: String::new(),
            config_contents: String::new(),
            auth_contents: String::new(),
            use_common_config: true,
            context_window: String::new(),
            auto_compact_limit: String::new(),
            model_insert_mode: codex_plus_core::settings::RelayModelInsertMode::default(),
            model_list: String::new(),
            model_windows: String::new(),
            model_vlm: String::new(),
            vlm_api_key: String::new(),
            vlm_model: String::new(),
            vlm_base_url: String::new(),
            user_agent: String::new(),
            official_codex_fingerprint: false,
            sub2api_enabled: false,
            sub2api_multiplier: String::new(),
            model_routes: Vec::new(),
        }],
        active_relay_id: "relay-chat".to_string(),
        ..BackendSettings::default()
    };
    let hooks = FakeHooks::new(events.clone()).with_settings(settings);

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58000,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();

    let before_stop = events.lock().unwrap().clone();
    assert!(before_stop.contains(&"select-helper:58000".to_string()));
    assert!(before_stop.contains(&"start-helper:58000".to_string()));
    assert!(!before_stop.contains(&"inject:9229:57321".to_string()));

    handle.wait_for_codex_exit().await.unwrap();

    let after_stop = events.lock().unwrap().clone();
    assert!(after_stop.contains(&"wait-codex".to_string()));
    assert!(after_stop.contains(&"shutdown-helper:58000".to_string()));
}

#[tokio::test]
async fn launch_starts_helper_on_selected_port_for_official_mix_api() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let settings = BackendSettings {
        enhancements_enabled: false,
        relay_profiles: vec![RelayProfile {
            id: "relay-official-mix".to_string(),
            name: "Official mix".to_string(),
            protocol: RelayProtocol::Responses,
            relay_mode: codex_plus_core::settings::RelayMode::Official,
            official_mix_api_key: true,
            config_contents: "openai_base_url = \"http://127.0.0.1:57321/v1\"\n".to_string(),
            ..RelayProfile::default()
        }],
        active_relay_id: "relay-official-mix".to_string(),
        ..BackendSettings::default()
    };
    let hooks = FakeHooks::new(events.clone()).with_settings(settings);

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58001,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();

    let events_before_stop = events.lock().unwrap().clone();
    assert!(events_before_stop.contains(&"select-helper:58001".to_string()));
    assert!(events_before_stop.contains(&"start-helper:58001".to_string()));
    assert!(!events_before_stop.contains(&"start-helper:57321".to_string()));

    handle.wait_for_codex_exit().await.unwrap();
    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"shutdown-helper:58001".to_string())
    );
}

#[tokio::test]
async fn launch_starts_helper_when_model_routing_is_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let settings = BackendSettings {
        enhancements_enabled: false,
        active_relay_id: "source".to_string(),
        relay_profiles: vec![
            RelayProfile {
                id: "source".to_string(),
                name: "Source".to_string(),
                base_url: "https://source.example.test/v1".to_string(),
                api_key: "sk-source".to_string(),
                model_routes: vec![RelayModelRoute {
                    model: "gpt-5.6-luna".to_string(),
                    target_relay_id: "target".to_string(),
                    target_model: String::new(),
                }],
                ..RelayProfile::default()
            },
            RelayProfile {
                id: "target".to_string(),
                name: "Target".to_string(),
                base_url: "https://target.example.test/v1".to_string(),
                api_key: "sk-target".to_string(),
                ..RelayProfile::default()
            },
        ],
        ..BackendSettings::default()
    };
    let hooks = FakeHooks::new(events.clone()).with_settings(settings);

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 58000,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();

    let before_stop = events.lock().unwrap().clone();
    assert!(before_stop.contains(&"select-helper:58000".to_string()));
    assert!(before_stop.contains(&"start-helper:58000".to_string()));
    assert!(!before_stop.contains(&"inject:9229:57321".to_string()));

    handle.wait_for_codex_exit().await.unwrap();
    let after_stop = events.lock().unwrap().clone();
    assert!(after_stop.contains(&"shutdown-helper:58000".to_string()));
}

#[tokio::test]
async fn launch_lifecycle_cleans_helper_and_codex_when_status_save_fails() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(temp.path().join("status-parent-file"), "not a directory").unwrap();
    let status_store = StatusStore::new(
        temp.path()
            .join("status-parent-file")
            .join("latest-status.json"),
    );
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks =
        FakeHooks::new(events.clone()).with_launch_result(CodexLaunch::PackagedActivation {
            app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".to_string(),
            arguments: "--remote-debugging-port=9229".to_string(),
            process_id: Some(4242),
        });

    let error = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("failed to create directory"));
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "start-helper:57321",
            "launch:9229",
            "inject:9229:57321",
            "shutdown-helper:57321",
            "terminate-packaged:4242",
            "status:failed",
        ]
    );
}

#[tokio::test]
async fn launch_lifecycle_keeps_packaged_process_id_running_and_retries_when_injection_fails() {
    let temp = tempfile::tempdir().unwrap();
    let app_dir = temp.path().join("Codex.app");
    std::fs::create_dir_all(&app_dir).unwrap();
    let status_store = StatusStore::new(temp.path().join("latest-status.json"));
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let hooks = FakeHooks::new(events.clone())
        .with_launch_result(CodexLaunch::PackagedActivation {
            app_user_model_id: "OpenAI.Codex_2p2nqsd0c76g0!App".to_string(),
            arguments: "--remote-debugging-port=9229".to_string(),
            process_id: Some(4242),
        })
        .with_inject_error("inject failed");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(app_dir),
            debug_port: 9229,
            helper_port: 57321,
            status_store,
        },
        &hooks,
    )
    .await
    .unwrap();

    assert!(
        !events
            .lock()
            .unwrap()
            .contains(&"terminate-packaged:4242".to_string())
    );
    handle.wait_for_codex_exit().await.unwrap();
}

#[tokio::test]
async fn default_provider_sync_enabled_fails_instead_of_silently_skipping() {
    let hooks = FakeHooks::new(Arc::new(Mutex::new(Vec::new()))).with_provider_sync_unsupported();

    let error = hooks
        .run_provider_sync()
        .await
        .expect_err("default-style provider sync should be explicit");

    assert!(
        error
            .to_string()
            .contains("provider sync requires launcher hooks")
    );
}

#[tokio::test]
async fn administrator_mode_starts_before_codex_and_stops_after_wait() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events.clone()).with_settings(settings);

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    handle.wait_for_codex_exit().await.unwrap();

    let events = events.lock().unwrap();
    let start = events
        .iter()
        .position(|event| event == "start-admin")
        .unwrap();
    let launch = events
        .iter()
        .position(|event| event.starts_with("launch:9229"))
        .unwrap();
    let wait = events
        .iter()
        .position(|event| event == "wait-codex")
        .unwrap();
    let stop = events
        .iter()
        .position(|event| event == "stop-admin")
        .unwrap();
    assert!(start < launch && launch < wait && wait < stop);
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "stopped");
    assert_eq!(status.administrator_mode.state, "stopped");
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
    assert_eq!(status.administrator_mode.error_component, None);
}

#[tokio::test]
async fn administrator_computer_use_broker_failure_terminates_codex_and_revokes_active_status() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let (fatal_tx, fatal_rx) = tokio::sync::watch::channel(None);
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_admin_lease(AdminModeLease::testing_with_health(fatal_rx));

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    fatal_tx
        .send(Some(
            "administrator Computer Use broker stopped unexpectedly".to_owned(),
        ))
        .unwrap();
    let error = handle.wait_for_codex_exit().await.unwrap_err();

    assert!(error.to_string().contains("Computer Use broker"));
    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"terminate-codex".to_string())
    );
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "failed");
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("computer_use")
    );
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
}

#[tokio::test]
async fn administrator_exec_broker_failure_terminates_codex_and_revokes_exec_status() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let (fatal_tx, fatal_rx) = tokio::sync::watch::channel(None);
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_admin_lease(AdminModeLease::testing_with_health(fatal_rx));

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    fatal_tx
        .send(Some(
            "administrator exec broker stopped unexpectedly".to_owned(),
        ))
        .unwrap();
    let error = handle.wait_for_codex_exit().await.unwrap_err();

    assert!(error.to_string().contains("exec broker"));
    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"terminate-codex".to_string())
    );
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "failed");
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("exec")
    );
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
}

#[tokio::test]
async fn administrator_exec_failure_interrupts_pending_post_launch_work() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = true;
    let (fatal_tx, fatal_rx) = tokio::sync::watch::channel(None);
    let injection_started = Arc::new(Notify::new());
    let injection_release = Arc::new(Notify::new());
    let cleanup_started = Arc::new(Notify::new());
    let cleanup_release = Arc::new(Notify::new());
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_admin_lease(AdminModeLease::testing_with_health(fatal_rx))
        .with_injection_gate(injection_started.clone(), injection_release)
        .with_admin_stop_gate(cleanup_started.clone(), cleanup_release.clone());

    let launch = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    );
    tokio::pin!(launch);
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            _ = injection_started.notified() => {}
            result = &mut launch => panic!("launch completed before injection gate: {result:?}"),
        }
    })
    .await
    .expect("injection should start");
    fatal_tx
        .send(Some(
            "administrator exec broker stopped unexpectedly".to_owned(),
        ))
        .expect("publish exec failure");
    tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            _ = cleanup_started.notified() => {}
            result = &mut launch => panic!("launch completed before cleanup gate: {result:?}"),
        }
    })
    .await
    .expect("administrator cleanup should start");
    assert!(
        events
            .lock()
            .unwrap()
            .contains(&"terminate-codex".to_string()),
        "Codex must terminate before administrator cleanup can block"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut launch)
            .await
            .is_err(),
        "launch should remain pending while administrator cleanup is gated"
    );
    cleanup_release.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(1), &mut launch)
        .await
        .expect("broker failure must interrupt post-launch work")
        .expect_err("launch must fail closed");

    assert!(error.to_string().contains("exec broker"));
    let events = events.lock().unwrap();
    assert!(events.contains(&"terminate-codex".to_string()));
    assert!(events.contains(&"stop-admin".to_string()));
    assert!(events.contains(&"shutdown-helper:57321".to_string()));
    drop(events);
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "failed");
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("exec")
    );
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
}

#[tokio::test]
async fn administrator_exec_failure_remains_primary_when_cleanup_also_fails() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let (fatal_tx, fatal_rx) = tokio::sync::watch::channel(None);
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_admin_lease(AdminModeLease::testing_with_health(fatal_rx))
        .with_admin_stop_error("secret-cleanup-error");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    fatal_tx
        .send(Some(
            "administrator exec broker stopped unexpectedly".to_owned(),
        ))
        .unwrap();
    let error = handle.wait_for_codex_exit().await.unwrap_err();

    assert!(error.to_string().contains("exec broker"));
    assert!(error.to_string().contains("secret-cleanup-error"));
    assert!(events.lock().unwrap().contains(&"stop-admin".to_owned()));
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("exec")
    );
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
    assert!(!status.message.contains("secret-cleanup"));
}

#[tokio::test]
async fn administrator_mode_failure_never_launches_codex_and_is_secret_free() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_admin_start_error("computer_use: secret-proof-must-not-leak");

    let error = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("computer_use"));
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.starts_with("launch:"))
    );
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("computer_use")
    );
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("secret-proof")
    );
}

#[tokio::test]
async fn administrator_mode_stops_when_waiting_for_codex_errors() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_wait_error("wait failed with secret-wait-must-not-leak");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    assert!(handle.wait_for_codex_exit().await.is_err());
    assert!(events.lock().unwrap().contains(&"stop-admin".to_string()));
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "failed");
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("codex_wait")
    );
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
    assert!(!status.message.contains("secret-wait"));
}

#[tokio::test]
async fn administrator_mode_cleanup_error_is_reported_and_secret_free() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events)
        .with_settings(settings)
        .with_admin_stop_error("cleanup failed with secret-cleanup-must-not-leak");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    let error = handle.wait_for_codex_exit().await.unwrap_err();

    assert!(error.to_string().contains("secret-cleanup"));
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "failed");
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("cleanup")
    );
    assert!(!status.administrator_mode.exec_elevated);
    assert!(!status.administrator_mode.computer_use_elevated);
    assert!(!status.message.contains("secret-cleanup"));
}

#[tokio::test]
async fn administrator_mode_cleanup_task_failure_is_persisted() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events)
        .with_settings(settings)
        .with_admin_stop_abort();
    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();

    assert!(handle.wait_for_codex_exit().await.is_err());
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.status, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("cleanup")
    );
    assert!(!status.message.contains("secret-cleanup-task"));
}

#[tokio::test]
async fn administrator_mode_wait_and_cleanup_errors_are_combined() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events)
        .with_settings(settings)
        .with_wait_error("secret-wait-error")
        .with_admin_stop_error("secret-cleanup-error");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: status_store.clone(),
        },
        &hooks,
    )
    .await
    .unwrap();
    let error = handle.wait_for_codex_exit().await.unwrap_err().to_string();

    assert!(error.contains("secret-wait-error"));
    assert!(error.contains("secret-cleanup-error"));
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("cleanup")
    );
    assert!(!status.message.contains("secret-wait"));
    assert!(!status.message.contains("secret-cleanup"));
}

#[tokio::test]
async fn dropping_launch_handle_restores_administrator_environment() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let temp = tempfile::tempdir().unwrap();
    let original = b"[user]\nvalue = 'preserve me'\n";
    let (lease, environment_path) = administrator_environment_lease(&temp, original);
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events)
        .with_settings(settings)
        .with_admin_lease(lease);

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: StatusStore::new(temp.path().join("status.json")),
        },
        &hooks,
    )
    .await
    .unwrap();
    assert_ne!(std::fs::read(&environment_path).unwrap(), original);

    drop(handle);

    assert_eq!(std::fs::read(environment_path).unwrap(), original);
}

#[tokio::test]
async fn cancelling_wait_during_cleanup_still_restores_environment() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let temp = tempfile::tempdir().unwrap();
    let original = b"original environment\n";
    let (lease, environment_path) = administrator_environment_lease(&temp, original);
    let cleanup_started = Arc::new(Notify::new());
    let cleanup_release = Arc::new(Notify::new());
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events)
        .with_settings(settings)
        .with_admin_lease(lease)
        .with_admin_stop_gate(cleanup_started.clone(), cleanup_release.clone());
    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: StatusStore::new(temp.path().join("status.json")),
        },
        &hooks,
    )
    .await
    .unwrap();

    let wait = handle.wait_for_codex_exit();
    tokio::pin!(wait);
    tokio::select! {
        _ = cleanup_started.notified() => {}
        result = &mut wait => panic!("wait completed before cleanup gate: {result:?}"),
    }
    drop(wait);
    cleanup_release.notify_one();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read(&environment_path).unwrap() == original {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached cleanup task must finish after wait cancellation");
}

#[tokio::test]
async fn concurrent_waits_start_exactly_one_administrator_cleanup() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let temp = tempfile::tempdir().unwrap();
    let original = b"original environment\n";
    let (lease, environment_path) = administrator_environment_lease(&temp, original);
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_admin_lease(lease);
    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: StatusStore::new(temp.path().join("status.json")),
        },
        &hooks,
    )
    .await
    .unwrap();

    let (first, second) = tokio::join!(handle.wait_for_codex_exit(), handle.wait_for_codex_exit());

    first.unwrap();
    second.unwrap();
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.as_str() == "stop-admin")
            .count(),
        1
    );
    assert_eq!(std::fs::read(environment_path).unwrap(), original);
}

fn administrator_environment_lease(
    temp: &tempfile::TempDir,
    original: &[u8],
) -> (AdminModeLease, PathBuf) {
    let codex_home = temp.path().join("codex-home");
    let state_dir = temp.path().join("state");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    let environment_path = codex_home.join("environments.toml");
    std::fs::write(&environment_path, original).unwrap();
    let shim_path = temp.path().join("codex-plus-admin-shim.exe");
    let proof_path = state_dir.join("administrator-mode-computer-use.v1.proof");
    let transaction = AdminEnvironmentTransaction::install(
        &codex_home,
        &state_dir,
        &AdminEnvironmentSpec {
            shim_path: &shim_path,
            pipe_name: r"\\.\pipe\codex-plus-admin-test",
            session_id: "test-session",
            proof_path: &proof_path,
        },
    )
    .unwrap();
    (
        AdminModeLease::testing_with_environment(transaction),
        environment_path,
    )
}

#[tokio::test]
async fn administrator_existing_window_polling_activates_a_delayed_process() {
    let mut polls = 0;
    let mut waits = 0;

    let process_id = activate_existing_administrator_session_with(
        4,
        || {
            polls += 1;
            if polls < 3 { Vec::new() } else { vec![42] }
        },
        |process_id| process_id == 42,
        || {
            waits += 1;
            std::future::ready(())
        },
    )
    .await
    .unwrap();

    assert_eq!(process_id, 42);
    assert_eq!(polls, 3);
    assert_eq!(waits, 2);
}

#[tokio::test]
async fn administrator_existing_window_polling_retries_failed_activation() {
    let mut activations = 0;

    let process_id = activate_existing_administrator_session_with(
        3,
        || vec![42],
        |_| {
            activations += 1;
            activations == 3
        },
        || std::future::ready(()),
    )
    .await
    .unwrap();

    assert_eq!(process_id, 42);
    assert_eq!(activations, 3);
}

#[tokio::test]
async fn administrator_existing_window_polling_times_out_without_a_window() {
    let mut polls = 0;
    let mut waits = 0;

    let error = activate_existing_administrator_session_with(
        3,
        || {
            polls += 1;
            Vec::new()
        },
        |_| false,
        || {
            waits += 1;
            std::future::ready(())
        },
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("before deadline"));
    assert_eq!(polls, 3);
    assert_eq!(waits, 2);
}

#[tokio::test]
async fn administrator_mode_stops_when_official_activation_fails() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let status_dir = tempfile::tempdir().unwrap();
    let status_store = StatusStore::new(status_dir.path().join("status.json"));
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = true;
    let hooks = FakeHooks::new(events.clone())
        .with_settings(settings)
        .with_launch_error("activation failed with secret-proof-must-not-leak");

    assert!(
        launch_and_inject_with_hooks(
            LaunchOptions {
                app_dir: Some(PathBuf::from("/Applications/Codex.app")),
                debug_port: 9229,
                helper_port: 57321,
                status_store: status_store.clone(),
            },
            &hooks,
        )
        .await
        .is_err()
    );
    let events = events.lock().unwrap();
    let launch = events
        .iter()
        .position(|event| event.starts_with("launch:"))
        .unwrap();
    let stop = events
        .iter()
        .position(|event| event == "stop-admin")
        .unwrap();
    assert!(launch < stop);
    let status = status_store.load_latest().unwrap().unwrap();
    assert_eq!(status.administrator_mode.state, "failed");
    assert_eq!(
        status.administrator_mode.error_component.as_deref(),
        Some("activation")
    );
    assert!(!status.message.contains("secret-proof"));
}

#[tokio::test]
async fn administrator_mode_disabled_never_touches_admin_hooks() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let temp = tempfile::tempdir().unwrap();
    let environments = temp.path().join("environments.toml");
    let transport = temp.path().join("helper_transport.js");
    let environments_before = b"default = 'official'\n";
    let transport_before = b"export const transport = 'official';\n";
    std::fs::write(&environments, environments_before).unwrap();
    std::fs::write(&transport, transport_before).unwrap();
    let mut settings = BackendSettings::default();
    settings.administrator_mode_enabled = false;
    settings.enhancements_enabled = false;
    let hooks = FakeHooks::new(events.clone()).with_settings(settings);

    launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: StatusStore::new(tempfile::tempdir().unwrap().path().join("status.json")),
        },
        &hooks,
    )
    .await
    .unwrap();
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.contains("admin"))
    );
    assert_eq!(std::fs::read(environments).unwrap(), environments_before);
    assert_eq!(std::fs::read(transport).unwrap(), transport_before);
}

#[tokio::test]
async fn launch_continues_when_plugin_marketplace_config_fails() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let hooks = FakeHooks::new(events.clone())
        .with_plugin_marketplace_error("config.toml TOML parse failed");

    let handle = launch_and_inject_with_hooks(
        LaunchOptions {
            app_dir: Some(PathBuf::from("/Applications/Codex.app")),
            debug_port: 9229,
            helper_port: 57321,
            status_store: StatusStore::new(tempfile::tempdir().unwrap().path().join("status.json")),
        },
        &hooks,
    )
    .await
    .unwrap();

    assert_eq!(handle.debug_port, 9229);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        [
            "select-debug:9229",
            "select-helper:57321",
            "load-settings",
            "plugin-marketplace",
            "start-helper:57321",
            "launch:9229",
            "inject:9229:57321",
            "status:running"
        ]
    );
}

#[test]
fn launcher_macos_cleanup_command_targets_specific_app_bundle() {
    let command = build_macos_cleanup_command(
        Path::new("/Applications/OpenAI Codex.app"),
        MacosCleanupPolicy::QuitIfNotPreviouslyRunning,
    )
    .expect("cleanup command should be allowed");

    assert_eq!(command[0], "osascript");
    assert!(command.iter().any(|part| part.contains("OpenAI Codex")));
    assert!(!command.iter().any(|part| part == "Codex"));
}

#[test]
fn launcher_macos_cleanup_is_skipped_when_app_was_already_running() {
    let command = build_macos_cleanup_command(
        Path::new("/Applications/OpenAI Codex.app"),
        MacosCleanupPolicy::SkipQuitBecauseAlreadyRunning,
    );

    assert_eq!(command, None);
}

#[cfg(target_os = "macos")]
#[test]
fn launcher_macos_debug_launch_starts_when_app_is_not_running() {
    assert_eq!(
        select_macos_debug_launch_action(false, false),
        MacosDebugLaunchAction::LaunchNew
    );
}

#[cfg(target_os = "macos")]
#[test]
fn launcher_macos_debug_launch_reuses_existing_codex_cdp_instance() {
    assert_eq!(
        select_macos_debug_launch_action(true, true),
        MacosDebugLaunchAction::ReuseRunningDebugApp
    );
}

#[cfg(target_os = "macos")]
#[test]
fn launcher_macos_debug_launch_restarts_existing_non_cdp_instance() {
    assert_eq!(
        select_macos_debug_launch_action(true, false),
        MacosDebugLaunchAction::RestartRunningApp
    );
}

#[tokio::test]
async fn default_launch_hooks_provider_sync_enabled_returns_explicit_error() {
    let error = DefaultLaunchHooks::default()
        .run_provider_sync()
        .await
        .expect_err("default provider sync should not silently skip");

    assert!(
        error
            .to_string()
            .contains("provider sync requires launcher hooks")
    );
}

#[test]
fn paused_dream_skin_does_not_reapply_the_native_base_theme_on_launch() {
    let source =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/launcher.rs")).unwrap();

    assert!(source.contains("!settings.codex_app_dream_skin_paused"));
}

#[derive(Clone)]
struct FakeHooks {
    events: Arc<Mutex<Vec<String>>>,
    settings: BackendSettings,
    launch_result: CodexLaunch,
    launch_error: Option<String>,
    inject_error: Option<String>,
    provider_sync_unsupported: bool,
    plugin_marketplace_error: Option<String>,
    admin_start_error: Option<String>,
    admin_stop_error: Option<String>,
    admin_stop_abort: bool,
    admin_lease: Arc<Mutex<Option<AdminModeLease>>>,
    admin_stop_started: Option<Arc<Notify>>,
    admin_stop_release: Option<Arc<Notify>>,
    injection_started: Option<Arc<Notify>>,
    injection_release: Option<Arc<Notify>>,
    wait_error: Option<String>,
    has_pending_remote_control_session_recoveries: bool,
    /// 还需要让 `start_helper` 报几次「端口被占用」，用来模拟旧 helper 尚未交还监听。
    remaining_helper_bind_conflicts: Arc<Mutex<u32>>,
}

impl FakeHooks {
    fn new(events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            events,
            settings: BackendSettings::default(),
            launch_result: CodexLaunch::Process {
                command: vec!["codex".to_string()],
                wait_strategy: codex_plus_core::launcher::ProcessWaitStrategy::TrackedChild,
                macos_cleanup_policy: None,
            },
            launch_error: None,
            inject_error: None,
            provider_sync_unsupported: false,
            plugin_marketplace_error: None,
            admin_start_error: None,
            admin_stop_error: None,
            admin_stop_abort: false,
            admin_lease: Arc::new(Mutex::new(None)),
            admin_stop_started: None,
            admin_stop_release: None,
            injection_started: None,
            injection_release: None,
            wait_error: None,
            has_pending_remote_control_session_recoveries: false,
            remaining_helper_bind_conflicts: Arc::new(Mutex::new(0)),
        }
    }

    fn with_helper_bind_conflicts(self, conflicts: u32) -> Self {
        *self.remaining_helper_bind_conflicts.lock().unwrap() = conflicts;
        self
    }

    fn with_settings(mut self, settings: BackendSettings) -> Self {
        self.settings = settings;
        self
    }

    fn with_launch_result(mut self, launch_result: CodexLaunch) -> Self {
        self.launch_result = launch_result;
        self
    }

    fn with_inject_error(mut self, message: &str) -> Self {
        self.inject_error = Some(message.to_string());
        self
    }

    fn with_launch_error(mut self, message: &str) -> Self {
        self.launch_error = Some(message.to_string());
        self
    }

    fn with_provider_sync_unsupported(mut self) -> Self {
        self.provider_sync_unsupported = true;
        self
    }

    fn with_plugin_marketplace_error(mut self, message: &str) -> Self {
        self.plugin_marketplace_error = Some(message.to_string());
        self
    }

    fn with_admin_start_error(mut self, message: &str) -> Self {
        self.admin_start_error = Some(message.to_string());
        self
    }

    fn with_wait_error(mut self, message: &str) -> Self {
        self.wait_error = Some(message.to_string());
        self
    }

    fn with_admin_stop_error(mut self, message: &str) -> Self {
        self.admin_stop_error = Some(message.to_string());
        self
    }

    fn with_admin_stop_abort(mut self) -> Self {
        self.admin_stop_abort = true;
        self
    }

    fn with_admin_lease(self, lease: AdminModeLease) -> Self {
        *self.admin_lease.lock().unwrap() = Some(lease);
        self
    }

    fn with_admin_stop_gate(mut self, started: Arc<Notify>, release: Arc<Notify>) -> Self {
        self.admin_stop_started = Some(started);
        self.admin_stop_release = Some(release);
        self
    }

    fn with_injection_gate(mut self, started: Arc<Notify>, release: Arc<Notify>) -> Self {
        self.injection_started = Some(started);
        self.injection_release = Some(release);
        self
    }

    fn with_pending_remote_control_session_recoveries(mut self) -> Self {
        self.has_pending_remote_control_session_recoveries = true;
        self
    }

    fn event(&self, event: impl Into<String>) {
        self.events.lock().unwrap().push(event.into());
    }
}

#[async_trait::async_trait(?Send)]
impl LaunchHooks for FakeHooks {
    fn resolve_app_dir(
        &self,
        app_dir: Option<&Path>,
        _settings: &BackendSettings,
    ) -> anyhow::Result<PathBuf> {
        app_dir
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("missing app dir"))
    }

    fn select_debug_port(&self, requested: u16) -> u16 {
        self.event(format!("select-debug:{requested}"));
        requested
    }

    fn select_helper_port(&self, requested: u16) -> u16 {
        self.event(format!("select-helper:{requested}"));
        requested
    }

    async fn load_settings(&self) -> anyhow::Result<BackendSettings> {
        self.event("load-settings");
        Ok(self.settings.clone())
    }

    async fn run_provider_sync(&self) -> anyhow::Result<()> {
        self.event("provider-sync");
        if self.provider_sync_unsupported {
            anyhow::bail!("provider sync requires launcher hooks");
        }
        Ok(())
    }

    fn has_pending_remote_control_session_recoveries(&self) -> bool {
        self.has_pending_remote_control_session_recoveries
    }

    async fn run_remote_control_session_recovery(&self) -> anyhow::Result<()> {
        self.event("remote-control-session-recovery");
        Ok(())
    }

    async fn apply_active_relay_profile(&self, settings: &BackendSettings) -> anyhow::Result<()> {
        if !settings.relay_profiles_enabled {
            return Ok(());
        }
        self.event("apply-relay");
        Ok(())
    }

    async fn ensure_plugin_marketplace_config(
        &self,
        _settings: &BackendSettings,
    ) -> anyhow::Result<()> {
        if let Some(message) = &self.plugin_marketplace_error {
            self.event("plugin-marketplace");
            anyhow::bail!(message.clone());
        }
        Ok(())
    }

    async fn start_helper(&self, helper_port: u16) -> anyhow::Result<()> {
        {
            let mut remaining = self.remaining_helper_bind_conflicts.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                self.event(format!("start-helper-busy:{helper_port}"));
                // 与真实 `start_helper` 一样把 io::Error 包在 context 下面，
                // 这样重试逻辑对错误链的判定也一并被测到。
                return Err(anyhow::Error::new(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "address already in use",
                ))
                .context(format!(
                    "failed to bind helper runtime on 127.0.0.1:{helper_port}"
                )));
            }
        }
        self.event(format!("start-helper:{helper_port}"));
        Ok(())
    }

    async fn start_administrator_mode(
        &self,
        settings: &BackendSettings,
        _app_dir: &Path,
    ) -> anyhow::Result<Option<AdminModeLease>> {
        if !settings.administrator_mode_enabled {
            return Ok(None);
        }
        self.event("start-admin");
        if let Some(message) = &self.admin_start_error {
            anyhow::bail!(message.clone());
        }
        Ok(Some(
            self.admin_lease
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(AdminModeLease::testing),
        ))
    }

    fn stop_administrator_mode(
        &self,
        lease: AdminModeLease,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        let events = Arc::clone(&self.events);
        let error = self.admin_stop_error.clone();
        let abort = self.admin_stop_abort;
        let started = self.admin_stop_started.clone();
        let release = self.admin_stop_release.clone();
        let task = tokio::spawn(async move {
            events.lock().unwrap().push("stop-admin".to_string());
            if let Some(started) = started {
                started.notify_one();
            }
            if let Some(release) = release {
                release.notified().await;
            }
            let cleanup = lease.shutdown().await.map(|_| ());
            if let Some(message) = error {
                anyhow::bail!(message);
            }
            cleanup
        });
        if abort {
            task.abort();
        }
        task
    }

    async fn launch_codex(
        &self,
        app_dir: &Path,
        debug_port: u16,
        settings: &BackendSettings,
        extra_args: &[String],
    ) -> anyhow::Result<CodexLaunch> {
        assert!(app_dir.ends_with("Codex.app"));
        let launch_detail = if extra_args.is_empty() {
            format!("launch:{debug_port}")
        } else {
            format!("launch:{debug_port}:{}", extra_args.join(","))
        };
        if settings.codex_app_native_menu_localization {
            self.event(launch_detail);
        } else {
            self.event(format!("{launch_detail}:native-menu-off"));
        }
        if let Some(message) = &self.launch_error {
            anyhow::bail!(message.clone());
        }
        Ok(self.launch_result.clone())
    }

    async fn inject(&self, debug_port: u16, helper_port: u16) -> anyhow::Result<()> {
        self.event(format!("inject:{debug_port}:{helper_port}"));
        if let Some(message) = &self.inject_error {
            anyhow::bail!(message.clone());
        }
        Ok(())
    }

    async fn ensure_injection(&self, debug_port: u16, helper_port: u16, _app_dir: &Path) -> bool {
        self.event(format!("inject:{debug_port}:{helper_port}"));
        if let Some(started) = &self.injection_started {
            started.notify_one();
        }
        if let Some(release) = &self.injection_release {
            release.notified().await;
        }
        self.inject_error.is_none()
    }

    async fn start_bridge_watchdog(
        &self,
        _debug_port: u16,
        _helper_port: u16,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn write_status(&self, status: &str) {
        self.event(format!("status:{status}"));
    }

    async fn wait_for_codex_exit(
        &self,
        _launch: &CodexLaunch,
        _debug_port: u16,
    ) -> anyhow::Result<()> {
        self.event("wait-codex");
        if let Some(message) = &self.wait_error {
            anyhow::bail!(message.clone());
        }
        Ok(())
    }

    async fn shutdown_helper(&self, helper_port: u16) {
        self.event(format!("shutdown-helper:{helper_port}"));
    }

    async fn terminate_codex(&self, launch: &CodexLaunch) {
        if let Some(process_id) = launch.process_id() {
            self.event(format!("terminate-packaged:{process_id}"));
        } else {
            self.event("terminate-codex");
        }
    }
}
