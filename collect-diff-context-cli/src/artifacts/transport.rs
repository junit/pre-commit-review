use super::contract::{ArtifactError, ArtifactPackRecord};
use crate::impact_context::cache::file_facts::{
    open_regular_file_no_follow, set_private_file_permissions,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
use url::Url;

const RELEASE_REPOSITORY: &str = "junit/pre-commit-review";
const HARD_MAX_RESPONSE_BYTES: u64 = 512 * 1024 * 1024;
const HARD_MAX_REDIRECTS: u8 = 3;
const HARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HARD_READ_TIMEOUT: Duration = Duration::from_secs(15);
const HARD_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECT_URL_BYTES: usize = 8 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportLimits {
    max_response_bytes: u64,
    max_redirects: u8,
    connect_timeout: Duration,
    read_timeout: Duration,
    total_timeout: Duration,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: HARD_MAX_RESPONSE_BYTES,
            max_redirects: HARD_MAX_REDIRECTS,
            connect_timeout: HARD_CONNECT_TIMEOUT,
            read_timeout: HARD_READ_TIMEOUT,
            total_timeout: HARD_TOTAL_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone)]
enum TransportKind {
    Local { path: PathBuf },
    ProjectAsset { url: Url },
}

#[derive(Debug, Clone)]
pub struct Transport {
    kind: TransportKind,
    expected_sha256: String,
}

#[derive(Debug)]
pub struct FetchedArtifact {
    file: NamedTempFile,
    size: u64,
    sha256: String,
}

impl FetchedArtifact {
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn open(&self) -> Result<File, ArtifactError> {
        self.file.reopen().map_err(|_| {
            ArtifactError::new(
                "transport-temporary-open",
                "could not reopen verified transport bytes",
            )
        })
    }
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: Url,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
    pub total_timeout: Duration,
}

pub struct HttpResponse {
    pub status: u16,
    pub location: Option<String>,
    pub content_length: Option<u64>,
    pub content_encoding: Option<String>,
    pub body: Box<dyn Read + Send>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpBackendError {
    Timeout,
    Connection,
    Protocol,
}

pub trait HttpBackend: Send + Sync {
    fn get(&self, request: HttpRequest) -> Result<HttpResponse, HttpBackendError>;
}

impl Transport {
    pub fn local(path: &Path, expected_sha256: &str) -> Result<Self, ArtifactError> {
        if !path.is_absolute() {
            return Err(error(
                "transport-path-not-absolute",
                "local artifact path must be absolute",
            ));
        }
        validate_sha256(expected_sha256)?;
        Ok(Self {
            kind: TransportKind::Local {
                path: path.to_path_buf(),
            },
            expected_sha256: expected_sha256.to_string(),
        })
    }

    pub fn project_asset(record: &ArtifactPackRecord) -> Result<Self, ArtifactError> {
        record.validate()?;
        let url = Url::parse(&format!(
            "https://github.com/{RELEASE_REPOSITORY}/releases/download/{}/{}",
            record.project_release_tag, record.project_asset_name
        ))
        .map_err(|_| {
            error(
                "transport-url",
                "project artifact URL could not be constructed",
            )
        })?;
        validate_project_url(&url, true)?;
        Ok(Self {
            kind: TransportKind::ProjectAsset { url },
            expected_sha256: record.pack_sha256.clone(),
        })
    }

    pub fn fetch(&self, record: &ArtifactPackRecord) -> Result<FetchedArtifact, ArtifactError> {
        match &self.kind {
            TransportKind::Local { .. } => self.fetch_local(record, &TransportLimits::default()),
            TransportKind::ProjectAsset { .. } => {
                self.fetch_with_backend(record, &TransportLimits::default(), &UreqBackend)
            }
        }
    }

    pub fn fetch_with_backend<B: HttpBackend>(
        &self,
        record: &ArtifactPackRecord,
        limits: &TransportLimits,
        backend: &B,
    ) -> Result<FetchedArtifact, ArtifactError> {
        self.validate_selection(record)?;
        match &self.kind {
            TransportKind::Local { .. } => self.fetch_local(record, limits),
            TransportKind::ProjectAsset { url } => {
                self.fetch_project(url.clone(), record, limits, backend)
            }
        }
    }

    fn fetch_local(
        &self,
        record: &ArtifactPackRecord,
        limits: &TransportLimits,
    ) -> Result<FetchedArtifact, ArtifactError> {
        self.validate_selection(record)?;
        let TransportKind::Local { path } = &self.kind else {
            return Err(error(
                "transport-source",
                "artifact transport source is inconsistent",
            ));
        };
        let file = open_regular_file_no_follow(path).map_err(|_| {
            error(
                "transport-local-open",
                "local artifact bytes could not be opened safely",
            )
        })?;
        copy_verified(
            file,
            record,
            limits.max_response_bytes,
            None,
            Instant::now(),
            limits.total_timeout,
        )
    }

    fn fetch_project<B: HttpBackend>(
        &self,
        mut url: Url,
        record: &ArtifactPackRecord,
        limits: &TransportLimits,
        backend: &B,
    ) -> Result<FetchedArtifact, ArtifactError> {
        let started = Instant::now();
        let mut redirects = 0_u8;
        loop {
            let remaining = limits
                .total_timeout
                .checked_sub(started.elapsed())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| error("transport-timeout", "artifact transport timed out"))?;
            let request = HttpRequest {
                url: url.clone(),
                connect_timeout: limits.connect_timeout.min(remaining),
                read_timeout: limits.read_timeout.min(remaining),
                total_timeout: remaining,
            };
            let response = backend.get(request).map_err(map_backend_error)?;
            if is_redirect(response.status) {
                if redirects >= limits.max_redirects.min(HARD_MAX_REDIRECTS) {
                    return Err(error(
                        "transport-redirect-limit",
                        "artifact transport exceeded its redirect limit",
                    ));
                }
                let location = response.location.as_deref().ok_or_else(|| {
                    error(
                        "transport-redirect-invalid",
                        "artifact redirect is missing its location",
                    )
                })?;
                if location.len() > MAX_REDIRECT_URL_BYTES {
                    return Err(error(
                        "transport-redirect-invalid",
                        "artifact redirect location exceeds its limit",
                    ));
                }
                let next = url.join(location).map_err(|_| {
                    error(
                        "transport-redirect-invalid",
                        "artifact redirect location is invalid",
                    )
                })?;
                validate_project_url(&next, false)?;
                redirects += 1;
                url = next;
                continue;
            }
            if response.status != 200 {
                return Err(error(
                    "transport-http-status",
                    "artifact transport returned an unsuccessful status",
                ));
            }
            if response
                .content_encoding
                .as_deref()
                .is_some_and(|encoding| {
                    !encoding.is_empty() && !encoding.eq_ignore_ascii_case("identity")
                })
            {
                return Err(error(
                    "transport-content-encoding",
                    "artifact transport must return identity encoded bytes",
                ));
            }
            let effective_limit = effective_limit(record, limits.max_response_bytes);
            if response
                .content_length
                .is_some_and(|length| length > effective_limit)
            {
                return Err(error(
                    "transport-byte-limit",
                    "artifact transport exceeded its byte limit",
                ));
            }
            if response
                .content_length
                .is_some_and(|length| length != record.expected_compressed_size)
            {
                return Err(error(
                    "transport-size-mismatch",
                    "artifact transport size does not match the selected record",
                ));
            }
            return copy_verified(
                response.body,
                record,
                limits.max_response_bytes,
                response.content_length,
                started,
                limits.total_timeout,
            );
        }
    }

    fn validate_selection(&self, record: &ArtifactPackRecord) -> Result<(), ArtifactError> {
        record.validate()?;
        if self.expected_sha256 != record.pack_sha256 {
            return Err(error(
                "transport-selection-mismatch",
                "artifact transport does not match the selected record",
            ));
        }
        if let TransportKind::ProjectAsset { url } = &self.kind {
            let expected = Self::project_asset(record)?;
            if !matches!(expected.kind, TransportKind::ProjectAsset { url: expected_url } if expected_url == *url)
            {
                return Err(error(
                    "transport-selection-mismatch",
                    "project artifact URL does not match the selected record",
                ));
            }
        }
        Ok(())
    }
}

fn copy_verified<R: Read>(
    mut reader: R,
    record: &ArtifactPackRecord,
    requested_limit: u64,
    content_length: Option<u64>,
    started: Instant,
    total_timeout: Duration,
) -> Result<FetchedArtifact, ArtifactError> {
    let limit = effective_limit(record, requested_limit);
    if record.expected_compressed_size > limit {
        return Err(error(
            "transport-byte-limit",
            "selected artifact exceeds the transport byte limit",
        ));
    }
    if content_length.is_some_and(|length| length > limit) {
        return Err(error(
            "transport-byte-limit",
            "artifact transport exceeded its byte limit",
        ));
    }
    let mut temporary = NamedTempFile::new().map_err(|_| {
        error(
            "transport-temporary-file",
            "could not create a private artifact transport file",
        )
    })?;
    set_private_file_permissions(temporary.as_file()).map_err(|_| {
        error(
            "transport-temporary-file",
            "could not protect the artifact transport file",
        )
    })?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        if started.elapsed() >= total_timeout {
            return Err(error("transport-timeout", "artifact transport timed out"));
        }
        let count = reader.read(&mut buffer).map_err(|_| {
            error(
                "transport-read",
                "artifact transport body could not be read",
            )
        })?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| error("transport-byte-limit", "artifact byte count overflowed"))?;
        if size > limit {
            return Err(error(
                "transport-byte-limit",
                "artifact transport exceeded its byte limit",
            ));
        }
        digest.update(&buffer[..count]);
        temporary.write_all(&buffer[..count]).map_err(|_| {
            error(
                "transport-temporary-write",
                "artifact transport bytes could not be staged",
            )
        })?;
    }
    if size != record.expected_compressed_size {
        return Err(error(
            "transport-size-mismatch",
            "artifact transport size does not match the selected record",
        ));
    }
    let sha256 = format!("{:x}", digest.finalize());
    if sha256 != record.pack_sha256 {
        return Err(error(
            "transport-digest-mismatch",
            "artifact transport digest does not match the selected record",
        ));
    }
    temporary.as_file().sync_all().map_err(|_| {
        error(
            "transport-temporary-sync",
            "artifact transport bytes could not be synchronized",
        )
    })?;
    Ok(FetchedArtifact {
        file: temporary,
        size,
        sha256,
    })
}

fn effective_limit(record: &ArtifactPackRecord, requested: u64) -> u64 {
    record
        .max_compressed_size
        .min(requested)
        .min(HARD_MAX_RESPONSE_BYTES)
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn validate_project_url(url: &Url, initial: bool) -> Result<(), ArtifactError> {
    if url.scheme() != "https" {
        return Err(error(
            "transport-protocol-downgrade",
            "artifact transport requires HTTPS",
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return Err(error(
            "transport-redirect-invalid",
            "artifact transport URL authority is invalid",
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        error(
            "transport-redirect-invalid",
            "artifact transport URL has no host",
        )
    })?;
    let allowed = if initial {
        host == "github.com"
    } else {
        matches!(
            host,
            "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
        )
    };
    if !allowed {
        return Err(error(
            "transport-redirect-host",
            "artifact transport redirect host is not authorized",
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ArtifactError> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(error(
            "transport-digest-invalid",
            "artifact transport digest is invalid",
        ));
    }
    Ok(())
}

fn map_backend_error(backend_error: HttpBackendError) -> ArtifactError {
    match backend_error {
        HttpBackendError::Timeout => error("transport-timeout", "artifact transport timed out"),
        HttpBackendError::Connection => error(
            "transport-connect",
            "artifact transport connection could not be established",
        ),
        HttpBackendError::Protocol => {
            error("transport-protocol", "artifact transport protocol failed")
        }
    }
}

struct UreqBackend;

impl HttpBackend for UreqBackend {
    fn get(&self, request: HttpRequest) -> Result<HttpResponse, HttpBackendError> {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .accept_encoding("identity")
            .max_response_header_size(32 * 1024)
            .timeout_global(Some(request.total_timeout))
            .timeout_connect(Some(request.connect_timeout))
            .timeout_recv_response(Some(request.read_timeout))
            .timeout_recv_body(Some(request.read_timeout))
            .build();
        let agent: ureq::Agent = config.into();
        let http_request = ureq::http::Request::get(request.url.as_str())
            .header("accept-encoding", "identity")
            .header("user-agent", "pre-commit-review-artifact-manager/1")
            .body(())
            .map_err(|_| HttpBackendError::Protocol)?;
        let response = agent.run(http_request).map_err(|error| match error {
            ureq::Error::Timeout(_) => HttpBackendError::Timeout,
            ureq::Error::Protocol(_)
            | ureq::Error::BadUri(_)
            | ureq::Error::RequireHttpsOnly(_)
            | ureq::Error::TooManyRedirects
            | ureq::Error::RedirectFailed
            | ureq::Error::LargeResponseHeader(_, _) => HttpBackendError::Protocol,
            _ => HttpBackendError::Connection,
        })?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(ureq::http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_encoding = response
            .headers()
            .get(ureq::http::header::CONTENT_ENCODING)
            .map(|value| value.to_str().map(str::to_string))
            .transpose()
            .map_err(|_| HttpBackendError::Protocol)?;
        let content_length = response.body().content_length();
        let body = response.into_body().into_reader();
        Ok(HttpResponse {
            status,
            location,
            content_length,
            content_encoding,
            body: Box::new(body),
        })
    }
}

fn error(code: &'static str, message: &'static str) -> ArtifactError {
    ArtifactError::new(code, message)
}
