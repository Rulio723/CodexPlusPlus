use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

const RELAY_LATENCY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RELAY_LATENCY_TOTAL_TIMEOUT: Duration = Duration::from_secs(8);
pub const RELAY_LATENCY_MAX_CONCURRENCY: usize = 4;

static RELAY_LATENCY_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayLatencyMeasurement {
    pub latency_ms: u64,
    pub http_status: u16,
}

fn relay_latency_semaphore() -> &'static Semaphore {
    RELAY_LATENCY_SEMAPHORE.get_or_init(|| Semaphore::new(RELAY_LATENCY_MAX_CONCURRENCY))
}

async fn with_relay_latency_permit<T, F>(operation: F) -> anyhow::Result<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let permit = relay_latency_semaphore()
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("供应商延迟检测队列已关闭"))?;
    let result = operation.await;
    drop(permit);
    result
}

pub async fn measure_relay_latency(
    target_url: &str,
    api_key: &str,
) -> anyhow::Result<RelayLatencyMeasurement> {
    measure_relay_latency_with_transport(target_url, api_key, false).await
}

pub async fn measure_relay_latency_with_transport(
    target_url: &str,
    api_key: &str,
    official_codex_fingerprint: bool,
) -> anyhow::Result<RelayLatencyMeasurement> {
    let url = reqwest::Url::parse(target_url.trim())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        anyhow::bail!("目标 URL 只支持有效的 HTTP 或 HTTPS 地址");
    }
    let api_key = api_key.trim();
    if api_key.is_empty() {
        anyhow::bail!("供应商 API Key 不能为空");
    }
    let models_url = crate::protocol_proxy::models_url(url.as_str());

    with_relay_latency_permit(async move {
        let client = crate::http_client::configured_client_builder_for_profile(
            &format!("CodexPlusPlus/{}", env!("CARGO_PKG_VERSION")),
            official_codex_fingerprint,
        )
        .no_proxy()
        .connect_timeout(RELAY_LATENCY_CONNECT_TIMEOUT)
        .timeout(RELAY_LATENCY_TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;
        let started = Instant::now();
        let request = client
            .get(models_url)
            .bearer_auth(api_key)
            .header(reqwest::header::ACCEPT, "application/json");
        let request = if official_codex_fingerprint {
            let session_id = uuid::Uuid::new_v4().to_string();
            crate::http_client::with_official_codex_request_headers(request, &session_id, None)
        } else {
            request
        };
        let response = request.send().await?;
        let latency_ms = started.elapsed().as_millis().max(1) as u64;
        let http_status = response.status().as_u16();
        if !response.status().is_success() {
            anyhow::bail!("模型列表验证返回 HTTP {http_status}");
        }

        Ok(RelayLatencyMeasurement {
            latency_ms,
            http_status,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn validates_models_endpoint_with_bearer_api_key() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer sk-test")
            );
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}",
                )
                .unwrap();
        });

        let result = measure_relay_latency(&format!("http://{address}/v1"), "sk-test")
            .await
            .unwrap();
        server.join().unwrap();

        assert_eq!(result.http_status, 200);
        assert!(result.latency_ms >= 1);
    }

    #[tokio::test]
    async fn official_transport_adds_codex_headers_to_latency_probe() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.starts_with("get /v1/models http/1.1"));
            assert!(request.contains("originator: codex_cli_rs"));
            assert!(request.contains("session-id: "));
            assert!(request.contains("user-agent: codex_cli_rs/1.0.0"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}",
                )
                .unwrap();
        });

        let result =
            measure_relay_latency_with_transport(&format!("http://{address}/v1"), "sk-test", true)
                .await
                .unwrap();
        server.join().unwrap();

        assert_eq!(result.http_status, 200);
        assert!(result.latency_ms >= 1);
    }

    #[tokio::test]
    async fn rejects_non_http_urls() {
        let error = measure_relay_latency("file:///tmp/config.toml", "sk-test")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("HTTP"));
    }

    #[tokio::test]
    async fn rejects_missing_api_key_before_sending_request() {
        let error = measure_relay_latency("https://api.example/v1", " ")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("API Key"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn limits_parallel_latency_operations() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks = (0..(RELAY_LATENCY_MAX_CONCURRENCY * 3)).map(|_| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            tokio::spawn(async move {
                with_relay_latency_permit(async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .unwrap();
            })
        });

        futures_util::future::join_all(tasks).await;

        assert!(peak.load(Ordering::SeqCst) <= RELAY_LATENCY_MAX_CONCURRENCY);
    }
}
