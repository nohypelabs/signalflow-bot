use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};
use log::info;

use crate::error::{BotError, Result};

pub struct Wallet {
    signing_key: SigningKey,
    address: String,
}

impl Wallet {
    pub fn new(private_key: &str) -> Result<Self> {
        let key_hex = private_key.strip_prefix("0x").unwrap_or(private_key);
        let key_bytes = hex::decode(key_hex)
            .map_err(|e| BotError::Signing(format!("Invalid private key hex: {}", e)))?;

        if key_bytes.len() != 32 {
            return Err(BotError::Signing(format!(
                "Private key must be 32 bytes, got {}",
                key_bytes.len()
            )));
        }

        let signing_key = SigningKey::from_bytes(key_bytes.as_slice().into())
            .map_err(|e| BotError::Signing(format!("Invalid private key: {}", e)))?;

        let verifying_key = k256::ecdsa::VerifyingKey::from(&signing_key);
        let public_key_bytes = verifying_key.to_encoded_point(false);
        let address = Self::pubkey_to_address(public_key_bytes.as_bytes());

        info!("Wallet initialized: {}", address);

        Ok(Self {
            signing_key,
            address,
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    fn pubkey_to_address(pub_key: &[u8]) -> String {
        let key_without_prefix = &pub_key[1..];
        let mut hasher = Keccak256::new();
        hasher.update(key_without_prefix);
        let hash = hasher.finalize();
        format!("0x{}", hex::encode(&hash[12..]))
    }
}
