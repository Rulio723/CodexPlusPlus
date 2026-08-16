use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

static TEST_LOG_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

const MAX_DIAGNOSTIC_LOG_BYTES: u64 = 50 * 1024 * 1024;
const COMPACTED_DIAGNOSTIC_LOG_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
struct DiagnosticRecord {
    timestamp_ms: u64,
    pid: u32,
    event: String,
    detail: Value,
}

pub fn append_diagnostic_log(event: &str, detail: impl Serialize) -> std::io::Result<()> {
    let path = diagnostic_log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let detail = serde_json::to_value(detail).unwrap_or_else(|error| {
        json!({
            "serialization_error": error.to_string()
        })
    });
    let record = DiagnosticRecord {
        timestamp_ms: now_ms(),
        pid: std::process::id(),
        event: event.to_string(),
        detail,
    };
    let line = serde_json::to_string(&record).unwrap_or_else(|error| {
        json!({
            "timestamp_ms": now_ms(),
            "pid": std::process::id(),
            "event": "diagnostic_log.serialization_failed",
            "detail": {
                "message": error.to_string()
            }
        })
        .to_string()
    });

    compact_diagnostic_log_if_needed(&path)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn sanitized_error_chain(error: &anyhow::Error) -> String {
    const MAX_DETAIL_CHARS: usize = 2_048;
    let mut detail = error
        .chain()
        .map(ToString::to_string)
        .map(|message| sanitize_error_message(&message))
        .collect::<Vec<_>>()
        .join(": ");
    if detail.chars().count() > MAX_DETAIL_CHARS {
        detail = detail.chars().take(MAX_DETAIL_CHARS).collect();
        detail.push_str("...");
    }
    detail
}

fn sanitize_error_message(message: &str) -> String {
    let mut sanitized = Vec::new();
    let mut redact_next = false;
    for token in message.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if redact_next || lower.contains(r"\\.\pipe\") {
            sanitized.push("<REDACTED>".to_string());
            redact_next = false;
            continue;
        }
        let label = lower
            .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_');
        if matches!(
            label,
            "proof" | "session" | "session_id" | "token" | "pipe" | "pipe_name"
        ) {
            sanitized.push(token.to_string());
            redact_next = true;
            continue;
        }
        if [
            "proof=",
            "session=",
            "session_id=",
            "token=",
            "pipe=",
            "pipe_name=",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        {
            let separator = if token.contains('=') { "=" } else { ":" };
            let label = token.split(['=', ':']).next().unwrap_or("sensitive");
            sanitized.push(format!("{label}{separator}<REDACTED>"));
            continue;
        }
        sanitized.push(token.to_string());
    }
    sanitized.join(" ")
}

pub fn clear_diagnostic_log() -> std::io::Result<()> {
    let path = diagnostic_log_path();
    clear_diagnostic_log_path(&path)
}

fn clear_diagnostic_log_path(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn diagnostic_log_path() -> PathBuf {
    if let Some(lock) = TEST_LOG_PATH.get() {
        if let Ok(guard) = lock.lock() {
            if let Some(path) = &*guard {
                return path.clone();
            }
        }
    }
    crate::paths::default_diagnostic_log_path()
}

#[doc(hidden)]
pub fn set_diagnostic_log_path_for_tests(path: Option<PathBuf>) {
    let lock = TEST_LOG_PATH.get_or_init(|| Mutex::new(None));
    *lock.lock().expect("test log path lock poisoned") = path;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn compact_diagnostic_log_if_needed(path: &PathBuf) -> std::io::Result<()> {
    compact_diagnostic_log(
        path,
        MAX_DIAGNOSTIC_LOG_BYTES,
        COMPACTED_DIAGNOSTIC_LOG_BYTES,
    )
}

fn compact_diagnostic_log(
    path: &PathBuf,
    max_bytes: u64,
    compacted_bytes: u64,
) -> std::io::Result<()> {
    let len = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if len <= max_bytes {
        return Ok(());
    }

    let keep = compacted_bytes.min(len);
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(len - keep))?;
    let mut tail = Vec::with_capacity(keep as usize);
    file.read_to_end(&mut tail)?;
    drop(file);
    if len > keep {
        if let Some(pos) = tail.iter().position(|byte| *byte == b'\n') {
            tail.drain(..=pos);
        }
    }

    crate::settings::atomic_write(path, &tail).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_diagnostic_log_keeps_tail_and_drops_partial_first_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("codex-plus.log");
        std::fs::write(&path, "line-1\nline-2\nline-3\nline-4\n").unwrap();

        compact_diagnostic_log(&path, 12, 16).unwrap();

        let contents = std::fs::read_to_string(path).unwrap();
        assert_eq!(contents, "line-3\nline-4\n");
    }

    #[test]
    fn clear_diagnostic_log_ignores_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing.log");

        clear_diagnostic_log_path(&path).unwrap();
    }

    #[test]
    fn sanitized_error_chain_keeps_stage_but_redacts_admin_secrets() {
        let error = anyhow::anyhow!(
            r"administrator_mode:exec: admin_exec_readiness failed for pipe \\.\pipe\codex-plus-secret with proof secret-proof session 1234 token=abcd"
        );

        let detail = sanitized_error_chain(&error);

        assert!(detail.contains("administrator_mode:exec"));
        assert!(detail.contains("admin_exec_readiness"));
        assert!(!detail.contains("codex-plus-secret"));
        assert!(!detail.contains("secret-proof"));
        assert!(!detail.contains("1234"));
        assert!(!detail.contains("abcd"));
    }

    #[test]
    fn sanitized_error_chain_is_bounded() {
        let error = anyhow::anyhow!("{}", "x".repeat(4_096));

        let detail = sanitized_error_chain(&error);

        assert!(detail.len() <= 2_051);
        assert!(detail.ends_with("..."));
    }
}
