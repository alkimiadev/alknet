//! Stealth decoy fallback for unknown paths (ADR-010, ADR-036).
//!
//! For paths not matched by the default surface (gateway endpoints,
//! `/healthz`, `/openapi.json`, the MCP route, the WS upgrade) nor by a
//! custom route (ADR-046), the HTTP handler serves a configurable decoy
//! (`DecoyConfig`): a fake nginx-style 404 (the default), a static site
//! served from a directory, or a redirect. The decoy must not leak alknet
//! presence — no alknet-specific headers, no alknet error format. See
//! `docs/architecture/crates/http/http-server.md` §"Stealth decoy".

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;

use crate::server::DecoyConfig;

pub async fn decoy_fallback(State(decoy): State<DecoyConfig>, request: Request) -> Response {
    match decoy {
        DecoyConfig::NotFound => fake_nginx_404(),
        DecoyConfig::StaticSite { root } => serve_static(&root, request).await,
        DecoyConfig::Redirect { to } => redirect(&to),
    }
}

pub fn fake_nginx_404() -> Response {
    let body = nginx_404_body();
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = StatusCode::NOT_FOUND;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp.headers_mut()
        .insert(header::SERVER, HeaderValue::from_static("nginx"));
    resp
}

pub fn redirect(to: &str) -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = StatusCode::FOUND;
    if let Ok(value) = HeaderValue::from_str(to) {
        resp.headers_mut().insert(header::LOCATION, value);
    }
    resp
}

pub async fn serve_static(root: &Path, request: Request) -> Response {
    let path = request.uri().path();
    let resolved = match resolve_static_path(root, path) {
        Some(p) => p,
        None => return fake_nginx_404(),
    };

    match tokio::fs::read(&resolved).await {
        Ok(bytes) => {
            let content_type = mime_for_path(&resolved);
            let mut resp = Response::new(Body::from(bytes));
            *resp.status_mut() = StatusCode::OK;
            resp.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            resp
        }
        Err(_) => fake_nginx_404(),
    }
}

fn resolve_static_path(root: &Path, request_path: &str) -> Option<PathBuf> {
    let trimmed = request_path.trim_start_matches('/');
    let relative = if trimmed.is_empty() {
        PathBuf::from("index.html")
    } else {
        let decoded = percent_decode(trimmed);
        PathBuf::from(decoded)
    };

    let mut safe = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if safe.as_os_str().is_empty() {
        return None;
    }

    let full = root.join(&safe);
    if full.is_dir() {
        return Some(full.join("index.html"));
    }
    if full.is_file() {
        return Some(full);
    }
    None
}

fn percent_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(((h << 4) | l) as char);
                i += 3;
                continue;
            }
        } else if b == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn mime_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn nginx_404_body() -> String {
    "<html>\r\n<head><title>404 Not Found</title></head>\r\n<body>\r\n<center><h1>404 Not Found</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;

    fn decoy_router(decoy: DecoyConfig) -> axum::Router {
        axum::Router::new()
            .fallback(decoy_fallback)
            .with_state(decoy)
    }

    async fn send(router: axum::Router, uri: &str) -> axum::response::Response {
        tower::ServiceExt::<Request<Body>>::oneshot(
            router,
            Request::builder().uri(uri).body(Body::empty()).unwrap(),
        )
        .await
        .unwrap()
    }

    #[test]
    fn decoy_config_default_is_not_found() {
        assert!(matches!(DecoyConfig::default(), DecoyConfig::NotFound));
    }

    #[tokio::test]
    async fn unknown_path_with_not_found_decoy_returns_404() {
        let resp = send(decoy_router(DecoyConfig::NotFound), "/nonexistent").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let server = resp
            .headers()
            .get(header::SERVER)
            .map(|v| v.to_str().unwrap().to_string());
        assert_eq!(server.as_deref(), Some("nginx"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(!body.contains("alknet"));
        assert!(body.contains("404 Not Found"));
    }

    #[tokio::test]
    async fn unknown_path_with_redirect_decoy_returns_redirect() {
        let decoy = DecoyConfig::Redirect {
            to: "https://example.com".to_string(),
        };
        let resp = send(decoy_router(decoy), "/anything").await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_string());
        assert_eq!(location.as_deref(), Some("https://example.com"));
    }

    #[tokio::test]
    async fn unknown_path_with_static_site_decoy_serves_file() {
        let dir = tempfile_dir();
        let file = dir.join("index.html");
        tokio::fs::write(&file, "<h1>hello</h1>").await.unwrap();

        let decoy = DecoyConfig::StaticSite { root: dir.clone() };
        let resp = send(decoy_router(decoy), "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ctype = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string());
        assert!(ctype.as_deref().unwrap_or("").starts_with("text/html"));
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"<h1>hello</h1>");
    }

    #[tokio::test]
    async fn static_site_decoy_serves_named_file() {
        let dir = tempfile_dir();
        tokio::fs::write(dir.join("about.html"), "<p>about</p>")
            .await
            .unwrap();

        let decoy = DecoyConfig::StaticSite { root: dir };
        let resp = send(decoy_router(decoy), "/about.html").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"<p>about</p>");
    }

    #[tokio::test]
    async fn static_site_decoy_missing_file_returns_fake_404() {
        let dir = tempfile_dir();
        let decoy = DecoyConfig::StaticSite { root: dir };
        let resp = send(decoy_router(decoy), "/missing.txt").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let server = resp
            .headers()
            .get(header::SERVER)
            .map(|v| v.to_str().unwrap().to_string());
        assert_eq!(server.as_deref(), Some("nginx"));
    }

    #[tokio::test]
    async fn static_site_decoy_path_traversal_is_blocked() {
        let dir = tempfile_dir();
        tokio::fs::write(dir.join("index.html"), "ok")
            .await
            .unwrap();
        let secret = dir.join("secret.txt");
        tokio::fs::write(&secret, "secret").await.unwrap();

        let parent = dir.parent().unwrap().to_path_buf();
        let decoy = DecoyConfig::StaticSite { root: dir.clone() };
        let resp = send(decoy_router(decoy), "/../secret.txt").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        drop(parent);
    }

    #[tokio::test]
    async fn not_found_decoy_does_not_leak_alknet_headers() {
        let resp = send(decoy_router(DecoyConfig::NotFound), "/whatever").await;
        for (name, value) in resp.headers().iter() {
            let name = name.as_str().to_lowercase();
            let value = value.to_str().unwrap_or("");
            assert!(
                !name.contains("alknet") && !value.contains("alknet"),
                "decoy leaked alknet: {name}={value}"
            );
        }
    }

    fn tempfile_dir() -> PathBuf {
        let dir =
            PathBuf::from("/tmp").join(format!("alknet-http-decoy-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
