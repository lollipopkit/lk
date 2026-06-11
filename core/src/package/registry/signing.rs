use std::{convert::TryInto, fs, path::Path};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{
    Signature, Signer, SigningKey as Ed25519SigningKey, Verifier, VerifyingKey as Ed25519VerifyingKey,
};
use serde::Serialize;

use super::{
    RegistryAsymmetricSigningKey, RegistryManifestSignature, RegistryPublicSigningKey, RegistryPublishManifest,
    RegistrySigningMaterial, set_private_file_permissions,
};

impl RegistryPublishManifest {
    pub fn sign_asymmetric(&self, key: &RegistryAsymmetricSigningKey) -> Result<RegistryManifestSignature> {
        key.validate()?;
        let signing_key = key.ed25519_signing_key()?;
        let signature = signing_key.sign(&self.signing_payload());
        Ok(RegistryManifestSignature {
            algorithm: "ed25519".to_string(),
            key_id: key.key_id.clone(),
            digest: BASE64.encode(signature.to_bytes()),
        })
    }

    pub fn verify_public_signature(
        &self,
        signature: &RegistryManifestSignature,
        key: &RegistryPublicSigningKey,
    ) -> Result<()> {
        key.validate()?;
        if signature.algorithm != "ed25519" {
            anyhow::bail!(
                "registry manifest signature uses unsupported public-key algorithm `{}`",
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
        let verifying_key = key.ed25519_verifying_key()?;
        let signature_bytes = BASE64
            .decode(&signature.digest)
            .context("decode registry manifest ed25519 signature")?;
        let signature_bytes: [u8; 64] = signature_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("registry manifest ed25519 signature must be 64 bytes"))?;
        let ed25519_signature = Signature::from_bytes(&signature_bytes);
        verifying_key
            .verify(&self.signing_payload(), &ed25519_signature)
            .with_context(|| {
                format!(
                    "registry manifest public signature mismatch for `{}` {}",
                    self.package, self.version
                )
            })
    }
}

impl RegistryAsymmetricSigningKey {
    pub fn generate(key_id: impl Into<String>) -> Result<Self> {
        let secret_bytes: [u8; 32] = rand::random();
        let signing_key = Ed25519SigningKey::from_bytes(&secret_bytes);
        let key = Self {
            key_id: key_id.into(),
            secret_key: BASE64.encode(secret_bytes),
            public_key: BASE64.encode(signing_key.verifying_key().to_bytes()),
        };
        key.validate()?;
        Ok(key)
    }

    pub fn public_key(&self) -> Result<RegistryPublicSigningKey> {
        self.validate()?;
        Ok(RegistryPublicSigningKey {
            key_id: self.key_id.clone(),
            public_key: self.public_key.clone(),
        })
    }

    pub fn read_json(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let body = fs::read(path).with_context(|| format!("read registry private signing key {}", path.display()))?;
        let key: Self = serde_json::from_slice(&body)
            .with_context(|| format!("parse registry private signing key {}", path.display()))?;
        key.validate()
            .with_context(|| format!("validate registry private signing key {}", path.display()))?;
        Ok(key)
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        write_private_json(path.as_ref(), self, "registry private signing key")
    }

    fn validate(&self) -> Result<()> {
        if self.key_id.trim().is_empty() {
            anyhow::bail!("registry private signing key id must not be empty");
        }
        let signing_key = self.ed25519_signing_key()?;
        let expected_public = BASE64.encode(signing_key.verifying_key().to_bytes());
        if expected_public != self.public_key {
            anyhow::bail!("registry private signing key public key does not match secret key");
        }
        Ok(())
    }

    fn ed25519_signing_key(&self) -> Result<Ed25519SigningKey> {
        let bytes = BASE64
            .decode(&self.secret_key)
            .context("decode registry private signing key secret")?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("registry private signing key secret must be 32 bytes"))?;
        Ok(Ed25519SigningKey::from_bytes(&bytes))
    }
}

impl RegistryPublicSigningKey {
    pub fn read_json(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let body = fs::read(path).with_context(|| format!("read registry public signing key {}", path.display()))?;
        let key: Self = serde_json::from_slice(&body)
            .with_context(|| format!("parse registry public signing key {}", path.display()))?;
        key.validate()
            .with_context(|| format!("validate registry public signing key {}", path.display()))?;
        Ok(key)
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create registry public signing key dir {}", parent.display()))?;
        }
        let body = serde_json::to_vec_pretty(self).context("serialize registry public signing key")?;
        fs::write(path, body).with_context(|| format!("write registry public signing key {}", path.display()))
    }

    fn validate(&self) -> Result<()> {
        if self.key_id.trim().is_empty() {
            anyhow::bail!("registry public signing key id must not be empty");
        }
        self.ed25519_verifying_key()?;
        Ok(())
    }

    fn ed25519_verifying_key(&self) -> Result<Ed25519VerifyingKey> {
        let bytes = BASE64
            .decode(&self.public_key)
            .context("decode registry public signing key")?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("registry public signing key must be 32 bytes"))?;
        Ed25519VerifyingKey::from_bytes(&bytes).context("parse registry public signing key")
    }
}

impl RegistrySigningMaterial {
    pub(super) fn sign_manifest(&self, manifest: &RegistryPublishManifest) -> Result<RegistryManifestSignature> {
        match self {
            RegistrySigningMaterial::Hmac(key) => manifest.sign(key),
            RegistrySigningMaterial::Ed25519(key) => manifest.sign_asymmetric(key),
        }
    }

    pub(super) fn verify_manifest_signature(
        &self,
        manifest: &RegistryPublishManifest,
        signature: &RegistryManifestSignature,
    ) -> Result<()> {
        match self {
            RegistrySigningMaterial::Hmac(key) => manifest.verify_signature(signature, key),
            RegistrySigningMaterial::Ed25519(key) => manifest.verify_public_signature(signature, &key.public_key()?),
        }
    }
}

fn write_private_json<T: Serialize>(path: &Path, value: &T, label: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("create {label} dir {}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(value).with_context(|| format!("serialize {label}"))?;
    fs::write(path, body).with_context(|| format!("write {label} {}", path.display()))?;
    set_private_file_permissions(path)?;
    Ok(())
}
