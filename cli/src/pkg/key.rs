use lk_core::package::{RegistryAsymmetricSigningKey, RegistrySigningKey, RegistrySigningKeyring};

use crate::PkgKeyCommand;

pub(super) fn run_key_command(command: PkgKeyCommand) -> anyhow::Result<()> {
    match command {
        PkgKeyCommand::Generate { out, key_id } => {
            let key = RegistrySigningKey::generate(key_id)?;
            key.write_json(&out)?;
            println!("Wrote registry signing key {}", out.display());
            Ok(())
        }
        PkgKeyCommand::GenerateAsymmetric {
            private_out,
            public_out,
            key_id,
        } => {
            let key = RegistryAsymmetricSigningKey::generate(key_id)?;
            key.write_json(&private_out)?;
            key.public_key()?.write_json(&public_out)?;
            println!(
                "Wrote registry private signing key {} and public signing key {}",
                private_out.display(),
                public_out.display()
            );
            Ok(())
        }
        PkgKeyCommand::InitKeyring { out, key_id } => {
            let keyring = RegistrySigningKeyring::generate(key_id)?;
            keyring.write_json(&out)?;
            println!("Wrote registry signing keyring {}", out.display());
            Ok(())
        }
        PkgKeyCommand::Rotate { keyring, key_id } => {
            let mut loaded = RegistrySigningKeyring::read_json(&keyring)?;
            loaded.rotate(key_id)?;
            loaded.write_json(&keyring)?;
            println!(
                "Rotated registry signing keyring {}; active key is {}",
                keyring.display(),
                loaded.active_key_id
            );
            Ok(())
        }
        PkgKeyCommand::Revoke { keyring, key_id } => {
            let mut loaded = RegistrySigningKeyring::read_json(&keyring)?;
            loaded.revoke(key_id)?;
            loaded.write_json(&keyring)?;
            println!("Updated registry signing keyring {}", keyring.display());
            Ok(())
        }
    }
}
