use privacyclaw::pii::{PiiPipeline, Locale};
use privacyclaw::pii::vault::PiiVault;
use privacyclaw::parser::Provider;

#[test]
fn pii_roundtrip_email() {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Please email john@acme.com tomorrow"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("integration-roundtrip");

    // Outbound: replace PII
    let modified = PiiPipeline::process_request_body(&body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs)
        .expect("should detect and replace PII");

    let modified_json: serde_json::Value = serde_json::from_slice(&modified).unwrap();
    let content = modified_json["messages"][0]["content"].as_str().unwrap();
    assert!(!content.contains("john@acme.com"), "PII not replaced: {content}");

    // Inbound: restore PII from synthetic
    let (restored, any) = vault.replace_synthetics(content);
    assert!(any, "no synthetic found in response");
    assert!(restored.contains("john@acme.com"), "PII not restored: {restored}");
}
