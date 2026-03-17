use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::pii::vault::VaultHandle;

use super::{BACKOFF_ATTEMPTS, BACKOFF_SLEEP_MS};

/// Wait up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms for the shared vault to be set by c_to_u.
pub(super) async fn get_vault_with_backoff(shared_vault: &Arc<Mutex<Option<VaultHandle>>>) -> Option<VaultHandle> {
    for _ in 0..BACKOFF_ATTEMPTS {
        {
            let g = shared_vault.lock().unwrap();
            if g.is_some() {
                tracing::debug!("u2c_pii: vault acquired from shared_vault");
                return g.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(BACKOFF_SLEEP_MS)).await;
    }
    let result = shared_vault.lock().unwrap().clone();
    if result.is_none() {
        tracing::warn!("u2c_pii: vault backoff timeout — shared_vault still None after 50ms; rep_buf will be None");
    }
    result
}

/// Wait up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms for the shared conv_id to be set by c_to_u (log_request).
pub(super) async fn wait_for_conv_id(shared_conv_id: &Arc<Mutex<Option<String>>>) -> Option<String> {
    for _ in 0..BACKOFF_ATTEMPTS {
        {
            let g = shared_conv_id.lock().unwrap();
            if g.is_some() { return g.clone(); }
        }
        tokio::time::sleep(Duration::from_millis(BACKOFF_SLEEP_MS)).await;
    }
    shared_conv_id.lock().unwrap().clone()
}
