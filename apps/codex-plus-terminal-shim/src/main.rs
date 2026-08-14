use std::io::Read;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const TERMINAL_PIPE_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_PIPE";
const TERMINAL_SESSION_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_SESSION";
const TERMINAL_PROOF_FILE_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_PROOF_FILE";
const MAX_PROOF_BYTES: usize = 4 * 1024;
const MAX_HELLO_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024;
const CONNECT_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    Terminal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Hello<'a> {
    protocol: u8,
    session_id: &'a str,
    mode: Mode,
    client_pid: u32,
    proof: &'a str,
    helper_args: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalAuthResponse {
    accepted: bool,
    #[serde(default)]
    process_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalCompletion {
    exit_code: i32,
}

struct Options {
    pipe: String,
    session: String,
    proof_file: String,
    helper_args: Vec<String>,
}

fn options_from_environment<I>(args: I) -> anyhow::Result<Options>
where
    I: IntoIterator<Item = String>,
{
    let cwd = std::env::current_dir()
        .context("administrator terminal working directory is unavailable")?
        .to_string_lossy()
        .into_owned();
    let mut helper_args = vec![cwd];
    helper_args.extend(args);
    Ok(Options {
        pipe: std::env::var(TERMINAL_PIPE_ENV).context("administrator terminal pipe is missing")?,
        session: std::env::var(TERMINAL_SESSION_ENV)
            .context("administrator terminal session is missing")?,
        proof_file: std::env::var(TERMINAL_PROOF_FILE_ENV)
            .context("administrator terminal proof file is missing")?,
        helper_args,
    })
}

fn read_proof(proof_file: &str) -> anyhow::Result<String> {
    let file = std::fs::File::open(proof_file).context("failed to open proof file")?;
    let mut bytes = Vec::with_capacity(MAX_PROOF_BYTES + 1);
    file.take((MAX_PROOF_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("failed to read proof file")?;
    if bytes.len() > MAX_PROOF_BYTES {
        bytes.fill(0);
        bail!("proof file exceeds the size limit");
    }
    let proof = String::from_utf8(bytes).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.fill(0);
        anyhow::anyhow!("proof file is not valid UTF-8")
    })?;
    ensure!(!proof.is_empty(), "proof file must not be empty");
    Ok(proof)
}

fn serialize_hello(options: &Options, proof: &str) -> anyhow::Result<Vec<u8>> {
    ensure!(!proof.is_empty(), "proof must not be empty");
    let encoded = serde_json::to_vec(&Hello {
        protocol: 1,
        session_id: &options.session,
        mode: Mode::Terminal,
        client_pid: std::process::id(),
        proof,
        helper_args: &options.helper_args,
    })
    .context("failed to serialize administrator terminal hello")?;
    ensure!(
        encoded.len() <= MAX_HELLO_BYTES,
        "terminal hello is too large"
    );
    Ok(encoded)
}

fn parse_auth_response(payload: &[u8]) -> anyhow::Result<u32> {
    let response: TerminalAuthResponse =
        serde_json::from_slice(payload).context("invalid administrator terminal response")?;
    ensure!(response.accepted, "administrator terminal was rejected");
    let process_id = response
        .process_id
        .context("administrator terminal response has no process id")?;
    ensure!(
        process_id != 0,
        "administrator terminal process id is invalid"
    );
    Ok(process_id)
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), payload: &[u8]) -> anyhow::Result<()> {
    let length = u32::try_from(payload.len()).context("frame is too large")?;
    writer
        .write_all(&length.to_le_bytes())
        .await
        .context("failed to write frame length")?;
    writer
        .write_all(payload)
        .await
        .context("failed to write frame payload")?;
    writer.flush().await.context("failed to flush frame")?;
    Ok(())
}

async fn write_secret_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    payload: &mut [u8],
) -> anyhow::Result<()> {
    let result = write_frame(writer, payload).await;
    payload.fill(0);
    result
}

async fn read_frame(
    reader: &mut (impl AsyncRead + Unpin),
    maximum: usize,
) -> anyhow::Result<Vec<u8>> {
    let length = reader
        .read_u32_le()
        .await
        .context("failed to read frame length")? as usize;
    ensure!(length <= maximum, "frame is too large");
    let mut payload = vec![0; length];
    reader
        .read_exact(&mut payload)
        .await
        .context("failed to read frame payload")?;
    Ok(payload)
}

#[cfg(windows)]
async fn connect_pipe(
    pipe_name: &str,
) -> anyhow::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;

    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(pipe) => return Ok(pipe),
            Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
}

async fn with_connect_auth_timeout<F, T>(future: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    tokio::time::timeout(CONNECT_AUTH_TIMEOUT, future)
        .await
        .context("timed out connecting or authenticating with administrator broker")?
}

#[cfg(windows)]
async fn run() -> anyhow::Result<i32> {
    let options = options_from_environment(std::env::args().skip(1))?;
    let mut proof = read_proof(&options.proof_file)?;
    let hello = serialize_hello(&options, &proof);
    unsafe {
        proof.as_bytes_mut().fill(0);
    }
    let mut hello = hello?;

    let mut pipe = with_connect_auth_timeout(async {
        let mut pipe = connect_pipe(&options.pipe).await?;
        write_secret_frame(&mut pipe, &mut hello).await?;
        let payload = read_frame(&mut pipe, MAX_RESPONSE_BYTES).await?;
        let _terminal_host_process_id = parse_auth_response(&payload)?;
        Ok(pipe)
    })
    .await?;

    let completion = read_frame(&mut pipe, MAX_RESPONSE_BYTES).await?;
    let completion: TerminalCompletion =
        serde_json::from_slice(&completion).context("invalid administrator terminal completion")?;
    Ok(completion.exit_code)
}

#[cfg(not(windows))]
async fn run() -> anyhow::Result<i32> {
    bail!("administrator terminal shim is unsupported on non-Windows platforms")
}

#[cfg(windows)]
fn diagnostic_event(message: &str) {
    use std::io::Write as _;
    let path = std::env::temp_dir().join("codex-plus-shim-events.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            file,
            "pid={} at={:?} terminal-client {}",
            std::process::id(),
            std::time::SystemTime::now(),
            message
        );
    }
}

#[tokio::main]
async fn main() {
    match run().await {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(error) => {
            eprintln!("administrator terminal shim failed: {error:#}");
            #[cfg(windows)]
            diagnostic_event(&format!("fatal error={error:#}"));
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_hello_preserves_cwd_and_shell_arguments() {
        let options = Options {
            pipe: r"\\.\pipe\terminal-test".into(),
            session: "session".into(),
            proof_file: "proof".into(),
            helper_args: vec![r"C:\work".into(), "-NoLogo".into()],
        };
        let hello: serde_json::Value =
            serde_json::from_slice(&serialize_hello(&options, "secret").unwrap()).unwrap();
        assert_eq!(hello["protocol"], 1);
        assert_eq!(hello["mode"], "terminal");
        assert_eq!(hello["sessionId"], "session");
        assert_eq!(hello["helperArgs"][0], r"C:\work");
        assert_eq!(hello["helperArgs"][1], "-NoLogo");
    }

    #[test]
    fn terminal_auth_requires_a_nonzero_broker_host_process() {
        assert_eq!(
            parse_auth_response(br#"{"accepted":true,"processId":42}"#).unwrap(),
            42
        );
        assert!(parse_auth_response(br#"{"accepted":true}"#).is_err());
        assert!(parse_auth_response(br#"{"accepted":true,"processId":0}"#).is_err());
        assert!(parse_auth_response(br#"{"accepted":false}"#).is_err());
    }

    #[test]
    fn proof_file_is_size_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let proof = directory.path().join("proof");
        std::fs::write(&proof, vec![b'x'; MAX_PROOF_BYTES + 1]).unwrap();
        assert!(read_proof(proof.to_str().unwrap()).is_err());
    }
}
