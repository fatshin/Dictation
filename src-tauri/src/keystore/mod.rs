use anyhow::Result;
use zeroize::Zeroize;

#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct SecretKey(pub [u8; 32]);

impl SecretKey {
    pub fn as_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

pub trait Keystore: Send + Sync {
    fn get_or_create_db_key(&self, service: &str) -> Result<SecretKey>;
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacKeystore;

#[cfg(not(target_os = "macos"))]
pub struct StubKeystore;

#[cfg(not(target_os = "macos"))]
impl Keystore for StubKeystore {
    fn get_or_create_db_key(&self, _service: &str) -> Result<SecretKey> {
        anyhow::bail!("Keystore not implemented for this platform")
    }
}
