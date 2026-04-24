use super::{Keystore, SecretKey};
use anyhow::{Context, Result};
use security_framework::passwords::{get_generic_password, set_generic_password};

const ACCOUNT: &str = "dictation-db-key";

pub struct MacKeystore;

impl Keystore for MacKeystore {
    fn get_or_create_db_key(&self, service: &str) -> Result<SecretKey> {
        match get_generic_password(service, ACCOUNT) {
            Ok(data) => {
                if data.len() != 32 {
                    anyhow::bail!("keychain key has wrong length: {}", data.len());
                }
                let mut key = [0u8; 32];
                key.copy_from_slice(&data);
                Ok(SecretKey(key))
            }
            Err(_) => {
                let mut key = [0u8; 32];
                getrandom(&mut key)?;
                set_generic_password(service, ACCOUNT, &key)
                    .context("failed to store key in Keychain")?;
                Ok(SecretKey(key))
            }
        }
    }
}

fn getrandom(buf: &mut [u8]) -> Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
    Ok(())
}
