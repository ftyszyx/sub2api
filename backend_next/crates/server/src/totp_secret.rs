use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::RngCore;
use thiserror::Error;

const NONCE_LEN: usize = 12;

#[derive(Debug, Error)]
pub enum TotpSecretError {
    #[error("totp encryption key is not configured")]
    MissingKey,

    #[error("invalid totp encryption key: {0}")]
    InvalidKey(String),

    #[error("invalid totp secret ciphertext")]
    InvalidCiphertext,

    #[error("totp secret encryption failed")]
    EncryptFailed,

    #[error("totp secret decryption failed")]
    DecryptFailed,
}

#[derive(Clone)]
pub struct TotpSecretEncryptor {
    key: Option<[u8; 32]>,
}

impl TotpSecretEncryptor {
    pub fn disabled() -> Self {
        Self { key: None }
    }

    pub fn from_hex_key(key: Option<&str>) -> Result<Self, TotpSecretError> {
        let Some(key) = key.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::disabled());
        };
        let decoded =
            hex::decode(key).map_err(|error| TotpSecretError::InvalidKey(error.to_string()))?;
        let key: [u8; 32] = decoded.try_into().map_err(|value: Vec<u8>| {
            TotpSecretError::InvalidKey(format!("expected 32 bytes, got {}", value.len()))
        })?;
        Ok(Self { key: Some(key) })
    }

    pub fn is_configured(&self) -> bool {
        self.key.is_some()
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String, TotpSecretError> {
        let key = self.key.as_ref().ok_or(TotpSecretError::MissingKey)?;
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| TotpSecretError::InvalidKey("AES-256-GCM key rejected".to_owned()))?;
        let mut nonce = [0_u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| TotpSecretError::EncryptFailed)?;
        let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(BASE64.encode(output))
    }

    pub fn decrypt(&self, ciphertext: &str) -> Result<String, TotpSecretError> {
        let key = self.key.as_ref().ok_or(TotpSecretError::MissingKey)?;
        let data = BASE64
            .decode(ciphertext)
            .map_err(|_| TotpSecretError::InvalidCiphertext)?;
        if data.len() <= NONCE_LEN {
            return Err(TotpSecretError::InvalidCiphertext);
        }
        let (nonce, ciphertext) = data.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| TotpSecretError::InvalidKey("AES-256-GCM key rejected".to_owned()))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| TotpSecretError::DecryptFailed)?;
        String::from_utf8(plaintext).map_err(|_| TotpSecretError::InvalidCiphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn encrypts_and_decrypts_totp_secret() {
        let encryptor = TotpSecretEncryptor::from_hex_key(Some(KEY)).unwrap();
        let ciphertext = encryptor.encrypt("JBSWY3DPEHPK3PXP").unwrap();

        assert_ne!(ciphertext, "JBSWY3DPEHPK3PXP");
        assert_eq!(encryptor.decrypt(&ciphertext).unwrap(), "JBSWY3DPEHPK3PXP");
    }

    #[test]
    fn rejects_missing_or_invalid_key() {
        let disabled = TotpSecretEncryptor::disabled();
        assert!(matches!(
            disabled.encrypt("secret"),
            Err(TotpSecretError::MissingKey)
        ));
        assert!(matches!(
            TotpSecretEncryptor::from_hex_key(Some("bad-key")),
            Err(TotpSecretError::InvalidKey(_))
        ));
    }
}
