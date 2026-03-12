use claudovka::pii::PiiPipeline;
use claudovka::pii::vault::PiiVault;
use claudovka::pii::Locale;
use claudovka::parser::Provider;

#[test]
fn same_pii_same_synthetic_across_turns() {
    let mut vault = PiiVault::new("conv-multiturn");

    let body1 = serde_json::json!({
        "messages": [{"role": "user", "content": "My name is Eve, email: eve@corp.com"}]
    });
    let b1 = serde_json::to_vec(&body1).unwrap();
    let r1 = PiiPipeline::process_request_body(&b1, &mut vault, Provider::OpenAI, &Locale::EnUs).unwrap();
    let j1: serde_json::Value = serde_json::from_slice(&r1).unwrap();
    let _synthetic1 = j1["messages"][0]["content"].as_str().unwrap().to_string();

    let body2 = serde_json::json!({
        "messages": [
            {"role": "user", "content": "My name is Eve, email: eve@corp.com"},
            {"role": "assistant", "content": "Got it."},
            {"role": "user", "content": "Remind me, I'm eve@corp.com"}
        ]
    });
    let b2 = serde_json::to_vec(&body2).unwrap();
    let r2 = PiiPipeline::process_request_body(&b2, &mut vault, Provider::OpenAI, &Locale::EnUs).unwrap();
    let j2: serde_json::Value = serde_json::from_slice(&r2).unwrap();

    // The same email in turn 1 and turn 3 must produce the same synthetic
    let turn1_content = j2["messages"][0]["content"].as_str().unwrap();
    let turn3_content = j2["messages"][2]["content"].as_str().unwrap();
    assert!(!turn1_content.contains("eve@corp.com"), "turn1 still has original PII: {turn1_content}");
    assert!(!turn3_content.contains("eve@corp.com"), "turn3 still has original PII: {turn3_content}");

    // Both turns must contain the same synthetic email
    let synth_email = vault.get_synthetic("eve@corp.com").expect("email should be in vault");
    assert!(turn1_content.contains(synth_email), "turn1 should contain synthetic: {turn1_content}");
    assert!(turn3_content.contains(synth_email), "turn3 should contain synthetic: {turn3_content}");
}
