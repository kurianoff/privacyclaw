use crate::pii::locale::Locale;
use crate::pii::vault::{PiiType, PiiVault};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

/// Generates synthetic PII values that preserve type and format.
///
/// Uses a seeded SmallRng derived from the vault's `rng_seed` so that
/// within a conversation the same original PII always gets the same synthetic.
pub struct SyntheticGenerator {
    rng: SmallRng,
}

impl SyntheticGenerator {
    /// Create from the vault's seed, advanced by `call_count` so each call
    /// produces a distinct value even if the type is the same.
    pub fn new(seed: u64) -> Self {
        Self { rng: SmallRng::seed_from_u64(seed) }
    }

    /// Get an existing synthetic or generate a new one, storing in vault.
    pub fn get_or_create(
        vault: &mut PiiVault,
        original: &str,
        pii_type: &PiiType,
        locale: &Locale,
        tier: u8,
        confidence: f32,
    ) -> String {
        tracing::trace!(
            original_len = original.len(),
            pii_type = pii_type.label(),
            tier,
            "synth: get_or_create enter"
        );
        if let Some(existing) = vault.get_synthetic(original) {
            tracing::trace!(original_len = original.len(), "synth: cache hit");
            return existing.to_string();
        }
        tracing::trace!(original_len = original.len(), "synth: cache miss, generating");
        // Advance seed by current vault size so each new entity gets unique output.
        let call_offset = vault.mapping_count() as u64;
        let mut gen = SyntheticGenerator::new(vault.rng_seed.wrapping_add(call_offset));
        let synthetic = gen.generate(pii_type, original, locale);
        // original/synthetic only at DEBUG — logging raw PII at INFO would defeat the purpose
        // of the proxy (PII would appear in every production log and log aggregator).
        tracing::debug!(
            original = %original,
            synthetic = %synthetic,
            pii_type = pii_type.label(),
            tier,
            "synthetic replacement applied"
        );
        vault.add_mapping(original.to_string(), synthetic.clone(), pii_type, tier, confidence);
        synthetic
    }

    /// Generate a synthetic replacement for the given PII type.
    pub fn generate(&mut self, pii_type: &PiiType, original: &str, locale: &Locale) -> String {
        tracing::trace!(pii_type = pii_type.label(), "synth: dispatching generator");
        match pii_type {
            PiiType::Email => self.gen_email(locale),
            PiiType::Phone => self.gen_phone(original),
            PiiType::Ssn => self.gen_ssn(),
            PiiType::CreditCard => self.gen_credit_card(original),
            PiiType::IpV4 => self.gen_ipv4(),
            PiiType::IpV6 => self.gen_ipv6(),
            PiiType::OpenAiApiKey => self.gen_api_key("sk-", original.len()),
            PiiType::AwsAccessKey => self.gen_aws_access_key(),
            PiiType::AwsSecretKey => self.gen_random_alphanumeric(40),
            PiiType::GitHubPat => self.gen_api_key("ghp_", original.len()),
            PiiType::BearerToken => self.gen_random_alphanumeric(original.len().max(32)),
            PiiType::SshPrivateKey => "[REDACTED_PRIVATE_KEY]".to_string(),
            PiiType::DbConnectionString => self.gen_db_connection(original),
            PiiType::UrlWithCreds => self.gen_url_with_creds(original),
            PiiType::PersonName => self.gen_person_name(locale),
            PiiType::OrgName => self.gen_org_name(),
            PiiType::Address => self.gen_address(locale),
            PiiType::DateOfBirth => self.gen_dob(original),
            PiiType::AadhaarNumber => self.gen_aadhaar(),
            PiiType::CpfNumber => self.gen_cpf(),
            _ => self.gen_random_alphanumeric(original.len().max(8)),
        }
    }

    // ── Generators ─────────────────────────────────────────────────────────

    fn gen_email(&mut self, _locale: &Locale) -> String {
        let first = FIRST_NAMES[self.rng.gen_range(0..FIRST_NAMES.len())];
        let last = LAST_NAMES[self.rng.gen_range(0..LAST_NAMES.len())];
        let sep = if self.rng.gen_bool(0.5) { "." } else { "" };
        format!("{}{}{}{}", first.to_lowercase(), sep, last.to_lowercase(), "@example.com")
    }

    fn gen_phone(&mut self, original: &str) -> String {
        // Preserve country code prefix if present.
        let prefix = if let Some(after_plus) = original.strip_prefix('+') {
            let code_end = after_plus.find(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(3);
            format!("+{} ", &after_plus[..code_end.min(3)])
        } else {
            String::new()
        };
        format!("{}555-{:04}-{:04}", prefix, self.rng.gen_range(0..10000u32), self.rng.gen_range(0..10000u32))
    }

    fn gen_ssn(&mut self) -> String {
        // Avoid real-looking area codes (000, 666, 900-999).
        let area = self.rng.gen_range(100u32..=665);
        let group = self.rng.gen_range(1u32..=99);
        let serial = self.rng.gen_range(1u32..=9999);
        format!("{:03}-{:02}-{:04}", area, group, serial)
    }

    fn gen_credit_card(&mut self, original: &str) -> String {
        // Preserve card brand by keeping the first digit.
        let first = original.chars().find(|c| c.is_ascii_digit()).unwrap_or('4');
        let digits: String = std::iter::once(first)
            .chain((1..15).map(|_| char::from_digit(self.rng.gen_range(0..10), 10).unwrap()))
            .collect();
        // Compute Luhn check digit.
        let check = luhn_check_digit(&digits);
        format!("{}{}", digits, check)
    }

    fn gen_ipv4(&mut self) -> String {
        // RFC 1918 private address.
        let second = self.rng.gen_range(0u8..=255);
        let third = self.rng.gen_range(0u8..=255);
        let fourth = self.rng.gen_range(1u8..=254);
        format!("10.{}.{}.{}", second, third, fourth)
    }

    fn gen_ipv6(&mut self) -> String {
        // fd00::/8 unique local — 2 groups to keep max_synthetic_key_len small.
        let g1 = format!("{:04x}", self.rng.gen_range(0u16..=0xffff));
        let g2 = format!("{:04x}", self.rng.gen_range(0u16..=0xffff));
        format!("fd{}:{}::1", g1, g2)
    }

    fn gen_api_key(&mut self, prefix: &str, original_len: usize) -> String {
        let suffix_len = original_len.saturating_sub(prefix.len()).max(32);
        let suffix = self.gen_random_alphanumeric(suffix_len);
        format!("{}{}", prefix, suffix)
    }

    fn gen_aws_access_key(&mut self) -> String {
        let suffix = self.gen_random_uppercase(12);
        format!("AKIAIOSFODNN7{}", suffix)
    }

    fn gen_db_connection(&mut self, original: &str) -> String {
        // Detect scheme and replace credentials only.
        let scheme_end = original.find("://").map(|i| i + 3).unwrap_or(0);
        let scheme = &original[..scheme_end];
        format!("{}synthetic:synthetic@localhost/db", scheme)
    }

    fn gen_url_with_creds(&mut self, original: &str) -> String {
        let scheme_end = original.find("://").map(|i| i + 3).unwrap_or(0);
        let scheme = &original[..scheme_end];
        let host_start = original[scheme_end..].find('@').map(|i| scheme_end + i + 1).unwrap_or(scheme_end);
        format!("{}user:password@{}", scheme, &original[host_start..])
    }

    fn gen_person_name(&mut self, _locale: &Locale) -> String {
        let first = FIRST_NAMES[self.rng.gen_range(0..FIRST_NAMES.len())];
        let last = LAST_NAMES[self.rng.gen_range(0..LAST_NAMES.len())];
        format!("{} {}", first, last)
    }

    fn gen_org_name(&mut self) -> String {
        ORG_NAMES[self.rng.gen_range(0..ORG_NAMES.len())].to_string()
    }

    fn gen_address(&mut self, _locale: &Locale) -> String {
        let num = self.rng.gen_range(1u32..=9999);
        let street = STREET_NAMES[self.rng.gen_range(0..STREET_NAMES.len())];
        let suffix = STREET_SUFFIXES[self.rng.gen_range(0..STREET_SUFFIXES.len())];
        format!("{} {} {}", num, street, suffix)
    }

    fn gen_dob(&mut self, original: &str) -> String {
        // Shift by deterministic offset (7-97 days) using the RNG.
        // Best-effort: if we can't parse the original, return a fixed fake date.
        let offset_days = self.rng.gen_range(7i64..=97);
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(original, "%Y-%m-%d") {
            let shifted = dt + chrono::Duration::days(offset_days);
            return shifted.format("%Y-%m-%d").to_string();
        }
        format!("{}-{:02}-{:02}",
            self.rng.gen_range(1950u32..=2005),
            self.rng.gen_range(1u32..=12),
            self.rng.gen_range(1u32..=28))
    }

    fn gen_aadhaar(&mut self) -> String {
        let a = self.rng.gen_range(2000u32..=9999);
        let b = self.rng.gen_range(0u32..=9999);
        let c = self.rng.gen_range(0u32..=9999);
        format!("{} {:04} {:04}", a, b, c)
    }

    fn gen_cpf(&mut self) -> String {
        let d: Vec<u8> = (0..9).map(|_| self.rng.gen_range(0u8..=9)).collect();
        let d10 = cpf_check_digit(&d, 10);
        let d11 = cpf_check_digit(&d, 11);
        format!("{}{}{}.{}{}{}.{}{}{}-{}{}", d[0],d[1],d[2],d[3],d[4],d[5],d[6],d[7],d[8],d10,d11)
    }

    fn gen_random_alphanumeric(&mut self, len: usize) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        (0..len).map(|_| CHARS[self.rng.gen_range(0..CHARS.len())] as char).collect()
    }

    fn gen_random_uppercase(&mut self, len: usize) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        (0..len).map(|_| CHARS[self.rng.gen_range(0..CHARS.len())] as char).collect()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Compute Luhn check digit for a string of digits.
fn luhn_check_digit(digits: &str) -> u8 {
    let mut sum = 0u32;
    let mut double = true;
    for c in digits.chars().rev() {
        let mut d = c.to_digit(10).unwrap_or(0);
        if double { d *= 2; if d > 9 { d -= 9; } }
        sum += d;
        double = !double;
    }
    ((10 - (sum % 10)) % 10) as u8
}

/// Compute CPF verification digit.
fn cpf_check_digit(digits: &[u8], multiplier_start: u8) -> u8 {
    let sum: u32 = digits.iter().enumerate()
        .map(|(i, &d)| d as u32 * (multiplier_start as u32 - i as u32))
        .sum();
    let rem = sum % 11;
    if rem < 2 { 0 } else { 11 - rem as u8 }
}

// ── Static word lists ─────────────────────────────────────────────────────────

static FIRST_NAMES: &[&str] = &[
    "Alice", "Bob", "Carol", "David", "Eve", "Frank", "Grace", "Henry",
    "Iris", "Jack", "Karen", "Liam", "Mia", "Noah", "Olivia", "Paul",
    "Quinn", "Rachel", "Sam", "Tara", "Uma", "Victor", "Wendy", "Xander",
    "Yara", "Zoe",
];

static LAST_NAMES: &[&str] = &[
    "Brown", "Chen", "Davis", "Evans", "Foster", "Garcia", "Harris", "Ibarra",
    "Johnson", "Kim", "Lee", "Martin", "Nguyen", "O'Brien", "Patel", "Quinn",
    "Roberts", "Smith", "Taylor", "Ueda", "Vargas", "Walker", "Xu", "Young",
    "Zhang",
];

static ORG_NAMES: &[&str] = &[
    "Acme Corp", "Initech", "Globex Corporation", "Umbrella Corp",
    "Soylent Corp", "Vandelay Industries", "Bluth Company", "Dunder Mifflin",
    "Hooli", "Pied Piper", "Sterling Cooper", "Wonka Industries",
];

static STREET_NAMES: &[&str] = &[
    "Main", "Oak", "Maple", "Cedar", "Pine", "Elm", "Walnut", "Birch",
    "Washington", "Lincoln", "Jefferson", "Madison", "Monroe", "Adams",
];

static STREET_SUFFIXES: &[&str] = &[
    "St", "Ave", "Blvd", "Dr", "Ln", "Rd", "Way", "Ct", "Pl", "Ter",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_with_same_seed() {
        let mut g1 = SyntheticGenerator::new(42);
        let mut g2 = SyntheticGenerator::new(42);
        let e1 = g1.generate(&PiiType::Email, "x@y.com", &Locale::EnUs);
        let e2 = g2.generate(&PiiType::Email, "x@y.com", &Locale::EnUs);
        assert_eq!(e1, e2);
    }

    #[test]
    fn test_different_seeds_produce_different_output() {
        let mut g1 = SyntheticGenerator::new(1);
        let mut g2 = SyntheticGenerator::new(999);
        let e1 = g1.generate(&PiiType::Email, "x@y.com", &Locale::EnUs);
        let e2 = g2.generate(&PiiType::Email, "x@y.com", &Locale::EnUs);
        // Very unlikely to be equal with different seeds
        assert_ne!(e1, e2);
    }

    #[test]
    fn test_ssn_format() {
        let mut g = SyntheticGenerator::new(7);
        let ssn = g.generate(&PiiType::Ssn, "123-45-6789", &Locale::EnUs);
        assert!(ssn.len() == 11);
        assert_eq!(&ssn[3..4], "-");
        assert_eq!(&ssn[6..7], "-");
    }

    #[test]
    fn test_email_format() {
        let mut g = SyntheticGenerator::new(3);
        let email = g.generate(&PiiType::Email, "foo@bar.com", &Locale::EnUs);
        assert!(email.ends_with("@example.com"), "got: {}", email);
    }

    #[test]
    fn test_get_or_create_idempotent() {
        let mut vault = PiiVault::new("test-synth-1");
        let s1 = SyntheticGenerator::get_or_create(&mut vault, "john@acme.com", &PiiType::Email, &Locale::EnUs, 1, 1.0);
        let s2 = SyntheticGenerator::get_or_create(&mut vault, "john@acme.com", &PiiType::Email, &Locale::EnUs, 1, 1.0);
        assert_eq!(s1, s2, "same original should always return same synthetic");
    }

    #[test]
    fn test_credit_card_luhn_valid() {
        let mut gen = SyntheticGenerator::new(99);
        let result = gen.generate(&PiiType::CreditCard, "4111111111111111", &Locale::EnUs);
        // Strip non-digit chars and verify length and Luhn validity.
        let digits: Vec<u32> = result
            .chars()
            .filter(|c| c.is_ascii_digit())
            .filter_map(|c| c.to_digit(10))
            .collect();
        assert!(
            digits.len() >= 13 && digits.len() <= 19,
            "expected 13-19 digit card, got {} digits in {:?}",
            digits.len(),
            result
        );
        let luhn_sum = digits
            .iter()
            .rev()
            .enumerate()
            .fold(0u32, |acc, (i, &d)| {
                let v = if i % 2 == 1 {
                    let x = d * 2;
                    if x > 9 { x - 9 } else { x }
                } else {
                    d
                };
                acc + v
            });
        assert_eq!(
            luhn_sum % 10,
            0,
            "generated credit card {:?} fails Luhn check (sum={})",
            result,
            luhn_sum
        );
    }

    #[test]
    fn test_ipv4_rfc1918_prefix() {
        let mut gen = SyntheticGenerator::new(42);
        let result = gen.generate(&PiiType::IpV4, "192.168.1.1", &Locale::EnUs);
        assert!(
            result.starts_with("10."),
            "expected RFC-1918 10.x.x.x prefix, got {:?}",
            result
        );
    }

    #[test]
    fn test_bearer_token_format() {
        let mut gen = SyntheticGenerator::new(7);
        let original = "eyJhbGci.eyJzdWIi.SflKxwRJSMeKKF";
        let result = gen.generate(&PiiType::BearerToken, original, &Locale::EnUs);
        // BearerToken generates gen_random_alphanumeric(original.len().max(32)).
        // The result should be at least 32 alphanumeric characters.
        assert!(
            result.len() >= 20,
            "bearer token too short: {:?}",
            result
        );
        assert!(
            result.chars().all(|c| c.is_ascii_alphanumeric()),
            "bearer token contains non-alphanumeric chars: {:?}",
            result
        );
    }

    #[test]
    fn test_openai_key_prefix_preserved() {
        let mut gen = SyntheticGenerator::new(13);
        let result = gen.generate(
            &PiiType::OpenAiApiKey,
            "sk-abcdefghijklmnopqrstuvwxyz12345678901234",
            &Locale::EnUs,
        );
        assert!(
            result.starts_with("sk-"),
            "OpenAI key synthetic should start with 'sk-', got {:?}",
            result
        );
    }

    #[test]
    fn test_phone_format() {
        let mut gen = SyntheticGenerator::new(5);
        let result = gen.generate(&PiiType::Phone, "+1 415-555-1234", &Locale::EnUs);
        assert!(!result.is_empty(), "generated phone is empty");
        // Should contain at least one digit and one separator character.
        let has_digit = result.chars().any(|c| c.is_ascii_digit());
        let has_sep = result.chars().any(|c| c == '-' || c == '(' || c == ')' || c == ' ');
        assert!(has_digit, "phone has no digits: {:?}", result);
        assert!(has_sep, "phone has no separator character: {:?}", result);
    }

    #[test]
    fn test_gen_ipv6_length() {
        // After the fix, gen_ipv6 produces "fd{4hex}:{4hex}::1" which is 14 chars.
        // Verify several generations are all <= 16 chars, start with "fd", end with "::1".
        for seed in 0..20u64 {
            let mut gen = SyntheticGenerator::new(seed);
            let result = gen.generate(&PiiType::IpV6, "2001:db8::1", &Locale::EnUs);
            assert!(
                result.len() <= 16,
                "gen_ipv6 produced {} chars ({:?}) with seed {}, expected <= 16",
                result.len(), result, seed
            );
            assert!(
                result.starts_with("fd"),
                "gen_ipv6 should start with 'fd', got {:?} with seed {}",
                result, seed
            );
            assert!(
                result.ends_with("::1"),
                "gen_ipv6 should end with '::1', got {:?} with seed {}",
                result, seed
            );
        }
    }

    #[test]
    fn test_gen_ipv6_format_is_valid_ula() {
        // Verify the format is "fdXXXX:XXXX::1" — valid ULA address.
        let mut gen = SyntheticGenerator::new(42);
        let result = gen.generate(&PiiType::IpV6, "::1", &Locale::EnUs);
        // Should match pattern: fd followed by 4 hex chars, colon, 4 hex chars, ::1
        let re = fancy_regex::Regex::new(r"^fd[0-9a-f]{4}:[0-9a-f]{4}::1$").unwrap();
        assert!(
            re.is_match(&result).unwrap_or(false),
            "gen_ipv6 output {:?} does not match fdXXXX:XXXX::1 pattern",
            result
        );
    }

    #[test]
    fn test_get_or_create_same_original_different_types() {
        // The vault uses the original string as the key for idempotent lookup
        // (see get_synthetic / add_mapping). The second call with a different
        // PiiType but the same original string hits the existing mapping and
        // returns the first synthetic unchanged.
        let mut vault = PiiVault::new("test-synth-diff-types");
        let s_email = SyntheticGenerator::get_or_create(
            &mut vault,
            "test",
            &PiiType::Email,
            &Locale::EnUs,
            1,
            1.0,
        );
        let s_phone = SyntheticGenerator::get_or_create(
            &mut vault,
            "test",
            &PiiType::Phone,
            &Locale::EnUs,
            1,
            1.0,
        );
        // Because add_mapping is idempotent on the original key, the second
        // call returns the first synthetic (vault ignores the new type).
        assert_eq!(
            s_email, s_phone,
            "same original with different PiiType should return the cached synthetic"
        );
        // Mapping count stays at 1 — no duplicate entry was added.
        assert_eq!(vault.mapping_count(), 1);
    }
}
