/// Tier 3: SLM sidecar for context-aware PII disambiguation.
///
/// Stub — full implementation requires a running llama-server process.
use crate::pii::vault::PiiSpan;

pub struct SlmSidecar;

impl SlmSidecar {
    /// Disambiguate ambiguous spans via LLM. Returns empty vec (stub).
    #[allow(dead_code)]
    pub fn disambiguate(_text: &str, _candidates: &[PiiSpan]) -> Vec<PiiSpan> {
        tracing::debug!("SlmSidecar: SLM sidecar not configured");
        vec![]
    }
}
