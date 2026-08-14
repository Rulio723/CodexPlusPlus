use codex_plus_core::admin_mode::{
    recover_stale_admin_mode,
    windows::{admin_pipe_name, admin_pipe_sddl},
};

#[test]
fn administrator_mode_recovery_preserves_unrelated_codex_state_byte_for_byte() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let state = temp.path().join("state");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let fixtures = [
        (
            home.join("auth.json"),
            b"{\"token\":\"preserve\"}\n".as_slice(),
        ),
        (
            home.join("relay-profiles.json"),
            b"relay-profile-bytes".as_slice(),
        ),
        (
            home.join("provider-profiles.json"),
            b"provider-profile-bytes".as_slice(),
        ),
        (
            home.join("sessions/session.db"),
            b"sqlite-session-bytes".as_slice(),
        ),
        (home.join("rollout.jsonl"), b"rollout-bytes\n".as_slice()),
    ];
    for (path, bytes) in &fixtures {
        std::fs::write(path, bytes).unwrap();
    }

    recover_stale_admin_mode(&home, &state).unwrap();

    for (path, bytes) in fixtures {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn administrator_mode_source_has_no_auth_provider_relay_or_session_storage_dependency() {
    let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/admin_mode");
    let mut source = String::new();
    for entry in std::fs::read_dir(source_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            let contents = std::fs::read_to_string(path).unwrap();
            let production_source = contents
                .split_once("#[cfg(test)]")
                .map(|(production, _)| production)
                .unwrap_or(&contents);
            source.push_str(production_source);
        }
    }
    for forbidden in [
        "auth.json",
        "relay_config",
        "relay_profiles",
        "official_accounts",
        "provider_profiles",
        "codex_sqlite",
        "rollout.jsonl",
        "sessions/",
    ] {
        assert!(
            !source.contains(forbidden),
            "administrator mode must not depend on {forbidden}"
        );
    }
}

#[cfg(windows)]
use codex_plus_core::admin_mode::computer_use::{AdminComputerUseConfig, AdminComputerUseRuntime};

#[cfg(windows)]
use codex_plus_core::admin_mode::exec::{AdminExecConfig, AdminExecRuntime};

#[cfg(windows)]
use codex_plus_core::admin_mode::windows::{
    KillOnCloseJob, current_windows_identity, process_has_high_integrity,
};

#[test]
fn admin_pipe_name_sanitizes_session_id() {
    let pipe_name = admin_pipe_name("A/B C");
    let suffix = pipe_name
        .strip_prefix(r"\\.\pipe\codex-plus-admin-")
        .expect("administrator pipe must use the local named-pipe namespace");

    assert!(!suffix.is_empty());
    assert!(
        suffix
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    );
}

#[test]
fn admin_pipe_name_bounds_long_session_ids_stably() {
    let session_id = "A/B C".repeat(200);

    let first = admin_pipe_name(&session_id);
    let second = admin_pipe_name(&session_id);
    let suffix = first
        .strip_prefix(r"\\.\pipe\codex-plus-admin-")
        .expect("administrator pipe must use the local named-pipe namespace");

    assert_eq!(first, second);
    assert!(first.len() <= 240);
    assert!(
        suffix
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    );
}

#[test]
fn admin_pipe_acl_excludes_world_access() {
    let sddl = admin_pipe_sddl("S-1-5-21-1-2-3-1001")
        .expect("canonical current-user SID must be accepted");

    assert!(sddl.contains("SY"));
    assert!(sddl.contains("BA"));
    assert!(sddl.contains("S-1-5-21-1-2-3-1001"));
    assert!(!sddl.contains("WD"));
    assert!(!sddl.contains("AN"));
}

#[test]
fn admin_pipe_acl_rejects_noncanonical_or_injectable_sids() {
    for sid in [
        "S-1-5-21)(A;;GA;;;WD",
        "S-1-5--21",
        "S-1-five-21",
        "S-256-5-21",
        "S-1-281474976710656-21",
        "S-1-5-4294967296",
        "S-01-5-21",
        " S-1-5-21",
        "S-1-5-21 ",
        "S-1-1-0",
        "S-1-5-7",
        "S-1-5-18",
        "S-1-5-19",
        "S-1-5-20",
        "S-1-5-32-544",
        "S-1-5-99-1-2-3-1001",
    ] {
        assert!(admin_pipe_sddl(sid).is_err(), "SID must be rejected: {sid}");
    }
}

#[cfg(windows)]
#[test]
fn windows_identity_reports_current_token() {
    let identity = current_windows_identity().expect("current Windows identity must be readable");

    assert!(identity.user_sid.starts_with("S-1-"));
    assert!(identity.logon_sid.starts_with("S-1-"));
    assert!(identity.integrity_rid > 0);
    assert!(admin_pipe_sddl(&identity.user_sid).is_ok());

    let high_integrity = process_has_high_integrity(std::process::id())
        .expect("current process integrity must be readable");
    assert_eq!(high_integrity, identity.integrity_rid >= 0x3000);
}

#[cfg(windows)]
#[test]
fn windows_job_accepts_child_process() {
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;
    use windows::Win32::Foundation::HANDLE;

    let job = KillOnCloseJob::new("codex-plus-admin-mode-test")
        .expect("kill-on-close job must be created");
    let mut child = Command::new("cmd")
        .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
        .spawn()
        .expect("test child must start");
    let process = HANDLE(child.as_raw_handle());

    job.assign_process_handle(process)
        .expect("child must be assignable to the job");
    drop(job);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child
            .try_wait()
            .expect("terminated job child status must be readable")
            .is_some()
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("kill-on-close job did not terminate child within 3 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(windows)]
#[tokio::test]
async fn computer_use_admin_rejects_non_official_helper_path_before_start() {
    let temp = tempfile::tempdir().unwrap();
    let helper = temp.path().join("not-official.exe");
    std::fs::write(&helper, b"not an executable").unwrap();
    let transport = temp.path().join("helper_transport.js");
    let descriptor = temp.path().join("descriptor.json");
    std::fs::write(&transport, b"not official transport").unwrap();
    let identity = current_windows_identity().unwrap();
    let job =
        KillOnCloseJob::new(&format!("computer-use-reject-{}", uuid::Uuid::new_v4())).unwrap();
    let result = AdminComputerUseRuntime::start(
        AdminComputerUseConfig {
            home: temp.path(),
            descriptor_path: &descriptor,
            shim_path: &helper,
            helper_exe: &helper,
            helper_transport: &transport,
            pipe_name: &admin_pipe_name("computer-use-reject"),
            session_id: "session",
            session_proof: "proof",
            expected_user_sid: &identity.user_sid,
            expected_logon_sid: &identity.logon_sid,
        },
        &job,
    )
    .await;
    assert!(result.is_err());
}

#[cfg(windows)]
mod admin_exec {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;

    use super::*;

    const SESSION: &str = "admin-exec-session";
    const PROOF: &str = "admin-exec-proof-token";

    fn fake_codex_exe() -> &'static Path {
        static EXE: OnceLock<PathBuf> = OnceLock::new();
        EXE.get_or_init(|| {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("target")
                .join("admin-exec-fixture");
            let resources = root.join("resources");
            std::fs::create_dir_all(&resources).expect("create fixture directory");
            let source = root.join("fake_codex.rs");
            let exe = resources.join("codex.exe");
            std::fs::write(&source, FAKE_CODEX_SOURCE).expect("write fake Codex source");
            let status = Command::new("rustc")
                .args(["--edition=2024", "-O"])
                .arg(&source)
                .arg("-o")
                .arg(&exe)
                .status()
                .expect("run rustc for fake Codex");
            assert!(status.success(), "fake Codex fixture must compile");
            exe
        })
    }

    #[tokio::test]
    async fn admin_exec_production_entrypoint_rejects_non_store_executable_before_spawn() {
        let identity = current_windows_identity().expect("identity");
        let pipe_name = admin_pipe_name(&format!("{SESSION}-{}", uuid::Uuid::new_v4()));
        let job =
            KillOnCloseJob::new(&format!("admin-exec-test-{}", uuid::Uuid::new_v4())).expect("job");
        let result = AdminExecRuntime::start(
            AdminExecConfig {
                codex_exe: fake_codex_exe(),
                readiness_probe_exe: fake_codex_exe(),
                pipe_name: &pipe_name,
                session_id: SESSION,
                session_proof: PROOF,
                expected_user_sid: &identity.user_sid,
                expected_logon_sid: &identity.logon_sid,
            },
            &job,
        )
        .await;
        let error = match result {
            Ok(runtime) => {
                runtime
                    .shutdown()
                    .await
                    .expect("shutdown unexpected runtime");
                panic!("user-controlled fixture must fail closed before spawn")
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("Store package verification"));
    }

    const FAKE_CODEX_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
use std::process::Command;

fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let bits = (a << 16) | (b << 8) | c;
        output.push(TABLE[((bits >> 18) & 63) as usize] as char);
        output.push(TABLE[((bits >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 { TABLE[((bits >> 6) & 63) as usize] as char } else { '=' });
        output.push(if chunk.len() > 2 { TABLE[(bits & 63) as usize] as char } else { '=' });
    }
    output
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args != ["exec-server", "--listen", "stdio"] { std::process::exit(64); }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout().lock();
    let mut line = String::new();
    let mut initialized = false;
    while input.read_line(&mut line).unwrap_or(0) != 0 {
        if !initialized && line.contains("\"method\":\"initialize\"") {
            writeln!(output, "{{\"id\":1,\"result\":{{}}}}").unwrap(); output.flush().unwrap();
        } else if !initialized && line.contains("\"method\":\"initialized\"") {
            initialized = true;
        } else if initialized && line.contains("\"method\":\"process/start\"") {
            let probe = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $m=[regex]::Match((whoami /groups | Out-String),'S-1-16-(\\d+)'); Write-Output ('SID='+$sid+';RID='+$m.Groups[1].Value)"]).output().unwrap();
            writeln!(output, "{{\"id\":2,\"result\":{{\"processId\":\"codex-plus-admin-readiness-probe\"}}}}").unwrap();
            writeln!(output, "{{\"method\":\"process/output\",\"params\":{{\"processId\":\"codex-plus-admin-readiness-probe\",\"seq\":1,\"stream\":\"stdout\",\"chunk\":\"{}\"}}}}", b64(&probe.stdout)).unwrap();
            writeln!(output, "{{\"method\":\"process/exited\",\"params\":{{\"processId\":\"codex-plus-admin-readiness-probe\",\"seq\":2,\"exitCode\":0,\"sandboxDenied\":false}}}}").unwrap(); output.flush().unwrap();
        } else {
            if line == "EXIT\n" { break; }
            output.write_all(line.as_bytes()).unwrap(); output.flush().unwrap();
        }
        line.clear();
    }
}
"#;
}
