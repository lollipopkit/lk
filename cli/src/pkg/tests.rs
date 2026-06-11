use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use lk_core::package::{
    RegistryAsymmetricSigningKey, RegistryPublishDependency, RegistryPublishIntegrity, RegistryPublishMacroProviders,
    RegistryPublishManifest, RegistrySigningKey, RegistrySigningKeyring,
};

use super::{
    RegistryAuthScope, RegistryPackageVersion, package_dir_checksum, registry_auth_token_from,
    registry_index_cache_path_from_home, registry_index_endpoint, registry_package_versions_endpoint,
    registry_publish_endpoint, registry_signature_policy_from, registry_yank_endpoint, select_registry_version,
    upload_registry_publish_manifest_with_token, upload_registry_yank,
    validate_registry_package_version_manifests_with_policy, verify_registry_checksum,
};

#[test]
fn registry_publish_endpoint_appends_api_path() {
    assert_eq!(
        registry_publish_endpoint("https://registry.lk.example"),
        "https://registry.lk.example/api/v1/packages"
    );
    assert_eq!(
        registry_publish_endpoint("https://registry.lk.example/root/"),
        "https://registry.lk.example/root/api/v1/packages"
    );
}

#[test]
fn registry_package_versions_endpoint_appends_package_path() {
    assert_eq!(
        registry_package_versions_endpoint("https://registry.lk.example", "helper"),
        "https://registry.lk.example/api/v1/packages/helper"
    );
    assert_eq!(
        registry_package_versions_endpoint("https://registry.lk.example/root/", "helper"),
        "https://registry.lk.example/root/api/v1/packages/helper"
    );
}

#[test]
fn registry_yank_endpoint_appends_yank_path() {
    assert_eq!(
        registry_yank_endpoint("https://registry.lk.example", "helper", "0.2.3"),
        "https://registry.lk.example/api/v1/packages/helper/0.2.3/yank"
    );
    assert_eq!(
        registry_yank_endpoint("https://registry.lk.example/root/", "helper", "0.2.3"),
        "https://registry.lk.example/root/api/v1/packages/helper/0.2.3/yank"
    );
}

#[test]
fn registry_index_endpoint_and_cache_path_are_stable() {
    let temp = tempfile::tempdir().expect("tempdir");

    assert_eq!(
        registry_index_endpoint("https://registry.lk.example/root/"),
        "https://registry.lk.example/root/api/v1/index"
    );
    assert_eq!(
        registry_index_cache_path_from_home(temp.path(), Some("local"), "https://registry.lk.example"),
        temp.path().join("registry").join("local").join("index.json")
    );
    assert_eq!(
        registry_index_cache_path_from_home(temp.path(), None, "https://registry.lk.example/root"),
        temp.path()
            .join("registry")
            .join("registry.lk.example_root")
            .join("index.json")
    );
}

#[test]
fn registry_auth_token_prefers_index_specific_token() {
    let values = BTreeMap::from([
        ("LK_REGISTRY_TOKEN", "fallback-token"),
        ("LK_REGISTRY_INDEX_TOKEN", "index-token"),
    ]);
    let lookup = |name: &str| values.get(name).map(|value| (*value).to_string());

    assert_eq!(
        registry_auth_token_from(RegistryAuthScope::Index, lookup).expect("index token"),
        "index-token"
    );
}

#[test]
fn registry_range_selection_uses_highest_non_yanked_compatible_version() {
    let requirement = semver::VersionReq::parse(">=0.2.0, <0.4.0").expect("version requirement");
    let selected = select_registry_version(
        "helper",
        &requirement,
        vec![
            RegistryPackageVersion {
                version: "0.1.9".to_string(),
                source: "git://old".to_string(),
                rev: "old".to_string(),
                checksum: Some("sha256:old".to_string()),
                yanked: false,
                publish_manifest: None,
                signature: None,
            },
            RegistryPackageVersion {
                version: "0.3.1".to_string(),
                source: "git://selected".to_string(),
                rev: "selected".to_string(),
                checksum: Some("sha256:selected".to_string()),
                yanked: false,
                publish_manifest: None,
                signature: None,
            },
            RegistryPackageVersion {
                version: "0.3.2".to_string(),
                source: "git://yanked".to_string(),
                rev: "yanked".to_string(),
                checksum: Some("sha256:yanked".to_string()),
                yanked: true,
                publish_manifest: None,
                signature: None,
            },
            RegistryPackageVersion {
                version: "0.4.0".to_string(),
                source: "git://new".to_string(),
                rev: "new".to_string(),
                checksum: Some("sha256:new".to_string()),
                yanked: false,
                publish_manifest: None,
                signature: None,
            },
        ],
    )
    .expect("compatible version should resolve");

    assert_eq!(selected.source, "git://selected");
    assert_eq!(selected.rev, "selected");
    assert_eq!(selected.checksum.as_deref(), Some("sha256:selected"));
}

#[test]
fn package_dir_checksum_is_stable_and_ignores_git_and_lock_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src");
    std::fs::create_dir_all(root.join(".git")).expect("create git dir");
    std::fs::write(root.join("Lk.toml"), "[package]\nname = \"helper\"\n").expect("write manifest");
    std::fs::write(root.join("src/mod.lk"), "fn helper() { return 1; }\n").expect("write source");
    std::fs::write(root.join(".git/HEAD"), "ignored").expect("write git metadata");
    std::fs::write(root.join("Lk.lock"), "ignored").expect("write lock");

    let first = package_dir_checksum(root).expect("first checksum");
    std::fs::write(root.join(".git/HEAD"), "changed").expect("change git metadata");
    std::fs::write(root.join("Lk.lock"), "changed").expect("change lock");
    let second = package_dir_checksum(root).expect("second checksum");
    std::fs::write(root.join("src/mod.lk"), "fn helper() { return 2; }\n").expect("change source");
    let third = package_dir_checksum(root).expect("third checksum");

    assert_eq!(first, second);
    assert_ne!(first, third);
}

#[test]
fn registry_checksum_verification_rejects_mismatches_and_unknown_algorithms() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src");
    std::fs::write(root.join("src/mod.lk"), "fn helper() { return 1; }\n").expect("write source");

    verify_registry_checksum("helper", root, None).expect("missing checksum is allowed");

    let bad_algorithm = verify_registry_checksum("helper", root, Some("sha512:abc"))
        .expect_err("unsupported checksum algorithm should fail");
    assert!(bad_algorithm.to_string().contains("sha256:<hex>"), "{bad_algorithm}");

    let mismatch =
        verify_registry_checksum("helper", root, Some("sha256:0000")).expect_err("checksum mismatch should fail");
    assert!(mismatch.to_string().contains("checksum mismatch"), "{mismatch}");
}

#[test]
fn registry_auth_token_prefers_scope_specific_tokens() {
    let values = BTreeMap::from([
        ("LK_REGISTRY_TOKEN", "fallback-token"),
        ("LK_REGISTRY_PUBLISH_TOKEN", "publish-token"),
        ("LK_REGISTRY_YANK_TOKEN", "yank-token"),
    ]);
    let lookup = |name: &str| values.get(name).map(|value| (*value).to_string());

    assert_eq!(
        registry_auth_token_from(RegistryAuthScope::Publish, lookup).expect("publish token"),
        "publish-token"
    );
    assert_eq!(
        registry_auth_token_from(RegistryAuthScope::Yank, lookup).expect("yank token"),
        "yank-token"
    );
}

#[test]
fn registry_publish_upload_posts_manifest_with_bearer_token() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept publish request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        let mut content_length = 0usize;
        let mut authorization = String::new();
        let mut content_type = String::new();
        let mut scope = String::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("read header");
            if header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.trim_end().split_once(':') {
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => {
                        content_length = value.trim().parse().expect("content length");
                    }
                    "authorization" => authorization = value.trim().to_string(),
                    "content-type" => content_type = value.trim().to_string(),
                    "x-lk-registry-scope" => scope = value.trim().to_string(),
                    _ => {}
                }
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).expect("read request body");
        stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok")
            .expect("write mock response");
        (
            request_line,
            authorization,
            content_type,
            scope,
            String::from_utf8(body).expect("request body utf8"),
        )
    });

    let mut manifest = RegistryPublishManifest {
        package: "app".to_string(),
        version: "0.2.3".to_string(),
        registry: "local".to_string(),
        registry_url,
        include: vec!["Lk.toml".to_string(), "src/**".to_string()],
        dependencies: vec![RegistryPublishDependency {
            name: "helper".to_string(),
            source: "path".to_string(),
            version_or_rev: Some("0.1.0".to_string()),
        }],
        macro_providers: RegistryPublishMacroProviders {
            derive: vec!["MakeAnswer".to_string()],
            attribute: vec!["route".to_string()],
            function_like: vec!["sql".to_string()],
            trusted_dependencies: vec!["helper".to_string()],
        },
        integrity: RegistryPublishIntegrity::default(),
    };
    manifest.integrity = manifest.integrity();

    upload_registry_publish_manifest_with_token(&manifest, "secret-token").expect("publish upload succeeds");
    let (request_line, authorization, content_type, scope, body) = server.join().expect("mock registry thread");
    assert_eq!(request_line, "POST /api/v1/packages HTTP/1.1\r\n");
    assert_eq!(authorization, "Bearer secret-token");
    assert!(content_type.starts_with("application/json"), "{content_type}");
    assert_eq!(scope, "publish");
    let body: serde_json::Value = serde_json::from_str(&body).expect("request body json");
    assert_eq!(body["package"], "app");
    assert_eq!(body["version"], "0.2.3");
    assert_eq!(body["macro_providers"]["function_like"][0], "sql");
    assert_eq!(body["dependencies"][0]["version_or_rev"], "0.1.0");
    assert_eq!(body["integrity"]["algorithm"], "sha256");
    assert_eq!(body["integrity"]["digest"].as_str().unwrap().len(), 64);
}

#[test]
fn registry_publish_upload_rejects_tampered_manifest_integrity() {
    let mut manifest = registry_publish_manifest_fixture("http://127.0.0.1:9");
    manifest.include.push("tampered/**".to_string());

    let err = upload_registry_publish_manifest_with_token(&manifest, "secret-token")
        .expect_err("tampered manifest should not be uploaded");
    assert!(
        err.to_string().contains("integrity mismatch"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn registry_version_publish_manifest_integrity_is_validated() {
    let registry_url = "https://registry.lk.example";
    let manifest = registry_publish_manifest_fixture(registry_url);
    let version = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: None,
    };
    let unsigned_policy = registry_signature_policy_from(|_| None).expect("unsigned policy");
    validate_registry_package_version_manifests_with_policy("app", registry_url, &[version], &unsigned_policy)
        .expect("matching publish manifest should validate");

    let mut tampered = manifest;
    tampered.registry_url = "https://evil.invalid".to_string();
    let version = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(tampered),
        signature: None,
    };
    let err =
        validate_registry_package_version_manifests_with_policy("app", registry_url, &[version], &unsigned_policy)
            .expect_err("tampered publish manifest should fail");
    assert!(
        err.to_string().contains("URL mismatch") || err.to_string().contains("integrity mismatch"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn registry_version_publish_manifest_signature_policy_is_enforced() {
    let registry_url = "https://registry.lk.example";
    let manifest = registry_publish_manifest_fixture(registry_url);
    let key = RegistrySigningKey::new("local-key", "registry-secret");
    let signature = manifest.sign(&key).expect("sign manifest");
    let policy = registry_signature_policy_from(|name| {
        BTreeMap::from([
            ("LK_REGISTRY_SIGNING_KEY_ID", "local-key"),
            ("LK_REGISTRY_SIGNING_SECRET", "registry-secret"),
        ])
        .get(name)
        .map(|value| (*value).to_string())
    })
    .expect("signed policy");

    let signed = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: Some(signature),
    };
    validate_registry_package_version_manifests_with_policy("app", registry_url, &[signed], &policy)
        .expect("signed publish manifest should validate");

    let unsigned = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: None,
    };
    let err = validate_registry_package_version_manifests_with_policy("app", registry_url, &[unsigned], &policy)
        .expect_err("signed policy must reject missing signatures");
    assert!(err.to_string().contains("signature missing"), "{err}");

    let mut tampered = manifest;
    tampered.include.push("extra/**".to_string());
    let tampered_version = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(tampered),
        signature: Some(
            registry_publish_manifest_fixture(registry_url)
                .sign(&key)
                .expect("sign original"),
        ),
    };
    let err =
        validate_registry_package_version_manifests_with_policy("app", registry_url, &[tampered_version], &policy)
            .expect_err("signed policy must reject tampered signed payloads");
    assert!(
        err.to_string().contains("integrity mismatch") || err.to_string().contains("signature mismatch"),
        "{err}"
    );
}

#[test]
fn registry_version_signature_policy_accepts_keyring_and_rejects_revoked_keys() {
    let registry_url = "https://registry.lk.example";
    let manifest = registry_publish_manifest_fixture(registry_url);
    let old_key = RegistrySigningKey::new("old-key", "old-secret");
    let mut keyring = RegistrySigningKeyring::new(old_key.clone()).expect("keyring");
    keyring.rotate("new-key").expect("rotate keyring");

    let temp = tempfile::tempdir().expect("tempdir");
    let keyring_path = temp.path().join("registry-keyring.json");
    keyring.write_json(&keyring_path).expect("write keyring");
    let policy = registry_signature_policy_from(|name| {
        (name == "LK_REGISTRY_SIGNING_KEYRING_FILE").then(|| keyring_path.display().to_string())
    })
    .expect("keyring policy");

    let old_signed = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: Some(manifest.sign(&old_key).expect("old signature")),
    };
    validate_registry_package_version_manifests_with_policy("app", registry_url, &[old_signed], &policy)
        .expect("old key remains trusted during rotation");

    keyring.revoke("old-key").expect("revoke old key");
    keyring.write_json(&keyring_path).expect("write revoked keyring");
    let revoked_policy = registry_signature_policy_from(|name| {
        (name == "LK_REGISTRY_SIGNING_KEYRING_FILE").then(|| keyring_path.display().to_string())
    })
    .expect("revoked keyring policy");
    let revoked = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: Some(manifest.sign(&old_key).expect("old signature")),
    };
    let err = validate_registry_package_version_manifests_with_policy("app", registry_url, &[revoked], &revoked_policy)
        .expect_err("revoked key should fail signature validation");
    assert!(format!("{err:#}").contains("revoked"), "{err:#}");
}

#[test]
fn registry_version_signature_policy_accepts_public_key_file() {
    let registry_url = "https://registry.lk.example";
    let manifest = registry_publish_manifest_fixture(registry_url);
    let key = RegistryAsymmetricSigningKey::generate("ed-key").expect("generate asymmetric key");
    let public_key = key.public_key().expect("public key");

    let temp = tempfile::tempdir().expect("tempdir");
    let public_key_path = temp.path().join("registry-public-key.json");
    public_key.write_json(&public_key_path).expect("write public key");
    let policy = registry_signature_policy_from(|name| {
        (name == "LK_REGISTRY_PUBLIC_KEY_FILE").then(|| public_key_path.display().to_string())
    })
    .expect("public key policy");

    let signed = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: Some(manifest.sign_asymmetric(&key).expect("ed25519 signature")),
    };
    validate_registry_package_version_manifests_with_policy("app", registry_url, &[signed], &policy)
        .expect("public key should validate ed25519 signature");

    let hmac_key = RegistrySigningKey::new("ed-key", "registry-secret");
    let wrong_algorithm = RegistryPackageVersion {
        version: "0.2.3".to_string(),
        source: "https://example.invalid/helper.git".to_string(),
        rev: "abc123".to_string(),
        checksum: None,
        yanked: false,
        publish_manifest: Some(manifest.clone()),
        signature: Some(manifest.sign(&hmac_key).expect("hmac signature")),
    };
    let err = validate_registry_package_version_manifests_with_policy("app", registry_url, &[wrong_algorithm], &policy)
        .expect_err("public key policy should reject hmac signatures");
    assert!(
        format!("{err:#}").contains("unsupported public-key algorithm"),
        "{err:#}"
    );
}

fn registry_publish_manifest_fixture(registry_url: &str) -> RegistryPublishManifest {
    let mut manifest = RegistryPublishManifest {
        package: "app".to_string(),
        version: "0.2.3".to_string(),
        registry: "local".to_string(),
        registry_url: registry_url.to_string(),
        include: vec!["Lk.toml".to_string(), "src/**".to_string()],
        dependencies: vec![RegistryPublishDependency {
            name: "helper".to_string(),
            source: "path".to_string(),
            version_or_rev: Some("0.1.0".to_string()),
        }],
        macro_providers: RegistryPublishMacroProviders {
            derive: vec!["MakeAnswer".to_string()],
            attribute: vec!["route".to_string()],
            function_like: vec!["sql".to_string()],
            trusted_dependencies: vec!["helper".to_string()],
        },
        integrity: RegistryPublishIntegrity::default(),
    };
    manifest.integrity = manifest.integrity();
    manifest
}

#[test]
fn registry_yank_upload_sends_scoped_authenticated_request() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept yank request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        let mut authorization = String::new();
        let mut scope = String::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("read header");
            if header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.trim_end().split_once(':') {
                match name.to_ascii_lowercase().as_str() {
                    "authorization" => authorization = value.trim().to_string(),
                    "x-lk-registry-scope" => scope = value.trim().to_string(),
                    _ => {}
                }
            }
        }
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .expect("write mock response");
        (request_line, authorization, scope)
    });

    upload_registry_yank(&registry_url, "helper", "0.2.3", false, "secret-token").expect("yank upload succeeds");
    let (request_line, authorization, scope) = server.join().expect("mock registry thread");
    assert_eq!(request_line, "POST /api/v1/packages/helper/0.2.3/yank HTTP/1.1\r\n");
    assert_eq!(authorization, "Bearer secret-token");
    assert_eq!(scope, "yank");
}
