use serde::{Deserialize, Serialize};

/// Supported locale packs for locale-specific PII patterns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Locale {
    #[default]
    EnUs,
    EnGb,
    DeDe,
    FrFr,
    InIn,
    KoKr,
    BrBr,
}

impl Locale {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "en-us" | "en" => Some(Locale::EnUs),
            "en-gb" => Some(Locale::EnGb),
            "de-de" | "de" => Some(Locale::DeDe),
            "fr-fr" | "fr" => Some(Locale::FrFr),
            "in-in" | "in" => Some(Locale::InIn),
            "ko-kr" | "ko" => Some(Locale::KoKr),
            "br-br" | "pt-br" => Some(Locale::BrBr),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Locale::EnUs => "en-US",
            Locale::EnGb => "en-GB",
            Locale::DeDe => "de-DE",
            Locale::FrFr => "fr-FR",
            Locale::InIn => "in-IN",
            Locale::KoKr => "ko-KR",
            Locale::BrBr => "br-BR",
        }
    }
}
