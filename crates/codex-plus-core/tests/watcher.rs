use codex_plus_core::watcher::{
    build_spawn_launcher_command, build_watcher_install_plan, cdp_listening, codex_process_ids,
    disable_watcher_at, enable_watcher_at, filter_killable_launcher_processes,
    process_id_is_running, process_ids_still_running, should_recover_stale_launcher,
    watcher_disabled_flag,
};

#[cfg(windows)]
use codex_plus_core::watcher::{
    WindowsProcessInfo, admin_recovery_process_ids_from_snapshot,
    find_codex_processes_from_snapshot,
    find_session_index_cleanup_blocking_processes_from_snapshot,
    stop_admin_recovery_processes_with_hooks, stop_windows_process_id_and_wait,
};

#[cfg(windows)]
use std::cell::{Cell, RefCell};
#[cfg(windows)]
use std::collections::VecDeque;
#[cfg(windows)]
use std::time::Duration;

#[test]
fn cdp_listening_returns_true_for_bound_loopback_port() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();

    assert!(cdp_listening(port));
}

#[test]
fn cdp_listening_returns_true_for_bound_ipv6_loopback_port() {
    let listener = std::net::TcpListener::bind("[::1]:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    assert!(cdp_listening(port));
}

#[test]
fn cdp_listening_returns_false_for_closed_port() {
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };

    assert!(!cdp_listening(port));
}

#[test]
fn watcher_enable_and_disable_toggle_flag() {
    let dir = tempfile::tempdir().unwrap();
    let flag = watcher_disabled_flag(dir.path());

    disable_watcher_at(dir.path()).unwrap();
    assert!(flag.exists());

    enable_watcher_at(dir.path()).unwrap();
    assert!(!flag.exists());
}

#[test]
fn watcher_install_plan_registers_rust_launcher_at_logon() {
    let plan = build_watcher_install_plan("C:/Tools/codex-plus-plus.exe".into(), 9333);

    assert_eq!(plan.run_value_name, "CodexPlusPlusWatcher");
    assert_eq!(
        plan.run_value,
        "\"C:/Tools/codex-plus-plus.exe\" --debug-port 9333"
    );
    assert_eq!(plan.shortcut_name, "CodexPlusPlusWatcher.lnk");
    assert_eq!(plan.shortcut_target, "C:/Tools/codex-plus-plus.exe");
    assert_eq!(plan.shortcut_arguments, "--debug-port 9333");
}

#[test]
fn spawn_launcher_command_points_to_silent_binary_only() {
    let command = build_spawn_launcher_command("C:/Tools/codex-plus-plus.exe", 9444);

    assert_eq!(command[0], "C:/Tools/codex-plus-plus.exe");
    assert!(command.contains(&"--debug-port".to_string()));
    assert!(command.contains(&"9444".to_string()));
    assert!(!command.iter().any(|part| part.contains("manager")));
}

#[test]
fn codex_process_filter_keeps_only_windowsapps_codex_processes() {
    let processes = [
        (
            11,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0.0.0_x64__abc\app\Codex.exe",
        ),
        (12, r"C:\Tools\Codex.exe"),
        (
            13,
            r"C:\Program Files\WindowsApps\Other.App_1.0.0.0_x64__abc\app\Codex.exe",
        ),
    ];

    assert_eq!(codex_process_ids(processes), vec![11]);
}

#[test]
fn codex_process_filter_keeps_chatgpt_desktop_package_processes() {
    let processes = [
        (
            21,
            r"C:\Program Files\WindowsApps\OpenAI.ChatGPT-Desktop_1.2026.133.0_x64__abc\app\ChatGPT.exe",
        ),
        (
            22,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__abc\app\ChatGPT.exe",
        ),
        (
            23,
            r"C:\Program Files\WindowsApps\OpenAI.ChatGPT-Desktop_1.2026.133.0_x64__abc\app\resources\ChatGPT.exe",
        ),
        (
            24,
            r"C:\Program Files\WindowsApps\Other.ChatGPT_1.0.0.0_x64__abc\app\ChatGPT.exe",
        ),
    ];

    assert_eq!(codex_process_ids(processes), vec![21, 22]);
}

#[test]
fn launcher_process_filter_protects_current_process_ancestry() {
    let processes = [
        (10, 0, "codex-plus-plus.exe"),
        (20, 10, "codex-plus-plus.exe"),
        (30, 20, "codex-plus-plus.exe"),
        (40, 10, "codex-plus-plus.exe"),
        (50, 10, "codex-plus-plus-manager.exe"),
    ];

    assert_eq!(filter_killable_launcher_processes(processes, 30), vec![40]);
}

#[test]
fn stale_launcher_recovery_only_runs_when_codex_and_cdp_are_absent() {
    assert!(should_recover_stale_launcher(false, false));
    assert!(!should_recover_stale_launcher(true, false));
    assert!(!should_recover_stale_launcher(false, true));
    assert!(!should_recover_stale_launcher(true, true));
}

#[test]
fn stop_wait_tracks_only_expected_process_ids() {
    assert_eq!(
        process_ids_still_running(&[10, 20, 30], [5, 20, 40, 30]),
        vec![20, 30]
    );
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[test]
fn process_liveness_distinguishes_current_and_missing_processes() {
    assert_eq!(process_id_is_running(std::process::id()), Some(true));
    assert_eq!(process_id_is_running(u32::MAX), Some(false));
}

#[cfg(windows)]
#[test]
fn find_codex_processes_finds_local_install_with_capitial_c() {
    let processes = [WindowsProcessInfo {
        process_id: 42,
        parent_process_id: 0,
        exe_file: "Codex.exe".to_string(),
        executable_path: Some(std::path::PathBuf::from(
            r"D:\360Downloads\codexapp\app\Codex.exe",
        )),
    }];

    assert_eq!(find_codex_processes_from_snapshot(&processes), vec![42]);
}

#[cfg(windows)]
#[test]
fn find_codex_processes_ignores_lowercase_local_cli_binary() {
    let processes = [WindowsProcessInfo {
        process_id: 43,
        parent_process_id: 0,
        exe_file: "codex.exe".to_string(),
        executable_path: Some(std::path::PathBuf::from(
            r"D:\360Downloads\codexapp\app\codex.exe",
        )),
    }];

    assert!(find_codex_processes_from_snapshot(&processes).is_empty());
}

#[cfg(windows)]
#[test]
fn find_codex_processes_ignores_npm_cli_binary() {
    let processes = [WindowsProcessInfo {
        process_id: 44,
        parent_process_id: 0,
        exe_file: "codex.exe".to_string(),
        executable_path: Some(std::path::PathBuf::from(
            r"C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe",
        )),
    }];

    assert!(find_codex_processes_from_snapshot(&processes).is_empty());
}

#[cfg(windows)]
#[test]
fn find_codex_processes_ignores_packaged_resource_cli_binary() {
    let processes = [WindowsProcessInfo {
        process_id: 45,
        parent_process_id: 0,
        exe_file: "codex.exe".to_string(),
        executable_path: Some(std::path::PathBuf::from(
            r"C:\Program Files\WindowsApps\OpenAI.Codex_1.0.0.0_x64__abc\app\resources\codex.exe",
        )),
    }];

    assert!(find_codex_processes_from_snapshot(&processes).is_empty());
}

#[cfg(windows)]
#[test]
fn find_codex_processes_combines_store_and_local_installs() {
    let processes = [
        WindowsProcessInfo {
            process_id: 11,
            parent_process_id: 0,
            exe_file: "ChatGPT.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(
                r"C:\Program Files\WindowsApps\OpenAI.ChatGPT-Desktop_1.2026.133.0_x64__abc\app\ChatGPT.exe",
            )),
        },
        WindowsProcessInfo {
            process_id: 42,
            parent_process_id: 0,
            exe_file: "Codex.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(
                r"D:\360Downloads\codexapp\app\Codex.exe",
            )),
        },
    ];

    assert_eq!(find_codex_processes_from_snapshot(&processes), vec![11, 42]);
}

#[cfg(windows)]
#[test]
fn session_index_cleanup_process_guard_blocks_desktop_apps_but_not_cli() {
    let processes = [
        WindowsProcessInfo {
            process_id: 11,
            parent_process_id: 0,
            exe_file: "ChatGPT.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(
                r"C:\Program Files\WindowsApps\OpenAI.ChatGPT-Desktop_1.2026.133.0_x64__abc\app\ChatGPT.exe",
            )),
        },
        WindowsProcessInfo {
            process_id: 12,
            parent_process_id: 0,
            exe_file: "ChatGPT.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(r"D:\Portable\ChatGPT\ChatGPT.exe")),
        },
        WindowsProcessInfo {
            process_id: 13,
            parent_process_id: 0,
            exe_file: "Codex.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(r"D:\Portable\Codex\Codex.exe")),
        },
        WindowsProcessInfo {
            process_id: 14,
            parent_process_id: 0,
            exe_file: "codex.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(
                r"C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.exe",
            )),
        },
    ];

    assert_eq!(
        find_session_index_cleanup_blocking_processes_from_snapshot(&processes),
        vec![11, 12, 13]
    );
    assert_eq!(find_codex_processes_from_snapshot(&processes), vec![11, 13]);
}

#[cfg(windows)]
#[test]
fn find_codex_processes_ignores_unrelated_processes() {
    let processes = [
        WindowsProcessInfo {
            process_id: 10,
            parent_process_id: 0,
            exe_file: "notepad.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(r"C:\Windows\notepad.exe")),
        },
        WindowsProcessInfo {
            process_id: 20,
            parent_process_id: 0,
            exe_file: "codex-plus-plus.exe".to_string(),
            executable_path: Some(std::path::PathBuf::from(
                r"D:\Programs\Codex++\codex-plus-plus.exe",
            )),
        },
    ];

    assert!(find_codex_processes_from_snapshot(&processes).is_empty());
}

#[cfg(windows)]
#[test]
fn admin_recovery_process_filter_matches_exact_names_and_excludes_current_pid() {
    let processes = [
        WindowsProcessInfo {
            process_id: 40,
            parent_process_id: 0,
            exe_file: "CHATGPT.EXE".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 41,
            parent_process_id: 0,
            exe_file: "codex-plus-plus-manager.exe".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 42,
            parent_process_id: 0,
            exe_file: "codex-plus-plus-manager.exe".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 43,
            parent_process_id: 0,
            exe_file: "codex-plus-plus.exe".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 44,
            parent_process_id: 0,
            exe_file: "codex-plus-plus-manager.exe.bak".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 45,
            parent_process_id: 0,
            exe_file: "notepad.exe".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 46,
            parent_process_id: 0,
            exe_file: "codex-plus-recovery.exe".to_string(),
            executable_path: None,
        },
        WindowsProcessInfo {
            process_id: 47,
            parent_process_id: 0,
            exe_file: "codex-plus-admin-shim.exe".to_string(),
            executable_path: None,
        },
    ];

    assert_eq!(
        admin_recovery_process_ids_from_snapshot(&processes, 46),
        vec![43, 41, 42, 47, 40]
    );
}

#[cfg(windows)]
#[test]
fn strict_windows_process_stop_terminates_exact_temporary_pid() {
    let mut child = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 30",
        ])
        .spawn()
        .expect("spawn harmless temporary process");
    let process_id = child.id();

    let result = stop_windows_process_id_and_wait(process_id);
    if result.is_err() {
        let _ = child.kill();
    }
    result.expect("terminate and wait for exact temporary process");
    assert!(child.try_wait().expect("reap temporary process").is_some());
}

#[cfg(windows)]
#[test]
fn strict_windows_process_stop_accepts_natural_exit_before_termination() {
    let mut child = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "exit 0",
        ])
        .spawn()
        .expect("spawn short-lived temporary process");
    let process_id = child.id();
    child.wait().expect("wait for temporary process to exit");

    stop_windows_process_id_and_wait(process_id)
        .expect("a target that exited before termination is already stopped");
}

#[cfg(windows)]
#[test]
fn strict_windows_process_stop_releases_running_exe_for_overwrite() {
    let system_exe = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .expect("Windows directory")
        .join("System32")
        .join("ping.exe");
    assert!(
        system_exe.is_file(),
        "trusted system fixture exists: {system_exe:?}"
    );

    let temp_dir = tempfile::tempdir().expect("temporary fixture directory");
    let target_exe = temp_dir.path().join("codex-plus-plus-manager.exe");
    std::fs::copy(&system_exe, &target_exe).expect("stage trusted system fixture");
    let expected_bytes = std::fs::read(&system_exe).expect("read trusted system fixture");

    let mut child = std::process::Command::new(&target_exe)
        .args(["-t", "127.0.0.1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn copied executable fixture");
    let process_id = child.id();
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        child
            .try_wait()
            .expect("check copied executable fixture")
            .is_none(),
        "copied executable fixture must remain running"
    );

    let locked_overwrite = std::fs::copy(&system_exe, &target_exe);
    let stop_result = stop_windows_process_id_and_wait(process_id);
    if stop_result.is_err() {
        let _ = child.kill();
    }
    stop_result.expect("stop copied executable fixture");
    assert!(
        child
            .try_wait()
            .expect("reap copied executable fixture")
            .is_some(),
        "stopped copied executable fixture must exit"
    );

    assert!(
        locked_overwrite.is_err(),
        "running executable image should reject overwrite"
    );
    std::fs::copy(&system_exe, &target_exe).expect("overwrite released executable image");
    assert_eq!(
        std::fs::read(&target_exe).expect("read overwritten executable image"),
        expected_bytes
    );
}

#[cfg(windows)]
#[test]
fn strict_windows_process_stop_rejects_current_and_invalid_pid() {
    let current_process_id = std::process::id();
    assert!(stop_windows_process_id_and_wait(current_process_id).is_err());
    assert!(stop_windows_process_id_and_wait(0).is_err());
    assert_eq!(std::process::id(), current_process_id);
}

#[cfg(windows)]
fn recovery_fixture_process(process_id: u32, exe_file: &str) -> WindowsProcessInfo {
    WindowsProcessInfo {
        process_id,
        parent_process_id: 0,
        exe_file: exe_file.to_string(),
        executable_path: None,
    }
}

#[cfg(windows)]
fn recovery_fixture_snapshot(
    current_process_id: u32,
    targets: &[(u32, &str)],
) -> Vec<WindowsProcessInfo> {
    let mut snapshot = vec![recovery_fixture_process(
        current_process_id,
        "watcher-test.exe",
    )];
    snapshot.extend(
        targets
            .iter()
            .map(|(process_id, exe_file)| recovery_fixture_process(*process_id, exe_file)),
    );
    snapshot
}

#[cfg(windows)]
#[test]
fn admin_recovery_retries_a_failed_termination_on_the_next_round() {
    let current_process_id = std::process::id();
    let mut snapshots = VecDeque::from([
        recovery_fixture_snapshot(current_process_id, &[(101, "codex-plus-plus.exe")]),
        recovery_fixture_snapshot(current_process_id, &[(101, "codex-plus-plus.exe")]),
        recovery_fixture_snapshot(current_process_id, &[]),
        recovery_fixture_snapshot(current_process_id, &[]),
    ]);
    let calls = RefCell::new(Vec::new());
    let attempts = Cell::new(0);

    let result = stop_admin_recovery_processes_with_hooks(
        current_process_id,
        move || snapshots.pop_front().expect("scripted recovery snapshot"),
        |process_id| {
            calls.borrow_mut().push(process_id);
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            attempt >= 2
        },
        |_| {},
        || Duration::ZERO,
        Duration::from_secs(1),
    );

    assert!(
        result.is_ok(),
        "recovery should retry a transient termination failure"
    );
    assert_eq!(&*calls.borrow(), &[101, 101]);
}

#[cfg(windows)]
#[test]
fn admin_recovery_treats_a_pid_that_disappears_before_retry_as_success() {
    let current_process_id = std::process::id();
    let mut snapshots = VecDeque::from([
        recovery_fixture_snapshot(current_process_id, &[(102, "codex-plus-plus.exe")]),
        recovery_fixture_snapshot(current_process_id, &[]),
        recovery_fixture_snapshot(current_process_id, &[]),
    ]);
    let calls = RefCell::new(Vec::new());

    let result = stop_admin_recovery_processes_with_hooks(
        current_process_id,
        move || snapshots.pop_front().expect("scripted recovery snapshot"),
        |process_id| {
            calls.borrow_mut().push(process_id);
            false
        },
        |_| {},
        || Duration::ZERO,
        Duration::from_secs(1),
    );

    assert!(
        result.is_ok(),
        "a naturally exited target is already stopped"
    );
    assert_eq!(&*calls.borrow(), &[102]);
}

#[cfg(windows)]
#[test]
fn admin_recovery_captures_a_restarted_target_with_a_new_pid() {
    let current_process_id = std::process::id();
    let mut snapshots = VecDeque::from([
        recovery_fixture_snapshot(current_process_id, &[(103, "codex-plus-plus.exe")]),
        recovery_fixture_snapshot(current_process_id, &[(104, "codex-plus-plus.exe")]),
        recovery_fixture_snapshot(current_process_id, &[]),
        recovery_fixture_snapshot(current_process_id, &[]),
    ]);
    let calls = RefCell::new(Vec::new());

    let result = stop_admin_recovery_processes_with_hooks(
        current_process_id,
        move || snapshots.pop_front().expect("scripted recovery snapshot"),
        |process_id| {
            calls.borrow_mut().push(process_id);
            true
        },
        |_| {},
        || Duration::ZERO,
        Duration::from_secs(1),
    );

    assert!(
        result.is_ok(),
        "recovery should stop a restarted target too"
    );
    assert_eq!(&*calls.borrow(), &[103, 104]);
}

#[cfg(windows)]
#[test]
fn admin_recovery_requires_two_consecutive_empty_rounds() {
    let current_process_id = std::process::id();
    let mut snapshots = VecDeque::from([
        recovery_fixture_snapshot(current_process_id, &[]),
        recovery_fixture_snapshot(current_process_id, &[]),
    ]);
    let rounds = std::rc::Rc::new(Cell::new(0));
    let rounds_for_enumerator = rounds.clone();
    let sleeps = Cell::new(0);

    let result = stop_admin_recovery_processes_with_hooks(
        current_process_id,
        move || {
            rounds_for_enumerator.set(rounds_for_enumerator.get() + 1);
            snapshots
                .pop_front()
                .expect("scripted empty recovery snapshot")
        },
        |_| panic!("empty rounds must not terminate a process"),
        |_| sleeps.set(sleeps.get() + 1),
        || Duration::ZERO,
        Duration::from_secs(1),
    );

    assert!(result.is_ok());
    assert_eq!(rounds.get(), 2);
    assert_eq!(sleeps.get(), 1);
}

#[cfg(windows)]
#[test]
fn admin_recovery_returns_timeout_error_for_a_persistent_target() {
    let current_process_id = std::process::id();
    let calls = RefCell::new(Vec::new());
    let elapsed_ticks = Cell::new(0);

    let result = stop_admin_recovery_processes_with_hooks(
        current_process_id,
        move || recovery_fixture_snapshot(current_process_id, &[(105, "codex-plus-plus.exe")]),
        |process_id| {
            calls.borrow_mut().push(process_id);
            false
        },
        |_| {},
        || {
            let tick = elapsed_ticks.get() + 1;
            elapsed_ticks.set(tick);
            Duration::from_millis(tick)
        },
        Duration::from_millis(3),
    );

    let error = result.expect_err("persistent target must hit the injected timeout");
    assert!(error.to_string().contains("timed out"));
    assert!(error.to_string().contains("105"));
    assert!(calls.borrow().len() >= 3);
}

#[cfg(windows)]
#[test]
fn force_termination_fallback_uses_absolute_pid_taskkill_without_tree_kill() {
    let source = include_str!("../src/watcher.rs");
    let start = source
        .find("fn force_terminate_process")
        .expect("force termination helper");
    let body = source[start..]
        .split("/// Select only the exact image names")
        .next()
        .expect("force termination helper body");

    assert!(body.contains("if !system_root.is_absolute()"));
    assert!(body.contains(".join(\"System32\")"));
    assert!(body.contains(".join(\"taskkill.exe\")"));
    assert!(body.contains(".args([\"/PID\", process_id_arg.as_str(), \"/F\"])"));
    assert!(body.contains(".creation_flags(CREATE_NO_WINDOW)"));
    assert!(!body.contains("/T"));
}
