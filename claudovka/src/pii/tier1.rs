use crate::pii::locale::Locale;
use crate::pii::vault::{PiiSpan, PiiType};
use fancy_regex::Regex;
use std::sync::OnceLock;

/// A compiled set of regex patterns for one entity type.
struct PatternSet {
    entity_type: PiiType,
    re: Regex,
    /// Optional post-match validator (e.g. Luhn for credit cards).
    validate: Option<fn(&str) -> bool>,
}

impl PatternSet {
    fn new(entity_type: PiiType, pattern: &str) -> Self {
        Self {
            entity_type,
            re: Regex::new(pattern).expect("invalid regex"),
            validate: None,
        }
    }

    fn with_validator(mut self, f: fn(&str) -> bool) -> Self {
        self.validate = Some(f);
        self
    }

    fn find_all(&self, text: &str) -> Vec<PiiSpan> {
        self.re.find_iter(text)
            .filter_map(|r| r.ok())
            .filter(|m| {
                self.validate.map(|f| f(m.as_str())).unwrap_or(true)
            })
            .map(|m| PiiSpan {
                start: m.start(),
                end: m.end(),
                entity_type: self.entity_type.clone(),
                confidence: 1.0,
                tier: 1,
            })
            .collect()
    }
}

/// Global (thread-safe) singleton pattern sets.
static UNIVERSAL_PATTERNS: OnceLock<Vec<PatternSet>> = OnceLock::new();

fn universal_patterns() -> &'static Vec<PatternSet> {
    UNIVERSAL_PATTERNS.get_or_init(|| {
        vec![
            // Email (RFC 5321 simplified)
            PatternSet::new(
                PiiType::Email,
                r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}"
            ),
            // US Phone — requires at least one separator so bare digit runs (SSNs, card numbers) don't match.
            PatternSet::new(
                PiiType::Phone,
                r"(?:\+1[\s\-.]?)?\(?\d{3}\)?[\s\-.](?:\d{3})[\s\-.](\d{4})"
            ),
            // US SSN — avoid matching things that look like dates or serial numbers
            PatternSet::new(
                PiiType::Ssn,
                r"(?<!\d)(?!000|666|9\d\d)\d{3}[-\s](?!00)\d{2}[-\s](?!0000)\d{4}(?!\d)"
            ),
            // Credit card — 13–19 digits optionally space/dash separated
            PatternSet::new(
                PiiType::CreditCard,
                r"(?<!\d)(?:4\d{3}|5[1-5]\d{2}|3[47]\d{2}|6011|65\d{2})[\s\-]?\d{4}[\s\-]?\d{4}[\s\-]?\d{4}(?:\d{3})?(?!\d)"
            ).with_validator(luhn_valid),
            // IPv4
            PatternSet::new(
                PiiType::IpV4,
                r"(?<!\d)(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)(?!\d)"
            ),
            // IPv6 (simplified — catches common forms)
            PatternSet::new(
                PiiType::IpV6,
                r"(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:)*::[0-9a-fA-F:]*"
            ),
            // OpenAI API key
            PatternSet::new(
                PiiType::OpenAiApiKey,
                r"sk-[A-Za-z0-9]{20,100}"
            ),
            // AWS Access Key ID
            PatternSet::new(
                PiiType::AwsAccessKey,
                r"AKIA[0-9A-Z]{16}"
            ),
            // AWS Secret Access Key (appears near "secret" keyword in env blocks)
            PatternSet::new(
                PiiType::AwsSecretKey,
                r#"(?i)aws.{0,20}secret.{0,10}[=:\s]['"]?([A-Za-z0-9/+]{40})"#
            ),
            // GitHub PAT (new format ghp_ and github_pat_)
            PatternSet::new(
                PiiType::GitHubPat,
                r"ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82}"
            ),
            // Bearer token
            PatternSet::new(
                PiiType::BearerToken,
                r"Bearer [A-Za-z0-9._~+/=\-]{20,}"
            ),
            // SSH private key block
            PatternSet::new(
                PiiType::SshPrivateKey,
                r"-----BEGIN (?:RSA|EC|OPENSSH|DSA|PGP) PRIVATE KEY-----"
            ),
            // Database connection string
            PatternSet::new(
                PiiType::DbConnectionString,
                r"(?i)(?:postgres|postgresql|mysql|mariadb|mongodb|redis|amqp)://[^:\s]+:[^@\s]+@[^\s]+"
            ),
            // URL with embedded credentials
            PatternSet::new(
                PiiType::UrlWithCreds,
                r"https?://[^:\s]+:[^@\s]+@[^\s]+"
            ),
        ]
    })
}

// ─── Tier1Detector ────────────────────────────────────────────────────────────

/// Fast regex-based PII detector (Tier 1).
pub struct Tier1Detector;

impl Tier1Detector {
    /// Detect all PII spans in a text string.
    ///
    /// Returns spans sorted by start offset, with overlapping spans from
    /// later patterns removed (first-found wins per character position).
    pub fn detect(text: &str, _locale: &Locale) -> Vec<PiiSpan> {
        let mut spans: Vec<PiiSpan> = Vec::new();

        for pat in universal_patterns() {
            for span in pat.find_all(text) {
                // Skip if overlapping with an already-found span.
                if !spans_overlap(&spans, &span) {
                    spans.push(span);
                }
            }
        }

        // Locale-specific patterns
        // (future: load from locale pack files)

        spans.sort_by_key(|s| s.start);
        spans
    }

    /// Apply detections to a text string, calling `on_span` for each found entity.
    /// Returns the text with all matched spans replaced by their synthetic equivalents.
    pub fn replace_in_text(
        text: &str,
        locale: &Locale,
        mut get_synthetic: impl FnMut(&str, &PiiType) -> String,
    ) -> (String, Vec<PiiSpan>) {
        let spans = Self::detect(text, locale);
        if spans.is_empty() {
            return (text.to_string(), vec![]);
        }

        let mut result = String::with_capacity(text.len());
        let mut last = 0;
        for span in &spans {
            result.push_str(&text[last..span.start]);
            let original = &text[span.start..span.end];
            let synthetic = get_synthetic(original, &span.entity_type);
            result.push_str(&synthetic);
            last = span.end;
        }
        result.push_str(&text[last..]);
        (result, spans)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn spans_overlap(existing: &[PiiSpan], candidate: &PiiSpan) -> bool {
    existing.iter().any(|s| s.start < candidate.end && candidate.start < s.end)
}

/// Luhn algorithm check for credit card numbers.
/// Strips all spaces and dashes before checking.
fn luhn_valid(s: &str) -> bool {
    let digits: Vec<u32> = s.chars()
        .filter(|c| c.is_ascii_digit())
        .filter_map(|c| c.to_digit(10))
        .collect();
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut val = d;
        if double {
            val *= 2;
            if val > 9 { val -= 9; }
        }
        sum += val;
        double = !double;
    }
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_detected() {
        let spans = Tier1Detector::detect("Contact me at john@acme.com for details.", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::Email), "no email found: {:?}", spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>());
    }

    #[test]
    fn test_ssn_detected() {
        let spans = Tier1Detector::detect("SSN: 123-45-6789", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::Ssn));
    }

    #[test]
    fn test_ssn_invalid_prefix_not_detected() {
        let spans = Tier1Detector::detect("SSN: 000-45-6789", &Locale::EnUs);
        assert!(!spans.iter().any(|s| s.entity_type == PiiType::Ssn));
    }

    #[test]
    fn test_credit_card_luhn_valid() {
        // 4532015112830366 is a valid Visa test number
        let spans = Tier1Detector::detect("Card: 4532015112830366", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::CreditCard),
            "valid Luhn card not detected");
    }

    #[test]
    fn test_credit_card_luhn_invalid() {
        // 4532015112830367 — last digit changed, fails Luhn
        let spans = Tier1Detector::detect("Card: 4532015112830367", &Locale::EnUs);
        assert!(!spans.iter().any(|s| s.entity_type == PiiType::CreditCard),
            "invalid Luhn card should not be detected");
    }

    #[test]
    fn test_openai_key_detected() {
        let spans = Tier1Detector::detect("key: sk-abcdefghijklmnopqrstuvwxyz12345678901234", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::OpenAiApiKey));
    }

    #[test]
    fn test_aws_access_key_detected() {
        let spans = Tier1Detector::detect("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::AwsAccessKey));
    }

    #[test]
    fn test_github_pat_detected() {
        let spans = Tier1Detector::detect("token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::GitHubPat));
    }

    #[test]
    fn test_bearer_token_detected() {
        let spans = Tier1Detector::detect("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::BearerToken));
    }

    #[test]
    fn test_db_connection_detected() {
        let spans = Tier1Detector::detect("postgres://user:password@localhost:5432/mydb", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::DbConnectionString));
    }

    #[test]
    fn test_no_false_positive_on_git_sha() {
        // A short hex string should not be detected as an API key
        let spans = Tier1Detector::detect("commit a3f5c2d7", &Locale::EnUs);
        assert!(!spans.iter().any(|s| s.entity_type == PiiType::OpenAiApiKey));
    }

    #[test]
    fn test_no_pii_text() {
        let spans = Tier1Detector::detect("Hello, how are you today?", &Locale::EnUs);
        assert!(spans.is_empty(), "unexpected PII: {:?}", spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>());
    }

    #[test]
    fn test_ipv4_detected() {
        let spans = Tier1Detector::detect("Server at 192.168.1.100", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::IpV4));
    }

    #[test]
    fn test_replace_in_text() {
        let text = "Email john@acme.com for info.";
        let (replaced, spans) = Tier1Detector::replace_in_text(text, &Locale::EnUs, |_orig, _typ| {
            "alice@example.com".to_string()
        });
        assert!(!spans.is_empty());
        assert!(replaced.contains("alice@example.com"), "got: {}", replaced);
        assert!(!replaced.contains("john@acme.com"), "original still present: {}", replaced);
    }

    #[test]
    fn test_luhn_valid() {
        assert!(luhn_valid("4532015112830366")); // valid Visa
        assert!(!luhn_valid("4532015112830367")); // invalid
    }
}
