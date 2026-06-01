use serde_json::Value;
use sha3::{Digest, Keccak256};
use tracing::debug;

use super::wallet::Wallet;
use crate::error::{BotError, Result};

// Hyperliquid chain ID (Arbitrum)
const HYPERLIQUID_CHAIN_ID: u64 = 42161;

/// EIP-712 signer for Hyperliquid
pub struct Signer {
    domain_separator: [u8; 32],
}

impl Signer {
    pub fn new() -> Self {
        Self {
            domain_separator: Self::compute_domain_separator(),
        }
    }

    /// Sign an action for Hyperliquid exchange API
    pub fn sign_action(
        &self,
        wallet: &Wallet,
        action_type: &str,
        action_data: Value,
        nonce: u64,
    ) -> Result<Value> {
        // Merge type into action data
        let mut action = action_data;
        if let Value::Object(ref mut map) = action {
            map.insert("type".to_string(), Value::String(action_type.to_string()));
        }

        debug!("Signing action: {}", action_type);

        // Compute EIP-712 hash
        let action_hash = Self::hash_action(&action);
        let eip712_hash = Self::compute_eip712_hash(&self.domain_separator, &action_hash);

        // Sign with recovery
        let (signature, recovery_id) = wallet
            .signing_key()
            .sign_prehash_recoverable(&eip712_hash)
            .map_err(|e| BotError::Signing(format!("Signing failed: {}", e)))?;

        let r = format!("0x{:064x}", signature.r());
        let s = format!("0x{:064x}", signature.s());
        let v = recovery_id.to_byte() + 27;

        Ok(serde_json::json!({
            "action": action,
            "nonce": nonce,
            "signature": { "r": r, "s": s, "v": v }
        }))
    }

    fn compute_domain_separator() -> [u8; 32] {
        let mut hasher = Keccak256::new();
        // EIP712Domain type hash
        hasher.update(keccak256(
            b"EIP712Domain(string name,string version,uint256 chainId)",
        ));
        hasher.update(keccak256(b"HyperliquidSignTransaction"));
        hasher.update(keccak256(b"1"));
        // chainId padded to 32 bytes
        let mut chain_id = [0u8; 32];
        chain_id[24..32].copy_from_slice(&HYPERLIQUID_CHAIN_ID.to_be_bytes());
        hasher.update(chain_id);
        to_array(hasher.finalize())
    }

    fn hash_action(action: &Value) -> [u8; 32] {
        let action_str = serde_json::to_string(action).unwrap_or_default();
        keccak256(action_str.as_bytes())
    }

    fn compute_eip712_hash(domain: &[u8; 32], struct_hash: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Keccak256::new();
        hasher.update(b"\x19\x01");
        hasher.update(domain);
        hasher.update(struct_hash);
        to_array(hasher.finalize())
    }
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    to_array(hasher.finalize())
}

fn to_array(digest: sha3::digest::Output<Keccak256>) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}
