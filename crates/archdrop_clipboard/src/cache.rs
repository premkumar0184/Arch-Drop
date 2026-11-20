use crate::error::ClipboardError;
use crate::threshold::CachedPayload;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info};

const DEFAULT_TTL_SECS: u64 = 120;

#[derive(Clone, Debug)]
pub struct ClipboardPayloadCache {
    cache: Arc<RwLock<HashMap<String, CachedPayload>>>,
    ttl: Duration,
}

impl Default for ClipboardPayloadCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_TTL_SECS))
    }
}

impl ClipboardPayloadCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Stores a payload in the volatile cache.
    pub async fn insert(&self, payload: CachedPayload) {
        let mut map = self.cache.write().await;
        info!(
            payload_id = %payload.payload_id,
            mime = %payload.mime_type,
            size = payload.data.len(),
            "Caching large clipboard payload (> 4 KB) in volatile memory"
        );
        map.insert(payload.payload_id.clone(), payload);
    }

    /// Retrieves payload bytes, validating the ephemeral token and checking TTL.
    pub async fn get(&self, payload_id: &str, token: &str) -> Result<Arc<[u8]>, ClipboardError> {
        let mut map = self.cache.write().await;
        let payload = map
            .get(payload_id)
            .ok_or_else(|| ClipboardError::PayloadNotFound(payload_id.to_string()))?;

        if payload.created_at.elapsed() > self.ttl {
            map.remove(payload_id);
            return Err(ClipboardError::Expired);
        }

        if payload.token != token {
            return Err(ClipboardError::InvalidToken);
        }

        Ok(payload.data.clone())
    }

    /// Clears all cached payloads immediately (used on session lock / zeroization).
    pub async fn clear(&self) {
        let mut map = self.cache.write().await;
        let count = map.len();
        map.clear();
        info!(cleared_entries = count, "Zeroized volatile clipboard payload cache");
    }

    /// Prunes expired items (> 120 seconds).
    pub async fn prune_expired(&self) {
        let mut map = self.cache.write().await;
        let ttl = self.ttl;
        map.retain(|id, item| {
            let valid = item.created_at.elapsed() <= ttl;
            if !valid {
                debug!(payload_id = %id, "Pruned expired clipboard payload");
            }
            valid
        });
    }

    /// Returns the number of currently cached items.
    pub async fn len(&self) -> usize {
        self.cache.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}
