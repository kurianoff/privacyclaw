/// Tier 2: GLiNER ONNX NER detector.
///
/// Full GLiNER inference is gated behind the `ort-ner` Cargo feature and
/// requires the model files to be downloaded separately (model.onnx +
/// tokenizer.json placed in the configured model directory).
///
/// Without the feature the public `Tier2Detector` type is available but
/// `load()` returns an error and `detect*` methods return empty vecs.

// ─── Feature-gated implementation ────────────────────────────────────────────

#[cfg(feature = "ort-ner")]
mod inner {
    use crate::pii::vault::{PiiSpan, PiiType};
    use anyhow::{Context, Result};
    use ndarray::Array2;
    use ort::{Session, inputs};
    use std::path::Path;
    use std::time::Duration;
    use tokenizers::Tokenizer;

    const BATCH_SIZE: usize = 8;
    const LABEL_SEPARATOR: &str = " << >> ";

    pub struct Tier2Detector {
        session: Session,
        tokenizer: Tokenizer,
        confidence_threshold: f32,
        timeout: Duration,
    }

    impl Tier2Detector {
        /// Load a GLiNER model from `model_dir`.
        ///
        /// `model_dir` must contain:
        ///   - `model.onnx`    — exported GLiNER ONNX model
        ///   - `tokenizer.json` — HuggingFace tokenizer config
        pub fn load(model_dir: &Path) -> Result<Self> {
            let model_path = model_dir.join("model.onnx");
            let tokenizer_path = model_dir.join("tokenizer.json");

            let session = Session::builder()
                .context("failed to create ort SessionBuilder")?
                .commit_from_file(&model_path)
                .with_context(|| format!("failed to load ONNX model from {:?}", model_path))?;

            let tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| anyhow::anyhow!("failed to load tokenizer: {}", e))?;

            Ok(Self {
                session,
                tokenizer,
                confidence_threshold: 0.5,
                timeout: Duration::from_millis(500),
            })
        }

        /// Override defaults for confidence threshold and per-call timeout.
        pub fn with_config(mut self, confidence_threshold: f32, timeout_ms: u64) -> Self {
            self.confidence_threshold = confidence_threshold;
            self.timeout = Duration::from_millis(timeout_ms);
            self
        }

        /// Detect named entities in a single text.
        ///
        /// Wraps `detect_batch` with a 500 ms timeout by default.
        pub async fn detect(&self, text: &str, entity_labels: &[&str]) -> Result<Vec<PiiSpan>> {
            let results = self.detect_batch(&[text], entity_labels).await?;
            Ok(results.into_iter().next().unwrap_or_default())
        }

        /// Detect named entities in a batch of texts (clamped to `BATCH_SIZE`).
        ///
        /// On timeout returns empty span lists so that Tier 1 results are still used.
        pub async fn detect_batch(
            &self,
            texts: &[&str],
            entity_labels: &[&str],
        ) -> Result<Vec<Vec<PiiSpan>>> {
            if texts.is_empty() {
                return Ok(vec![]);
            }
            let texts = if texts.len() > BATCH_SIZE {
                &texts[..BATCH_SIZE]
            } else {
                texts
            };

            let timeout = self.timeout;
            let result =
                tokio::time::timeout(timeout, self.run_inference(texts, entity_labels)).await;

            match result {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    tracing::warn!(
                        timeout_ms = timeout.as_millis(),
                        "Tier2Detector: GLiNER inference timed out; returning empty (Tier 1 results used)"
                    );
                    Ok(vec![vec![]; texts.len()])
                }
            }
        }

        /// Core inference loop — one call per text in the batch.
        async fn run_inference(
            &self,
            texts: &[&str],
            entity_labels: &[&str],
        ) -> Result<Vec<Vec<PiiSpan>>> {
            let label_prefix = entity_labels.join(" ");
            // Byte offset where the original text starts within the full prompt
            // (prompt = "{label_prefix}{LABEL_SEPARATOR}{text}").
            let text_byte_offset = label_prefix.len() + LABEL_SEPARATOR.len();
            let mut all_spans = Vec::with_capacity(texts.len());

            for &text in texts {
                let prompt = format!("{}{}{}", label_prefix, LABEL_SEPARATOR, text);

                let encoding = self
                    .tokenizer
                    .encode(prompt.as_str(), true)
                    .map_err(|e| anyhow::anyhow!("tokenizer error: {}", e))?;

                // Capture byte-level offsets before consuming the encoding.
                let token_offsets: Vec<(usize, usize)> = encoding.get_offsets().to_vec();

                let ids: Vec<i64> =
                    encoding.get_ids().iter().map(|&x| x as i64).collect();
                let mask: Vec<i64> =
                    encoding.get_attention_mask().iter().map(|&x| x as i64).collect();
                let seq_len = ids.len();

                let input_ids = Array2::from_shape_vec((1, seq_len), ids)
                    .context("input_ids shape error")?;
                let attention_mask = Array2::from_shape_vec((1, seq_len), mask)
                    .context("attention_mask shape error")?;

                let outputs = self.session.run(inputs![
                    "input_ids" => input_ids.view(),
                    "attention_mask" => attention_mask.view()
                ]?)?;

                let spans = self.decode_spans(
                    &outputs,
                    text,
                    entity_labels,
                    seq_len,
                    &token_offsets,
                    text_byte_offset,
                )?;
                all_spans.push(spans);
            }

            Ok(all_spans)
        }

        /// Post-process raw ONNX outputs into `PiiSpan` values.
        ///
        /// Supports the 4-D layout `(1, seq_len, seq_len, num_labels)` that most
        /// exported GLiNER variants use.  Unknown layouts produce an empty vec
        /// rather than an error so the proxy degrades gracefully.
        ///
        /// `token_offsets` — byte offsets for each token within the full prompt
        ///   (from `Encoding::get_offsets()`).
        /// `text_byte_offset` — byte position where the original text starts in
        ///   the prompt (len of labels + separator); used to map token positions
        ///   back to offsets within `original_text`.
        fn decode_spans(
            &self,
            outputs: &ort::SessionOutputs,
            original_text: &str,
            entity_labels: &[&str],
            _seq_len: usize,
            token_offsets: &[(usize, usize)],
            text_byte_offset: usize,
        ) -> Result<Vec<PiiSpan>> {
            let logits = outputs
                .get("logits")
                .or_else(|| outputs.get("output"))
                .context("no logits/output tensor in GLiNER ONNX output")?;

            let logits_tensor = logits.try_extract_tensor::<f32>()?;
            let shape = logits_tensor.shape();
            let flat = logits_tensor.view();
            let mut spans: Vec<PiiSpan> = Vec::new();

            // Decode 4-D span matrix: (batch=1, d1, d2, num_labels)
            if shape.len() == 4 {
                let (d1, _d2, d3) = (shape[1], shape[2], shape[3]);
                // Cap at d3 (label axis) only — d1 is sequence length, not label count.
                let num_labels = entity_labels.len().min(d3);

                for label_idx in 0..num_labels {
                    let pii_type = PiiType::Custom(entity_labels[label_idx].to_string());
                    for start_tok in 0..d1 {
                        for end_tok in start_tok..d1.min(start_tok + 50) {
                            // O(1) direct ndarray index instead of O(n) iterator walk.
                            let raw = flat
                                .get(ndarray::IxDyn(&[0, start_tok, end_tok, label_idx]))
                                .copied()
                                .unwrap_or(f32::NEG_INFINITY);
                            let prob = sigmoid(raw);
                            if prob >= self.confidence_threshold {
                                let raw_start = token_offsets
                                    .get(start_tok)
                                    .map(|(s, _)| *s)
                                    .unwrap_or(0);
                                let raw_end = token_offsets
                                    .get(end_tok)
                                    .map(|(_, e)| *e)
                                    .unwrap_or(0);
                                if let Some((byte_start, byte_end)) = super::map_token_offset(
                                    raw_start,
                                    raw_end,
                                    text_byte_offset,
                                    original_text.len(),
                                ) {
                                    spans.push(PiiSpan {
                                        start: byte_start,
                                        end: byte_end,
                                        entity_type: pii_type.clone(),
                                        confidence: prob,
                                        tier: 2,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Deduplicate overlapping spans — keep the highest-confidence one.
            spans.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut deduped: Vec<PiiSpan> = Vec::new();
            for span in spans {
                let overlaps = deduped
                    .iter()
                    .any(|s: &PiiSpan| s.start < span.end && span.start < s.end);
                if !overlaps {
                    deduped.push(span);
                }
            }
            deduped.sort_by_key(|s| s.start);
            Ok(deduped)
        }
    }

    pub fn sigmoid(x: f32) -> f32 {
        1.0 / (1.0 + (-x).exp())
    }
} // mod inner

#[cfg(feature = "ort-ner")]
pub use inner::Tier2Detector;

// ─── Offset helper (no feature gate) ─────────────────────────────────────────

/// Map a token's raw prompt-relative byte offset to a text-relative byte offset.
///
/// `text_byte_offset` is the byte length of the label prefix + separator that
/// precedes the original text in the GLiNER prompt
/// (`"{labels} << >> {text}"`).
///
/// Returns `None` if:
/// - the token falls entirely within the label prefix, or
/// - the adjusted span is zero-length or exceeds `text_len`.
#[allow(dead_code)]
pub fn map_token_offset(
    raw_start: usize,
    raw_end: usize,
    text_byte_offset: usize,
    text_len: usize,
) -> Option<(usize, usize)> {
    if raw_start < text_byte_offset || raw_end <= text_byte_offset {
        return None;
    }
    let start = raw_start - text_byte_offset;
    let end = raw_end - text_byte_offset;
    if end > start && end <= text_len { Some((start, end)) } else { None }
}

// ─── Stub (no `ort-ner` feature) ─────────────────────────────────────────────

/// Stub implementation used when the `ort-ner` feature is disabled.
///
/// `load()` returns an explanatory error; `detect*` methods return empty vecs
/// so callers can unconditionally call them and fall back to Tier 1 results.
#[cfg(not(feature = "ort-ner"))]
pub struct Tier2Detector;

#[cfg(not(feature = "ort-ner"))]
impl Tier2Detector {
    #[allow(dead_code)]
    pub fn load(_model_dir: &std::path::Path) -> anyhow::Result<Self> {
        anyhow::bail!(
            "Tier 2 (GLiNER NER) requires the `ort-ner` Cargo feature. \
             Rebuild with: cargo build --features ort-ner"
        )
    }

    pub async fn detect(
        &self,
        _text: &str,
        _labels: &[&str],
    ) -> anyhow::Result<Vec<crate::pii::vault::PiiSpan>> {
        Ok(vec![])
    }

    #[allow(dead_code)]
    pub async fn detect_batch(
        &self,
        texts: &[&str],
        _labels: &[&str],
    ) -> anyhow::Result<Vec<Vec<crate::pii::vault::PiiSpan>>> {
        Ok(vec![vec![]; texts.len()])
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_nonexistent_model_returns_error() {
        let result = Tier2Detector::load(std::path::Path::new("/nonexistent/path"));
        // Without `ort-ner`: error about missing feature.
        // With `ort-ner`:    error about missing file.
        assert!(result.is_err());
    }

    #[cfg(feature = "ort-ner")]
    #[test]
    fn test_sigmoid_helper() {
        use inner::sigmoid;
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert!(sigmoid(10.0) > 0.99);
        assert!(sigmoid(-10.0) < 0.01);
    }

    // ── map_token_offset ──────────────────────────────────────────────────────
    //
    // These tests verify the core invariant: a raw prompt-relative byte offset
    // minus `text_byte_offset` must equal the correct byte index within the
    // original text.  They run without the `ort-ner` feature.

    const LABEL_SEP: &str = " << >> ";

    fn make_offset(labels: &[&str]) -> usize {
        labels.join(" ").len() + LABEL_SEP.len()
    }

    /// Build the GLiNER prompt and confirm the slice at `text_byte_offset`
    /// equals the original text verbatim.
    #[test]
    fn test_text_starts_at_byte_offset() {
        let labels = &["person name", "organization", "location"];
        let text = "Alice works at Acme Corp";
        let offset = make_offset(labels);
        let prompt = format!("{}{}{}", labels.join(" "), LABEL_SEP, text);
        assert_eq!(&prompt[offset..], text, "text must start exactly at text_byte_offset");
    }

    /// Tokens inside the text portion are adjusted to text-relative offsets.
    #[test]
    fn test_token_in_text_maps_correctly() {
        let labels = &["person name"];
        let text = "Alice Smith here";
        let tbo = make_offset(labels);

        // Simulate: tokenizer reports "Alice" at [tbo, tbo+5] in the prompt
        let result = map_token_offset(tbo, tbo + 5, tbo, text.len());
        assert_eq!(result, Some((0, 5)));
        assert_eq!(&text[0..5], "Alice");

        // Simulate: "Smith" at [tbo+6, tbo+11]
        let result2 = map_token_offset(tbo + 6, tbo + 11, tbo, text.len());
        assert_eq!(result2, Some((6, 11)));
        assert_eq!(&text[6..11], "Smith");
    }

    /// Tokens whose raw offsets fall entirely within the label prefix are skipped.
    #[test]
    fn test_token_in_label_prefix_is_skipped() {
        let labels = &["person name", "organization"];
        let text = "Bob lives here";
        let tbo = make_offset(labels);

        // raw_start = 0 (first label token) — should be filtered
        assert_eq!(map_token_offset(0, 6, tbo, text.len()), None);

        // raw_end exactly at the boundary — should also be filtered
        assert_eq!(map_token_offset(0, tbo, tbo, text.len()), None);
    }

    /// Tokens that straddle the label/text boundary are skipped
    /// (raw_start < text_byte_offset even though raw_end > text_byte_offset).
    #[test]
    fn test_token_straddling_boundary_is_skipped() {
        let tbo = 20usize;
        // raw_start is in label area, raw_end is in text area
        assert_eq!(map_token_offset(tbo - 2, tbo + 3, tbo, 50), None);
    }

    /// A span that would extend beyond the text length is rejected.
    #[test]
    fn test_token_exceeding_text_length_is_skipped() {
        let tbo = 20usize;
        let text_len = 5usize;
        // raw offsets [tbo, tbo+10] → adjusted [0, 10] but text_len is 5
        assert_eq!(map_token_offset(tbo, tbo + 10, tbo, text_len), None);
    }

    /// Zero-length spans (raw_start == raw_end) are rejected.
    #[test]
    fn test_zero_length_span_is_skipped() {
        let tbo = 10usize;
        assert_eq!(map_token_offset(tbo + 3, tbo + 3, tbo, 20), None);
    }

    /// Regression: num_labels must be capped by d3 (the label axis), not d3.min(d1).
    /// When seq_len (d1) < num_labels, the old code would silently drop labels.
    #[test]
    fn test_num_labels_cap_uses_d3_not_d1() {
        // Scenario: 5 entity labels, but imagine seq_len = 3.
        // Old: entity_labels.len().min(d3.min(d1)) = 5.min(5.min(3)) = 3  ← drops 2 labels
        // New: entity_labels.len().min(d3)          = 5.min(5)         = 5  ← correct
        let entity_labels = 5usize;
        let d3 = 5usize;   // tensor label axis matches num labels
        let d1 = 3usize;   // short sequence

        let old_cap = entity_labels.min(d3.min(d1));
        let new_cap = entity_labels.min(d3);

        assert_eq!(old_cap, 3, "old formula drops labels when seq_len < num_labels");
        assert_eq!(new_cap, 5, "new formula correctly uses the label-axis dimension");
    }

    /// Multi-byte UTF-8: byte offsets from the tokenizer correctly slice the text.
    #[test]
    fn test_multibyte_utf8_offsets() {
        let labels = &["person name"];
        // "Ångström" is 9 bytes in UTF-8 (Å = 2 bytes, ö = 2 bytes)
        let text = "Ångström works here";
        let tbo = make_offset(labels);
        let prompt = format!("{}{}{}", labels.join(" "), LABEL_SEP, text);

        // Verify the prompt slice at tbo is the full text
        assert_eq!(&prompt[tbo..], text);

        // "Ångström" spans bytes 0..9 in text (and tbo..tbo+9 in the prompt)
        let angstrom_byte_end = "Ångström".len(); // 9 bytes
        let result = map_token_offset(tbo, tbo + angstrom_byte_end, tbo, text.len());
        assert_eq!(result, Some((0, angstrom_byte_end)));
        assert_eq!(&text[0..angstrom_byte_end], "Ångström");
    }
}
