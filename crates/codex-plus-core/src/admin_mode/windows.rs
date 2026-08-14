use anyhow::{Context, ensure};
use sha2::{Digest, Sha256};

const ADMIN_PIPE_PREFIX: &str = r"\\.\pipe\codex-plus-admin-";
const MAX_ADMIN_PIPE_NAME_LEN: usize = 240;

pub fn admin_pipe_name(session_id: &str) -> String {
    let mut sanitized = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();

    let max_component_len = MAX_ADMIN_PIPE_NAME_LEN - ADMIN_PIPE_PREFIX.len();
    if sanitized.len() > max_component_len {
        let digest = Sha256::digest(session_id.as_bytes());
        let digest = format!("{digest:x}");
        let retained_len = max_component_len - digest.len() - 1;
        sanitized.truncate(retained_len);
        sanitized.push('-');
        sanitized.push_str(&digest);
    }

    format!("{ADMIN_PIPE_PREFIX}{sanitized}")
}

pub fn admin_pipe_sddl(user_sid: &str) -> anyhow::Result<String> {
    validate_canonical_sid(user_sid)?;
    Ok(format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user_sid})"))
}

fn validate_canonical_sid(sid: &str) -> anyhow::Result<()> {
    let mut segments = sid.split('-');
    ensure!(segments.next() == Some("S"), "SID must start with S-");

    let revision = parse_canonical_decimal(segments.next(), u8::MAX as u64, "revision")?;
    ensure!(revision == 1, "SID revision must be 1");
    let authority = parse_canonical_decimal(
        segments.next(),
        0x0000_ffff_ffff_ffff,
        "identifier authority",
    )?;
    ensure!(authority == 5, "SID must use the NT identifier authority");

    let subauthorities = segments.collect::<Vec<_>>();
    ensure!(
        !subauthorities.is_empty(),
        "SID must contain at least one subauthority"
    );
    ensure!(
        subauthorities.len() == 5,
        "SID must be a domain or local account SID"
    );
    let subauthorities = subauthorities
        .into_iter()
        .map(|segment| parse_canonical_decimal(Some(segment), u32::MAX as u64, "subauthority"))
        .collect::<anyhow::Result<Vec<_>>>()?;
    ensure!(
        subauthorities[0] == 21,
        "SID must be a domain or local account SID"
    );
    Ok(())
}

fn parse_canonical_decimal(
    segment: Option<&str>,
    maximum: u64,
    field: &str,
) -> anyhow::Result<u64> {
    let segment = segment.with_context(|| format!("SID is missing {field}"))?;
    ensure!(!segment.is_empty(), "SID {field} must not be empty");
    ensure!(
        segment.bytes().all(|byte| byte.is_ascii_digit()),
        "SID {field} must contain only ASCII decimal digits"
    );
    ensure!(
        segment == "0" || !segment.starts_with('0'),
        "SID {field} must use canonical decimal form"
    );
    let value = segment
        .parse::<u64>()
        .with_context(|| format!("SID {field} is outside the supported range"))?;
    ensure!(
        value <= maximum,
        "SID {field} is outside the supported range"
    );
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsIdentity {
    pub user_sid: String,
    pub logon_sid: String,
    pub elevated: bool,
    pub integrity_rid: u32,
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::mem::{size_of, size_of_val};

    use anyhow::{Context, anyhow};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, PSID, TOKEN_ELEVATION,
        TOKEN_GROUPS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenElevation,
        TokenIntegrityLevel, TokenLogonSid, TokenUser,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::core::{PCWSTR, PWSTR};

    use super::WindowsIdentity;

    const SECURITY_MANDATORY_HIGH_RID: u32 = 0x3000;

    #[derive(Debug)]
    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn new(handle: HANDLE) -> Self {
            Self(handle)
        }

        fn get(&self) -> HANDLE {
            self.0
        }
    }

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
                ConvertSidToStringSidW(sid, &mut value)
                    .context("failed to convert Windows SID to a string")?;
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
                    .context("Windows returned an invalid SID string")
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

    struct TokenBuffer {
        storage: Vec<usize>,
    }

    impl TokenBuffer {
        fn query(
            token: HANDLE,
            class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
        ) -> anyhow::Result<Self> {
            let mut byte_len = 0;
            let first_result = unsafe { GetTokenInformation(token, class, None, 0, &mut byte_len) };
            if byte_len == 0 {
                return Err(first_result
                    .err()
                    .map(anyhow::Error::from)
                    .unwrap_or_else(|| {
                        anyhow!("Windows returned an empty token-information buffer")
                    }));
            }

            let word_len = (byte_len as usize)
                .checked_add(size_of::<usize>() - 1)
                .context("token-information buffer length overflow")?
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
            Ok(Self { storage })
        }

        fn as_ptr<T>(&self) -> *const T {
            self.storage.as_ptr().cast()
        }
    }

    fn open_process_token(process: HANDLE) -> anyhow::Result<OwnedHandle> {
        let mut token = HANDLE::default();
        unsafe {
            OpenProcessToken(process, TOKEN_QUERY, &mut token)
                .context("failed to open Windows process token")?;
        }
        Ok(OwnedHandle::new(token))
    }

    fn sid_string(sid: PSID) -> anyhow::Result<String> {
        LocalWideString::from_sid(sid)?.to_string()
    }

    fn token_user_sid(token: HANDLE) -> anyhow::Result<String> {
        let buffer = TokenBuffer::query(token, TokenUser)?;
        let user = unsafe { &*buffer.as_ptr::<TOKEN_USER>() };
        sid_string(user.User.Sid).context("failed to read Windows token user SID")
    }

    fn token_logon_sid(token: HANDLE) -> anyhow::Result<String> {
        let buffer = TokenBuffer::query(token, TokenLogonSid)?;
        let groups = unsafe { &*buffer.as_ptr::<TOKEN_GROUPS>() };
        if groups.GroupCount == 0 {
            return Err(anyhow!("Windows token has no logon SID"));
        }
        sid_string(groups.Groups[0].Sid).context("failed to read Windows token logon SID")
    }

    fn token_elevated(token: HANDLE) -> anyhow::Result<bool> {
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0;
        unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
                size_of_val(&elevation) as u32,
                &mut returned,
            )
            .context("failed to read Windows token elevation")?;
        }
        Ok(elevation.TokenIsElevated != 0)
    }

    fn token_integrity_rid(token: HANDLE) -> anyhow::Result<u32> {
        let buffer = TokenBuffer::query(token, TokenIntegrityLevel)?;
        let label = unsafe { &*buffer.as_ptr::<TOKEN_MANDATORY_LABEL>() };
        let count = unsafe { GetSidSubAuthorityCount(label.Label.Sid).as_ref() }
            .copied()
            .context("Windows token integrity SID has no sub-authority count")?;
        if count == 0 {
            return Err(anyhow!("Windows token integrity SID has no RID"));
        }
        unsafe { GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)).as_ref() }
            .copied()
            .context("Windows token integrity SID has no final RID")
    }

    fn identity_from_token(token: HANDLE) -> anyhow::Result<WindowsIdentity> {
        Ok(WindowsIdentity {
            user_sid: token_user_sid(token)?,
            logon_sid: token_logon_sid(token)?,
            elevated: token_elevated(token)?,
            integrity_rid: token_integrity_rid(token)?,
        })
    }

    pub fn current_windows_identity() -> anyhow::Result<WindowsIdentity> {
        let token = open_process_token(unsafe { GetCurrentProcess() })?;
        identity_from_token(token.get())
    }

    pub fn process_windows_identity(process_id: u32) -> anyhow::Result<WindowsIdentity> {
        let process = OwnedHandle::new(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
                .context("failed to open Windows process")?
        });
        let token = open_process_token(process.get())?;
        identity_from_token(token.get())
    }

    pub fn process_has_high_integrity(process_id: u32) -> anyhow::Result<bool> {
        let process = OwnedHandle::new(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
                .with_context(|| format!("failed to open Windows process {process_id}"))?
        });
        let token = open_process_token(process.get())?;
        Ok(token_integrity_rid(token.get())? >= SECURITY_MANDATORY_HIGH_RID)
    }

    #[derive(Debug)]
    pub struct KillOnCloseJob {
        handle: OwnedHandle,
    }

    impl KillOnCloseJob {
        pub fn new(name: &str) -> anyhow::Result<Self> {
            let wide_name = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let handle = OwnedHandle::new(unsafe {
                CreateJobObjectW(None, PCWSTR(wide_name.as_ptr()))
                    .context("failed to create Windows Job Object")?
            });
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            unsafe {
                SetInformationJobObject(
                    handle.get(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
                    size_of_val(&limits) as u32,
                )
                .context("failed to configure kill-on-close Windows Job Object")?;
            }
            Ok(Self { handle })
        }

        pub fn assign_process_handle(&self, process: HANDLE) -> anyhow::Result<()> {
            unsafe {
                AssignProcessToJobObject(self.handle.get(), process)
                    .context("failed to assign process to Windows Job Object")
            }
        }

        pub(crate) fn raw_handle(&self) -> HANDLE {
            self.handle.get()
        }
    }
}

#[cfg(windows)]
pub use platform::{
    KillOnCloseJob, current_windows_identity, process_has_high_integrity, process_windows_identity,
};

#[cfg(not(windows))]
mod platform {
    use anyhow::bail;

    use super::WindowsIdentity;

    pub fn current_windows_identity() -> anyhow::Result<WindowsIdentity> {
        bail!("Windows administrator primitives are unsupported on non-Windows platforms")
    }

    pub fn process_has_high_integrity(_process_id: u32) -> anyhow::Result<bool> {
        bail!("Windows administrator primitives are unsupported on non-Windows platforms")
    }

    pub fn process_windows_identity(_process_id: u32) -> anyhow::Result<WindowsIdentity> {
        bail!("Windows administrator primitives are unsupported on non-Windows platforms")
    }

    #[derive(Debug)]
    pub struct KillOnCloseJob;

    impl KillOnCloseJob {
        pub fn new(_name: &str) -> anyhow::Result<Self> {
            bail!("Windows Job Objects are unsupported on non-Windows platforms")
        }

        pub fn assign_process_handle(&self, _process: *mut std::ffi::c_void) -> anyhow::Result<()> {
            bail!("Windows Job Objects are unsupported on non-Windows platforms")
        }
    }
}

#[cfg(not(windows))]
pub use platform::{
    KillOnCloseJob, current_windows_identity, process_has_high_integrity, process_windows_identity,
};
