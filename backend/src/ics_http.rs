use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display, Formatter},
    future::Future,
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    time::Duration,
};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use url::{Host, Url};

#[derive(Clone, Debug)]
pub struct SafeHttpConfig {
    pub allowed_schemes: HashSet<String>,
    pub max_redirects: usize,
    pub connect_timeout: Duration,
    pub total_timeout: Duration,
    pub max_compressed_bytes: usize,
    pub max_decompressed_bytes: usize,
}

impl Default for SafeHttpConfig {
    fn default() -> Self {
        Self {
            allowed_schemes: HashSet::from(["https".to_owned()]),
            max_redirects: 3,
            connect_timeout: Duration::from_secs(3),
            total_timeout: Duration::from_secs(10),
            max_compressed_bytes: 2 * 1024 * 1024,
            max_decompressed_bytes: 10 * 1024 * 1024,
        }
    }
}

pub trait DnsResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, DnsError>> + Send + 'a>>;
}

pub trait Transport: Send + Sync {
    fn send(
        &self,
        request: TransportRequest,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, TransportError>> + Send + '_>>;
}

#[derive(Clone, Debug)]
pub struct TransportRequest {
    url: Url,
    addresses: Vec<IpAddr>,
    connect_timeout: Duration,
    max_compressed_bytes: usize,
    headers: HashMap<String, String>,
}

impl TransportRequest {
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }
}

#[derive(Clone, Debug)]
pub struct TransportResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl TransportResponse {
    pub fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body,
        }
    }

    pub fn redirect(location: impl Into<String>) -> Self {
        Self {
            status: 302,
            headers: HashMap::from([("location".to_owned(), location.into())]),
            body: Vec::new(),
        }
    }

    pub fn encoded(body: Vec<u8>, encoding: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: HashMap::from([("content-encoding".to_owned(), encoding.into())]),
            body,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeHttpResponse {
    status: u16,
    body: Vec<u8>,
    headers: HashMap<String, String>,
}

impl SafeHttpResponse {
    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafeHttpErrorCode {
    InvalidConfiguration,
    InvalidUrl,
    SchemeNotAllowed,
    CredentialsNotAllowed,
    ResolutionFailed,
    DestinationNotAllowed,
    TransportFailed,
    InvalidRedirect,
    RedirectLimit,
    Timeout,
    CompressedTooLarge,
    DecompressedTooLarge,
    UnsupportedEncoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafeHttpError {
    code: SafeHttpErrorCode,
}

impl SafeHttpError {
    fn new(code: SafeHttpErrorCode) -> Self {
        Self { code }
    }

    pub fn code(&self) -> SafeHttpErrorCode {
        self.code
    }
}

impl Display for SafeHttpError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            SafeHttpErrorCode::InvalidConfiguration => "invalid outbound HTTP configuration",
            SafeHttpErrorCode::InvalidUrl => "invalid calendar URL",
            SafeHttpErrorCode::SchemeNotAllowed => "calendar URL scheme is not allowed",
            SafeHttpErrorCode::CredentialsNotAllowed => "calendar URL credentials are not allowed",
            SafeHttpErrorCode::ResolutionFailed => "calendar host could not be resolved",
            SafeHttpErrorCode::DestinationNotAllowed => "calendar destination is not allowed",
            SafeHttpErrorCode::TransportFailed => "calendar retrieval failed",
            SafeHttpErrorCode::InvalidRedirect => "calendar redirect is invalid",
            SafeHttpErrorCode::RedirectLimit => "calendar redirect limit exceeded",
            SafeHttpErrorCode::Timeout => "calendar retrieval timed out",
            SafeHttpErrorCode::CompressedTooLarge => "calendar response is too large",
            SafeHttpErrorCode::DecompressedTooLarge => "calendar response is too large",
            SafeHttpErrorCode::UnsupportedEncoding => "calendar response encoding is not supported",
        })
    }
}

impl std::error::Error for SafeHttpError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DnsError;

impl DnsError {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DnsError {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    Failed,
    Timeout,
    CompressedTooLarge,
}

#[derive(Clone)]
pub struct SafeHttpClient<R, T> {
    config: SafeHttpConfig,
    resolver: R,
    transport: T,
}

impl<R, T> SafeHttpClient<R, T>
where
    R: DnsResolver,
    T: Transport,
{
    pub fn new(config: SafeHttpConfig, resolver: R, transport: T) -> Result<Self, SafeHttpError> {
        let schemes_are_valid = !config.allowed_schemes.is_empty()
            && config
                .allowed_schemes
                .iter()
                .all(|scheme| scheme == "http" || scheme == "https");
        let limits_are_valid = !config.connect_timeout.is_zero()
            && !config.total_timeout.is_zero()
            && config.max_compressed_bytes > 0
            && config.max_decompressed_bytes > 0;

        if !schemes_are_valid || !limits_are_valid {
            return Err(SafeHttpError::new(SafeHttpErrorCode::InvalidConfiguration));
        }

        Ok(Self {
            config,
            resolver,
            transport,
        })
    }

    pub async fn fetch(&self, input: &str) -> Result<SafeHttpResponse, SafeHttpError> {
        self.fetch_with_headers(input, &HashMap::new()).await
    }

    pub async fn fetch_with_headers(
        &self,
        input: &str,
        headers: &HashMap<String, String>,
    ) -> Result<SafeHttpResponse, SafeHttpError> {
        tokio::time::timeout(
            self.config.total_timeout,
            self.fetch_with_redirects(input, headers),
        )
        .await
        .map_err(|_| SafeHttpError::new(SafeHttpErrorCode::Timeout))?
    }

    async fn fetch_with_redirects(
        &self,
        input: &str,
        headers: &HashMap<String, String>,
    ) -> Result<SafeHttpResponse, SafeHttpError> {
        let mut url =
            Url::parse(input).map_err(|_| SafeHttpError::new(SafeHttpErrorCode::InvalidUrl))?;

        for redirect_count in 0..=self.config.max_redirects {
            tracing::info!(
                target: "commoncal::ics_http",
                destination = %SafeLogDestination(&url),
                "fetching external calendar"
            );
            self.validate_url(&url)?;
            let addresses = self.resolve_and_validate(&url).await?;
            let response = self
                .transport
                .send(TransportRequest {
                    url: url.clone(),
                    addresses,
                    connect_timeout: self.config.connect_timeout,
                    max_compressed_bytes: self.config.max_compressed_bytes,
                    headers: headers.clone(),
                })
                .await
                .map_err(map_transport_error)?;

            if response.body.len() > self.config.max_compressed_bytes {
                return Err(SafeHttpError::new(SafeHttpErrorCode::CompressedTooLarge));
            }
            if is_redirect(response.status) {
                if redirect_count == self.config.max_redirects {
                    return Err(SafeHttpError::new(SafeHttpErrorCode::RedirectLimit));
                }
                let location = response
                    .headers
                    .get("location")
                    .ok_or_else(|| SafeHttpError::new(SafeHttpErrorCode::InvalidRedirect))?;
                url = url
                    .join(location)
                    .map_err(|_| SafeHttpError::new(SafeHttpErrorCode::InvalidRedirect))?;
                continue;
            }

            let body = decode_body(
                response.body,
                response.headers.get("content-encoding"),
                self.config.max_decompressed_bytes,
            )?;
            return Ok(SafeHttpResponse {
                status: response.status,
                body,
                headers: response.headers,
            });
        }

        Err(SafeHttpError::new(SafeHttpErrorCode::RedirectLimit))
    }

    fn validate_url(&self, url: &Url) -> Result<(), SafeHttpError> {
        if !self.config.allowed_schemes.contains(url.scheme()) {
            return Err(SafeHttpError::new(SafeHttpErrorCode::SchemeNotAllowed));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(SafeHttpError::new(SafeHttpErrorCode::CredentialsNotAllowed));
        }
        if url.host().is_none() {
            return Err(SafeHttpError::new(SafeHttpErrorCode::InvalidUrl));
        }
        Ok(())
    }

    async fn resolve_and_validate(&self, url: &Url) -> Result<Vec<IpAddr>, SafeHttpError> {
        let addresses = match url.host().expect("host checked before resolution") {
            Host::Ipv4(address) => vec![IpAddr::V4(address)],
            Host::Ipv6(address) => vec![IpAddr::V6(address)],
            Host::Domain(host) => self
                .resolver
                .resolve(
                    host,
                    url.port_or_known_default()
                        .ok_or_else(|| SafeHttpError::new(SafeHttpErrorCode::InvalidUrl))?,
                )
                .await
                .map_err(|_| SafeHttpError::new(SafeHttpErrorCode::ResolutionFailed))?,
        };

        if addresses.is_empty() {
            return Err(SafeHttpError::new(SafeHttpErrorCode::ResolutionFailed));
        }
        if addresses
            .iter()
            .any(|address| !is_public_destination(*address))
        {
            return Err(SafeHttpError::new(SafeHttpErrorCode::DestinationNotAllowed));
        }
        Ok(addresses)
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn map_transport_error(error: TransportError) -> SafeHttpError {
    SafeHttpError::new(match error {
        TransportError::Failed => SafeHttpErrorCode::TransportFailed,
        TransportError::Timeout => SafeHttpErrorCode::Timeout,
        TransportError::CompressedTooLarge => SafeHttpErrorCode::CompressedTooLarge,
    })
}

fn decode_body(
    body: Vec<u8>,
    encoding: Option<&String>,
    max_decompressed_bytes: usize,
) -> Result<Vec<u8>, SafeHttpError> {
    let encoding = encoding.map(|value| value.trim().to_ascii_lowercase());
    let mut reader: Box<dyn Read> = match encoding.as_deref() {
        None | Some("") | Some("identity") => Box::new(body.as_slice()),
        Some("gzip") => Box::new(GzDecoder::new(body.as_slice())),
        Some(_) => {
            return Err(SafeHttpError::new(SafeHttpErrorCode::UnsupportedEncoding));
        }
    };
    let mut decoded = Vec::with_capacity(max_decompressed_bytes.min(8 * 1024));
    reader
        .by_ref()
        .take(max_decompressed_bytes as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|_| SafeHttpError::new(SafeHttpErrorCode::UnsupportedEncoding))?;
    if decoded.len() > max_decompressed_bytes {
        return Err(SafeHttpError::new(SafeHttpErrorCode::DecompressedTooLarge));
    }
    Ok(decoded)
}

fn is_public_destination(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_ipv4(mapped);
            }
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !is_deprecated_site_local(address)
                && address != "fd00:ec2::254".parse::<Ipv6Addr>().unwrap()
        }
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    !address.is_loopback()
        && !address.is_private()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_unspecified()
        && address != Ipv4Addr::new(169, 254, 169, 254)
        && address != Ipv4Addr::new(100, 100, 100, 200)
}

fn is_deprecated_site_local(address: Ipv6Addr) -> bool {
    (address.segments()[0] & 0xffc0) == 0xfec0
}

struct SafeLogDestination<'a>(&'a Url);

impl Display for SafeLogDestination<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.scheme())?;
        formatter.write_str("://")?;
        formatter.write_str(self.0.host_str().unwrap_or("[invalid-host]"))?;
        if let Some(port) = self.0.port() {
            write!(formatter, ":{port}")?;
        }
        formatter.write_str("/[REDACTED]")
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TokioDnsResolver;

impl DnsResolver for TokioDnsResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, DnsError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::net::lookup_host((host, port))
                .await
                .map(|addresses| addresses.map(|address| address.ip()).collect())
                .map_err(|_| DnsError::new())
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReqwestTransport;

impl Transport for ReqwestTransport {
    fn send(
        &self,
        request: TransportRequest,
    ) -> Pin<Box<dyn Future<Output = Result<TransportResponse, TransportError>> + Send + '_>> {
        Box::pin(async move {
            let host = request.url.host_str().ok_or(TransportError::Failed)?;
            let port = request
                .url
                .port_or_known_default()
                .ok_or(TransportError::Failed)?;
            let socket_addresses: Vec<_> = request
                .addresses
                .iter()
                .map(|address| SocketAddr::new(*address, port))
                .collect();
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(request.connect_timeout)
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .no_zstd()
                .resolve_to_addrs(host, &socket_addresses)
                .build()
                .map_err(|_| TransportError::Failed)?;
            let mut request_builder = client.get(request.url);
            for (name, value) in request.headers {
                let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| TransportError::Failed)?;
                let value = reqwest::header::HeaderValue::from_str(&value)
                    .map_err(|_| TransportError::Failed)?;
                request_builder = request_builder.header(name, value);
            }
            let response = request_builder.send().await.map_err(|error| {
                if error.is_timeout() {
                    TransportError::Timeout
                } else {
                    TransportError::Failed
                }
            })?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_owned(), value.to_owned()))
                })
                .collect();
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| {
                    if error.is_timeout() {
                        TransportError::Timeout
                    } else {
                        TransportError::Failed
                    }
                })?;
                if body.len().saturating_add(chunk.len()) > request.max_compressed_bytes {
                    return Err(TransportError::CompressedTooLarge);
                }
                body.extend_from_slice(&chunk);
            }

            Ok(TransportResponse {
                status,
                headers,
                body,
            })
        })
    }
}

impl SafeHttpClient<TokioDnsResolver, ReqwestTransport> {
    pub fn production(config: SafeHttpConfig) -> Result<Self, SafeHttpError> {
        Self::new(config, TokioDnsResolver, ReqwestTransport)
    }
}
