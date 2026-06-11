use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Manifest, PackageGraph, dependency_publish_source, dependency_version, is_publish_version, registry_include,
};

mod signing;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishManifest {
    pub package: String,
    pub version: String,
    pub registry: String,
    pub registry_url: String,
    pub include: Vec<String>,
    pub dependencies: Vec<RegistryPublishDependency>,
    pub macro_providers: RegistryPublishMacroProviders,
    pub integrity: RegistryPublishIntegrity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishDependency {
    pub name: String,
    pub source: String,
    pub version_or_rev: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishMacroProviders {
    pub derive: Vec<String>,
    pub attribute: Vec<String>,
    pub function_like: Vec<String>,
    pub trusted_dependencies: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishIntegrity {
    pub algorithm: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishServerValidation {
    pub immutable_manifest_key: String,
    pub signing_payload_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySigningKey {
    pub key_id: String,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySigningKeyring {
    pub active_key_id: String,
    pub keys: Vec<RegistrySigningKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked_key_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryAsymmetricSigningKey {
    pub key_id: String,
    pub secret_key: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublicSigningKey {
    pub key_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryManifestSignature {
    pub algorithm: String,
    pub key_id: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishStoredManifest {
    pub path: PathBuf,
    pub validation: RegistryPublishServerValidation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryStoredVersion {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    pub manifest_key: String,
    pub signing_payload_sha256: String,
    pub yanked: bool,
    pub checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<RegistryManifestSignature>,
    pub macro_providers: RegistryPublishMacroProviders,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPackageIndex {
    pub versions: Vec<RegistryStoredVersion>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub packages: BTreeMap<String, RegistryPackageIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryStoredPackage {
    pub manifest_path: PathBuf,
    pub index_path: PathBuf,
    pub version: RegistryStoredVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPublishRequest {
    pub source: String,
    pub rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    pub publish_manifest: RegistryPublishManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPackageVersionResponse {
    pub version: String,
    pub source: String,
    pub rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_manifest: Option<RegistryPublishManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<RegistryManifestSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPackageIndexResponse {
    pub name: String,
    #[serde(default)]
    pub versions: Vec<RegistryPackageVersionResponse>,
    #[serde(default)]
    pub macro_providers: RegistryPublishMacroProviders,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryIndexResponse {
    #[serde(default)]
    pub packages: Vec<RegistryPackageIndexResponse>,
}

#[derive(Clone)]
pub struct RegistryService {
    storage_root: PathBuf,
    registry_url: String,
    signing_material: Option<RegistrySigningMaterial>,
}

#[derive(Clone)]
enum RegistrySigningMaterial {
    Hmac(RegistrySigningKey),
    Ed25519(RegistryAsymmetricSigningKey),
}

#[derive(Serialize)]
struct RegistryPublishManifestDigestPayload<'a> {
    package: &'a str,
    version: &'a str,
    registry: &'a str,
    registry_url: &'a str,
    include: &'a [String],
    dependencies: &'a [RegistryPublishDependency],
    macro_providers: &'a RegistryPublishMacroProviders,
}

impl Manifest {
    pub fn registry_publish_manifest(
        &self,
        dependency_versions: &BTreeMap<String, Option<String>>,
    ) -> Result<RegistryPublishManifest> {
        let package = self
            .package
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("registry publish requires a [package] section"))?;
        let version = package
            .version
            .as_ref()
            .filter(|version| is_publish_version(version))
            .ok_or_else(|| anyhow::anyhow!("registry publish requires [package].version to be a semantic version"))?
            .clone();
        let registry = self
            .registry
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("registry publish requires a [registry] section"))?;
        let registry_url = registry
            .url
            .as_ref()
            .filter(|url| is_registry_url(url))
            .ok_or_else(|| {
                anyhow::anyhow!("registry publish requires [registry].url to start with http:// or https://")
            })?
            .clone();

        let dependencies = self
            .dependencies
            .iter()
            .map(|(name, spec)| RegistryPublishDependency {
                name: name.clone(),
                source: dependency_publish_source(spec),
                version_or_rev: dependency_versions
                    .get(name)
                    .cloned()
                    .flatten()
                    .or_else(|| dependency_version(spec)),
            })
            .collect();
        let macro_providers = RegistryPublishMacroProviders {
            derive: self.macros.derive.keys().cloned().collect(),
            attribute: self.macros.attribute.keys().cloned().collect(),
            function_like: self.macros.function_like.keys().cloned().collect(),
            trusted_dependencies: self.macros.trusted_dependencies.clone(),
        };
        let mut publish = RegistryPublishManifest {
            package: package.name.clone(),
            version,
            registry: registry.name.clone().unwrap_or_else(|| "default".to_string()),
            registry_url,
            include: registry_include(registry),
            dependencies,
            macro_providers,
            integrity: RegistryPublishIntegrity::default(),
        };
        publish.integrity = publish.integrity();
        Ok(publish)
    }
}

impl PackageGraph {
    pub fn registry_publish_manifest(&self) -> Result<RegistryPublishManifest> {
        if !self.missing.is_empty() {
            return Err(anyhow::anyhow!(
                "registry publish requires all dependencies to resolve; missing: {}",
                self.missing.join(", ")
            ));
        }
        self.validate_macro_distribution_for_manifest(&self.manifest_path)?;
        let dependency_versions = self
            .modules
            .iter()
            .filter(|module| module.manifest_path != self.manifest_path)
            .filter_map(|module| {
                Manifest::read(&module.manifest_path).ok().map(|manifest| {
                    (
                        module.name.clone(),
                        manifest.package.and_then(|package| package.version),
                    )
                })
            })
            .collect();
        self.manifest.registry_publish_manifest(&dependency_versions)
    }
}

impl RegistryPublishManifest {
    fn digest_payload_bytes(&self) -> Vec<u8> {
        let payload = RegistryPublishManifestDigestPayload {
            package: &self.package,
            version: &self.version,
            registry: &self.registry,
            registry_url: &self.registry_url,
            include: &self.include,
            dependencies: &self.dependencies,
            macro_providers: &self.macro_providers,
        };
        serde_json::to_vec(&payload).expect("serialize registry publish manifest digest payload")
    }

    pub fn integrity(&self) -> RegistryPublishIntegrity {
        let digest = Sha256::digest(self.digest_payload_bytes());
        RegistryPublishIntegrity {
            algorithm: "sha256".to_string(),
            digest: format!("{digest:x}"),
        }
    }

    pub fn verify_integrity(&self) -> bool {
        self.integrity == self.integrity()
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        self.digest_payload_bytes()
    }

    pub fn sign(&self, key: &RegistrySigningKey) -> Result<RegistryManifestSignature> {
        key.validate()?;
        Ok(RegistryManifestSignature {
            algorithm: "hmac-sha256".to_string(),
            key_id: key.key_id.clone(),
            digest: hmac_sha256_hex(key.secret.as_bytes(), &self.signing_payload()),
        })
    }

    pub fn verify_signature(&self, signature: &RegistryManifestSignature, key: &RegistrySigningKey) -> Result<()> {
        key.validate()?;
        if signature.algorithm != "hmac-sha256" {
            anyhow::bail!(
                "registry manifest signature uses unsupported algorithm `{}`",
                signature.algorithm
            );
        }
        if signature.key_id != key.key_id {
            anyhow::bail!(
                "registry manifest signature key mismatch: expected `{}`, found `{}`",
                key.key_id,
                signature.key_id
            );
        }
        let expected = hmac_sha256_hex(key.secret.as_bytes(), &self.signing_payload());
        if !constant_time_eq_hex(&expected, &signature.digest) {
            anyhow::bail!(
                "registry manifest signature mismatch for `{}` {}",
                self.package,
                self.version
            );
        }
        Ok(())
    }

    pub fn validate_for_registry_server(&self, expected_registry_url: &str) -> Result<RegistryPublishServerValidation> {
        if self.package.trim().is_empty() {
            anyhow::bail!("registry publish manifest package must not be empty");
        }
        if self.package.contains('/') || self.package.contains('\\') {
            anyhow::bail!(
                "registry publish manifest package `{}` must not contain path separators",
                self.package
            );
        }
        if !is_publish_version(&self.version) {
            anyhow::bail!(
                "registry publish manifest version `{}` must be a semantic version",
                self.version
            );
        }
        if self.registry_url.trim_end_matches('/') != expected_registry_url.trim_end_matches('/') {
            anyhow::bail!(
                "registry publish manifest URL mismatch for `{}` {}: expected {}, found {}",
                self.package,
                self.version,
                expected_registry_url,
                self.registry_url
            );
        }
        if self.integrity.algorithm != "sha256" || self.integrity.digest.len() != 64 {
            anyhow::bail!(
                "registry publish manifest integrity for `{}` {} must use sha256 with a 64-character hex digest",
                self.package,
                self.version
            );
        }
        if !self.integrity.digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
            anyhow::bail!(
                "registry publish manifest integrity for `{}` {} must be hex encoded",
                self.package,
                self.version
            );
        }
        if !self.verify_integrity() {
            anyhow::bail!(
                "registry publish manifest integrity mismatch for `{}` {}",
                self.package,
                self.version
            );
        }
        let signing_digest = Sha256::digest(self.signing_payload());
        Ok(RegistryPublishServerValidation {
            immutable_manifest_key: format!(
                "packages/{}/{}/manifest-{}.json",
                self.package, self.version, self.integrity.digest
            ),
            signing_payload_sha256: format!("{signing_digest:x}"),
        })
    }

    pub fn store_immutable_manifest(
        &self,
        storage_root: &Path,
        expected_registry_url: &str,
    ) -> Result<RegistryPublishStoredManifest> {
        let validation = self.validate_for_registry_server(expected_registry_url)?;
        let path = storage_root.join(&validation.immutable_manifest_key);
        let body = serde_json::to_vec_pretty(self).context("serialize registry publish manifest")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create registry storage {}", parent.display()))?;
        }
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&body)
                    .with_context(|| format!("write immutable registry manifest {}", path.display()))?;
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                let existing =
                    fs::read(&path).with_context(|| format!("read immutable registry manifest {}", path.display()))?;
                if existing != body {
                    anyhow::bail!(
                        "immutable registry manifest collision at {} for {} {}",
                        path.display(),
                        self.package,
                        self.version
                    );
                }
            }
            Err(err) => {
                return Err(err).with_context(|| format!("create immutable registry manifest {}", path.display()));
            }
        }
        Ok(RegistryPublishStoredManifest { path, validation })
    }
}

impl RegistryIndex {
    pub fn read_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let body = fs::read_to_string(path).with_context(|| format!("read registry index {}", path.display()))?;
        serde_json::from_str(&body).with_context(|| format!("parse registry index {}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create registry index directory {}", parent.display()))?;
        }
        let body = serde_json::to_vec_pretty(self).context("serialize registry index")?;
        fs::write(path, body).with_context(|| format!("write registry index {}", path.display()))
    }

    pub fn publish_manifest(
        storage_root: &Path,
        expected_registry_url: &str,
        manifest: &RegistryPublishManifest,
        checksum: Option<String>,
    ) -> Result<RegistryStoredPackage> {
        Self::publish_manifest_signed(storage_root, expected_registry_url, manifest, checksum, None)
    }

    pub fn publish_manifest_signed(
        storage_root: &Path,
        expected_registry_url: &str,
        manifest: &RegistryPublishManifest,
        checksum: Option<String>,
        signing_key: Option<&RegistrySigningKey>,
    ) -> Result<RegistryStoredPackage> {
        let signing_material = signing_key.cloned().map(RegistrySigningMaterial::Hmac);
        Self::publish_manifest_record_with_signer(
            storage_root,
            expected_registry_url,
            manifest,
            checksum,
            None,
            None,
            signing_material.as_ref(),
        )
    }

    fn publish_manifest_record_with_signer(
        storage_root: &Path,
        expected_registry_url: &str,
        manifest: &RegistryPublishManifest,
        checksum: Option<String>,
        source: Option<String>,
        rev: Option<String>,
        signing_material: Option<&RegistrySigningMaterial>,
    ) -> Result<RegistryStoredPackage> {
        let stored = manifest.store_immutable_manifest(storage_root, expected_registry_url)?;
        let index_path = storage_root.join("index.json");
        let mut index = Self::read_or_default(&index_path)?;
        let package = index.packages.entry(manifest.package.clone()).or_default();
        let signature = signing_material
            .map(|material| material.sign_manifest(manifest))
            .transpose()?;
        if let (Some(signature), Some(material)) = (&signature, signing_material) {
            material.verify_manifest_signature(manifest, signature)?;
        }
        let version = RegistryStoredVersion {
            version: manifest.version.clone(),
            source,
            rev,
            manifest_key: stored.validation.immutable_manifest_key.clone(),
            signing_payload_sha256: stored.validation.signing_payload_sha256.clone(),
            yanked: false,
            checksum,
            signature,
            macro_providers: manifest.macro_providers.clone(),
        };
        if let Some(existing) = package
            .versions
            .iter_mut()
            .find(|entry| entry.version == manifest.version)
        {
            if existing.manifest_key != version.manifest_key {
                anyhow::bail!(
                    "registry index collision for {} {}: existing {}, new {}",
                    manifest.package,
                    manifest.version,
                    existing.manifest_key,
                    version.manifest_key
                );
            }
            existing.signing_payload_sha256 = version.signing_payload_sha256.clone();
            existing.source = version.source.clone();
            existing.rev = version.rev.clone();
            existing.yanked = false;
            existing.checksum = version.checksum.clone();
            existing.signature = version.signature.clone();
            existing.macro_providers = version.macro_providers.clone();
        } else {
            package.versions.push(version.clone());
            package.versions.sort_by(|left, right| left.version.cmp(&right.version));
        }
        index.write(&index_path)?;
        Ok(RegistryStoredPackage {
            manifest_path: stored.path,
            index_path,
            version,
        })
    }

    pub fn set_yanked(storage_root: &Path, package: &str, version: &str, yanked: bool) -> Result<()> {
        let index_path = storage_root.join("index.json");
        let mut index = Self::read_or_default(&index_path)?;
        let Some(package_index) = index.packages.get_mut(package) else {
            anyhow::bail!("registry index has no package `{package}`");
        };
        let Some(version_entry) = package_index.versions.iter_mut().find(|entry| entry.version == version) else {
            anyhow::bail!("registry index has no version `{package}` {version}");
        };
        version_entry.yanked = yanked;
        index.write(&index_path)
    }
}

impl RegistryService {
    pub fn new(storage_root: impl Into<PathBuf>, registry_url: impl Into<String>) -> Self {
        Self {
            storage_root: storage_root.into(),
            registry_url: registry_url.into(),
            signing_material: None,
        }
    }

    pub fn with_signing_key(mut self, signing_key: RegistrySigningKey) -> Self {
        self.signing_material = Some(RegistrySigningMaterial::Hmac(signing_key));
        self
    }

    pub fn with_asymmetric_signing_key(mut self, signing_key: RegistryAsymmetricSigningKey) -> Self {
        self.signing_material = Some(RegistrySigningMaterial::Ed25519(signing_key));
        self
    }

    pub fn publish_request_json(&self, body: &str) -> Result<String> {
        let request: RegistryPublishRequest =
            serde_json::from_str(body).context("parse registry publish request JSON")?;
        let stored = RegistryIndex::publish_manifest_record_with_signer(
            &self.storage_root,
            &self.registry_url,
            &request.publish_manifest,
            request.checksum.clone(),
            Some(request.source.clone()),
            Some(request.rev.clone()),
            self.signing_material.as_ref(),
        )?;
        let response = self.version_response(&request.publish_manifest.package, &stored.version)?;
        serde_json::to_string_pretty(&response).context("serialize registry publish response JSON")
    }

    pub fn index_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.index_response()?).context("serialize registry index JSON")
    }

    pub fn package_versions_json(&self, package: &str) -> Result<String> {
        let response = self.package_response(package)?;
        serde_json::to_string_pretty(&response).context("serialize registry package versions JSON")
    }

    pub fn package_version_json(&self, package: &str, version: &str) -> Result<String> {
        let index_path = self.storage_root.join("index.json");
        let index = RegistryIndex::read_or_default(&index_path)?;
        let package_index = index
            .packages
            .get(package)
            .ok_or_else(|| anyhow::anyhow!("registry index has no package `{package}`"))?;
        let version = package_index
            .versions
            .iter()
            .find(|entry| entry.version == version)
            .ok_or_else(|| anyhow::anyhow!("registry index has no version `{package}` {version}"))?;
        let response = self.version_response(package, version)?;
        serde_json::to_string_pretty(&response).context("serialize registry package version JSON")
    }

    pub fn yank_version(&self, package: &str, version: &str, yanked: bool) -> Result<()> {
        RegistryIndex::set_yanked(&self.storage_root, package, version, yanked)
    }

    fn index_response(&self) -> Result<RegistryIndexResponse> {
        let index_path = self.storage_root.join("index.json");
        let index = RegistryIndex::read_or_default(&index_path)?;
        let mut packages = Vec::new();
        for (name, _package) in index.packages {
            packages.push(self.package_response(&name)?);
        }
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(RegistryIndexResponse { packages })
    }

    fn package_response(&self, package: &str) -> Result<RegistryPackageIndexResponse> {
        let index_path = self.storage_root.join("index.json");
        let index = RegistryIndex::read_or_default(&index_path)?;
        let package_index = index
            .packages
            .get(package)
            .ok_or_else(|| anyhow::anyhow!("registry index has no package `{package}`"))?;
        let mut versions = Vec::new();
        for version in &package_index.versions {
            versions.push(self.version_response(package, version)?);
        }
        versions.sort_by(|left, right| left.version.cmp(&right.version));
        let macro_providers = versions
            .last()
            .and_then(|version| version.publish_manifest.as_ref())
            .map(|manifest| manifest.macro_providers.clone())
            .unwrap_or_default();
        Ok(RegistryPackageIndexResponse {
            name: package.to_string(),
            versions,
            macro_providers,
        })
    }

    fn version_response(
        &self,
        package: &str,
        version: &RegistryStoredVersion,
    ) -> Result<RegistryPackageVersionResponse> {
        let manifest = self.read_stored_manifest(version)?;
        if manifest.package != package || manifest.version != version.version {
            anyhow::bail!(
                "registry stored manifest mismatch for `{package}` {}: found `{}` {}",
                version.version,
                manifest.package,
                manifest.version
            );
        }
        Ok(RegistryPackageVersionResponse {
            version: version.version.clone(),
            source: version
                .source
                .clone()
                .ok_or_else(|| anyhow::anyhow!("registry version `{package}` {} has no source", version.version))?,
            rev: version
                .rev
                .clone()
                .ok_or_else(|| anyhow::anyhow!("registry version `{package}` {} has no rev", version.version))?,
            checksum: version.checksum.clone(),
            yanked: version.yanked,
            publish_manifest: Some(manifest),
            signature: version.signature.clone(),
        })
    }

    fn read_stored_manifest(&self, version: &RegistryStoredVersion) -> Result<RegistryPublishManifest> {
        let path = self.storage_root.join(&version.manifest_key);
        let body = fs::read(&path).with_context(|| format!("read registry stored manifest {}", path.display()))?;
        serde_json::from_slice(&body).with_context(|| format!("parse registry stored manifest {}", path.display()))
    }
}

impl RegistrySigningKey {
    pub fn new(key_id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            secret: secret.into(),
        }
    }

    pub fn generate(key_id: impl Into<String>) -> Result<Self> {
        let secret = random_secret_hex();
        let key = Self::new(key_id, secret);
        key.validate()?;
        Ok(key)
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let body = fs::read(path).with_context(|| format!("read registry signing key {}", path.display()))?;
        let key: Self =
            serde_json::from_slice(&body).with_context(|| format!("parse registry signing key {}", path.display()))?;
        key.validate()
            .with_context(|| format!("validate registry signing key {}", path.display()))?;
        Ok(key)
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create registry signing key dir {}", parent.display()))?;
        }
        let body = serde_json::to_vec_pretty(self).context("serialize registry signing key")?;
        fs::write(path, body).with_context(|| format!("write registry signing key {}", path.display()))?;
        set_private_file_permissions(path)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.key_id.trim().is_empty() {
            anyhow::bail!("registry signing key id must not be empty");
        }
        if self.secret.is_empty() {
            anyhow::bail!("registry signing key secret must not be empty");
        }
        Ok(())
    }
}

impl RegistrySigningKeyring {
    pub fn new(active_key: RegistrySigningKey) -> Result<Self> {
        active_key.validate()?;
        let keyring = Self {
            active_key_id: active_key.key_id.clone(),
            keys: vec![active_key],
            revoked_key_ids: Vec::new(),
        };
        keyring.validate()?;
        Ok(keyring)
    }

    pub fn generate(active_key_id: impl Into<String>) -> Result<Self> {
        Self::new(RegistrySigningKey::generate(active_key_id)?)
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let body = fs::read(path).with_context(|| format!("read registry signing keyring {}", path.display()))?;
        let keyring: Self = serde_json::from_slice(&body)
            .with_context(|| format!("parse registry signing keyring {}", path.display()))?;
        keyring
            .validate()
            .with_context(|| format!("validate registry signing keyring {}", path.display()))?;
        Ok(keyring)
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create registry signing keyring dir {}", parent.display()))?;
        }
        let body = serde_json::to_vec_pretty(self).context("serialize registry signing keyring")?;
        fs::write(path, body).with_context(|| format!("write registry signing keyring {}", path.display()))?;
        set_private_file_permissions(path)?;
        Ok(())
    }

    pub fn active_key(&self) -> Result<&RegistrySigningKey> {
        self.validate()?;
        self.keys
            .iter()
            .find(|key| key.key_id == self.active_key_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "registry signing keyring active key `{}` is missing",
                    self.active_key_id
                )
            })
    }

    pub fn verification_key(&self, key_id: &str) -> Result<&RegistrySigningKey> {
        self.validate()?;
        if self.revoked_key_ids.iter().any(|revoked| revoked == key_id) {
            anyhow::bail!("registry signing key `{key_id}` is revoked");
        }
        self.keys
            .iter()
            .find(|key| key.key_id == key_id)
            .ok_or_else(|| anyhow::anyhow!("registry signing key `{key_id}` is not trusted"))
    }

    pub fn rotate(&mut self, new_active_key_id: impl Into<String>) -> Result<()> {
        let key = RegistrySigningKey::generate(new_active_key_id)?;
        if self.keys.iter().any(|existing| existing.key_id == key.key_id) {
            anyhow::bail!("registry signing key `{}` already exists", key.key_id);
        }
        self.active_key_id = key.key_id.clone();
        self.keys.push(key);
        self.validate()
    }

    pub fn revoke(&mut self, key_id: impl Into<String>) -> Result<()> {
        let key_id = key_id.into();
        if key_id == self.active_key_id {
            anyhow::bail!("registry signing keyring cannot revoke active key `{key_id}`");
        }
        if !self.keys.iter().any(|key| key.key_id == key_id) {
            anyhow::bail!("registry signing key `{key_id}` is not in the keyring");
        }
        if !self.revoked_key_ids.iter().any(|revoked| revoked == &key_id) {
            self.revoked_key_ids.push(key_id);
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<()> {
        if self.active_key_id.trim().is_empty() {
            anyhow::bail!("registry signing keyring active key id must not be empty");
        }
        if self.keys.is_empty() {
            anyhow::bail!("registry signing keyring must contain at least one key");
        }
        let mut seen = BTreeSet::new();
        for key in &self.keys {
            key.validate()?;
            if !seen.insert(key.key_id.as_str()) {
                anyhow::bail!("registry signing keyring contains duplicate key `{}`", key.key_id);
            }
        }
        if !seen.contains(self.active_key_id.as_str()) {
            anyhow::bail!(
                "registry signing keyring active key `{}` is missing",
                self.active_key_id
            );
        }
        let mut revoked = BTreeSet::new();
        for key_id in &self.revoked_key_ids {
            if key_id.trim().is_empty() {
                anyhow::bail!("registry signing keyring revoked key id must not be empty");
            }
            if !seen.contains(key_id.as_str()) {
                anyhow::bail!("registry signing keyring revoked key `{key_id}` is missing");
            }
            if key_id == &self.active_key_id {
                anyhow::bail!("registry signing keyring active key `{key_id}` cannot be revoked");
            }
            if !revoked.insert(key_id.as_str()) {
                anyhow::bail!("registry signing keyring contains duplicate revoked key `{key_id}`");
            }
        }
        Ok(())
    }
}

fn random_secret_hex() -> String {
    let bytes: [u8; 32] = rand::random();
    hex_bytes(&bytes)
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("read registry signing key metadata {}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("set registry signing key permissions {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn is_registry_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn hmac_sha256_hex(key: &[u8], payload: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36u8; BLOCK_SIZE];
    let mut outer_pad = [0x5cu8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(payload);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

fn constant_time_eq_hex(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::super::MANIFEST_FILE;
    use super::*;

    #[test]
    fn registry_publish_manifest_records_versioned_macro_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("tools"))?;
        fs::write(root.join("src/mod.lk"), "return 1;\n")?;
        fs::write(root.join("tools/sql"), "#!/bin/sh\n")?;
        fs::write(
            root.join(MANIFEST_FILE),
            r#"
                [package]
                name = "macro_app"
                version = "0.2.3"

                [registry]
                name = "local"
                url = "https://registry.lk.example"
                include = ["Lk.toml", "src/**", "tools/sql"]

                [macros.function_like.sql]
                command = "./tools/sql"
            "#,
        )?;

        let graph = PackageGraph::discover(root)?.unwrap();
        let publish = graph.registry_publish_manifest()?;

        assert_eq!(publish.package, "macro_app");
        assert_eq!(publish.version, "0.2.3");
        assert_eq!(publish.registry, "local");
        assert_eq!(publish.registry_url, "https://registry.lk.example");
        assert_eq!(publish.include, vec!["Lk.toml", "src/**", "tools/sql"]);
        assert_eq!(publish.macro_providers.function_like, vec!["sql"]);
        assert_eq!(publish.integrity.algorithm, "sha256");
        assert_eq!(publish.integrity.digest.len(), 64);
        assert!(publish.verify_integrity());

        let mut tampered = publish.clone();
        tampered.include.push("extra/**".to_string());
        assert!(!tampered.verify_integrity());
        Ok(())
    }

    #[test]
    fn registry_publish_manifest_server_validation_returns_immutable_storage_and_signing_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "0.2.3"

                [registry]
                name = "local"
                url = "https://registry.lk.example"

                [macros.derive.MakeAnswer]
                command = "./tools/derive"
            "#,
        )?;
        let publish = manifest.registry_publish_manifest(&BTreeMap::new())?;

        let validation = publish.validate_for_registry_server("https://registry.lk.example/")?;

        assert_eq!(
            validation.immutable_manifest_key,
            format!("packages/macro_app/0.2.3/manifest-{}.json", publish.integrity.digest)
        );
        assert_eq!(
            validation.signing_payload_sha256,
            format!("{:x}", Sha256::digest(publish.signing_payload()))
        );

        let mut tampered = publish.clone();
        tampered.macro_providers.derive.push("Other".to_string());
        let err = tampered
            .validate_for_registry_server("https://registry.lk.example")
            .expect_err("tampered manifest should fail validation");
        assert!(err.to_string().contains("integrity mismatch"), "{err}");

        let mut bad_name = publish;
        bad_name.package = "macro/app".to_string();
        let err = bad_name
            .validate_for_registry_server("https://registry.lk.example")
            .expect_err("path-like package names should fail validation");
        assert!(err.to_string().contains("path separators"), "{err}");

        let stored = manifest
            .registry_publish_manifest(&BTreeMap::new())?
            .store_immutable_manifest(temp.path(), "https://registry.lk.example")?;
        assert_eq!(
            stored.validation.immutable_manifest_key,
            validation.immutable_manifest_key
        );
        assert!(stored.path.exists());
        let stored_manifest: RegistryPublishManifest = serde_json::from_slice(&fs::read(&stored.path)?)?;
        assert_eq!(stored_manifest, manifest.registry_publish_manifest(&BTreeMap::new())?);

        let duplicate = stored_manifest.store_immutable_manifest(temp.path(), "https://registry.lk.example")?;
        assert_eq!(duplicate.path, stored.path);

        fs::write(&stored.path, b"{\"tampered\":true}")?;
        let err = stored_manifest
            .store_immutable_manifest(temp.path(), "https://registry.lk.example")
            .expect_err("immutable storage must reject conflicting content");
        assert!(
            err.to_string().contains("immutable registry manifest collision"),
            "{err}"
        );
        Ok(())
    }

    #[test]
    fn registry_index_persists_publish_manifest_and_yank_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "0.2.3"

                [registry]
                name = "local"
                url = "https://registry.lk.example"

                [macros.function_like.sql]
                command = "./tools/sql"
            "#,
        )?;
        let publish = manifest.registry_publish_manifest(&BTreeMap::new())?;

        let stored = RegistryIndex::publish_manifest(
            temp.path(),
            "https://registry.lk.example",
            &publish,
            Some("sha256:package".to_string()),
        )?;

        assert!(stored.manifest_path.exists());
        assert!(stored.index_path.exists());
        assert_eq!(stored.version.version, "0.2.3");
        assert_eq!(stored.version.macro_providers.function_like, vec!["sql"]);
        assert!(!stored.version.yanked);

        RegistryIndex::set_yanked(temp.path(), "macro_app", "0.2.3", true)?;
        let index = RegistryIndex::read_or_default(&stored.index_path)?;
        let version = index.packages["macro_app"]
            .versions
            .iter()
            .find(|version| version.version == "0.2.3")
            .expect("published version in index");
        assert!(version.yanked);
        assert_eq!(version.manifest_key, stored.version.manifest_key);
        assert_eq!(version.checksum.as_deref(), Some("sha256:package"));
        Ok(())
    }

    #[test]
    fn registry_manifest_signature_is_verified_and_persisted() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "0.2.3"

                [registry]
                name = "local"
                url = "https://registry.lk.example"

                [macros.derive.MakeAnswer]
                command = "./tools/derive"
            "#,
        )?;
        let publish = manifest.registry_publish_manifest(&BTreeMap::new())?;
        let key = RegistrySigningKey::new("local-key", "registry-secret");

        let signature = publish.sign(&key)?;
        publish.verify_signature(&signature, &key)?;

        let mut tampered = publish.clone();
        tampered.include.push("extra/**".to_string());
        let err = tampered
            .verify_signature(&signature, &key)
            .expect_err("signature must reject tampered payload");
        assert!(err.to_string().contains("signature mismatch"), "{err}");

        let wrong_key = RegistrySigningKey::new("local-key", "wrong-secret");
        let err = publish
            .verify_signature(&signature, &wrong_key)
            .expect_err("signature must reject wrong secret");
        assert!(err.to_string().contains("signature mismatch"), "{err}");

        let stored = RegistryIndex::publish_manifest_signed(
            temp.path(),
            "https://registry.lk.example",
            &publish,
            None,
            Some(&key),
        )?;
        let stored_signature = stored.version.signature.expect("stored signature");
        assert_eq!(stored_signature, signature);

        let index = RegistryIndex::read_or_default(&stored.index_path)?;
        let version = &index.packages["macro_app"].versions[0];
        publish.verify_signature(version.signature.as_ref().expect("index signature"), &key)?;
        Ok(())
    }

    #[test]
    fn registry_signing_key_json_round_trips_and_validates() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("keys").join("local.json");
        let key = RegistrySigningKey::generate("local-key")?;

        assert_eq!(key.key_id, "local-key");
        assert_eq!(key.secret.len(), 64);
        assert!(key.secret.chars().all(|ch| ch.is_ascii_hexdigit()));

        key.write_json(&path)?;
        let loaded = RegistrySigningKey::read_json(&path)?;
        assert_eq!(loaded, key);

        fs::write(&path, r#"{"key_id":"","secret":"registry-secret"}"#)?;
        let err = RegistrySigningKey::read_json(&path).expect_err("empty key id should be rejected");
        assert!(format!("{err:#}").contains("registry signing key id"), "{err:#}");

        fs::write(&path, r#"{"key_id":"local-key","secret":""}"#)?;
        let err = RegistrySigningKey::read_json(&path).expect_err("empty secret should be rejected");
        assert!(format!("{err:#}").contains("registry signing key secret"), "{err:#}");
        Ok(())
    }

    #[test]
    fn registry_signing_keyring_rotates_and_rejects_revoked_keys() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("keys").join("registry.json");
        let old_key = RegistrySigningKey::new("old-key", "old-secret");
        let mut keyring = RegistrySigningKeyring::new(old_key.clone())?;

        assert_eq!(keyring.active_key()?.key_id, "old-key");
        keyring.rotate("new-key")?;
        assert_eq!(keyring.active_key()?.key_id, "new-key");
        assert_eq!(keyring.verification_key("old-key")?.secret, old_key.secret);

        keyring.revoke("old-key")?;
        let err = keyring
            .verification_key("old-key")
            .expect_err("revoked key should not verify");
        assert!(err.to_string().contains("revoked"), "{err}");

        keyring.write_json(&path)?;
        let loaded = RegistrySigningKeyring::read_json(&path)?;
        assert_eq!(loaded, keyring);

        let mut active_revoke = loaded.clone();
        let err = active_revoke
            .revoke("new-key")
            .expect_err("active key cannot be revoked");
        assert!(err.to_string().contains("active key"), "{err}");
        Ok(())
    }

    #[test]
    fn registry_asymmetric_signing_key_signs_and_public_key_verifies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let private_path = temp.path().join("keys").join("private.json");
        let public_path = temp.path().join("keys").join("public.json");
        let key = RegistryAsymmetricSigningKey::generate("ed-key")?;
        let public_key = key.public_key()?;

        key.write_json(&private_path)?;
        public_key.write_json(&public_path)?;
        assert_eq!(RegistryAsymmetricSigningKey::read_json(&private_path)?, key);
        assert_eq!(RegistryPublicSigningKey::read_json(&public_path)?, public_key);

        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "0.2.3"

                [registry]
                name = "local"
                url = "https://registry.lk.example"
                include = ["Lk.toml"]
            "#,
        )?;
        let manifest = manifest.registry_publish_manifest(&BTreeMap::new())?;
        let signature = manifest.sign_asymmetric(&key)?;
        assert_eq!(signature.algorithm, "ed25519");
        manifest.verify_public_signature(&signature, &public_key)?;

        let wrong_key = RegistryAsymmetricSigningKey::generate("ed-key")?.public_key()?;
        let err = manifest
            .verify_public_signature(&signature, &wrong_key)
            .expect_err("wrong public key should reject signature");
        assert!(format!("{err:#}").contains("public signature mismatch"), "{err:#}");
        Ok(())
    }

    #[test]
    fn registry_service_publishes_signed_index_and_version_responses() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "0.2.3"

                [registry]
                name = "local"
                url = "https://registry.lk.example"

                [macros.function_like.sql]
                command = "./tools/sql"
            "#,
        )?;
        let publish = manifest.registry_publish_manifest(&BTreeMap::new())?;
        let service = RegistryService::new(temp.path(), "https://registry.lk.example")
            .with_signing_key(RegistrySigningKey::new("local-key", "registry-secret"));
        let request = RegistryPublishRequest {
            source: "https://example.invalid/macro_app.git".to_string(),
            rev: "abc123".to_string(),
            checksum: Some("sha256:package".to_string()),
            publish_manifest: publish.clone(),
        };

        let response: RegistryPackageVersionResponse =
            serde_json::from_str(&service.publish_request_json(&serde_json::to_string(&request)?)?)?;
        assert_eq!(response.version, "0.2.3");
        assert_eq!(response.source, "https://example.invalid/macro_app.git");
        assert_eq!(response.rev, "abc123");
        assert_eq!(response.checksum.as_deref(), Some("sha256:package"));
        assert_eq!(response.publish_manifest.as_ref(), Some(&publish));
        publish.verify_signature(
            response.signature.as_ref().expect("publish response signature"),
            &RegistrySigningKey::new("local-key", "registry-secret"),
        )?;

        let package_response: RegistryPackageIndexResponse =
            serde_json::from_str(&service.package_versions_json("macro_app")?)?;
        assert_eq!(package_response.name, "macro_app");
        assert_eq!(package_response.versions.len(), 1);
        assert_eq!(package_response.macro_providers.function_like, vec!["sql"]);

        let version_response: RegistryPackageVersionResponse =
            serde_json::from_str(&service.package_version_json("macro_app", "0.2.3")?)?;
        assert_eq!(version_response.source, "https://example.invalid/macro_app.git");
        assert!(!version_response.yanked);

        service.yank_version("macro_app", "0.2.3", true)?;
        let index_response: RegistryIndexResponse = serde_json::from_str(&service.index_json()?)?;
        assert_eq!(index_response.packages.len(), 1);
        assert!(index_response.packages[0].versions[0].yanked);
        Ok(())
    }

    #[test]
    fn registry_publish_manifest_requires_registry_and_semver() -> Result<()> {
        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "dev"
            "#,
        )?;
        let err = manifest
            .registry_publish_manifest(&BTreeMap::new())
            .expect_err("non-semver packages are not publishable");
        assert!(err.to_string().contains("semantic version"), "{err}");

        let manifest: Manifest = toml::from_str(
            r#"
                [package]
                name = "macro_app"
                version = "0.1.0"
            "#,
        )?;
        let err = manifest
            .registry_publish_manifest(&BTreeMap::new())
            .expect_err("registry metadata is required");
        assert!(err.to_string().contains("[registry]"), "{err}");
        Ok(())
    }
}
