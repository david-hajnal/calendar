use std::{
    collections::VecDeque,
    future::Future,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use commoncal_backend::ics_http::{
    DnsError, DnsResolver, SafeHttpClient, SafeHttpConfig, SafeHttpErrorCode, Transport,
    TransportError, TransportRequest, TransportResponse,
};
use tracing::{Instrument, instrument::WithSubscriber};
use tracing_subscriber::fmt::MakeWriter;

#[tokio::test]
async fn accepts_a_public_https_target() {
    let resolver = StubResolver::returning(vec![public_ip()]);
    let transport = StubTransport::returning(TransportResponse::ok(b"calendar".to_vec()));
    let client = client(resolver, transport);

    let response = client
        .fetch("https://calendar.example/basic.ics")
        .await
        .expect("public HTTPS target should be fetched");

    assert_eq!(response.body(), b"calendar");
}

#[tokio::test]
async fn rejects_localhost_and_private_ranges() {
    for address in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "169.254.169.254",
        "::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "::",
        "fd00:ec2::254",
    ] {
        let resolver = StubResolver::returning(vec![address.parse().unwrap()]);
        let transport = StubTransport::returning(TransportResponse::ok(Vec::new()));
        let client = client(resolver, transport);

        let error = client
            .fetch("https://calendar.example/feed.ics")
            .await
            .expect_err(address);

        assert_eq!(error.code(), SafeHttpErrorCode::DestinationNotAllowed);
    }
}

#[tokio::test]
async fn rejects_ipv4_mapped_ipv6_bypass() {
    let resolver = StubResolver::returning(vec!["::ffff:127.0.0.1".parse().unwrap()]);
    let transport = StubTransport::returning(TransportResponse::ok(Vec::new()));
    let client = client(resolver, transport);

    let error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("mapped loopback must be rejected");

    assert_eq!(error.code(), SafeHttpErrorCode::DestinationNotAllowed);
}

#[tokio::test]
async fn rejects_alternative_numeric_address_representation() {
    let resolver = StubResolver::returning(vec![public_ip()]);
    let transport = StubTransport::returning(TransportResponse::ok(Vec::new()));
    let client = client(resolver, transport);

    let error = client
        .fetch("https://2130706433/feed.ics")
        .await
        .expect_err("integer loopback representation must be rejected");

    assert_eq!(error.code(), SafeHttpErrorCode::DestinationNotAllowed);
}

#[tokio::test]
async fn rejects_redirect_to_private_address() {
    let resolver = StubResolver::with_results([
        Ok(vec![public_ip()]),
        Ok(vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))]),
    ]);
    let transport = StubTransport::with_results([
        Ok(TransportResponse::redirect(
            "https://internal.example/feed.ics",
        )),
        Ok(TransportResponse::ok(Vec::new())),
    ]);
    let client = client(resolver, transport.clone());

    let error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("private redirect must be rejected");

    assert_eq!(error.code(), SafeHttpErrorCode::DestinationNotAllowed);
    assert_eq!(transport.request_count(), 1);
}

#[tokio::test]
async fn pins_the_validated_dns_result_for_the_connection() {
    let resolver = StubResolver::with_results([
        Ok(vec![public_ip()]),
        Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]),
    ]);
    let transport = StubTransport::returning(TransportResponse::ok(Vec::new()));
    let client = client(resolver.clone(), transport.clone());

    client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect("validated address should be pinned");

    assert_eq!(resolver.call_count(), 1);
    assert_eq!(transport.requests()[0].addresses(), &[public_ip()]);
}

#[tokio::test]
async fn rejects_when_any_dns_result_is_not_public() {
    let resolver = StubResolver::returning(vec![public_ip(), IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    let transport = StubTransport::returning(TransportResponse::ok(Vec::new()));
    let client = client(resolver, transport.clone());

    let error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("mixed DNS results must fail closed");

    assert_eq!(error.code(), SafeHttpErrorCode::DestinationNotAllowed);
    assert_eq!(transport.request_count(), 0);
}

#[tokio::test]
async fn rejects_oversized_compressed_response() {
    let resolver = StubResolver::returning(vec![public_ip()]);
    let transport = StubTransport::returning(TransportResponse::ok(vec![b'a'; 33]));
    let client = client(resolver, transport);

    let error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("compressed limit must be enforced");

    assert_eq!(error.code(), SafeHttpErrorCode::CompressedTooLarge);
}

#[tokio::test]
async fn rejects_oversized_decompressed_response() {
    let resolver = StubResolver::returning(vec![public_ip()]);
    let compressed = gzip(&[b'a'; 33]);
    let transport = StubTransport::returning(TransportResponse::encoded(compressed, "gzip"));
    let mut config = config();
    config.max_decompressed_bytes = 32;
    let client = SafeHttpClient::new(config, resolver, transport).unwrap();

    let error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("decompressed limit must be enforced");

    assert_eq!(error.code(), SafeHttpErrorCode::DecompressedTooLarge);
}

#[tokio::test]
async fn rejects_slow_response() {
    let resolver = StubResolver::returning(vec![public_ip()]);
    let transport =
        StubTransport::delayed(Duration::from_millis(50), TransportResponse::ok(Vec::new()));
    let mut config = config();
    config.total_timeout = Duration::from_millis(5);
    let client = SafeHttpClient::new(config, resolver, transport).unwrap();

    let error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("total timeout must be enforced");

    assert_eq!(error.code(), SafeHttpErrorCode::Timeout);
}

#[tokio::test]
async fn rejects_credentials_and_redacts_sensitive_url_parts_in_logs() {
    let (writer, captured) = CapturedWriter::new();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(writer)
        .finish();
    let resolver = StubResolver::returning(vec![public_ip()]);
    let transport = StubTransport::returning(TransportResponse::ok(Vec::new()));
    let client = client(resolver, transport);
    let secret = "query-secret";
    let password = "password-secret";

    let error = client
        .fetch(&format!(
            "https://user:{password}@calendar.example/private/feed.ics?token={secret}"
        ))
        .instrument(tracing::info_span!(parent: None, "ics_fetch_test"))
        .with_subscriber(subscriber)
        .await
        .expect_err("URL credentials must be rejected");

    assert_eq!(error.code(), SafeHttpErrorCode::CredentialsNotAllowed);
    let output = captured.output();
    assert!(!output.contains(secret));
    assert!(!output.contains(password));
    assert!(!output.contains("/private/feed.ics"));
}

#[tokio::test]
async fn accepts_only_configured_schemes_and_limits_redirects() {
    let resolver = StubResolver::with_results([
        Ok(vec![public_ip()]),
        Ok(vec![public_ip()]),
        Ok(vec![public_ip()]),
    ]);
    let transport = StubTransport::with_results([
        Ok(TransportResponse::redirect(
            "https://calendar.example/again.ics",
        )),
        Ok(TransportResponse::redirect(
            "https://calendar.example/again.ics",
        )),
        Ok(TransportResponse::redirect(
            "https://calendar.example/again.ics",
        )),
    ]);
    let client = client(resolver.clone(), transport);

    let scheme_error = client
        .fetch("http://calendar.example/feed.ics")
        .await
        .expect_err("HTTP is disabled by default");
    assert_eq!(scheme_error.code(), SafeHttpErrorCode::SchemeNotAllowed);

    let redirect_error = client
        .fetch("https://calendar.example/feed.ics")
        .await
        .expect_err("redirect loop must be bounded");
    assert_eq!(redirect_error.code(), SafeHttpErrorCode::RedirectLimit);
}

fn public_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))
}

fn config() -> SafeHttpConfig {
    SafeHttpConfig {
        allowed_schemes: ["https".to_owned()].into_iter().collect(),
        max_redirects: 2,
        connect_timeout: Duration::from_millis(100),
        total_timeout: Duration::from_secs(1),
        max_compressed_bytes: 32,
        max_decompressed_bytes: 32,
    }
}

fn client(
    resolver: StubResolver,
    transport: StubTransport,
) -> SafeHttpClient<StubResolver, StubTransport> {
    SafeHttpClient::new(config(), resolver, transport).unwrap()
}

#[derive(Clone)]
struct StubResolver {
    results: Arc<Mutex<VecDeque<ResolutionResult>>>,
    calls: Arc<Mutex<usize>>,
}

type ResolutionResult = Result<Vec<IpAddr>, DnsError>;

impl StubResolver {
    fn returning(addresses: Vec<IpAddr>) -> Self {
        Self::with_results([Ok(addresses)])
    }

    fn with_results(results: impl IntoIterator<Item = ResolutionResult>) -> Self {
        Self {
            results: Arc::new(Mutex::new(results.into_iter().collect())),
            calls: Arc::new(Mutex::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl DnsResolver for StubResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        _port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, DnsError>> + Send + 'a>> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(DnsError::new()))
        })
    }
}

#[derive(Clone)]
struct StubTransport {
    results: Arc<Mutex<VecDeque<StubResult>>>,
    requests: Arc<Mutex<Vec<TransportRequest>>>,
}

enum StubResult {
    Immediate(Result<TransportResponse, TransportError>),
    Delayed(Duration, TransportResponse),
}

impl StubTransport {
    fn returning(response: TransportResponse) -> Self {
        Self::with_results([Ok(response)])
    }

    fn delayed(delay: Duration, response: TransportResponse) -> Self {
        Self {
            results: Arc::new(Mutex::new(VecDeque::from([StubResult::Delayed(
                delay, response,
            )]))),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_results(
        results: impl IntoIterator<Item = Result<TransportResponse, TransportError>>,
    ) -> Self {
        Self {
            results: Arc::new(Mutex::new(
                results.into_iter().map(StubResult::Immediate).collect(),
            )),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn requests(&self) -> Vec<TransportRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Transport for StubTransport {
    fn send(
        &self,
        request: TransportRequest,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, TransportError>> + Send + '_>> {
        self.requests.lock().unwrap().push(request);
        let result = self.results.lock().unwrap().pop_front().unwrap();
        Box::pin(async move {
            match result {
                StubResult::Immediate(result) => result,
                StubResult::Delayed(delay, response) => {
                    tokio::time::sleep(delay).await;
                    Ok(response)
                }
            }
        })
    }
}

fn gzip(input: &[u8]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).unwrap();
    encoder.finish().unwrap()
}

#[derive(Clone)]
struct CapturedWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl CapturedWriter {
    fn new() -> (Self, CapturedOutput) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                bytes: Arc::clone(&bytes),
            },
            CapturedOutput { bytes },
        )
    }
}

impl<'a> MakeWriter<'a> for CapturedWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CapturedOutput {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl CapturedOutput {
    fn output(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}
