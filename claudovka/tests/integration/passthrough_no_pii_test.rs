use claudovka::pii::PiiPipeline;
use claudovka::pii::vault::PiiVault;
use claudovka::pii::Locale;
use claudovka::parser::Provider;

#[test]
fn no_pii_returns_none() {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "What is the capital of France?"}
        ]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("conv-no-pii");

    let result = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs);
    assert!(result.is_none(), "no PII should return None, got Some");
    assert_eq!(vault.mapping_count(), 0, "vault should remain empty");
}
