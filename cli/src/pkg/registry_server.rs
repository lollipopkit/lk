use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
};

use anyhow::Context;

// Maximum header line size to prevent DoS via unbounded header lines.
const MAX_HEADER_LINE_SIZE: usize = 8192;
// Maximum total header bytes to prevent DoS via many small header lines.
const MAX_HEADER_TOTAL_SIZE: usize = 65536;
// Maximum request body size to prevent OOM from untrusted content-length.
const MAX_CONTENT_LENGTH: usize = 10 * 1024 * 1024; // 10 MiB
use lk_core::package::{RegistryAsymmetricSigningKey, RegistryService, RegistrySigningKey, RegistrySigningKeyring};
use serde::Deserialize;

#[allow(clippy::too_many_arguments)]
pub(super) fn serve_registry(
    addr: String,
    storage: PathBuf,
    registry_url: String,
    token: Option<String>,
    auth_policy: Option<PathBuf>,
    signing_key_file: Option<PathBuf>,
    signing_keyring_file: Option<PathBuf>,
    signing_private_key_file: Option<PathBuf>,
    signing_key_id: Option<String>,
    signing_secret: Option<String>,
) -> anyhow::Result<()> {
    let auth = RegistryAuth::load(token, auth_policy)?;
    let service = registry_service(
        storage,
        registry_url,
        signing_key_file,
        signing_keyring_file,
        signing_private_key_file,
        signing_key_id,
        signing_secret,
    )?;
    let listener = TcpListener::bind(&addr).with_context(|| format!("bind registry server {addr}"))?;
    eprintln!("Serving LK registry at http://{addr}");
    for stream in listener.incoming() {
        let mut stream = stream.context("accept registry connection")?;
        let service = service.clone();
        let auth = auth.clone();
        std::thread::spawn(move || {
            if let Err(error) = handle_stream(&mut stream, &service, &auth) {
                let body = format!("registry request failed: {error:#}");
                let _ = write_response(&mut stream, 500, "Internal Server Error", "text/plain", body.as_bytes());
            }
        });
    }
    Ok(())
}

fn registry_service(
    storage: PathBuf,
    registry_url: String,
    signing_key_file: Option<PathBuf>,
    signing_keyring_file: Option<PathBuf>,
    signing_private_key_file: Option<PathBuf>,
    signing_key_id: Option<String>,
    signing_secret: Option<String>,
) -> anyhow::Result<RegistryService> {
    let mut service = RegistryService::new(storage, registry_url);
    match resolve_signing_material(
        signing_key_file,
        signing_keyring_file,
        signing_private_key_file,
        signing_key_id,
        signing_secret,
    )? {
        Some(RegistryServerSigningMaterial::Hmac(signing_key)) => {
            service = service.with_signing_key(signing_key);
        }
        Some(RegistryServerSigningMaterial::Ed25519(signing_key)) => {
            service = service.with_asymmetric_signing_key(signing_key);
        }
        None => {}
    }
    Ok(service)
}

#[derive(Debug)]
enum RegistryServerSigningMaterial {
    Hmac(RegistrySigningKey),
    Ed25519(RegistryAsymmetricSigningKey),
}

fn resolve_signing_material(
    signing_key_file: Option<PathBuf>,
    signing_keyring_file: Option<PathBuf>,
    signing_private_key_file: Option<PathBuf>,
    signing_key_id: Option<String>,
    signing_secret: Option<String>,
) -> anyhow::Result<Option<RegistryServerSigningMaterial>> {
    let configured = usize::from(signing_key_file.is_some())
        + usize::from(signing_keyring_file.is_some())
        + usize::from(signing_private_key_file.is_some())
        + usize::from(signing_key_id.is_some() || signing_secret.is_some());
    if configured > 1 {
        anyhow::bail!(
            "configure only one registry signing source: --signing-private-key-file, --signing-keyring-file, --signing-key-file, or --signing-key-id/--signing-secret"
        );
    }
    if let Some(path) = signing_private_key_file {
        return Ok(Some(RegistryServerSigningMaterial::Ed25519(
            RegistryAsymmetricSigningKey::read_json(path)?,
        )));
    }
    if let Some(path) = signing_keyring_file {
        return Ok(Some(RegistryServerSigningMaterial::Hmac(
            RegistrySigningKeyring::read_json(path)?.active_key()?.clone(),
        )));
    }
    if let Some(path) = signing_key_file {
        return Ok(Some(RegistryServerSigningMaterial::Hmac(
            RegistrySigningKey::read_json(path)?,
        )));
    }
    match (signing_key_id, signing_secret) {
        (Some(key_id), Some(secret)) => Ok(Some(RegistryServerSigningMaterial::Hmac(RegistrySigningKey::new(
            key_id, secret,
        )))),
        (None, None) => Ok(None),
        (Some(_), None) => anyhow::bail!("--signing-key-id requires --signing-secret"),
        (None, Some(_)) => anyhow::bail!("--signing-secret requires --signing-key-id"),
    }
}

fn handle_stream(stream: &mut TcpStream, service: &RegistryService, auth: &RegistryAuth) -> anyhow::Result<()> {
    let request = read_http_request(stream)?;
    let response = handle_registry_http_request(service, auth, &request)?;
    stream.write_all(&response).context("write registry response")
}

fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut reader = BufReader::new(stream.try_clone().context("clone registry stream")?);
    let mut request = Vec::new();
    let mut content_length = 0usize;
    let mut total_header_bytes = 0usize;
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line).context("read registry request")?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_HEADER_LINE_SIZE {
            anyhow::bail!("header line exceeds {MAX_HEADER_LINE_SIZE} bytes");
        }
        total_header_bytes += line.len();
        if total_header_bytes > MAX_HEADER_TOTAL_SIZE {
            anyhow::bail!("total header bytes exceed {MAX_HEADER_TOTAL_SIZE}");
        }
        if let Some(header) = std::str::from_utf8(&line)
            .ok()
            .and_then(|line| line.trim_end().split_once(':'))
            && header.0.eq_ignore_ascii_case("content-length")
        {
            content_length = header.1.trim().parse().context("parse content-length")?;
        }
        request.extend_from_slice(&line);
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    if content_length > 0 {
        if content_length > MAX_CONTENT_LENGTH {
            anyhow::bail!("content-length {content_length} exceeds maximum {MAX_CONTENT_LENGTH}");
        }
        let start = request.len();
        request.resize(start + content_length, 0);
        reader
            .read_exact(&mut request[start..])
            .context("read registry request body")?;
    }
    Ok(request)
}

pub(super) fn handle_registry_http_request(
    service: &RegistryService,
    auth: &RegistryAuth,
    request: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let request = ParsedRequest::parse(request)?;
    let result = route_registry_request(service, auth, &request);
    Ok(match result {
        Ok(RegistryResponse::Json(body)) => http_response(200, "OK", "application/json", body.as_bytes()),
        Ok(RegistryResponse::NoContent) => http_response(204, "No Content", "text/plain", b""),
        Err(error) => http_response(error.status, error.reason, "text/plain", error.message.as_bytes()),
    })
}

fn route_registry_request(
    service: &RegistryService,
    auth: &RegistryAuth,
    request: &ParsedRequest,
) -> Result<RegistryResponse, RegistryRouteError> {
    let path = request.path.trim_matches('/');
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.as_slice() == ["api", "v1", "index"] && request.method == "GET" {
        require_scope(request, "index")?;
        require_bearer_scope(request, auth, "index", false)?;
        return Ok(RegistryResponse::Json(service.index_json().map_err(server_error)?));
    }
    if segments.as_slice() == ["api", "v1", "packages"] && request.method == "POST" {
        require_scope(request, "publish")?;
        require_bearer_scope(request, auth, "publish", true)?;
        return Ok(RegistryResponse::Json(
            service
                .publish_request_json(request.body_str().map_err(bad_request)?)
                .map_err(server_error)?,
        ));
    }
    if segments.len() == 4 && segments[0..3] == ["api", "v1", "packages"] && request.method == "GET" {
        return Ok(RegistryResponse::Json(
            service.package_versions_json(segments[3]).map_err(server_error)?,
        ));
    }
    if segments.len() == 5 && segments[0..3] == ["api", "v1", "packages"] && request.method == "GET" {
        return Ok(RegistryResponse::Json(
            service
                .package_version_json(segments[3], segments[4])
                .map_err(server_error)?,
        ));
    }
    if segments.len() == 6 && segments[0..3] == ["api", "v1", "packages"] && segments[5] == "yank" {
        require_scope(request, "yank")?;
        require_bearer_scope(request, auth, "yank", true)?;
        match request.method.as_str() {
            "POST" => {
                service
                    .yank_version(segments[3], segments[4], true)
                    .map_err(server_error)?;
                return Ok(RegistryResponse::NoContent);
            }
            "DELETE" => {
                service
                    .yank_version(segments[3], segments[4], false)
                    .map_err(server_error)?;
                return Ok(RegistryResponse::NoContent);
            }
            _ => {}
        }
    }
    Err(RegistryRouteError::new(404, "Not Found", "registry route not found"))
}

fn require_scope(request: &ParsedRequest, expected: &str) -> Result<(), RegistryRouteError> {
    let scope = request
        .headers
        .get("x-lk-registry-scope")
        .map(String::as_str)
        .unwrap_or_default();
    if scope == expected {
        Ok(())
    } else {
        Err(RegistryRouteError::new(
            403,
            "Forbidden",
            format!("registry route requires X-LK-Registry-Scope: {expected}"),
        ))
    }
}

#[derive(Debug, Clone)]
pub(super) enum RegistryAuth {
    Public,
    SharedToken(String),
    Policy(RegistryAuthPolicy),
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct RegistryAuthPolicy {
    #[serde(default)]
    tokens: Vec<RegistryAuthToken>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegistryAuthToken {
    token: String,
    scopes: Vec<String>,
}

impl RegistryAuth {
    fn load(token: Option<String>, auth_policy: Option<PathBuf>) -> anyhow::Result<Self> {
        match (token, auth_policy) {
            (Some(_), Some(_)) => anyhow::bail!("--token cannot be combined with --auth-policy"),
            (Some(token), None) if !token.trim().is_empty() => Ok(Self::SharedToken(token)),
            (Some(_), None) | (None, None) => Ok(Self::Public),
            (None, Some(path)) => Ok(Self::Policy(RegistryAuthPolicy::read_json(path)?)),
        }
    }
}

impl RegistryAuthPolicy {
    fn read_json(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let body = fs::read(path).with_context(|| format!("read registry auth policy {}", path.display()))?;
        let policy: Self =
            serde_json::from_slice(&body).with_context(|| format!("parse registry auth policy {}", path.display()))?;
        policy
            .validate()
            .with_context(|| format!("validate registry auth policy {}", path.display()))?;
        Ok(policy)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.tokens.is_empty() {
            anyhow::bail!("registry auth policy must contain at least one token");
        }
        let mut seen = BTreeSet::new();
        for entry in &self.tokens {
            if entry.token.trim().is_empty() {
                anyhow::bail!("registry auth policy token must not be empty");
            }
            if !seen.insert(entry.token.as_str()) {
                anyhow::bail!("registry auth policy contains duplicate token");
            }
            if entry.scopes.is_empty() {
                anyhow::bail!("registry auth policy token must contain at least one scope");
            }
            for scope in &entry.scopes {
                if !matches!(scope.as_str(), "index" | "publish" | "yank" | "*") {
                    anyhow::bail!("registry auth policy contains unsupported scope `{scope}`");
                }
            }
        }
        Ok(())
    }

    fn token_has_scope(&self, token: &str, expected_scope: &str) -> bool {
        self.tokens.iter().any(|entry| {
            entry.token == token && entry.scopes.iter().any(|scope| scope == expected_scope || scope == "*")
        })
    }
}

fn require_bearer_scope(
    request: &ParsedRequest,
    auth: &RegistryAuth,
    expected_scope: &str,
    required_in_shared_mode: bool,
) -> Result<(), RegistryRouteError> {
    match auth {
        RegistryAuth::Public => Ok(()),
        RegistryAuth::SharedToken(expected) => {
            if required_in_shared_mode {
                require_shared_bearer_token(request, expected)
            } else {
                Ok(())
            }
        }
        RegistryAuth::Policy(policy) => {
            let token = bearer_token(request);
            if policy.token_has_scope(token, expected_scope) {
                Ok(())
            } else {
                Err(RegistryRouteError::new(
                    401,
                    "Unauthorized",
                    format!("registry route requires bearer token with `{expected_scope}` scope"),
                ))
            }
        }
    }
}

fn require_shared_bearer_token(request: &ParsedRequest, expected: &str) -> Result<(), RegistryRouteError> {
    let Some(expected) = Some(expected.trim()).filter(|token| !token.is_empty()) else {
        return Ok(());
    };
    let actual = bearer_token(request).as_bytes();
    let expected_bytes = expected.as_bytes();
    // Constant-time comparison to prevent timing attacks.
    let eq = actual.len() == expected_bytes.len()
        && actual
            .iter()
            .zip(expected_bytes.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if eq {
        Ok(())
    } else {
        Err(RegistryRouteError::new(
            401,
            "Unauthorized",
            "registry route requires a matching bearer token",
        ))
    }
}

fn bearer_token(request: &ParsedRequest) -> &str {
    let authorization = request
        .headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or_default();
    authorization.strip_prefix("Bearer ").unwrap_or_default()
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    stream
        .write_all(&http_response(status, reason, content_type, body))
        .context("write registry response")
}

fn http_response(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

enum RegistryResponse {
    Json(String),
    NoContent,
}

#[derive(Debug)]
struct RegistryRouteError {
    status: u16,
    reason: &'static str,
    message: String,
}

impl RegistryRouteError {
    fn new(status: u16, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            reason,
            message: message.into(),
        }
    }
}

fn bad_request(error: anyhow::Error) -> RegistryRouteError {
    RegistryRouteError::new(400, "Bad Request", format!("{error:#}"))
}

fn server_error(error: anyhow::Error) -> RegistryRouteError {
    RegistryRouteError::new(500, "Internal Server Error", format!("{error:#}"))
}

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl ParsedRequest {
    fn parse(raw: &[u8]) -> anyhow::Result<Self> {
        let split = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| (index, 4))
            .or_else(|| {
                raw.windows(2)
                    .position(|window| window == b"\n\n")
                    .map(|index| (index, 2))
            })
            .ok_or_else(|| anyhow::anyhow!("registry HTTP request has no header terminator"))?;
        let header = std::str::from_utf8(&raw[..split.0]).context("parse registry request header")?;
        let body = raw[split.0 + split.1..].to_vec();
        let mut lines = header.lines();
        let request_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("registry HTTP request has no request line"))?;
        let mut parts = request_line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("registry HTTP request has no method"))?
            .to_string();
        let path = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("registry HTTP request has no path"))?
            .split_once('?')
            .map_or_else(|| parts_path(request_line), |(path, _)| path.to_string());
        let mut headers = BTreeMap::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        Ok(Self {
            method,
            path,
            headers,
            body,
        })
    }

    fn body_str(&self) -> anyhow::Result<&str> {
        std::str::from_utf8(&self.body).context("registry request body is not UTF-8")
    }
}

fn parts_path(request_line: &str) -> String {
    request_line.split_whitespace().nth(1).unwrap_or("/").to_string()
}

#[cfg(test)]
mod tests {
    use lk_core::package::{
        RegistryAsymmetricSigningKey, RegistryPackageVersionResponse, RegistryPublishManifest, RegistryPublishRequest,
        RegistrySigningKeyring,
    };

    use super::*;

    #[test]
    fn registry_http_handler_serves_publish_index_version_and_yank_routes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = RegistryService::new(temp.path(), "https://registry.lk.example")
            .with_signing_key(RegistrySigningKey::new("local-key", "registry-secret"));
        let publish = publish_manifest("https://registry.lk.example");
        let request = RegistryPublishRequest {
            source: "https://example.invalid/app.git".to_string(),
            rev: "abc123".to_string(),
            checksum: Some("sha256:package".to_string()),
            publish_manifest: publish,
        };
        let body = serde_json::to_string(&request).expect("request json");
        let auth = RegistryAuth::SharedToken("secret-token".to_string());
        let response = handle_registry_http_request(
            &service,
            &auth,
            format!(
                "POST /api/v1/packages HTTP/1.1\r\nAuthorization: Bearer secret-token\r\nX-LK-Registry-Scope: publish\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        )
        .expect("publish response");
        assert!(
            response.starts_with(b"HTTP/1.1 200 OK\r\n"),
            "{}",
            String::from_utf8_lossy(&response)
        );
        assert!(String::from_utf8_lossy(&response).contains("\"signature\""));

        let index = handle_registry_http_request(
            &service,
            &RegistryAuth::Public,
            b"GET /api/v1/index HTTP/1.1\r\nX-LK-Registry-Scope: index\r\n\r\n",
        )
        .expect("index response");
        assert!(String::from_utf8_lossy(&index).contains("\"packages\""));

        let version = handle_registry_http_request(
            &service,
            &RegistryAuth::Public,
            b"GET /api/v1/packages/app/0.2.3 HTTP/1.1\r\n\r\n",
        )
        .expect("version response");
        assert!(String::from_utf8_lossy(&version).contains("\"abc123\""));

        let yank = handle_registry_http_request(
            &service,
            &auth,
            b"POST /api/v1/packages/app/0.2.3/yank HTTP/1.1\r\nAuthorization: Bearer secret-token\r\nX-LK-Registry-Scope: yank\r\n\r\n",
        )
        .expect("yank response");
        assert!(yank.starts_with(b"HTTP/1.1 204 No Content\r\n"));
    }

    #[test]
    fn registry_http_handler_rejects_bad_scope_and_token() {
        let temp = tempfile::tempdir().expect("tempdir");
        let service = RegistryService::new(temp.path(), "https://registry.lk.example");
        let auth = RegistryAuth::SharedToken("secret-token".to_string());
        let response = handle_registry_http_request(
            &service,
            &auth,
            b"POST /api/v1/packages HTTP/1.1\r\nX-LK-Registry-Scope: publish\r\n\r\n{}",
        )
        .expect("auth response");
        assert!(response.starts_with(b"HTTP/1.1 401 Unauthorized\r\n"));

        let response =
            handle_registry_http_request(&service, &RegistryAuth::Public, b"GET /api/v1/index HTTP/1.1\r\n\r\n")
                .expect("scope response");
        assert!(response.starts_with(b"HTTP/1.1 403 Forbidden\r\n"));
    }

    #[test]
    fn registry_auth_policy_enforces_scoped_bearer_tokens() {
        let temp = tempfile::tempdir().expect("tempdir");
        let policy_path = temp.path().join("auth.json");
        std::fs::write(
            &policy_path,
            r#"{
              "tokens": [
                { "token": "index-token", "scopes": ["index"] },
                { "token": "publish-token", "scopes": ["publish"] },
                { "token": "admin-token", "scopes": ["*"] }
              ]
            }"#,
        )
        .expect("write auth policy");
        let auth = RegistryAuth::load(None, Some(policy_path)).expect("auth policy");
        let service = RegistryService::new(temp.path().join("registry"), "https://registry.lk.example");

        let index = handle_registry_http_request(
            &service,
            &auth,
            b"GET /api/v1/index HTTP/1.1\r\nAuthorization: Bearer index-token\r\nX-LK-Registry-Scope: index\r\n\r\n",
        )
        .expect("index response");
        assert!(index.starts_with(b"HTTP/1.1 200 OK\r\n"));

        let index_without_token = handle_registry_http_request(
            &service,
            &auth,
            b"GET /api/v1/index HTTP/1.1\r\nX-LK-Registry-Scope: index\r\n\r\n",
        )
        .expect("index auth response");
        assert!(index_without_token.starts_with(b"HTTP/1.1 401 Unauthorized\r\n"));

        let wrong_scope = handle_registry_http_request(
            &service,
            &auth,
            b"POST /api/v1/packages/app/0.2.3/yank HTTP/1.1\r\nAuthorization: Bearer publish-token\r\nX-LK-Registry-Scope: yank\r\n\r\n",
        )
        .expect("wrong scope response");
        assert!(wrong_scope.starts_with(b"HTTP/1.1 401 Unauthorized\r\n"));

        let admin = handle_registry_http_request(
            &service,
            &auth,
            b"GET /api/v1/index HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nX-LK-Registry-Scope: index\r\n\r\n",
        )
        .expect("admin response");
        assert!(admin.starts_with(b"HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn registry_server_loads_signing_key_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = temp.path().join("registry");
        let key_path = temp.path().join("keys").join("local.json");
        let key = RegistrySigningKey::new("local-key", "registry-secret");
        key.write_json(&key_path).expect("write key");

        let service = registry_service(
            storage,
            "https://registry.lk.example".to_string(),
            Some(key_path),
            None,
            None,
            None,
            None,
        )
        .expect("registry service");
        let publish = publish_manifest("https://registry.lk.example");
        let request = RegistryPublishRequest {
            source: "https://example.invalid/app.git".to_string(),
            rev: "abc123".to_string(),
            checksum: None,
            publish_manifest: publish,
        };
        let response = service
            .publish_request_json(&serde_json::to_string(&request).expect("request json"))
            .expect("publish response");
        let version: RegistryPackageVersionResponse = serde_json::from_str(&response).expect("version response");
        assert_eq!(version.signature.as_ref().expect("signature").key_id, "local-key");
    }

    #[test]
    fn registry_server_loads_signing_keyring_file_active_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = temp.path().join("registry");
        let keyring_path = temp.path().join("keys").join("registry.json");
        let old_key = RegistrySigningKey::new("old-key", "old-secret");
        let mut keyring = RegistrySigningKeyring::new(old_key).expect("keyring");
        keyring.rotate("new-key").expect("rotate keyring");
        keyring.write_json(&keyring_path).expect("write keyring");

        let service = registry_service(
            storage,
            "https://registry.lk.example".to_string(),
            None,
            Some(keyring_path),
            None,
            None,
            None,
        )
        .expect("registry service");
        let publish = publish_manifest("https://registry.lk.example");
        let request = RegistryPublishRequest {
            source: "https://example.invalid/app.git".to_string(),
            rev: "abc123".to_string(),
            checksum: None,
            publish_manifest: publish,
        };
        let response = service
            .publish_request_json(&serde_json::to_string(&request).expect("request json"))
            .expect("publish response");
        let version: RegistryPackageVersionResponse = serde_json::from_str(&response).expect("version response");
        assert_eq!(version.signature.as_ref().expect("signature").key_id, "new-key");
    }

    #[test]
    fn registry_server_loads_asymmetric_private_key_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = temp.path().join("registry");
        let private_key_path = temp.path().join("keys").join("private.json");
        let key = RegistryAsymmetricSigningKey::generate("ed-key").expect("generate key");
        key.write_json(&private_key_path).expect("write private key");

        let service = registry_service(
            storage,
            "https://registry.lk.example".to_string(),
            None,
            None,
            Some(private_key_path),
            None,
            None,
        )
        .expect("registry service");
        let publish = publish_manifest("https://registry.lk.example");
        let request = RegistryPublishRequest {
            source: "https://example.invalid/app.git".to_string(),
            rev: "abc123".to_string(),
            checksum: None,
            publish_manifest: publish.clone(),
        };
        let response = service
            .publish_request_json(&serde_json::to_string(&request).expect("request json"))
            .expect("publish response");
        let version: RegistryPackageVersionResponse = serde_json::from_str(&response).expect("version response");
        let signature = version.signature.as_ref().expect("signature");
        assert_eq!(signature.algorithm, "ed25519");
        assert_eq!(signature.key_id, "ed-key");
        publish
            .verify_public_signature(signature, &key.public_key().expect("public key"))
            .expect("public signature");
    }

    #[test]
    fn registry_server_rejects_conflicting_signing_key_options() {
        let temp = tempfile::tempdir().expect("tempdir");
        let key_path = temp.path().join("local.json");
        RegistrySigningKey::new("local-key", "registry-secret")
            .write_json(&key_path)
            .expect("write key");

        let err = resolve_signing_material(
            Some(key_path),
            None,
            None,
            Some("inline-key".to_string()),
            Some("inline-secret".to_string()),
        )
        .expect_err("conflicting key options should fail");
        assert!(err.to_string().contains("configure only one"), "{err}");
    }

    fn publish_manifest(registry_url: &str) -> RegistryPublishManifest {
        let mut manifest = RegistryPublishManifest {
            package: "app".to_string(),
            version: "0.2.3".to_string(),
            registry: "local".to_string(),
            registry_url: registry_url.to_string(),
            include: vec!["Lk.toml".to_string(), "src/**".to_string()],
            dependencies: Vec::new(),
            macro_providers: Default::default(),
            integrity: Default::default(),
        };
        manifest.integrity = manifest.integrity();
        manifest
    }
}
