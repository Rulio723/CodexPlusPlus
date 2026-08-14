use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::json;

use crate::admin_mode::AdminAppServerBootstrap;

const BOOTSTRAP_RETRIES: usize = 40;
const BOOTSTRAP_RETRY_DELAY: Duration = Duration::from_millis(250);

const APP_SERVER_PIPE_ENV: &str = "CODEX_PLUS_ADMIN_APP_SERVER_PIPE";
const APP_SERVER_SESSION_ENV: &str = "CODEX_PLUS_ADMIN_APP_SERVER_SESSION";
const APP_SERVER_PROOF_FILE_ENV: &str = "CODEX_PLUS_ADMIN_APP_SERVER_PROOF_FILE";
const OFFICIAL_CODEX_EXE_ENV: &str = "CODEX_PLUS_ADMIN_OFFICIAL_CODEX_EXE";
const WINDOWS_COMPUTER_USE_ENV: &str = "CODEX_ELECTRON_ENABLE_WINDOWS_COMPUTER_USE";
const TERMINAL_PIPE_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_PIPE";
const TERMINAL_SESSION_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_SESSION";
const TERMINAL_PROOF_FILE_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_PROOF_FILE";

pub async fn install_and_resume(
    inspector_port: u16,
    bootstrap: &AdminAppServerBootstrap,
) -> anyhow::Result<()> {
    let script = bootstrap_script(bootstrap)?;
    install_script_and_resume(
        inspector_port,
        &script,
        "administrator_mode.app_server_bootstrap",
        "administrator app-server bootstrap",
    )
    .await
}

pub async fn install_windows_computer_use_and_resume(inspector_port: u16) -> anyhow::Result<()> {
    install_script_and_resume(
        inspector_port,
        windows_computer_use_bootstrap_script(),
        "windows_computer_use.runtime_bootstrap",
        "Windows Computer Use runtime bootstrap",
    )
    .await
}

async fn install_script_and_resume(
    inspector_port: u16,
    script: &str,
    diagnostic_prefix: &str,
    error_label: &str,
) -> anyhow::Result<()> {
    let mut last_error = None;
    for attempt in 1..=BOOTSTRAP_RETRIES {
        match try_install_and_resume(inspector_port, &script).await {
            Ok(result) => {
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    &format!("{diagnostic_prefix}_installed"),
                    json!({
                        "inspector_port": inspector_port,
                        "attempt": attempt,
                        "result": result,
                    }),
                );
                return Ok(());
            }
            Err(error) => {
                let message = error.to_string();
                last_error = Some(error);
                let _ = crate::diagnostic_log::append_diagnostic_log(
                    &format!("{diagnostic_prefix}_retry_failed"),
                    json!({
                        "inspector_port": inspector_port,
                        "attempt": attempt,
                        "message": message,
                    }),
                );
                tokio::time::sleep(BOOTSTRAP_RETRY_DELAY).await;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("{error_label} failed")))
}

pub fn bootstrap_script(bootstrap: &AdminAppServerBootstrap) -> anyhow::Result<String> {
    let official = path_text(&bootstrap.official_codex_exe, "official Codex executable")?;
    let terminal_shim = path_text(
        &bootstrap.terminal_shim_path,
        "administrator terminal compatibility shim",
    )?;
    let terminal_proof = path_text(
        &bootstrap.terminal_proof_path,
        "administrator terminal proof file",
    )?;
    Ok(format!(
        r#"
(() => {{
  if (typeof process !== "object" || !process.env) throw new Error("Electron main process environment is unavailable");
  process.env.{WINDOWS_COMPUTER_USE_ENV} = "1";
  process.env.CODEX_CLI_PATH = {official};
  process.env.SHELL = {terminal_shim};
  process.env.{TERMINAL_PIPE_ENV} = {terminal_pipe};
  process.env.{TERMINAL_SESSION_ENV} = {terminal_session};
  process.env.{TERMINAL_PROOF_FILE_ENV} = {terminal_proof};
  delete process.env.{APP_SERVER_PIPE_ENV};
  delete process.env.{APP_SERVER_SESSION_ENV};
  delete process.env.{APP_SERVER_PROOF_FILE_ENV};
  delete process.env.{OFFICIAL_CODEX_EXE_ENV};

  // Codex resolves and caches its preferred shell before a non-pausing
  // `--inspect` endpoint becomes available. Patching SHELL here is therefore
  // too late for the cached path, even though it is still needed in the PTY
  // environment. node-pty is looked up again for every terminal creation, so
  // wrap its spawn export and replace only PowerShell terminal launches that
  // carry this administrator session's authenticated environment.
  const moduleApi = process.getBuiltinModule?.("module");
  const pathApi = process.getBuiltinModule?.("path");
  if (!moduleApi?.createRequire || !pathApi?.win32) throw new Error("Electron module bootstrap API is unavailable");
  const requireFromApp = moduleApi.createRequire(
    `${{process.resourcesPath}}/app.asar/.vite/build/codex-plus-bootstrap.js`,
  );
  const nodePty = requireFromApp("node-pty");
  const hookKey = Symbol.for("codex-plus.admin-terminal.node-pty-hook.v1");
  if (!nodePty[hookKey]) {{
    const originalSpawn = nodePty.spawn;
    if (typeof originalSpawn !== "function") throw new Error("node-pty spawn is unavailable");
    const hookState = {{ installed: true, spawnCount: 0, replacedCount: 0 }};
    const wrappedSpawn = function(file, args, options) {{
      hookState.spawnCount += 1;
      const basename = pathApi.win32.basename(String(file ?? "")).toLowerCase();
      const env = options?.env;
      const isPowerShell = basename === "pwsh" || basename === "pwsh.exe"
        || basename === "powershell" || basename === "powershell.exe";
      const isAdministratorTerminal = Boolean(
        env?.{TERMINAL_PIPE_ENV}
        && env?.{TERMINAL_SESSION_ENV}
        && env?.{TERMINAL_PROOF_FILE_ENV},
      );
      if (isPowerShell && isAdministratorTerminal) {{
        hookState.replacedCount += 1;
        file = {terminal_shim};
      }}
      return Reflect.apply(originalSpawn, this, [file, args, options]);
    }};
    Object.defineProperty(nodePty, hookKey, {{
      value: hookState,
      configurable: false,
      enumerable: false,
      writable: false,
    }});
    nodePty.spawn = wrappedSpawn;
  }}
  return JSON.stringify({{
    status: "ok",
    pid: process.pid,
    appServerElevation: false,
    computerUse: true,
    terminalElevation: true,
    terminalHook: true,
  }});
}})()
"#,
        official = serde_json::to_string(official)?,
        terminal_shim = serde_json::to_string(terminal_shim)?,
        terminal_pipe = serde_json::to_string(&bootstrap.terminal_pipe_name)?,
        terminal_session = serde_json::to_string(&bootstrap.terminal_session_id)?,
        terminal_proof = serde_json::to_string(terminal_proof)?,
    ))
}

pub fn windows_computer_use_bootstrap_script() -> &'static str {
    r#"
(() => {
  if (typeof process !== "object" || !process.env) throw new Error("Electron main process environment is unavailable");
  process.env.CODEX_ELECTRON_ENABLE_WINDOWS_COMPUTER_USE = "1";
  return JSON.stringify({ status: "ok", pid: process.pid, computerUse: true });
})()
"#
}

async fn try_install_and_resume(
    inspector_port: u16,
    script: &str,
) -> anyhow::Result<serde_json::Value> {
    let targets = crate::cdp::list_targets(inspector_port).await?;
    let target = targets
        .iter()
        .find(|target| {
            target.target_type == "node"
                && target
                    .web_socket_debugger_url
                    .as_deref()
                    .is_some_and(|url| !url.is_empty())
        })
        .or_else(|| {
            targets.iter().find(|target| {
                target
                    .web_socket_debugger_url
                    .as_deref()
                    .is_some_and(|url| !url.is_empty())
            })
        })
        .context("Electron main-process inspector target is unavailable")?;
    let websocket_url = target
        .web_socket_debugger_url
        .as_deref()
        .context("Electron inspector target has no websocket URL")?;
    let result = crate::bridge::evaluate_script_and_run_if_waiting(websocket_url, script)
        .await
        .context("failed to install administrator app-server bootstrap")?;
    if let Some(exception) = result
        .get("result")
        .and_then(|value| value.get("exceptionDetails"))
    {
        bail!("administrator app-server bootstrap threw: {exception}");
    }
    Ok(result)
}

fn path_text<'a>(path: &'a std::path::Path, label: &str) -> anyhow::Result<&'a str> {
    path.to_str()
        .with_context(|| format!("{label} path is not valid Unicode"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn bootstrap_script_keeps_main_app_server_standard_and_computer_use_enabled() {
        let script = bootstrap_script(&AdminAppServerBootstrap {
            official_codex_exe: PathBuf::from(r"C:\runtime\codex.exe"),
            terminal_shim_path: PathBuf::from(r"C:\runtime\admin-terminal\pwsh.exe"),
            terminal_pipe_name: r"\\.\pipe\codex-plus-terminal".to_string(),
            terminal_session_id: "session-123".to_string(),
            terminal_proof_path: PathBuf::from(r"C:\runtime\terminal.proof"),
        })
        .unwrap();

        assert!(script.contains("process.env.CODEX_CLI_PATH"));
        assert!(script.contains(r"C:\\runtime\\codex.exe"));
        assert!(!script.contains("codex-plus-admin-shim.exe"));
        assert!(script.contains(WINDOWS_COMPUTER_USE_ENV));
        assert!(script.contains(&format!("delete process.env.{APP_SERVER_PIPE_ENV}")));
        assert!(script.contains(&format!("delete process.env.{APP_SERVER_SESSION_ENV}")));
        assert!(script.contains(&format!("delete process.env.{APP_SERVER_PROOF_FILE_ENV}")));
        assert!(script.contains(&format!("delete process.env.{OFFICIAL_CODEX_EXE_ENV}")));
        assert!(script.contains("appServerElevation: false"));
        assert!(script.contains("process.env.SHELL"));
        assert!(script.contains(r"admin-terminal\\pwsh.exe"));
        assert!(script.contains(TERMINAL_PIPE_ENV));
        assert!(script.contains(TERMINAL_SESSION_ENV));
        assert!(script.contains(TERMINAL_PROOF_FILE_ENV));
        assert!(script.contains("terminalElevation: true"));
        assert!(script.contains("requireFromApp(\"node-pty\")"));
        assert!(script.contains("codex-plus.admin-terminal.node-pty-hook.v1"));
        assert!(script.contains("nodePty.spawn = wrappedSpawn"));
        assert!(script.contains("isAdministratorTerminal"));
        assert!(script.contains("terminalHook: true"));
        assert!(!script.contains("config.toml"));
        assert!(!script.contains("auth.json"));
        assert!(!script.contains("localStorage"));
    }

    #[test]
    fn windows_computer_use_bootstrap_is_ephemeral_and_account_independent() {
        let script = windows_computer_use_bootstrap_script();

        assert!(script.contains("process.env.CODEX_ELECTRON_ENABLE_WINDOWS_COMPUTER_USE"));
        assert!(!script.contains("auth.json"));
        assert!(!script.contains("config.toml"));
        assert!(!script.contains("localStorage"));
        assert!(!script.contains("OPENAI_API_KEY"));
    }
}
