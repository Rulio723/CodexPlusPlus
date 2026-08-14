use std::time::Duration;

pub const OFFICIAL_CODEX_ORIGINATOR: &str = "codex_cli_rs";
const OFFICIAL_CODEX_USER_AGENT: &str = "codex_cli_rs/1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersionMode {
    Auto,
    Http1,
    Http2,
}

impl Default for HttpVersionMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMinVersion {
    Default,
    Tls12,
    Tls13,
}

impl Default for TlsMinVersion {
    fn default() -> Self {
        Self::Default
    }
}

/// 通用上游传输兼容参数。
///
/// 默认使用 reqwest 的协议协商；需要固定行为时显式选择 `http1` 或 `http2`。
/// 环境变量只用于兼容性排查，不改变 Codex++ 的真实客户端标识，也不复制
/// 其他客户端的专有 TLS 指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportConfig {
    pub http_version: HttpVersionMode,
    pub tls_min_version: TlsMinVersion,
    pub tcp_nodelay: bool,
    pub http2_adaptive_window: Option<bool>,
    pub http2_initial_stream_window_size: Option<u32>,
    pub http2_initial_connection_window_size: Option<u32>,
    pub http2_max_frame_size: Option<u32>,
    pub http2_max_header_list_size: Option<u32>,
    pub http2_keep_alive_interval_secs: Option<u64>,
    pub http2_keep_alive_timeout_secs: Option<u64>,
    pub http2_keep_alive_while_idle: Option<bool>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            http_version: HttpVersionMode::Auto,
            tls_min_version: TlsMinVersion::Default,
            tcp_nodelay: true,
            http2_adaptive_window: None,
            http2_initial_stream_window_size: None,
            http2_initial_connection_window_size: None,
            http2_max_frame_size: None,
            http2_max_header_list_size: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
        }
    }
}

impl TransportConfig {
    /// 返回官方 Codex 风格的通用 TLS/HTTP2 协商参数。
    ///
    /// 该配置使用标准 reqwest/rustls 能力，不依赖私有客户端文件或服务端专有
    /// 标识；HTTP 仍保持 ALPN 自动协商，以免把只支持 HTTP/1.1 的供应商切断。
    pub fn official_codex_compat() -> Self {
        let mut config = Self::from_environment();
        config.http_version = HttpVersionMode::Auto;
        config.tls_min_version = TlsMinVersion::Default;
        config.tcp_nodelay = true;
        config.http2_adaptive_window = Some(true);
        config.http2_initial_stream_window_size = Some(1_048_576);
        config.http2_initial_connection_window_size = Some(1_048_576);
        config.http2_max_frame_size = Some(16_384);
        config.http2_max_header_list_size = Some(65_536);
        config.http2_keep_alive_interval_secs = None;
        config.http2_keep_alive_timeout_secs = None;
        config.http2_keep_alive_while_idle = None;
        config
    }

    pub fn from_environment() -> Self {
        Self {
            http_version: parse_http_version(std::env::var("CODEX_PLUS_HTTP_VERSION").ok()),
            tls_min_version: parse_tls_min_version(
                std::env::var("CODEX_PLUS_TLS_MIN_VERSION").ok(),
            ),
            tcp_nodelay: parse_bool(std::env::var("CODEX_PLUS_TCP_NODELAY").ok(), true),
            http2_adaptive_window: parse_optional_bool(
                std::env::var("CODEX_PLUS_HTTP2_ADAPTIVE_WINDOW").ok(),
            ),
            http2_initial_stream_window_size: parse_optional_u32(
                std::env::var("CODEX_PLUS_HTTP2_INITIAL_STREAM_WINDOW").ok(),
            ),
            http2_initial_connection_window_size: parse_optional_u32(
                std::env::var("CODEX_PLUS_HTTP2_INITIAL_CONNECTION_WINDOW").ok(),
            ),
            http2_max_frame_size: parse_optional_u32(
                std::env::var("CODEX_PLUS_HTTP2_MAX_FRAME_SIZE").ok(),
            ),
            http2_max_header_list_size: parse_optional_u32(
                std::env::var("CODEX_PLUS_HTTP2_MAX_HEADER_LIST_SIZE").ok(),
            ),
            http2_keep_alive_interval_secs: parse_optional_u64(
                std::env::var("CODEX_PLUS_HTTP2_KEEPALIVE_INTERVAL_SECS").ok(),
            ),
            http2_keep_alive_timeout_secs: parse_optional_u64(
                std::env::var("CODEX_PLUS_HTTP2_KEEPALIVE_TIMEOUT_SECS").ok(),
            ),
            http2_keep_alive_while_idle: parse_optional_bool(
                std::env::var("CODEX_PLUS_HTTP2_KEEPALIVE_WHILE_IDLE").ok(),
            ),
        }
    }
}

pub fn proxied_client(user_agent: &str) -> anyhow::Result<reqwest::Client> {
    configured_client_builder(user_agent)
        .build()
        .map_err(Into::into)
}

/// 为单个供应商构建 HTTP client。
///
/// `official_codex_fingerprint` 是 profile 级 opt-in 开关；关闭时完全复用
/// 原有环境配置，开启时使用固定的官方 Codex 风格协商参数。
pub fn proxied_client_for_profile(
    user_agent: &str,
    official_codex_fingerprint: bool,
) -> anyhow::Result<reqwest::Client> {
    configured_client_builder_for_profile(user_agent, official_codex_fingerprint)
        .build()
        .map_err(Into::into)
}

pub fn proxied_client_for_profile_and_url(
    user_agent: &str,
    official_codex_fingerprint: bool,
    target_url: &str,
) -> anyhow::Result<reqwest::Client> {
    let mut builder = configured_client_builder_for_profile(user_agent, official_codex_fingerprint);
    if is_loopback_url(target_url) {
        builder = builder.no_proxy();
    }
    builder.build().map_err(Into::into)
}

pub fn configured_client_builder_for_profile(
    user_agent: &str,
    official_codex_fingerprint: bool,
) -> reqwest::ClientBuilder {
    let config = if official_codex_fingerprint {
        TransportConfig::official_codex_compat()
    } else {
        TransportConfig::from_environment()
    };
    let user_agent = effective_user_agent_for_profile(user_agent, official_codex_fingerprint);
    let mut builder = apply_transport_config(reqwest::Client::builder(), &user_agent, config);
    if official_codex_fingerprint {
        builder = builder.default_headers(official_codex_default_headers());
    }
    builder
}

fn official_codex_default_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::HeaderName::from_static("originator"),
        reqwest::header::HeaderValue::from_static(OFFICIAL_CODEX_ORIGINATOR),
    );
    headers
}

/// 为开启官方 Codex 兼容的单次请求补齐官方客户端会话头。
///
/// `originator` 由 profile client 作为默认头注入；这里补充每次请求都不同的
/// session/thread 标识，避免把它们硬编码成全局值。关闭开关的调用方不应使用此函数。
pub fn with_official_codex_request_headers(
    request: reqwest::RequestBuilder,
    session_id: &str,
    thread_id: Option<&str>,
) -> reqwest::RequestBuilder {
    let session_id = session_id.trim();
    let mut request = if session_id.is_empty() {
        request
    } else {
        request.header("session-id", session_id)
    };
    if let Some(thread_id) = thread_id.map(str::trim).filter(|value| !value.is_empty()) {
        request = request
            .header("thread-id", thread_id)
            .header("x-client-request-id", thread_id);
    }
    request
}

pub fn configured_client_builder(user_agent: &str) -> reqwest::ClientBuilder {
    apply_transport_config(
        reqwest::Client::builder(),
        &effective_user_agent(user_agent),
        TransportConfig::from_environment(),
    )
}

pub fn build_client_with_config(
    user_agent: &str,
    config: TransportConfig,
) -> anyhow::Result<reqwest::Client> {
    apply_transport_config(
        reqwest::Client::builder(),
        &effective_user_agent(user_agent),
        config,
    )
    .build()
    .map_err(Into::into)
}

fn apply_transport_config(
    mut builder: reqwest::ClientBuilder,
    user_agent: &str,
    config: TransportConfig,
) -> reqwest::ClientBuilder {
    builder = builder
        .user_agent(user_agent)
        .tcp_nodelay(config.tcp_nodelay);

    builder = match config.http_version {
        HttpVersionMode::Auto => builder,
        HttpVersionMode::Http1 => builder.http1_only(),
        HttpVersionMode::Http2 => builder.http2_prior_knowledge(),
    };

    builder = match config.tls_min_version {
        TlsMinVersion::Default => builder,
        TlsMinVersion::Tls12 => builder.min_tls_version(reqwest::tls::Version::TLS_1_2),
        TlsMinVersion::Tls13 => builder.min_tls_version(reqwest::tls::Version::TLS_1_3),
    };

    if let Some(value) = config.http2_adaptive_window {
        builder = builder.http2_adaptive_window(value);
    }
    if let Some(value) = config.http2_initial_stream_window_size {
        builder = builder.http2_initial_stream_window_size(value);
    }
    if let Some(value) = config.http2_initial_connection_window_size {
        builder = builder.http2_initial_connection_window_size(value);
    }
    if let Some(value) = config.http2_max_frame_size {
        builder = builder.http2_max_frame_size(value);
    }
    if let Some(value) = config.http2_max_header_list_size {
        builder = builder.http2_max_header_list_size(value);
    }
    if let Some(value) = config.http2_keep_alive_interval_secs {
        builder = builder.http2_keep_alive_interval(Duration::from_secs(value));
    }
    if let Some(value) = config.http2_keep_alive_timeout_secs {
        builder = builder.http2_keep_alive_timeout(Duration::from_secs(value));
    }
    if let Some(value) = config.http2_keep_alive_while_idle {
        builder = builder.http2_keep_alive_while_idle(value);
    }

    builder
}

fn effective_user_agent(user_agent: &str) -> String {
    effective_user_agent_for_profile(user_agent, false)
}

fn effective_user_agent_for_profile(user_agent: &str, official_codex_fingerprint: bool) -> String {
    if official_codex_fingerprint {
        return OFFICIAL_CODEX_USER_AGENT.to_string();
    }
    if user_agent.trim().is_empty() {
        format!("CodexPlusPlus/{}", env!("CARGO_PKG_VERSION"))
    } else {
        user_agent.trim().to_string()
    }
}

fn is_loopback_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn parse_http_version(value: Option<String>) -> HttpVersionMode {
    match value
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("auto") | Some("negotiate") => HttpVersionMode::Auto,
        Some("http1") | Some("http/1.1") | Some("1.1") => HttpVersionMode::Http1,
        Some("http2") | Some("http/2") | Some("2") => HttpVersionMode::Http2,
        _ => HttpVersionMode::Auto,
    }
}

fn parse_tls_min_version(value: Option<String>) -> TlsMinVersion {
    match value
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("1.2") | Some("tls1.2") | Some("tls_1_2") => TlsMinVersion::Tls12,
        Some("1.3") | Some("tls1.3") | Some("tls_1_3") => TlsMinVersion::Tls13,
        _ => TlsMinVersion::Default,
    }
}

fn parse_optional_bool(value: Option<String>) -> Option<bool> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_bool(value: Option<String>, default: bool) -> bool {
    parse_optional_bool(value).unwrap_or(default)
}

fn parse_optional_u32(value: Option<String>) -> Option<u32> {
    value?.trim().parse().ok()
}

fn parse_optional_u64(value: Option<String>) -> Option<u64> {
    value?.trim().parse().ok()
}

/// VLM 专用 HTTP client（带超时）。
/// 不复用通用 proxied_client，避免 VLM 服务无响应时永久阻塞整个代理。
pub fn vlm_http_client() -> anyhow::Result<reqwest::Client> {
    vlm_http_client_with_timeout(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(30),
    )
}

pub(crate) fn vlm_http_client_with_timeout(
    connect: std::time::Duration,
    total: std::time::Duration,
) -> anyhow::Result<reqwest::Client> {
    Ok(
        configured_client_builder(&format!("CodexPlusPlus-VLM/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(connect)
            .timeout(total)
            .build()?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_http_versions() {
        assert_eq!(
            parse_http_version(Some("auto".to_string())),
            HttpVersionMode::Auto
        );
        assert_eq!(
            parse_http_version(Some("http/1.1".to_string())),
            HttpVersionMode::Http1
        );
        assert_eq!(
            parse_http_version(Some("HTTP2".to_string())),
            HttpVersionMode::Http2
        );
        assert_eq!(
            parse_http_version(Some("unknown".to_string())),
            HttpVersionMode::Auto
        );
    }

    #[test]
    fn parses_tls_minimum_versions() {
        assert_eq!(
            parse_tls_min_version(Some("tls_1_2".to_string())),
            TlsMinVersion::Tls12
        );
        assert_eq!(
            parse_tls_min_version(Some("1.3".to_string())),
            TlsMinVersion::Tls13
        );
        assert_eq!(
            parse_tls_min_version(Some("default".to_string())),
            TlsMinVersion::Default
        );
    }

    #[test]
    fn invalid_numeric_and_boolean_overrides_are_ignored() {
        assert_eq!(parse_optional_u32(Some("bad".to_string())), None);
        assert_eq!(parse_optional_u64(Some("bad".to_string())), None);
        assert_eq!(parse_optional_bool(Some("bad".to_string())), None);
        assert!(parse_bool(Some("bad".to_string()), true));
    }

    #[test]
    fn default_transport_builds_with_codexplusplus_identity() {
        let client = build_client_with_config("", TransportConfig::default()).unwrap();
        drop(client);
    }

    #[test]
    fn explicit_http2_and_tls13_transport_builds() {
        let client = build_client_with_config(
            "CodexPlusPlus/Test",
            TransportConfig {
                http_version: HttpVersionMode::Http2,
                tls_min_version: TlsMinVersion::Tls13,
                tcp_nodelay: false,
                http2_adaptive_window: Some(false),
                http2_initial_stream_window_size: Some(65_535),
                http2_initial_connection_window_size: Some(65_535),
                http2_max_frame_size: Some(16_384),
                http2_max_header_list_size: Some(16_384),
                http2_keep_alive_interval_secs: Some(30),
                http2_keep_alive_timeout_secs: Some(10),
                http2_keep_alive_while_idle: Some(true),
            },
        )
        .unwrap();
        drop(client);
    }

    #[test]
    fn official_codex_compat_transport_builds_with_fixed_profile_defaults() {
        let config = TransportConfig::official_codex_compat();
        assert_eq!(config.http_version, HttpVersionMode::Auto);
        assert_eq!(config.http2_adaptive_window, Some(true));
        assert_eq!(config.http2_initial_stream_window_size, Some(1_048_576));
        let client = proxied_client_for_profile("", true).unwrap();
        drop(client);
    }

    #[test]
    fn official_profile_adds_codex_originator_and_request_headers() {
        assert_eq!(
            official_codex_default_headers()
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some(OFFICIAL_CODEX_ORIGINATOR)
        );
        assert_eq!(
            effective_user_agent_for_profile("Custom/UA", true),
            "codex_cli_rs/1.0.0"
        );

        let request = with_official_codex_request_headers(
            reqwest::Client::new()
                .post("https://example.test/v1/responses")
                .body("{}"),
            "fixture-session",
            Some("fixture-thread"),
        )
        .build()
        .unwrap();
        assert_eq!(
            request
                .headers()
                .get("session-id")
                .and_then(|value| value.to_str().ok()),
            Some("fixture-session")
        );
        assert_eq!(
            request
                .headers()
                .get("thread-id")
                .and_then(|value| value.to_str().ok()),
            Some("fixture-thread")
        );
        assert_eq!(
            request
                .headers()
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok()),
            Some("fixture-thread")
        );
    }

    #[test]
    fn non_official_profile_keeps_custom_user_agent() {
        assert_eq!(
            effective_user_agent_for_profile("Custom/UA", false),
            "Custom/UA"
        );
    }

    #[test]
    fn loopback_urls_bypass_environment_proxies() {
        assert!(is_loopback_url("http://127.0.0.1:58400/v1/responses"));
        assert!(is_loopback_url("http://127.12.34.56/v1/models"));
        assert!(is_loopback_url("http://[::1]:58400/v1/responses"));
        assert!(is_loopback_url("http://localhost:58400/v1/responses"));
        assert!(!is_loopback_url("https://api.example.test/v1/responses"));
        assert!(!is_loopback_url("not-a-url"));
    }
}
