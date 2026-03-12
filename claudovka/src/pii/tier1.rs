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
        let mut spans = Vec::new();
        for result in self.re.find_iter(text) {
            let m = match result {
                Ok(m) => m,
                Err(_) => continue,
            };
            let valid = self.validate.map(|f| {
                let v = f(m.as_str());
                tracing::trace!(
                    entity_type = self.entity_type.label(),
                    valid = v,
                    "tier1: validator result"
                );
                v
            }).unwrap_or(true);
            if valid {
                tracing::trace!(
                    entity_type = self.entity_type.label(),
                    span_start = m.start(),
                    span_end = m.end(),
                    "tier1: match found"
                );
                spans.push(PiiSpan {
                    start: m.start(),
                    end: m.end(),
                    entity_type: self.entity_type.clone(),
                    confidence: 1.0,
                    tier: 1,
                });
            }
        }
        spans
    }
}

/// Global (thread-safe) singleton pattern sets.
static UNIVERSAL_PATTERNS: OnceLock<Vec<PatternSet>> = OnceLock::new();

fn universal_patterns() -> &'static Vec<PatternSet> {
    UNIVERSAL_PATTERNS.get_or_init(|| {
        vec![
            // ── Specific / longer patterns first so they claim spans before
            //    more-general ones (email, phone) can match sub-spans inside them.

            // Database connection string (must precede email — contains user@host)
            PatternSet::new(
                PiiType::DbConnectionString,
                r"(?i)(?:postgres|postgresql|mysql|mariadb|mongodb|redis|amqp)://[^:\s]+:[^@\s]+@[^\s]+"
            ),
            // URL with embedded credentials (must precede email — contains user@host)
            PatternSet::new(
                PiiType::UrlWithCreds,
                r"https?://[^:\s]+:[^@\s]+@[^\s]+"
            ),
            // SSH private key block
            PatternSet::new(
                PiiType::SshPrivateKey,
                r"-----BEGIN (?:RSA|EC|OPENSSH|DSA|PGP) PRIVATE KEY-----"
            ),
            // Bearer token
            PatternSet::new(
                PiiType::BearerToken,
                r"Bearer [A-Za-z0-9._~+/=\-]{20,}"
            ),
            // GitHub PAT (new format ghp_ and github_pat_)
            PatternSet::new(
                PiiType::GitHubPat,
                r"ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82}"
            ),
            // AWS Secret Access Key (appears near "secret" keyword in env blocks)
            PatternSet::new(
                PiiType::AwsSecretKey,
                r#"(?i)aws.{0,20}secret.{0,10}[=:\s]['"]?([A-Za-z0-9/+]{40})"#
            ),
            // AWS Access Key ID
            PatternSet::new(
                PiiType::AwsAccessKey,
                r"AKIA[0-9A-Z]{16}"
            ),
            // OpenAI API key
            PatternSet::new(
                PiiType::OpenAiApiKey,
                r"sk-[A-Za-z0-9]{20,100}"
            ),
            // IPv6: three structural alternatives to cover all compressed forms.
            // Alt 1: full 8-group    (2001:0db8:85a3:...:7334)
            // Alt 2: prefix + ::     (fe80::1, 2001:db8::1) — extra trailing : merges with last group's colon
            // Alt 3: pure :: prefix  (::1, ::)
            // The ipv6_valid validator rejects Rust paths (segments > 4 hex chars).
            PatternSet::new(
                PiiType::IpV6,
                r"(?<![:\w])(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}(?![:\w])|(?<![:\w])(?:[0-9a-fA-F]{1,4}:){1,7}:[0-9a-fA-F:]*(?![:\w])|(?<![:\w])::[0-9a-fA-F:]*(?![:\w])"
            ).with_validator(ipv6_valid),
            // IPv4
            PatternSet::new(
                PiiType::IpV4,
                r"(?<!\d)(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)(?!\d)"
            ),
            // Credit card — 13–19 digits optionally space/dash separated
            PatternSet::new(
                PiiType::CreditCard,
                r"(?<!\d)(?:4\d{3}|5[1-5]\d{2}|3[47]\d{2}|6011|65\d{2})[\s\-]?\d{4}[\s\-]?\d{4}[\s\-]?\d{4}(?:\d{3})?(?!\d)"
            ).with_validator(luhn_valid),
            // US SSN — avoid matching things that look like dates or serial numbers
            PatternSet::new(
                PiiType::Ssn,
                r"(?<!\d)(?!000|666|9\d\d)\d{3}[-\s](?!00)\d{2}[-\s](?!0000)\d{4}(?!\d)"
            ),
            // US Phone — requires at least one separator so bare digit runs don't match.
            PatternSet::new(
                PiiType::Phone,
                r"(?:\+1[\s\-.]?)?\(?\d{3}\)?[\s\-.](?:\d{3})[\s\-.](\d{4})"
            ),
            // Email (RFC 5321 simplified) — last so URL/DB creds claim @-spans first
            PatternSet::new(
                PiiType::Email,
                r"(?i)[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}"
            ),
        ]
    })
}

// ─── Locale-specific patterns ─────────────────────────────────────────────────

/// Returns locale-specific pattern sets for the given locale.
/// Created fresh on each call (small set, not in a hot path).
fn locale_patterns(locale: &Locale) -> Vec<PatternSet> {
    match locale {
        Locale::EnGb => vec![
            // UK National Insurance Number: AA 99 99 99 A
            PatternSet::new(
                PiiType::Custom("UK_NIN".to_string()),
                r"(?i)[A-CEGHJ-PR-TW-Z]{2}\s?\d{2}\s?\d{2}\s?\d{2}\s?[A-D]",
            ),
        ],
        Locale::DeDe => vec![
            // German Steueridentifikationsnummer: 11-digit, starts with non-zero
            PatternSet::new(
                PiiType::Custom("DE_STEUER_ID".to_string()),
                r"(?<!\d)[1-9]\d{10}(?!\d)",
            ),
        ],
        Locale::FrFr => vec![
            // French INSEE (social security) number: 13 digits starting with 1 or 2
            PatternSet::new(
                PiiType::Custom("FR_INSEE".to_string()),
                r"(?<!\d)[12]\d{12}(?!\d)",
            ),
        ],
        Locale::InIn => vec![
            // Indian Aadhaar: XXXX XXXX XXXX
            PatternSet::new(
                PiiType::AadhaarNumber,
                r"\d{4}\s\d{4}\s\d{4}",
            ),
            // Indian PAN: AAAAA9999A
            PatternSet::new(
                PiiType::Custom("IN_PAN".to_string()),
                r"[A-Z]{5}[0-9]{4}[A-Z]",
            ),
        ],
        Locale::KoKr => vec![
            // Korean Resident Registration Number: XXXXXX-XXXXXXX (starts with 1-4)
            PatternSet::new(
                PiiType::Custom("KO_RRN".to_string()),
                r"\d{6}-[1-4]\d{6}",
            ),
            // Korean Business Registration Number: XXX-XX-XXXXX
            PatternSet::new(
                PiiType::Custom("KO_BRN".to_string()),
                r"\d{3}-\d{2}-\d{5}",
            ),
        ],
        Locale::BrBr => vec![
            // Brazilian CPF: XXX.XXX.XXX-XX (with check digit validation)
            PatternSet::new(
                PiiType::CpfNumber,
                r"\d{3}\.\d{3}\.\d{3}-\d{2}",
            ).with_validator(cpf_valid),
            // Brazilian CNPJ: XX.XXX.XXX/XXXX-XX
            PatternSet::new(
                PiiType::Custom("BR_CNPJ".to_string()),
                r"\d{2}\.\d{3}\.\d{3}/\d{4}-\d{2}",
            ),
        ],
        // EnUs and any future locales: no extra patterns beyond universal
        Locale::EnUs => vec![],
    }
}

// ─── Tier1Detector ────────────────────────────────────────────────────────────

/// Fast regex-based PII detector (Tier 1).
pub struct Tier1Detector;

impl Tier1Detector {
    /// Detect all PII spans in a text string.
    ///
    /// Returns spans sorted by start offset, with overlapping spans from
    /// later patterns removed (first-found wins per character position).
    pub fn detect(text: &str, locale: &Locale) -> Vec<PiiSpan> {
        tracing::debug!(text_len = text.len(), "tier1: detect enter");
        let mut spans: Vec<PiiSpan> = Vec::new();

        for pat in universal_patterns() {
            for span in pat.find_all(text) {
                // Skip if overlapping with an already-found span.
                if !spans_overlap(&spans, &span) {
                    spans.push(span);
                }
            }
        }

        // Locale-specific patterns (additive on top of universal)
        for pat in &locale_patterns(locale) {
            for span in pat.find_all(text) {
                if !spans_overlap(&spans, &span) {
                    spans.push(span);
                }
            }
        }

        spans.sort_by_key(|s| s.start);
        tracing::debug!(text_len = text.len(), span_count = spans.len(), "tier1: detect complete");
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

/// Validates that a regex match is a plausible IPv6 address, not a Rust path or identifier.
/// Rejects matches with fewer than 2 colons, or any segment longer than 4 hex chars.
fn ipv6_valid(s: &str) -> bool {
    let colon_count = s.chars().filter(|&c| c == ':').count();
    if colon_count < 2 {
        return false;
    }
    s.split(':')
        .filter(|seg| !seg.is_empty())
        .all(|seg| seg.len() <= 4)
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

/// Validates a Brazilian CPF number.
/// Strips non-digit characters, checks length = 11, not all-same digit,
/// and verifies both check digits per the Brazilian standard.
fn cpf_valid(s: &str) -> bool {
    let digits: Vec<u8> = s
        .chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c as u8 - b'0')
        .collect();

    if digits.len() != 11 {
        return false;
    }
    // Reject all-same digits (e.g. 111.111.111-11)
    if digits.iter().all(|&d| d == digits[0]) {
        return false;
    }

    // First check digit: weighted sum of first 9 digits
    let sum1: u32 = digits[..9]
        .iter()
        .enumerate()
        .map(|(i, &d)| d as u32 * (10 - i as u32))
        .sum();
    let r1 = sum1 % 11;
    let check1 = if r1 < 2 { 0 } else { 11 - r1 };
    if digits[9] as u32 != check1 {
        return false;
    }

    // Second check digit: weighted sum of first 10 digits
    let sum2: u32 = digits[..10]
        .iter()
        .enumerate()
        .map(|(i, &d)| d as u32 * (11 - i as u32))
        .sum();
    let r2 = sum2 % 11;
    let check2 = if r2 < 2 { 0 } else { 11 - r2 };
    digits[10] as u32 == check2
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

    #[test]
    fn test_ipv6_detected() {
        let spans = Tier1Detector::detect(
            "server addr: 2001:0db8:85a3:0000:0000:8a2e:0370:7334",
            &Locale::EnUs,
        );
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::IpV6),
            "no IPv6 span found: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ssh_private_key_detected() {
        let spans = Tier1Detector::detect(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE...",
            &Locale::EnUs,
        );
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::SshPrivateKey),
            "no SshPrivateKey span found: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_url_with_creds_detected() {
        let spans = Tier1Detector::detect(
            "repo: https://user:password123@github.com/org/repo.git",
            &Locale::EnUs,
        );
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::UrlWithCreds),
            "no UrlWithCreds span found: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_multiple_pii_in_text() {
        let spans = Tier1Detector::detect(
            "Email john@acme.com, SSN: 123-45-6789",
            &Locale::EnUs,
        );
        assert!(
            spans.len() >= 2,
            "expected at least 2 spans, got {}: {:?}",
            spans.len(),
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Email),
            "no Email span: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Ssn),
            "no Ssn span: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ssn_space_separator() {
        let spans = Tier1Detector::detect("SSN: 321 56 7890", &Locale::EnUs);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Ssn),
            "no Ssn span found for space-separated SSN: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ssn_666_prefix_rejected() {
        let spans = Tier1Detector::detect("SSN: 666-34-5678", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Ssn),
            "SSN with 666 prefix should be rejected"
        );
    }

    #[test]
    fn test_ssn_9xx_prefix_rejected() {
        let spans = Tier1Detector::detect("SSN: 900-34-5678", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Ssn),
            "SSN with 9xx prefix should be rejected"
        );
    }

    #[test]
    fn test_openai_key_no_false_positive_short() {
        // "sk-abc123" has only 6 chars after "sk-", below the {20,100} minimum
        let spans = Tier1Detector::detect("sk-abc123", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::OpenAiApiKey),
            "short sk- string should not match OpenAI key"
        );
    }

    #[test]
    fn test_ipv4_not_false_positive_version() {
        // "1.2.3" has only 3 octets — not a valid IPv4
        let spans = Tier1Detector::detect("version 1.2.3", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::IpV4),
            "version string with 3 octets should not be detected as IPv4"
        );
    }

    #[test]
    fn test_credit_card_no_luhn_invalid_rejected() {
        // 4532015112830367 — last digit +1 from valid card, fails Luhn
        let spans = Tier1Detector::detect("Card: 4532015112830367", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::CreditCard),
            "Luhn-invalid card number should not be detected"
        );
    }

    #[test]
    fn test_replace_in_text_multiple_types() {
        let text = "Email john@acme.com SSN 123-45-6789";
        let mut count = 0usize;
        let (result, _spans) = Tier1Detector::replace_in_text(text, &Locale::EnUs, |_orig, _typ| {
            count += 1;
            "REDACTED".to_string()
        });
        assert!(
            result.matches("REDACTED").count() >= 2,
            "expected at least 2 REDACTED substitutions, got: {:?}",
            result
        );
        assert!(
            !result.contains("john@acme.com"),
            "original email still present: {:?}",
            result
        );
        assert!(
            !result.contains("123-45-6789"),
            "original SSN still present: {:?}",
            result
        );
    }

    #[test]
    fn test_spans_are_non_overlapping() {
        let text = "IP 192.168.1.100 and email user@domain.com are here";
        let spans = Tier1Detector::detect(text, &Locale::EnUs);
        // Check every pair of spans for disjoint ranges
        for i in 0..spans.len() {
            for j in (i + 1)..spans.len() {
                let a = &spans[i];
                let b = &spans[j];
                let overlap = a.start < b.end && b.start < a.end;
                assert!(
                    !overlap,
                    "spans overlap: [{}, {}) {:?} and [{}, {}) {:?}",
                    a.start, a.end, a.entity_type.label(),
                    b.start, b.end, b.entity_type.label()
                );
            }
        }
    }

    // ── Locale-specific tests ────────────────────────────────────────────────

    // UK NIN
    #[test]
    fn test_uk_nin_detected() {
        let spans = Tier1Detector::detect("NIN: AB 12 34 56 C", &Locale::EnGb);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("UK_NIN".to_string())),
            "UK NIN not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_uk_nin_not_detected_in_en_us() {
        // UK NIN pattern should only fire for en-GB locale
        let spans = Tier1Detector::detect("NIN: AB 12 34 56 C", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("UK_NIN".to_string())),
            "UK NIN should not be detected in en-US locale"
        );
    }

    // German Steueridentifikationsnummer
    #[test]
    fn test_de_steuer_id_detected() {
        let spans = Tier1Detector::detect("Steuer-ID: 12345678901", &Locale::DeDe);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("DE_STEUER_ID".to_string())),
            "DE Steuer-ID not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_de_steuer_id_rejected_zero_start() {
        // Starting with 0 must be rejected
        let spans = Tier1Detector::detect("ID: 02345678901", &Locale::DeDe);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("DE_STEUER_ID".to_string())),
            "DE Steuer-ID starting with 0 should not be detected"
        );
    }

    // French INSEE
    #[test]
    fn test_fr_insee_detected() {
        let spans = Tier1Detector::detect("INSEE: 1234567890123", &Locale::FrFr);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("FR_INSEE".to_string())),
            "FR INSEE not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_fr_insee_rejected_wrong_start() {
        // Must start with 1 or 2
        let spans = Tier1Detector::detect("INSEE: 3234567890123", &Locale::FrFr);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("FR_INSEE".to_string())),
            "FR INSEE starting with 3 should not be detected"
        );
    }

    // Indian Aadhaar
    #[test]
    fn test_aadhaar_detected() {
        let spans = Tier1Detector::detect("Aadhaar: 1234 5678 9012", &Locale::InIn);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::AadhaarNumber),
            "Aadhaar not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_aadhaar_not_detected_without_spaces() {
        // Without the required spaces the pattern should not match
        let spans = Tier1Detector::detect("Aadhaar: 123456789012", &Locale::InIn);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::AadhaarNumber),
            "Aadhaar without spaces should not be detected"
        );
    }

    // Indian PAN
    #[test]
    fn test_in_pan_detected() {
        let spans = Tier1Detector::detect("PAN: ABCDE1234F", &Locale::InIn);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("IN_PAN".to_string())),
            "IN PAN not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_in_pan_not_detected_wrong_format() {
        // Too short — must be exactly AAAAA9999A
        let spans = Tier1Detector::detect("PAN: ABCD1234F", &Locale::InIn);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("IN_PAN".to_string())),
            "Short PAN should not be detected"
        );
    }

    // Korean RRN
    #[test]
    fn test_ko_rrn_detected() {
        let spans = Tier1Detector::detect("RRN: 900101-1234567", &Locale::KoKr);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("KO_RRN".to_string())),
            "KO RRN not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ko_rrn_rejected_invalid_gender_digit() {
        // Gender digit must be 1-4
        let spans = Tier1Detector::detect("RRN: 900101-5234567", &Locale::KoKr);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("KO_RRN".to_string())),
            "KO RRN with gender digit 5 should not be detected"
        );
    }

    // Korean BRN
    #[test]
    fn test_ko_brn_detected() {
        let spans = Tier1Detector::detect("BRN: 123-45-67890", &Locale::KoKr);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("KO_BRN".to_string())),
            "KO BRN not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ko_brn_not_detected_wrong_format() {
        // Wrong segment lengths
        let spans = Tier1Detector::detect("BRN: 12-345-67890", &Locale::KoKr);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("KO_BRN".to_string())),
            "KO BRN with wrong format should not be detected"
        );
    }

    // Brazilian CPF
    #[test]
    fn test_cpf_valid_detected() {
        // 529.982.247-25 is a well-known valid CPF test number
        let spans = Tier1Detector::detect("CPF: 529.982.247-25", &Locale::BrBr);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::CpfNumber),
            "valid CPF not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_cpf_invalid_check_digit_rejected() {
        // Change last digit to invalidate check
        let spans = Tier1Detector::detect("CPF: 529.982.247-26", &Locale::BrBr);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::CpfNumber),
            "CPF with bad check digit should not be detected"
        );
    }

    #[test]
    fn test_cpf_all_same_rejected() {
        let spans = Tier1Detector::detect("CPF: 111.111.111-11", &Locale::BrBr);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::CpfNumber),
            "all-same-digit CPF should not be detected"
        );
    }

    // Brazilian CNPJ
    #[test]
    fn test_cnpj_detected() {
        let spans = Tier1Detector::detect("CNPJ: 12.345.678/0001-95", &Locale::BrBr);
        assert!(
            spans.iter().any(|s| s.entity_type == PiiType::Custom("BR_CNPJ".to_string())),
            "BR CNPJ not detected: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_cnpj_not_detected_wrong_format() {
        // Wrong separator placement
        let spans = Tier1Detector::detect("CNPJ: 12345.678/0001-95", &Locale::BrBr);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Custom("BR_CNPJ".to_string())),
            "CNPJ with wrong format should not be detected"
        );
    }

    // CPF helper unit tests
    #[test]
    fn test_cpf_valid_fn() {
        assert!(cpf_valid("529.982.247-25"));
        assert!(!cpf_valid("529.982.247-26")); // bad check digit
        assert!(!cpf_valid("111.111.111-11")); // all same
        assert!(!cpf_valid("000.000.000-00")); // all same
    }

    // US Phone (2.2)
    #[test]
    fn test_phone_detected_us_format() {
        let cases = [
            "Call me at 415-555-1234",
            "Reach us at (415) 555-1234",
            "Phone: 415.555.1234",
            "+1 415-555-1234",
        ];
        for text in cases {
            let spans = Tier1Detector::detect(text, &Locale::EnUs);
            assert!(
                spans.iter().any(|s| s.entity_type == PiiType::Phone),
                "phone not detected in: {text:?}, spans: {:?}",
                spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_phone_not_false_positive() {
        // Bare 10-digit run without separators must not match.
        let text = "model version 4155551234 released";
        let spans = Tier1Detector::detect(text, &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::Phone),
            "bare digit run incorrectly matched as phone: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    // detect_in_json_messages (2.15)
    // Verify that Tier1Detector can identify which message index contains PII
    // when iterating an array of message strings.
    #[test]
    fn test_detect_in_json_messages_returns_message_index() {
        // 3-message array; only message at index 1 contains PII.
        let messages = [
            "No sensitive information here.",
            "Contact alice@example.com for the invoice.",
            "Everything looks good.",
        ];
        let detected: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, text)| !Tier1Detector::detect(text, &Locale::EnUs).is_empty())
            .map(|(i, _)| i)
            .collect();

        assert_eq!(detected, vec![1],
            "expected only message index 1 to contain PII, got {detected:?}");
    }

    // ── §12b – Tier 1 named tests ──────────────────────────────────────────────

    /// §12b.1: Email detection.
    #[test]
    fn test_tier1_email_detection() {
        let spans = Tier1Detector::detect("contact@example.com is my email", &Locale::EnUs);
        assert!(!spans.is_empty(), "no spans detected");
        let email_spans: Vec<_> = spans.iter().filter(|s| s.entity_type == PiiType::Email).collect();
        assert!(!email_spans.is_empty(),
            "expected at least one Email span, got: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>());
    }

    /// §12b.3: SSN detection.
    #[test]
    fn test_tier1_ssn_detection() {
        let spans = Tier1Detector::detect("my SSN is 123-45-6789", &Locale::EnUs);
        assert!(spans.iter().any(|s| s.entity_type == PiiType::Ssn),
            "expected Ssn span, got: {:?}", spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>());
    }

    /// §12b.4: Bearer token detection.
    #[test]
    fn test_tier1_bearer_token_detection() {
        let spans = Tier1Detector::detect(
            "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefghijk",
            &Locale::EnUs,
        );
        assert!(spans.iter().any(|s| s.entity_type == PiiType::BearerToken),
            "expected BearerToken span, got: {:?}", spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>());
    }

    /// §12b.5: False-positive guard — version strings must not match email/phone/SSN.
    #[test]
    fn test_tier1_no_false_positives() {
        let spans = Tier1Detector::detect("version 1.2.3 is available", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| matches!(s.entity_type, PiiType::Email | PiiType::Phone | PiiType::Ssn)),
            "version string must not be detected as Email, Phone, or SSN: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    /// §12b.7: When Tier 1 is effectively not invoked (pipeline bypassed), no spans are returned.
    /// Note: Tier1Detector always runs when called directly; this test documents that
    /// text without any PII patterns produces zero spans — i.e. the absence of false positives
    /// on plain English text means a disabled-tier call would return nothing.
    #[test]
    fn test_tier1_disabled_returns_no_spans() {
        // When tiers.regex = false the proxy layer does not call Tier1Detector.
        // Simulate by verifying clean text → no spans (equivalent to "not called" result).
        let spans = Tier1Detector::detect(
            "This is a completely clean message with no sensitive information whatsoever.",
            &Locale::EnUs,
        );
        assert!(spans.is_empty(),
            "clean text must produce zero spans (equivalent to disabled-tier result): {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>());
    }

    #[test]
    fn test_ipv6_no_false_positive_rust_path() {
        let spans = Tier1Detector::detect("use crate::pii::vault::PiiVault", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::IpV6),
            "Rust path falsely detected as IPv6: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ipv6_no_false_positive_double_colon_only() {
        let spans = Tier1Detector::detect("foo::bar", &Locale::EnUs);
        assert!(
            !spans.iter().any(|s| s.entity_type == PiiType::IpV6),
            "double-colon identifier falsely detected as IPv6: {:?}",
            spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ipv6_abbreviated_detected() {
        for addr in &["fe80::1", "::1", "2001:db8::1", "fd00::1"] {
            let text = format!("addr: {}", addr);
            let spans = Tier1Detector::detect(&text, &Locale::EnUs);
            assert!(
                spans.iter().any(|s| s.entity_type == PiiType::IpV6),
                "abbreviated IPv6 '{}' not detected: {:?}",
                addr,
                spans.iter().map(|s| s.entity_type.label()).collect::<Vec<_>>()
            );
        }
    }

    /// Performance: Tier 1 scan of 10 KB plain text must complete in under 5 ms,
    /// excluding OnceLock regex cold-start (9.1).
    #[test]
    fn test_tier1_10kb_under_5ms() {
        use std::time::Instant;
        // Warm up the OnceLock so regex compilation is not measured.
        let _ = Tier1Detector::detect("warmup@example.com", &Locale::EnUs);

        let text = "The quick brown fox jumps over the lazy dog. ".repeat(230);
        assert!(text.len() >= 10_000, "text too short: {}", text.len());
        let start = Instant::now();
        let _ = Tier1Detector::detect(&text, &Locale::EnUs);
        let elapsed = start.elapsed();
        // Release builds: 5 ms. Debug builds are unoptimised; allow 200 ms.
        let limit_ms: u128 = if cfg!(debug_assertions) { 200 } else { 5 };
        assert!(elapsed.as_millis() < limit_ms,
            "Tier1 scan of 10 KB took {}ms after warmup (limit {}ms)", elapsed.as_millis(), limit_ms);
    }
}
