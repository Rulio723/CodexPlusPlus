#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    Exec,
    ComputerUse,
    AppServer,
    Terminal,
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    mode: Mode,
    pipe: String,
    session: String,
    proof_file: String,
    helper_args: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Client(Options),
    IdentityProbe,
    TerminalHost {
        client_pid: u32,
        cwd: String,
        shell: String,
        shell_args: Vec<String>,
    },
    Direct {
        executable: String,
        args: Vec<String>,
    },
}

const APP_SERVER_PIPE_ENV: &str = "CODEX_PLUS_ADMIN_APP_SERVER_PIPE";
const APP_SERVER_SESSION_ENV: &str = "CODEX_PLUS_ADMIN_APP_SERVER_SESSION";
const APP_SERVER_PROOF_FILE_ENV: &str = "CODEX_PLUS_ADMIN_APP_SERVER_PROOF_FILE";
const OFFICIAL_CODEX_EXE_ENV: &str = "CODEX_PLUS_ADMIN_OFFICIAL_CODEX_EXE";
const TERMINAL_PIPE_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_PIPE";
const TERMINAL_SESSION_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_SESSION";
const TERMINAL_PROOF_FILE_ENV: &str = "CODEX_PLUS_ADMIN_TERMINAL_PROOF_FILE";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Hello<'a> {
    protocol: u8,
    session_id: &'a str,
    mode: Mode,
    client_pid: u32,
    proof: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    helper_args: Option<&'a [String]>,
}

const MAX_PROOF_BYTES: usize = 4 * 1024;
const MAX_HELLO_BYTES: usize = 64 * 1024;
const MAX_AUTH_RESPONSE_BYTES: usize = 4 * 1024;
const MAX_MUX_PAYLOAD: usize = 64 * 1024;
const CONNECT_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

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
            "pid={} at={:?} {}",
            std::process::id(),
            std::time::SystemTime::now(),
            message
        );
    }
}

const MUX_STDIN_DATA: u8 = 1;
const MUX_STDIN_EOF: u8 = 2;
const MUX_STDOUT_DATA: u8 = 3;
const MUX_STDOUT_EOF: u8 = 4;
const MUX_STDERR_DATA: u8 = 5;
const MUX_STDERR_EOF: u8 = 6;
const MUX_EXIT: u8 = 7;

fn parse_args<I>(args: I) -> anyhow::Result<Options>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mode = match args.next().as_deref() {
        Some("exec-client") => Mode::Exec,
        Some("computer-use-client") => Mode::ComputerUse,
        _ => bail!("expected exec-client or computer-use-client mode"),
    };

    let pipe = required_option(&mut args, "--pipe")?;
    let session = required_option(&mut args, "--session")?;
    let proof_file = required_option(&mut args, "--proof-file")?;
    let helper_args = match mode {
        Mode::Exec => {
            ensure!(args.next().is_none(), "unexpected exec-client argument");
            Vec::new()
        }
        Mode::ComputerUse => {
            ensure!(
                args.next().as_deref() == Some("--"),
                "expected -- separator"
            );
            args.collect()
        }
        Mode::AppServer | Mode::Terminal => {
            unreachable!("environment-backed modes are parsed separately")
        }
    };

    Ok(Options {
        mode,
        pipe,
        session,
        proof_file,
        helper_args,
    })
}

fn parse_invocation<I>(args: I) -> anyhow::Result<Invocation>
where
    I: IntoIterator<Item = String>,
{
    parse_invocation_with_env(args, |name| std::env::var(name).ok())
}

fn parse_invocation_with_env<I, F>(args: I, env: F) -> anyhow::Result<Invocation>
where
    I: IntoIterator<Item = String>,
    F: Fn(&str) -> Option<String>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "identity-probe") {
        ensure!(args.len() == 1, "unexpected identity-probe argument");
        return Ok(Invocation::IdentityProbe);
    }
    if args.first().is_some_and(|arg| arg == "terminal-host") {
        let mut args = args.into_iter();
        let _ = args.next();
        let client_pid = required_option(&mut args, "--client-pid")?
            .parse::<u32>()
            .context("terminal client pid is invalid")?;
        let cwd = required_option(&mut args, "--cwd")?;
        let shell = required_option(&mut args, "--shell")?;
        let shell_args = match args.next() {
            Some(separator) => {
                ensure!(separator == "--", "expected terminal-host -- separator");
                args.collect()
            }
            None => Vec::new(),
        };
        return Ok(Invocation::TerminalHost {
            client_pid,
            cwd,
            shell,
            shell_args,
        });
    }
    if matches!(
        args.first().map(String::as_str),
        Some("exec-client" | "computer-use-client")
    ) {
        return parse_args(args).map(Invocation::Client);
    }

    if let (Some(pipe), Some(session), Some(proof_file)) = (
        env(TERMINAL_PIPE_ENV),
        env(TERMINAL_SESSION_ENV),
        env(TERMINAL_PROOF_FILE_ENV),
    ) {
        return Ok(Invocation::Client(Options {
            mode: Mode::Terminal,
            pipe,
            session,
            proof_file,
            // node-pty may add flags such as -NoLogo. Preserve them for the
            // elevated PowerShell instead of misclassifying the invocation as
            // a direct Codex CLI launch.
            helper_args: args,
        }));
    }

    let official_executable = env(OFFICIAL_CODEX_EXE_ENV);
    if args.iter().any(|arg| arg == "app-server") {
        let pipe = env(APP_SERVER_PIPE_ENV).context("administrator app-server pipe is missing")?;
        let session =
            env(APP_SERVER_SESSION_ENV).context("administrator app-server session is missing")?;
        let proof_file = env(APP_SERVER_PROOF_FILE_ENV)
            .context("administrator app-server proof file is missing")?;
        ensure!(
            official_executable.is_some(),
            "official Codex executable is missing"
        );
        return Ok(Invocation::Client(Options {
            mode: Mode::AppServer,
            pipe,
            session,
            proof_file,
            helper_args: args,
        }));
    }

    Ok(Invocation::Direct {
        executable: official_executable.context("official Codex executable is missing")?,
        args,
    })
}

fn format_identity_probe_record(user_sid: &str, logon_sid: &str, integrity_rid: u32) -> String {
    format!("SID={user_sid};LOGON={logon_sid};RID={integrity_rid}\n")
}

#[cfg(windows)]
fn run_identity_probe() -> anyhow::Result<()> {
    let identity = identity_probe::current_identity()?;
    let mut record = format_identity_probe_record(
        &identity.user_sid,
        &identity.logon_sid,
        identity.integrity_rid,
    );
    let result = std::io::stdout()
        .write_all(record.as_bytes())
        .and_then(|_| std::io::stdout().flush())
        .context("failed to write identity probe output");
    unsafe {
        record.as_bytes_mut().fill(0);
    }
    result
}

#[cfg(windows)]
mod identity_probe {
    use std::mem::size_of;

    use anyhow::{Context, anyhow};
    use windows::Win32::Foundation::{CloseHandle, FALSE, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, PSID, TOKEN_GROUPS,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenIntegrityLevel, TokenLogonSid,
        TokenUser,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::core::PWSTR;

    pub(super) struct Identity {
        pub(super) user_sid: String,
        pub(super) logon_sid: String,
        pub(super) integrity_rid: u32,
    }

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    struct LocalWideString(PWSTR);

    impl LocalWideString {
        fn from_sid(sid: PSID) -> anyhow::Result<Self> {
            let mut value = PWSTR::null();
            unsafe {
                ConvertSidToStringSidW(sid, &mut value).context("failed to convert Windows SID")?;
            }
            if value.is_null() {
                return Err(anyhow!("Windows returned a null SID string"));
            }
            Ok(Self(value))
        }

        fn to_string(&self) -> anyhow::Result<String> {
            unsafe {
                self.0
                    .to_string()
                    .context("Windows returned an invalid SID")
            }
        }
    }

    impl Drop for LocalWideString {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(HLOCAL(self.0.0.cast()));
            }
        }
    }

    struct TokenBuffer(Vec<usize>);

    impl TokenBuffer {
        fn query(
            token: HANDLE,
            class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
        ) -> anyhow::Result<Self> {
            let mut byte_len = 0;
            let first = unsafe { GetTokenInformation(token, class, None, 0, &mut byte_len) };
            if byte_len == 0 {
                return Err(first
                    .err()
                    .map(anyhow::Error::from)
                    .unwrap_or_else(|| anyhow!("Windows returned empty token information")));
            }
            let word_len = (byte_len as usize)
                .checked_add(size_of::<usize>() - 1)
                .context("token buffer length overflow")?
                / size_of::<usize>();
            let mut storage = vec![0usize; word_len];
            unsafe {
                GetTokenInformation(
                    token,
                    class,
                    Some(storage.as_mut_ptr().cast()),
                    byte_len,
                    &mut byte_len,
                )
                .context("failed to read Windows token information")?;
            }
            Ok(Self(storage))
        }

        fn as_ptr<T>(&self) -> *const T {
            self.0.as_ptr().cast()
        }
    }

    fn sid_string(sid: PSID) -> anyhow::Result<String> {
        LocalWideString::from_sid(sid)?.to_string()
    }

    fn token_user_sid(token: HANDLE) -> anyhow::Result<String> {
        let buffer = TokenBuffer::query(token, TokenUser)?;
        let user = unsafe { &*buffer.as_ptr::<TOKEN_USER>() };
        sid_string(user.User.Sid).context("failed to read token user SID")
    }

    fn token_logon_sid(token: HANDLE) -> anyhow::Result<String> {
        let buffer = TokenBuffer::query(token, TokenLogonSid)?;
        let groups = unsafe { &*buffer.as_ptr::<TOKEN_GROUPS>() };
        if groups.GroupCount == 0 {
            return Err(anyhow!("Windows token has no logon SID"));
        }
        sid_string(groups.Groups[0].Sid).context("failed to read token logon SID")
    }

    fn token_integrity_rid(token: HANDLE) -> anyhow::Result<u32> {
        let buffer = TokenBuffer::query(token, TokenIntegrityLevel)?;
        let label = unsafe { &*buffer.as_ptr::<TOKEN_MANDATORY_LABEL>() };
        let count = unsafe { GetSidSubAuthorityCount(label.Label.Sid).as_ref() }
            .copied()
            .context("integrity SID has no sub-authority count")?;
        if count == 0 {
            return Err(anyhow!("integrity SID has no RID"));
        }
        unsafe { GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)).as_ref() }
            .copied()
            .context("integrity SID has no final RID")
    }

    pub(super) fn current_identity() -> anyhow::Result<Identity> {
        let mut token = HANDLE::default();
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .context("failed to open current process token")?;
        }
        let token = OwnedHandle(token);
        Ok(Identity {
            user_sid: token_user_sid(token.0)?,
            logon_sid: token_logon_sid(token.0)?,
            integrity_rid: token_integrity_rid(token.0)?,
        })
    }

    pub(super) fn process_integrity_rid(process_id: u32) -> anyhow::Result<u32> {
        let process = OwnedHandle(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, process_id)
                .context("failed to open PowerShell process")?
        });
        let mut token = HANDLE::default();
        unsafe {
            OpenProcessToken(process.0, TOKEN_QUERY, &mut token)
                .context("failed to open PowerShell process token")?;
        }
        let token = OwnedHandle(token);
        token_integrity_rid(token.0)
    }
}

fn required_option(
    args: &mut impl Iterator<Item = String>,
    expected_name: &str,
) -> anyhow::Result<String> {
    ensure!(
        args.next().as_deref() == Some(expected_name),
        "missing required option"
    );
    let value = args.next().context("missing required option value")?;
    ensure!(!value.is_empty(), "required option value must not be empty");
    Ok(value)
}

fn serialize_hello(options: &Options, proof: &str, client_pid: u32) -> anyhow::Result<Vec<u8>> {
    ensure!(!proof.is_empty(), "proof must not be empty");
    let helper_args = match options.mode {
        Mode::Exec => None,
        Mode::ComputerUse | Mode::AppServer | Mode::Terminal => {
            Some(options.helper_args.as_slice())
        }
    };
    let encoded = serde_json::to_vec(&Hello {
        protocol: 1,
        session_id: &options.session,
        mode: options.mode,
        client_pid,
        proof,
        helper_args,
    })
    .context("failed to serialize authentication hello")?;
    ensure!(
        encoded.len() <= MAX_HELLO_BYTES,
        "authentication hello is too large"
    );
    Ok(encoded)
}

fn read_proof(_proof_file: &str) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(_proof_file).context("failed to open proof file")?;
    let mut bytes = Vec::with_capacity(MAX_PROOF_BYTES + 1);
    read_proof_from(&mut file, &mut bytes)
}

fn read_proof_from<R: Read>(reader: &mut R, bytes: &mut Vec<u8>) -> anyhow::Result<String> {
    bytes.clear();
    if reader
        .take((MAX_PROOF_BYTES + 1) as u64)
        .read_to_end(bytes)
        .is_err()
    {
        bytes.fill(0);
        bail!("failed to read proof file");
    }
    if bytes.len() > MAX_PROOF_BYTES {
        bytes.fill(0);
        bail!("proof file exceeds the size limit");
    }
    let proof = String::from_utf8(std::mem::take(bytes)).map_err(|error| {
        let mut bytes = error.into_bytes();
        bytes.fill(0);
        anyhow::anyhow!("proof file is not valid UTF-8")
    })?;
    ensure!(!proof.is_empty(), "proof file must not be empty");
    Ok(proof)
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthResponse {
    accepted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalAuthResponse {
    accepted: bool,
    #[serde(default)]
    process_id: Option<u32>,
    #[serde(default)]
    token_handle: Option<u64>,
    #[serde(default)]
    shell: Option<String>,
}

#[derive(Debug)]
enum TerminalAuthorization {
    BrokerHost { process_id: u32 },
    LegacyToken { token_handle: u64, shell: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalCompletion {
    exit_code: i32,
}

fn parse_auth_response(payload: &[u8]) -> anyhow::Result<AuthResponse> {
    let response: AuthResponse =
        serde_json::from_slice(payload).context("invalid authentication response")?;
    ensure!(response.accepted, "administrator session was rejected");
    Ok(response)
}

fn parse_terminal_auth_response(payload: &[u8]) -> anyhow::Result<TerminalAuthorization> {
    let response: TerminalAuthResponse =
        serde_json::from_slice(payload).context("invalid administrator terminal response")?;
    ensure!(response.accepted, "administrator terminal was rejected");
    match (response.process_id, response.token_handle, response.shell) {
        (Some(process_id), None, None) if process_id != 0 => {
            Ok(TerminalAuthorization::BrokerHost { process_id })
        }
        (None, Some(token_handle), Some(shell)) if token_handle != 0 && !shell.is_empty() => {
            Ok(TerminalAuthorization::LegacyToken {
                token_handle,
                shell,
            })
        }
        _ => bail!("administrator terminal response is incomplete or ambiguous"),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct MuxFrame {
    channel: u8,
    payload: Vec<u8>,
}

async fn write_mux_frame(
    writer: &mut (impl AsyncWrite + Unpin),
    channel: u8,
    payload: &[u8],
) -> anyhow::Result<()> {
    ensure!(
        payload.len() <= MAX_MUX_PAYLOAD,
        "multiplex payload is too large"
    );
    writer.write_all(&[channel, 0]).await?;
    writer
        .write_all(&(payload.len() as u32).to_le_bytes())
        .await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_mux_frame(reader: &mut (impl AsyncRead + Unpin)) -> anyhow::Result<MuxFrame> {
    let channel = reader.read_u8().await.context("read multiplex channel")?;
    let flags = reader.read_u8().await.context("read multiplex flags")?;
    ensure!(flags == 0, "unsupported multiplex flags");
    let length = reader
        .read_u32_le()
        .await
        .context("read multiplex length")? as usize;
    ensure!(length <= MAX_MUX_PAYLOAD, "multiplex payload is too large");
    let mut payload = vec![0; length];
    reader
        .read_exact(&mut payload)
        .await
        .context("read multiplex payload")?;
    Ok(MuxFrame { channel, payload })
}

async fn relay_computer_use_streams<I, O, E, S>(
    mut input: I,
    mut output: O,
    mut error: E,
    stream: S,
) -> anyhow::Result<i32>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut pipe_reader, mut pipe_writer) = tokio::io::split(stream);
    let input_to_broker = async {
        let mut buffer = vec![0; MAX_MUX_PAYLOAD];
        loop {
            let read = input.read(&mut buffer).await.context("read helper stdin")?;
            if read == 0 {
                write_mux_frame(&mut pipe_writer, MUX_STDIN_EOF, &[]).await?;
                break;
            }
            write_mux_frame(&mut pipe_writer, MUX_STDIN_DATA, &buffer[..read]).await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let broker_to_output = async {
        let mut stdout_eof = false;
        let mut stderr_eof = false;
        loop {
            let frame = read_mux_frame(&mut pipe_reader).await?;
            match frame.channel {
                MUX_STDOUT_DATA => {
                    ensure!(!stdout_eof, "stdout data after EOF");
                    ensure!(!frame.payload.is_empty(), "empty stdout data frame");
                    output.write_all(&frame.payload).await?;
                    output.flush().await?;
                }
                MUX_STDOUT_EOF => {
                    ensure!(frame.payload.is_empty(), "stdout EOF payload must be empty");
                    ensure!(!stdout_eof, "duplicate stdout EOF");
                    stdout_eof = true;
                    output.shutdown().await?;
                }
                MUX_STDERR_DATA => {
                    ensure!(!stderr_eof, "stderr data after EOF");
                    ensure!(!frame.payload.is_empty(), "empty stderr data frame");
                    error.write_all(&frame.payload).await?;
                    error.flush().await?;
                }
                MUX_STDERR_EOF => {
                    ensure!(frame.payload.is_empty(), "stderr EOF payload must be empty");
                    ensure!(!stderr_eof, "duplicate stderr EOF");
                    stderr_eof = true;
                    error.shutdown().await?;
                }
                MUX_EXIT => {
                    ensure!(stdout_eof && stderr_eof, "exit arrived before output EOF");
                    ensure!(frame.payload.len() == 4, "exit payload must be four bytes");
                    return Ok(i32::from_le_bytes(frame.payload.try_into().unwrap()));
                }
                _ => bail!("invalid broker-to-client multiplex channel"),
            }
        }
    };
    tokio::pin!(input_to_broker);
    tokio::pin!(broker_to_output);
    tokio::select! {
        result = &mut broker_to_output => result,
        result = &mut input_to_broker => {
            result?;
            broker_to_output.await
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

async fn relay_streams<I, O, S>(mut input: I, mut output: O, stream: S) -> anyhow::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut pipe_reader, mut pipe_writer) = tokio::io::split(stream);

    let input_to_pipe = async {
        tokio::io::copy(&mut input, &mut pipe_writer)
            .await
            .context("failed to relay stdin")?;
        pipe_writer
            .shutdown()
            .await
            .context("failed to propagate stdin EOF")
    };
    let pipe_to_output = async {
        tokio::io::copy(&mut pipe_reader, &mut output)
            .await
            .context("failed to relay stdout")?;
        output.flush().await.context("failed to flush stdout")?;
        output.shutdown().await.context("failed to close stdout")
    };

    let (input_result, output_result) = tokio::join!(input_to_pipe, pipe_to_output);
    input_result?;
    output_result?;
    Ok(())
}

async fn relay_duplex<S>(stream: S) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    relay_streams(tokio::io::stdin(), tokio::io::stdout(), stream).await
}

#[cfg(windows)]
async fn run_elevated_terminal(
    mut pipe: impl AsyncRead + AsyncWrite + Unpin,
    auth: TerminalAuthorization,
    cwd: &str,
    shell_args: &[String],
) -> anyhow::Result<i32> {
    match auth {
        TerminalAuthorization::BrokerHost { process_id } => {
            let _ = process_id;
            let completion = read_frame(&mut pipe, MAX_AUTH_RESPONSE_BYTES).await?;
            let completion: TerminalCompletion = serde_json::from_slice(&completion)
                .context("invalid administrator terminal completion")?;
            Ok(completion.exit_code)
        }
        TerminalAuthorization::LegacyToken {
            token_handle,
            shell,
        } => {
            let child = spawn_legacy_elevated_terminal(token_handle, cwd, &shell, shell_args)?;
            let verification = serde_json::to_vec(&serde_json::json!({
                "verified": true,
                "processId": child.process_id,
            }))?;
            write_frame(&mut pipe, &verification)
                .await
                .context("failed to verify legacy administrator terminal process")?;
            let response = with_connect_auth_timeout(async {
                let payload = read_frame(&mut pipe, MAX_AUTH_RESPONSE_BYTES).await?;
                parse_auth_response(&payload)
            })
            .await
            .context("legacy administrator terminal verification failed")?;
            let _ = response;
            child.wait()
        }
    }
}

#[cfg(windows)]
struct LegacyTerminalChild {
    process: windows::Win32::Foundation::HANDLE,
    process_id: u32,
}

#[cfg(windows)]
impl LegacyTerminalChild {
    fn wait(mut self) -> anyhow::Result<i32> {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, INFINITE, WaitForSingleObject,
        };

        let wait = unsafe { WaitForSingleObject(self.process, INFINITE) };
        let result = if wait == WAIT_OBJECT_0 {
            let mut exit_code = 1u32;
            unsafe { GetExitCodeProcess(self.process, &mut exit_code) }
                .context("failed to read legacy administrator terminal exit code")?;
            Ok(exit_code as i32)
        } else {
            bail!("failed to wait for legacy administrator terminal process")
        };
        unsafe {
            let _ = CloseHandle(self.process);
        }
        self.process = windows::Win32::Foundation::HANDLE::default();
        result
    }
}

#[cfg(windows)]
impl Drop for LegacyTerminalChild {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        if !self.process.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.process);
            }
            self.process = windows::Win32::Foundation::HANDLE::default();
        }
    }
}

#[cfg(windows)]
struct ImpersonationGuard;

#[cfg(windows)]
impl Drop for ImpersonationGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Security::RevertToSelf();
        }
    }
}

#[cfg(windows)]
fn quote_windows_argument(argument: &std::ffi::OsStr) -> String {
    let value = argument.to_string_lossy();
    if !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return value.into_owned();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for character in value.chars() {
        if character == '\\' {
            backslashes += 1;
            continue;
        }
        if character == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
            quoted.push('"');
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
            quoted.push(character);
        }
        backslashes = 0;
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(windows)]
fn spawn_legacy_elevated_terminal(
    token_handle: u64,
    cwd: &str,
    shell: &str,
    shell_args: &[String],
) -> anyhow::Result<LegacyTerminalChild> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE};
    use windows::Win32::Security::{
        DuplicateTokenEx, ImpersonateLoggedOnUser, SecurityImpersonation, TOKEN_ACCESS_MASK,
        TokenImpersonation, TokenPrimary,
    };
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_PROCESS_LOGON_FLAGS, CREATE_UNICODE_ENVIRONMENT,
        CreateProcessAsUserW, CreateProcessWithTokenW, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows::core::{PCWSTR, PWSTR};

    let raw = isize::try_from(token_handle).context("legacy terminal token handle is invalid")?;
    let incoming = HANDLE(raw as *mut _);
    ensure!(
        !incoming.is_invalid(),
        "legacy terminal token handle is invalid"
    );

    let mut impersonation = HANDLE::default();
    let impersonation_result = unsafe {
        DuplicateTokenEx(
            incoming,
            TOKEN_ACCESS_MASK(0),
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &mut impersonation,
        )
    };
    let mut primary = HANDLE::default();
    let primary_result = unsafe {
        DuplicateTokenEx(
            incoming,
            TOKEN_ACCESS_MASK(0),
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
    };
    unsafe {
        let _ = CloseHandle(incoming);
    }
    impersonation_result.context("failed to create legacy administrator impersonation token")?;
    if let Err(error) = primary_result {
        unsafe {
            let _ = CloseHandle(impersonation);
        }
        return Err(error).context("failed to create legacy administrator primary token");
    }

    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let impersonation = Token(impersonation);
    let primary = Token(primary);
    unsafe { ImpersonateLoggedOnUser(impersonation.0) }
        .context("failed to impersonate legacy administrator terminal token")?;
    let _impersonation = ImpersonationGuard;

    let current_exe = std::env::current_exe()
        .context("legacy administrator terminal shim path is unavailable")?;
    let terminal_host = current_exe
        .parent()
        .and_then(std::path::Path::parent)
        .map(|root| root.join("codex-plus-admin-shim.exe"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| current_exe.clone());
    diagnostic_event(&format!(
        "legacy-terminal-host image={:?} cwd={cwd:?}",
        terminal_host
    ));
    let mut host_args = vec![
        terminal_host.as_os_str().to_owned(),
        "terminal-host".into(),
        "--client-pid".into(),
        std::process::id().to_string().into(),
        "--cwd".into(),
        cwd.into(),
        "--shell".into(),
        shell.into(),
    ];
    if !shell_args.is_empty() {
        host_args.push("--".into());
        host_args.extend(shell_args.iter().map(Into::into));
    }
    let command_line = host_args
        .iter()
        .map(|argument| quote_windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let mut command_line = std::ffi::OsStr::new(&command_line)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let application = terminal_host
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    let flags = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;
    let first_error = unsafe {
        CreateProcessWithTokenW(
            primary.0,
            CREATE_PROCESS_LOGON_FLAGS(0),
            PCWSTR(application.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            flags,
            None,
            PCWSTR::null(),
            &startup,
            &mut process,
        )
    }
    .err();
    if let Some(first_error) = first_error {
        process = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessAsUserW(
                primary.0,
                PCWSTR(application.as_ptr()),
                PWSTR(command_line.as_mut_ptr()),
                None,
                None,
                BOOL(0),
                flags,
                None,
                PCWSTR::null(),
                &startup,
                &mut process,
            )
        }
        .with_context(|| {
            format!(
                "failed to start legacy administrator terminal host after CreateProcessWithTokenW: {first_error}"
            )
        })?;
    }
    unsafe {
        let _ = CloseHandle(process.hThread);
    }
    let child = LegacyTerminalChild {
        process: process.hProcess,
        process_id: process.dwProcessId,
    };
    ensure!(
        identity_probe::process_integrity_rid(child.process_id)? >= 0x3000,
        "legacy administrator terminal host is not high integrity"
    );
    Ok(child)
}

#[cfg(windows)]
fn run_terminal_host(
    client_pid: u32,
    cwd: &str,
    shell: &str,
    shell_args: &[String],
) -> anyhow::Result<i32> {
    use std::fs::OpenOptions;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Stdio;
    use windows::Win32::System::Console::{
        AttachConsole, FreeConsole, SetConsoleCP, SetConsoleOutputCP,
    };

    ensure!(
        is_supported_terminal_shell(Path::new(shell)),
        "administrator terminal shell is not PowerShell 7 or Windows PowerShell 5.1"
    );
    unsafe {
        let _ = FreeConsole();
        AttachConsole(client_pid).context("failed to attach administrator terminal console")?;
        // Keep the attached pseudo console on UTF-8 so both PowerShell 7 and
        // Windows PowerShell 5.1 output are decoded consistently by xterm.
        SetConsoleCP(65001).context("failed to set administrator terminal input encoding")?;
        SetConsoleOutputCP(65001)
            .context("failed to set administrator terminal output encoding")?;
    }
    // The broker intentionally starts terminal-host with null stdio. Merely
    // calling AttachConsole does not replace those STARTF_USESTDHANDLES
    // values, so bind the child explicitly to the newly attached ConPTY.
    let input = OpenOptions::new()
        .read(true)
        .open("CONIN$")
        .context("failed to open administrator terminal input")?;
    let output = OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .context("failed to open administrator terminal output")?;
    let error = output
        .try_clone()
        .context("failed to clone administrator terminal output")?;
    let mut child = std::process::Command::new(shell)
        .args(shell_args)
        .current_dir(cwd)
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(error))
        .creation_flags(0)
        .spawn()
        .context("failed to start administrator PowerShell")?;
    let process_id = child.id();
    let integrity_rid = match identity_probe::process_integrity_rid(process_id) {
        Ok(rid) => rid,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("failed to verify administrator PowerShell integrity");
        }
    };
    if integrity_rid < 0x3000 {
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "administrator PowerShell integrity is 0x{integrity_rid:04X}, expected at least 0x3000"
        );
    }
    let status = child
        .wait()
        .context("failed to wait for administrator PowerShell")?;
    Ok(status.code().unwrap_or(1))
}

fn is_supported_terminal_shell(path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|name| {
        name.eq_ignore_ascii_case("pwsh.exe") || name.eq_ignore_ascii_case("powershell.exe")
    })
}

#[cfg(windows)]
async fn run() -> anyhow::Result<i32> {
    let mut options = match parse_invocation(std::env::args().skip(1))? {
        Invocation::IdentityProbe => {
            run_identity_probe()?;
            return Ok(0);
        }
        Invocation::TerminalHost {
            client_pid,
            cwd,
            shell,
            shell_args,
        } => return run_terminal_host(client_pid, &cwd, &shell, &shell_args),
        Invocation::Client(options) => options,
        Invocation::Direct { executable, args } => {
            return run_official_direct(&executable, &args);
        }
    };
    if options.mode == Mode::Terminal {
        let cwd = std::env::current_dir()
            .context("administrator terminal working directory is unavailable")?
            .to_string_lossy()
            .into_owned();
        options.helper_args.insert(0, cwd);
    }
    let mut proof = read_proof(&options.proof_file)?;
    let hello = serialize_hello(&options, &proof, std::process::id());
    unsafe {
        proof.as_bytes_mut().fill(0);
    }
    let mut hello = hello?;

    let (pipe, terminal_auth) = with_connect_auth_timeout(async {
        let mut pipe = connect_pipe(&options.pipe).await?;
        write_secret_frame(&mut pipe, &mut hello).await?;
        let payload = read_frame(&mut pipe, MAX_AUTH_RESPONSE_BYTES).await?;
        let terminal_auth = if options.mode == Mode::Terminal {
            Some(parse_terminal_auth_response(&payload)?)
        } else {
            parse_auth_response(&payload)?;
            None
        };
        Ok((pipe, terminal_auth))
    })
    .await?;

    match options.mode {
        Mode::Exec | Mode::AppServer => {
            relay_duplex(pipe).await?;
            Ok(0)
        }
        Mode::ComputerUse => {
            relay_computer_use_streams(
                tokio::io::stdin(),
                tokio::io::stdout(),
                tokio::io::stderr(),
                pipe,
            )
            .await
        }
        Mode::Terminal => {
            let cwd = options
                .helper_args
                .first()
                .context("administrator terminal working directory is unavailable")?;
            run_elevated_terminal(
                pipe,
                terminal_auth.context("administrator terminal host is unavailable")?,
                cwd,
                &options.helper_args[1..],
            )
            .await
        }
    }
}

#[cfg(windows)]
fn run_official_direct(executable: &str, args: &[String]) -> anyhow::Result<i32> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new(executable)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("failed to start official Codex CLI")?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(not(windows))]
async fn run() -> anyhow::Result<i32> {
    bail!("administrator shim is unsupported on non-Windows platforms")
}

#[tokio::main]
async fn main() {
    match run().await {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(error) => {
            eprintln!("administrator shim failed: {error:#}");
            #[cfg(windows)]
            diagnostic_event(&format!("fatal error={error:#}"));
            if let Some(path) = std::env::var_os("CODEX_PLUS_SHIM_ERROR_LOG") {
                use std::io::Write as _;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(file, "administrator shim failed: {error:#}");
                }
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_identity_probe_only_when_it_is_the_exact_invocation() {
        assert_eq!(
            parse_invocation(strings(&["identity-probe"])).unwrap(),
            Invocation::IdentityProbe
        );
        assert!(parse_invocation(strings(&["identity-probe", "extra"])).is_err());
        assert!(parse_invocation_with_env(strings(&["--identity-probe"]), |_| None).is_err());
    }

    #[test]
    fn parses_terminal_host_exactly() {
        assert_eq!(
            parse_invocation(strings(&[
                "terminal-host",
                "--client-pid",
                "1234",
                "--cwd",
                r"C:\workspace",
                "--shell",
                r"C:\Program Files\PowerShell\7\pwsh.exe",
            ]))
            .unwrap(),
            Invocation::TerminalHost {
                client_pid: 1234,
                cwd: r"C:\workspace".to_string(),
                shell: r"C:\Program Files\PowerShell\7\pwsh.exe".to_string(),
                shell_args: Vec::new(),
            }
        );
    }

    #[test]
    fn parses_terminal_host_and_preserves_shell_arguments() {
        assert_eq!(
            parse_invocation(strings(&[
                "terminal-host",
                "--client-pid",
                "1234",
                "--cwd",
                r"C:\workspace",
                "--shell",
                r"C:\Program Files\PowerShell\7\pwsh.exe",
                "--",
                "-NoLogo",
                "-NoProfile",
            ]))
            .unwrap(),
            Invocation::TerminalHost {
                client_pid: 1234,
                cwd: r"C:\workspace".to_string(),
                shell: r"C:\Program Files\PowerShell\7\pwsh.exe".to_string(),
                shell_args: vec!["-NoLogo".to_string(), "-NoProfile".to_string()],
            }
        );
    }

    #[test]
    fn accepts_power_shell_7_and_windows_power_shell_5_1_shells() {
        assert!(is_supported_terminal_shell(std::path::Path::new(
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        )));
        assert!(is_supported_terminal_shell(std::path::Path::new(
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        )));
        assert!(!is_supported_terminal_shell(std::path::Path::new(
            r"C:\Windows\System32\cmd.exe",
        )));
    }

    #[test]
    fn empty_shell_invocation_uses_administrator_terminal_environment() {
        let invocation = parse_invocation_with_env(Vec::<String>::new(), |name| {
            Some(
                match name {
                    TERMINAL_PIPE_ENV => r"\\.\pipe\codex-plus-terminal",
                    TERMINAL_SESSION_ENV => "session-123",
                    TERMINAL_PROOF_FILE_ENV => "terminal.proof",
                    _ => return None,
                }
                .to_string(),
            )
        })
        .unwrap();
        let Invocation::Client(options) = invocation else {
            panic!("expected terminal client")
        };
        assert_eq!(options.mode, Mode::Terminal);
        assert_eq!(options.pipe, r"\\.\pipe\codex-plus-terminal");
        assert_eq!(options.session, "session-123");
        assert_eq!(options.proof_file, "terminal.proof");
    }

    #[test]
    fn shell_arguments_still_use_administrator_terminal_environment() {
        let invocation = parse_invocation_with_env(strings(&["-NoLogo", "-NoProfile"]), |name| {
            Some(
                match name {
                    TERMINAL_PIPE_ENV => r"\\.\pipe\codex-plus-terminal",
                    TERMINAL_SESSION_ENV => "session-123",
                    TERMINAL_PROOF_FILE_ENV => "terminal.proof",
                    _ => return None,
                }
                .to_string(),
            )
        })
        .unwrap();
        let Invocation::Client(options) = invocation else {
            panic!("expected terminal client")
        };
        assert_eq!(options.mode, Mode::Terminal);
        assert_eq!(options.helper_args, ["-NoLogo", "-NoProfile"]);
    }

    #[test]
    fn identity_probe_record_matches_exec_readiness_protocol() {
        assert_eq!(
            format_identity_probe_record("S-1-5-21-1", "S-1-5-5-10-20", 0x3000),
            "SID=S-1-5-21-1;LOGON=S-1-5-5-10-20;RID=12288\n"
        );
    }

    #[test]
    fn parses_exec_client_exactly() {
        let options = parse_args(strings(&[
            "exec-client",
            "--pipe",
            r"\\.\pipe\codex-plus-admin-session",
            "--session",
            "session-123",
            "--proof-file",
            "proof.bin",
        ]))
        .expect("parse exec client");

        assert_eq!(options.mode, Mode::Exec);
        assert_eq!(options.pipe, r"\\.\pipe\codex-plus-admin-session");
        assert_eq!(options.session, "session-123");
        assert_eq!(options.proof_file, "proof.bin");
        assert!(options.helper_args.is_empty());
    }

    #[test]
    fn parses_computer_use_and_preserves_every_argument_after_separator() {
        let helper_args = ["--flag", "value with spaces", "--", "", r"C:\literal\path"];
        let mut args = strings(&[
            "computer-use-client",
            "--pipe",
            "pipe-name",
            "--session",
            "session-123",
            "--proof-file",
            "proof.bin",
            "--",
        ]);
        args.extend(strings(&helper_args));

        let options = parse_args(args).expect("parse computer-use client");

        assert_eq!(options.mode, Mode::ComputerUse);
        assert_eq!(options.helper_args, strings(&helper_args));
    }

    #[test]
    fn app_server_invocation_uses_ephemeral_environment_and_preserves_cli_args() {
        let args = strings(&[
            "-c",
            "features.code_mode_host=true",
            "app-server",
            "--analytics-default-enabled",
        ]);
        let invocation = parse_invocation_with_env(args.clone(), |name| {
            Some(
                match name {
                    APP_SERVER_PIPE_ENV => r"\\.\pipe\codex-plus-admin-app-server",
                    APP_SERVER_SESSION_ENV => "session-123",
                    APP_SERVER_PROOF_FILE_ENV => "proof.bin",
                    OFFICIAL_CODEX_EXE_ENV => r"C:\Program Files\WindowsApps\Codex\codex.exe",
                    _ => return None,
                }
                .to_owned(),
            )
        })
        .expect("parse administrator app-server invocation");

        let Invocation::Client(options) = invocation else {
            panic!("expected app-server client")
        };
        assert_eq!(options.mode, Mode::AppServer);
        assert_eq!(options.pipe, r"\\.\pipe\codex-plus-admin-app-server");
        assert_eq!(options.session, "session-123");
        assert_eq!(options.proof_file, "proof.bin");
        assert_eq!(options.helper_args, args);
    }

    #[test]
    fn non_app_server_cli_invocation_runs_the_official_binary_directly() {
        let args = strings(&["--version"]);
        let invocation = parse_invocation_with_env(args.clone(), |name| {
            (name == OFFICIAL_CODEX_EXE_ENV).then(|| r"C:\Codex\codex.exe".to_owned())
        })
        .expect("parse direct invocation");

        assert_eq!(
            invocation,
            Invocation::Direct {
                executable: r"C:\Codex\codex.exe".to_owned(),
                args,
            }
        );
    }

    #[test]
    fn app_server_invocation_fails_closed_when_bootstrap_is_incomplete() {
        assert!(
            parse_invocation_with_env(strings(&["app-server"]), |_| None).is_err(),
            "app-server must never fall back to standard-integrity direct execution"
        );
    }

    #[test]
    fn rejects_missing_values_and_nonempty_requirements() {
        for args in [
            strings(&["exec-client"]),
            strings(&[
                "exec-client",
                "--pipe",
                "",
                "--session",
                "session",
                "--proof-file",
                "proof",
            ]),
            strings(&[
                "exec-client",
                "--pipe",
                "pipe",
                "--session",
                "",
                "--proof-file",
                "proof",
            ]),
            strings(&[
                "exec-client",
                "--pipe",
                "pipe",
                "--session",
                "session",
                "--proof-file",
            ]),
        ] {
            assert!(parse_args(args).is_err());
        }
    }

    #[test]
    fn rejects_unknown_or_misplaced_arguments() {
        assert!(
            parse_args(strings(&[
                "exec-client",
                "--pipe",
                "pipe",
                "--session",
                "session",
                "--proof-file",
                "proof",
                "extra",
            ]))
            .is_err()
        );
        assert!(
            parse_args(strings(&[
                "computer-use-client",
                "--pipe",
                "pipe",
                "--session",
                "session",
                "--proof-file",
                "proof",
                "helper-without-separator",
            ]))
            .is_err()
        );
    }

    #[test]
    fn hello_serialization_has_only_authenticated_protocol_fields() {
        let options = Options {
            mode: Mode::Exec,
            pipe: "pipe".to_owned(),
            session: "session-123".to_owned(),
            proof_file: "secret-path".to_owned(),
            helper_args: Vec::new(),
        };

        let encoded = serialize_hello(&options, "proof-token", 1234).expect("serialize hello");
        let value: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");
        let object = value.as_object().expect("JSON object");

        assert_eq!(object.len(), 5);
        assert_eq!(value["protocol"], 1);
        assert_eq!(value["sessionId"], "session-123");
        assert_eq!(value["mode"], "exec");
        assert_eq!(value["clientPid"], 1234);
        assert_eq!(value["proof"], "proof-token");
        assert!(value.get("proofFile").is_none());
        assert!(value.get("environment").is_none());
        assert!(value.get("env").is_none());
    }

    #[test]
    fn computer_use_hello_includes_exact_helper_arguments() {
        let options = Options {
            mode: Mode::ComputerUse,
            pipe: "pipe".to_owned(),
            session: "session-123".to_owned(),
            proof_file: "proof".to_owned(),
            helper_args: strings(&["--flag", "value with spaces", "--", ""]),
        };

        let encoded = serialize_hello(&options, "proof-token", 7).expect("serialize hello");
        let value: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");

        assert_eq!(
            value["helperArgs"],
            serde_json::json!(["--flag", "value with spaces", "--", ""])
        );
    }

    #[test]
    fn app_server_hello_includes_exact_official_cli_arguments() {
        let options = Options {
            mode: Mode::AppServer,
            pipe: "pipe".to_owned(),
            session: "session-123".to_owned(),
            proof_file: "proof".to_owned(),
            helper_args: strings(&["-c", "features.code_mode_host=true", "app-server"]),
        };

        let encoded = serialize_hello(&options, "proof-token", 9).expect("serialize hello");
        let value: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");

        assert_eq!(value["mode"], "app-server");
        assert_eq!(
            value["helperArgs"],
            serde_json::json!(["-c", "features.code_mode_host=true", "app-server"])
        );
    }

    #[test]
    fn rejects_hello_larger_than_64_kib() {
        let options = Options {
            mode: Mode::ComputerUse,
            pipe: "pipe".to_owned(),
            session: "session".to_owned(),
            proof_file: "proof".to_owned(),
            helper_args: vec!["x".repeat(65 * 1024)],
        };

        assert!(serialize_hello(&options, "proof", 1).is_err());
    }

    #[tokio::test]
    async fn serialized_hello_bytes_are_cleared_after_write() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let mut hello = b"serialized-secret-hello".to_vec();
        write_secret_frame(&mut writer, &mut hello).await.unwrap();
        assert!(hello.iter().all(|byte| *byte == 0));
        assert_eq!(
            read_frame(&mut reader, 1024).await.unwrap(),
            b"serialized-secret-hello"
        );
    }

    #[test]
    fn proof_reader_rejects_oversized_files_without_leaking_secret_or_path() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("secret-proof-name");
        let secret = "sensitive-proof-material".repeat(200);
        std::fs::write(&path, &secret).expect("write proof");

        let error = read_proof(path.to_str().expect("UTF-8 path"))
            .expect_err("oversized proof must fail")
            .to_string();

        assert!(!error.contains("secret-proof-name"));
        assert!(!error.contains("sensitive-proof-material"));
    }

    #[tokio::test]
    async fn framed_reader_rejects_authentication_response_over_4_kib() {
        use tokio::io::AsyncWriteExt;

        let (mut writer, mut reader) = tokio::io::duplex(8 * 1024);
        writer
            .write_all(&((4 * 1024 + 1) as u32).to_le_bytes())
            .await
            .expect("write frame length");

        let error = read_frame(&mut reader, 4 * 1024)
            .await
            .expect_err("oversized response must fail");
        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn authentication_response_rejects_stale_session() {
        let error =
            parse_auth_response(br#"{"accepted":false}"#).expect_err("rejected session must fail");
        assert!(error.to_string().contains("rejected"));
    }

    #[test]
    fn terminal_authentication_requires_the_verified_host_process() {
        let response =
            parse_terminal_auth_response(br#"{"accepted":true,"processId":4660}"#).unwrap();
        assert!(matches!(
            response,
            TerminalAuthorization::BrokerHost { process_id: 4660 }
        ));
        assert!(parse_terminal_auth_response(br#"{"accepted":true,"processId":0}"#).is_err());
    }

    #[test]
    fn legacy_terminal_authentication_is_parsed_without_weakening_new_protocol() {
        let response = parse_terminal_auth_response(
            br#"{"accepted":true,"tokenHandle":4660,"shell":"C:\\\\Program Files\\\\PowerShell\\\\7\\\\pwsh.exe"}"#,
        )
        .unwrap();
        assert!(matches!(
            response,
            TerminalAuthorization::LegacyToken {
                token_handle: 4660,
                ..
            }
        ));
        assert!(
            parse_terminal_auth_response(
                br#"{"accepted":true,"processId":1,"tokenHandle":2,"shell":"pwsh.exe"}"#
            )
            .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn connection_and_authentication_share_one_ten_second_deadline() {
        let started = tokio::time::Instant::now();
        let result = with_connect_auth_timeout(async {
            tokio::time::sleep(Duration::from_secs(11)).await;
            Ok::<_, anyhow::Error>(())
        })
        .await;

        assert!(result.is_err());
        assert_eq!(started.elapsed(), CONNECT_AUTH_TIMEOUT);
    }

    #[tokio::test]
    async fn computer_use_multiplex_relays_stdin_stdout_stderr_and_exit() {
        let (mut input_writer, input_reader) = tokio::io::duplex(128);
        let (output_writer, mut output_reader) = tokio::io::duplex(128);
        let (error_writer, mut error_reader) = tokio::io::duplex(128);
        let (client, mut broker) = tokio::io::duplex(1024);

        input_writer.write_all(b"request\n").await.unwrap();
        input_writer.shutdown().await.unwrap();
        let relay = tokio::spawn(relay_computer_use_streams(
            input_reader,
            output_writer,
            error_writer,
            client,
        ));

        let stdin = read_mux_frame(&mut broker).await.unwrap();
        assert_eq!(stdin.channel, MUX_STDIN_DATA);
        assert_eq!(stdin.payload, b"request\n");
        let eof = read_mux_frame(&mut broker).await.unwrap();
        assert_eq!(eof.channel, MUX_STDIN_EOF);
        assert!(eof.payload.is_empty());

        write_mux_frame(&mut broker, MUX_STDOUT_DATA, b"response\n")
            .await
            .unwrap();
        write_mux_frame(&mut broker, MUX_STDERR_DATA, b"warning\n")
            .await
            .unwrap();
        write_mux_frame(&mut broker, MUX_STDOUT_EOF, &[])
            .await
            .unwrap();
        write_mux_frame(&mut broker, MUX_STDERR_EOF, &[])
            .await
            .unwrap();
        write_mux_frame(&mut broker, MUX_EXIT, &23i32.to_le_bytes())
            .await
            .unwrap();

        assert_eq!(relay.await.unwrap().unwrap(), 23);
        let mut stdout = Vec::new();
        output_reader.read_to_end(&mut stdout).await.unwrap();
        let mut stderr = Vec::new();
        error_reader.read_to_end(&mut stderr).await.unwrap();
        assert_eq!(stdout, b"response\n");
        assert_eq!(stderr, b"warning\n");
    }

    #[tokio::test]
    async fn computer_use_multiplex_rejects_oversize_and_duplicate_eof() {
        let (mut writer, mut reader) = tokio::io::duplex(16);
        writer.write_all(&[MUX_STDOUT_DATA, 0]).await.unwrap();
        writer
            .write_all(&((MAX_MUX_PAYLOAD + 1) as u32).to_le_bytes())
            .await
            .unwrap();
        assert!(read_mux_frame(&mut reader).await.is_err());

        let (input_writer, input_reader) = tokio::io::duplex(16);
        drop(input_writer);
        let (output_writer, _output_reader) = tokio::io::duplex(16);
        let (error_writer, _error_reader) = tokio::io::duplex(16);
        let (client, mut broker) = tokio::io::duplex(128);
        let relay = tokio::spawn(relay_computer_use_streams(
            input_reader,
            output_writer,
            error_writer,
            client,
        ));
        let _ = read_mux_frame(&mut broker).await.unwrap();
        write_mux_frame(&mut broker, MUX_STDOUT_EOF, &[])
            .await
            .unwrap();
        write_mux_frame(&mut broker, MUX_STDOUT_EOF, &[])
            .await
            .unwrap();
        assert!(relay.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn relay_keeps_reading_pipe_after_stdin_eof() {
        let (mut input_writer, input_reader) = tokio::io::duplex(128);
        let (output_writer, mut output_reader) = tokio::io::duplex(128);
        let (client, mut server) = tokio::io::duplex(128);
        input_writer
            .write_all(b"request")
            .await
            .expect("write input");
        input_writer.shutdown().await.expect("close input");

        let relay = tokio::spawn(relay_streams(input_reader, output_writer, client));
        let mut request = Vec::new();
        server
            .read_to_end(&mut request)
            .await
            .expect("read request");
        server.write_all(b"response").await.expect("write response");
        server.shutdown().await.expect("close response");

        let mut response = Vec::new();
        output_reader
            .read_to_end(&mut response)
            .await
            .expect("read response");
        relay.await.expect("relay task").expect("relay succeeds");
        assert_eq!(request, b"request");
        assert_eq!(response, b"response");
    }

    #[tokio::test]
    async fn relay_keeps_accepting_input_after_pipe_output_half_closes() {
        let (mut input_writer, input_reader) = tokio::io::duplex(128);
        let (output_writer, mut output_reader) = tokio::io::duplex(128);
        let (client, server) = tokio::io::duplex(128);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);

        let relay = tokio::spawn(relay_streams(input_reader, output_writer, client));
        server_writer
            .write_all(b"response")
            .await
            .expect("write response");
        server_writer.shutdown().await.expect("close output half");

        let mut response = Vec::new();
        output_reader
            .read_to_end(&mut response)
            .await
            .expect("read response");
        input_writer
            .write_all(b"after-half-close")
            .await
            .expect("write input after output EOF");
        input_writer.shutdown().await.expect("close input");

        let mut request = Vec::new();
        server_reader
            .read_to_end(&mut request)
            .await
            .expect("read input after output EOF");
        relay.await.expect("relay task").expect("relay succeeds");
        assert_eq!(request, b"after-half-close");
        assert_eq!(response, b"response");
    }

    #[tokio::test]
    async fn relay_finishes_when_server_closes_whole_pipe_and_stdin_reaches_eof() {
        let (mut input_writer, input_reader) = tokio::io::duplex(32);
        let (output_writer, _output_reader) = tokio::io::duplex(32);
        let (client, server) = tokio::io::duplex(32);
        input_writer.shutdown().await.expect("close input");
        drop(server);

        tokio::time::timeout(
            Duration::from_secs(1),
            relay_streams(input_reader, output_writer, client),
        )
        .await
        .expect("relay must not hang")
        .expect("closed pipe is a clean EOF");
    }

    struct PartialThenErrorReader {
        emitted: bool,
    }

    impl Read for PartialThenErrorReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.emitted {
                return Err(io::Error::other("synthetic read failure"));
            }
            self.emitted = true;
            let secret = b"sensitive-partial-proof";
            buffer[..secret.len()].copy_from_slice(secret);
            Ok(secret.len())
        }
    }

    #[test]
    fn proof_reader_zeroes_partial_bytes_before_returning_read_error() {
        let mut reader = PartialThenErrorReader { emitted: false };
        let mut observed_buffer = Vec::new();

        let error = read_proof_from(&mut reader, &mut observed_buffer)
            .expect_err("partial read failure must be reported")
            .to_string();

        assert!(!observed_buffer.is_empty());
        assert!(observed_buffer.iter().all(|byte| *byte == 0));
        assert!(!error.contains("sensitive-partial-proof"));
    }
}
