/// Tier 2: GLiNER ONNX NER detector.
///
/// This module is a stub. Full GLiNER inference is gated behind the `ort-ner`
/// Cargo feature and requires the model to be downloaded separately.
use crate::pii::vault::PiiSpan;

pub struct Tier2Detector;

impl Tier2Detector {
    /// Detect named entities in text. Returns empty vec (stub).
    #[allow(dead_code)]
    pub fn detect(_text: &str) -> Vec<PiiSpan> {
        tracing::debug!("Tier2Detector: GLiNER inference not compiled (enable ort-ner feature)");
        vec![]
    }
}
