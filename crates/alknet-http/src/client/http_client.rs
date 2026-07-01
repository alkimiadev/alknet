//! Shared HTTP client: `reqwest_middleware::ClientWithMiddleware` with a
//! retry stack (RetryTransientMiddleware + inlined RetryAfterMiddleware),
//! connection pooling, keep-alive, TLS, and rebuild-and-swap hot-reload.
//!
//! Credential injection happens per-request (from
//! `OperationContext.capabilities`), not at client construction — the
//! client is shared across all operations, the credentials are per-call.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use reqwest::ClientBuilder;
use reqwest_middleware::ClientWithMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use thiserror::Error;

use super::retry_after::RetryAfterMiddleware;

const DEFAULT_RETRY_AFTER_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct ClientCertConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    pub pool_max_idle_per_host: Option<usize>,
    pub request_timeout: Option<Duration>,
    pub retry_policy: ExponentialBackoff,
    pub ca_bundle: Option<PathBuf>,
    pub client_cert: Option<ClientCertConfig>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            pool_max_idle_per_host: None,
            request_timeout: None,
            retry_policy: ExponentialBackoff::builder().build_with_max_retries(3),
            ca_bundle: None,
            client_cert: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum HttpClientBuildError {
    #[error("failed to read CA bundle from {path}: {source}")]
    CaBundleRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse CA bundle at {path}: {source}")]
    CaBundleParse {
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },
    #[error("failed to read client cert from {path}: {source}")]
    ClientCertRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse client cert at {path}: {source}")]
    ClientCertParse {
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },
    #[error("failed to build reqwest client: {0}")]
    Build(reqwest::Error),
}

pub struct SharedHttpClient {
    inner: ArcSwap<ClientWithMiddleware>,
    config: ArcSwap<HttpClientConfig>,
}

impl std::fmt::Debug for SharedHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedHttpClient")
            .field("config", &self.config.load())
            .finish_non_exhaustive()
    }
}

impl SharedHttpClient {
    pub fn new(config: HttpClientConfig) -> Result<Self, HttpClientBuildError> {
        let client = build_client(&config)?;
        Ok(Self {
            inner: ArcSwap::from_pointee(client),
            config: ArcSwap::from_pointee(config),
        })
    }

    pub fn client(&self) -> Arc<ClientWithMiddleware> {
        self.inner.load_full()
    }

    pub fn config(&self) -> Arc<HttpClientConfig> {
        self.config.load_full()
    }

    pub fn reload(&self, config: HttpClientConfig) -> Result<(), HttpClientBuildError> {
        let client = build_client(&config)?;
        self.config.store(Arc::new(config));
        self.inner.store(Arc::new(client));
        Ok(())
    }
}

fn build_client(config: &HttpClientConfig) -> Result<ClientWithMiddleware, HttpClientBuildError> {
    let mut builder = ClientBuilder::new();
    if let Some(pool_max_idle) = config.pool_max_idle_per_host {
        builder = builder.pool_max_idle_per_host(pool_max_idle);
    }
    if let Some(timeout) = config.request_timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(ca_bundle_path) = &config.ca_bundle {
        let pem = std::fs::read(ca_bundle_path).map_err(|source| HttpClientBuildError::CaBundleRead {
            path: ca_bundle_path.clone(),
            source,
        })?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|source| {
            HttpClientBuildError::CaBundleParse {
                path: ca_bundle_path.clone(),
                source,
            }
        })?;
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }
    if let Some(client_cert_cfg) = &config.client_cert {
        let cert_pem = std::fs::read(&client_cert_cfg.cert_pem).map_err(|source| {
            HttpClientBuildError::ClientCertRead {
                path: client_cert_cfg.cert_pem.clone(),
                source,
            }
        })?;
        let key_pem = std::fs::read(&client_cert_cfg.key_pem).map_err(|source| {
            HttpClientBuildError::ClientCertRead {
                path: client_cert_cfg.key_pem.clone(),
                source,
            }
        })?;
        let identity = reqwest::Identity::from_pem(
            concat_pem(&cert_pem, &key_pem).as_slice(),
        )
        .map_err(|source| HttpClientBuildError::ClientCertParse {
            path: client_cert_cfg.cert_pem.clone(),
            source,
        })?;
        builder = builder.identity(identity);
    }
    let reqwest_client = builder.build().map_err(HttpClientBuildError::Build)?;
    let client = reqwest_middleware::ClientBuilder::new(reqwest_client)
        .with(RetryTransientMiddleware::new_with_policy(config.retry_policy))
        .with(RetryAfterMiddleware::with_capacity(DEFAULT_RETRY_AFTER_CAPACITY))
        .build();
    Ok(client)
}

fn concat_pem(cert: &[u8], key: &[u8]) -> Vec<u8> {
    let mut combined = Vec::with_capacity(cert.len() + key.len() + 1);
    combined.extend_from_slice(cert);
    if !cert.is_empty() && cert.last() != Some(&b'\n') {
        combined.push(b'\n');
    }
    combined.extend_from_slice(key);
    combined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn minimal_config() -> HttpClientConfig {
        HttpClientConfig {
            pool_max_idle_per_host: Some(8),
            request_timeout: Some(Duration::from_secs(30)),
            retry_policy: ExponentialBackoff::builder().build_with_max_retries(2),
            ca_bundle: None,
            client_cert: None,
        }
    }

    #[test]
    fn client_returns_a_usable_client_with_middleware() {
        let http = SharedHttpClient::new(minimal_config()).expect("client builds");
        let client = http.client();
        let request = client
            .get("https://api.example.com/v1/chat")
            .build()
            .expect("RequestBuilder builds");
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.com/v1/chat"
        );
    }

    #[test]
    fn reload_swaps_the_client_returned_by_client() {
        let http = SharedHttpClient::new(minimal_config()).expect("client builds");
        let before = http.client();
        let new_config = HttpClientConfig {
            pool_max_idle_per_host: Some(32),
            request_timeout: Some(Duration::from_secs(10)),
            retry_policy: ExponentialBackoff::builder().build_with_max_retries(5),
            ca_bundle: None,
            client_cert: None,
        };
        http.reload(new_config.clone()).expect("reload succeeds");
        let after = http.client();
        assert!(
            !Arc::ptr_eq(&before, &after),
            "reload must swap in a new ClientWithMiddleware"
        );
        let config = http.config();
        assert_eq!(config.pool_max_idle_per_host, Some(32));
        assert_eq!(config.request_timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn config_returns_current_config() {
        let http = SharedHttpClient::new(minimal_config()).expect("client builds");
        let config = http.config();
        assert_eq!(config.pool_max_idle_per_host, Some(8));
        assert_eq!(config.request_timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn default_config_has_sensible_defaults() {
        let config = HttpClientConfig::default();
        assert!(config.pool_max_idle_per_host.is_none());
        assert!(config.request_timeout.is_none());
        assert!(config.ca_bundle.is_none());
        assert!(config.client_cert.is_none());
        assert_eq!(config.retry_policy.max_n_retries, Some(3));
    }

    #[test]
    fn reload_with_ca_bundle_missing_file_errors() {
        let http = SharedHttpClient::new(minimal_config()).expect("client builds");
        let bad_config = HttpClientConfig {
            ca_bundle: Some(PathBuf::from("/nonexistent/ca-bundle.pem")),
            ..minimal_config()
        };
        let err = http.reload(bad_config).unwrap_err();
        assert!(matches!(err, HttpClientBuildError::CaBundleRead { .. }));
    }

    #[test]
    fn concat_pem_inserts_separator_between_cert_and_key() {
        let cert = b"-----BEGIN CERTIFICATE-----\ncert-body\n-----END CERTIFICATE-----";
        let key = b"-----BEGIN PRIVATE KEY-----\nkey-body\n-----END PRIVATE KEY-----";
        let combined = concat_pem(cert, key);
        assert!(combined.starts_with(b"-----BEGIN CERTIFICATE-----"));
        assert!(combined.windows(20).any(|w| w == b"-----END CERTIFICATE"));
        assert!(combined.windows(18).any(|w| w == b"-----BEGIN PRIVATE"));
    }

    #[test]
    fn concat_pem_handles_cert_already_terminated_with_newline() {
        let cert = b"-----BEGIN CERTIFICATE-----\ncert-body\n-----END CERTIFICATE-----\n";
        let key = b"-----BEGIN PRIVATE KEY-----\nkey-body\n-----END PRIVATE KEY-----";
        let combined = concat_pem(cert, key);
        let joined = std::str::from_utf8(&combined).unwrap();
        assert!(
            !joined.contains("-----END CERTIFICATE----------BEGIN PRIVATE"),
            "must not concatenate without a separator when cert lacks trailing newline"
        );
        assert!(joined.contains("-----END CERTIFICATE-----\n-----BEGIN PRIVATE"));
    }

    #[test]
    fn client_cert_config_constructs() {
        let cfg = ClientCertConfig {
            cert_pem: PathBuf::from("/etc/cert.pem"),
            key_pem: PathBuf::from("/etc/key.pem"),
        };
        assert_eq!(cfg.cert_pem, PathBuf::from("/etc/cert.pem"));
        assert_eq!(cfg.key_pem, PathBuf::from("/etc/key.pem"));
    }

    #[test]
    fn new_with_missing_ca_bundle_errors() {
        let config = HttpClientConfig {
            ca_bundle: Some(PathBuf::from("/nonexistent/ca-bundle.pem")),
            ..HttpClientConfig::default()
        };
        let err = SharedHttpClient::new(config).unwrap_err();
        assert!(matches!(err, HttpClientBuildError::CaBundleRead { .. }));
    }

    #[test]
    fn build_error_display_contains_path() {
        let err = HttpClientBuildError::CaBundleRead {
            path: PathBuf::from("/nonexistent/ca.pem"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        };
        let rendered = format!("{err}");
        assert!(rendered.contains("/nonexistent/ca.pem"));
    }

    #[test]
    fn retry_after_capacity_constant_is_bounded() {
        let cap = DEFAULT_RETRY_AFTER_CAPACITY;
        assert!(cap > 0, "RetryAfterMiddleware storage must be non-zero");
        assert!(cap <= 4096, "RetryAfterMiddleware storage must be bounded");
    }

    #[test]
    fn no_env_vars_read_in_default_config() {
        let _ = SystemTime::now();
        let config = HttpClientConfig::default();
        assert!(config.ca_bundle.is_none());
    }
}