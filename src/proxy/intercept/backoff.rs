use std::sync::{Arc, Mutex};
use std::time::Duration;
use crate::pii::vault::VaultHandle;

use super::{BACKOFF_ATTEMPTS, BACKOFF_SLEEP_MS};

/// Poll `shared` up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms, returning
/// the contained value as soon as it is `Some`. Returns `None` on timeout.
///
/// Generic core shared by `get_vault_with_backoff` and `wait_for_conv_id`.
async fn poll_shared<T: Clone>(shared: &Arc<Mutex<Option<T>>>) -> Option<T> {
    for _ in 0..BACKOFF_ATTEMPTS {
        {
            let g = shared.lock().unwrap();
            if g.is_some() {
                return g.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(BACKOFF_SLEEP_MS)).await;
    }
    shared.lock().unwrap().clone()
}

/// Wait up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms for the shared vault to be set by c_to_u.
pub(super) async fn get_vault_with_backoff(shared_vault: &Arc<Mutex<Option<VaultHandle>>>) -> Option<VaultHandle> {
    let result = poll_shared(shared_vault).await;
    if result.is_some() {
        tracing::debug!("u2c_pii: vault acquired from shared_vault");
    } else {
        tracing::warn!("u2c_pii: vault backoff timeout — shared_vault still None after 50ms; rep_buf will be None");
    }
    result
}

/// Wait up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms for the shared conv_id to be set by c_to_u (log_request).
pub(super) async fn wait_for_conv_id(shared_conv_id: &Arc<Mutex<Option<String>>>) -> Option<String> {
    poll_shared(shared_conv_id).await
}
