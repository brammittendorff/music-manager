//! Copyright status estimation based on EU Directive 2011/77/EU.
//!
//! DISCLAIMER: This module provides estimates only and does NOT constitute
//! legal advice. Always verify copyright status with a qualified professional
//! before uploading or redistributing any recording.

use chrono::Datelike;
use mm_config::CopyrightConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyrightStatus {
    /// Year unknown - cannot estimate
    Unknown,
    /// Published 70+ years ago - EU sound recording term has expired
    PublicDomain,
    /// Within review buffer years of expiry - manual verification recommended
    LikelyPublicDomain,
    /// Between 50–70 years - pre-2013 rules may apply, check required
    CheckRequired,
    /// Less than 50 years - clearly under copyright
    UnderCopyright,
}

impl CopyrightStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::PublicDomain => "PUBLIC_DOMAIN",
            Self::LikelyPublicDomain => "LIKELY_PUBLIC_DOMAIN",
            Self::CheckRequired => "CHECK_REQUIRED",
            Self::UnderCopyright => "UNDER_COPYRIGHT",
        }
    }

    /// Returns true when redistribution is likely safe (still verify!).
    pub fn likely_safe_to_upload(&self) -> bool {
        matches!(self, Self::PublicDomain | Self::LikelyPublicDomain)
    }
}

impl std::fmt::Display for CopyrightStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Estimate the copyright status of a sound recording.
///
/// EU rules (Directive 2011/77/EU):
///   - Sound recordings: expire 70 years after **first publication**
///   - Musical composition: expires 70 years after **death of author** (not handled here)
///
/// This function only addresses sound recording rights.
pub fn estimate(publication_year: Option<i32>, cfg: &CopyrightConfig) -> (CopyrightStatus, String) {
    let Some(year) = publication_year else {
        return (
            CopyrightStatus::Unknown,
            "Publication year unknown - cannot estimate copyright status.".into(),
        );
    };

    let current_year = chrono::Utc::now().year();
    let age = current_year - year;
    let term = cfg.sound_recording_term_years as i32;
    let buffer = cfg.review_buffer_years as i32;

    if age >= term {
        (
            CopyrightStatus::PublicDomain,
            format!(
                "Published {year}. Sound recording term of {term} years has expired ({} years ago). \
                 Likely public domain in the EU. Verify before redistribution.",
                age - term
            ),
        )
    } else if age >= term - buffer {
        (
            CopyrightStatus::LikelyPublicDomain,
            format!(
                "Published {year}. Sound recording expires in approximately {} year(s). \
                 Verify before redistribution.",
                term - age
            ),
        )
    } else if age >= 50 {
        (
            CopyrightStatus::CheckRequired,
            format!(
                "Published {year} ({age} years ago). Pre-2013 EU rules (50-year term) may apply \
                 to recordings published before 2013. Manual verification required."
            ),
        )
    } else {
        (
            CopyrightStatus::UnderCopyright,
            format!(
                "Published {year} ({age} years ago). Clearly under copyright for \
                 approximately {} more years.",
                term - age
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_config::CopyrightConfig;

    fn cfg() -> CopyrightConfig {
        CopyrightConfig {
            sound_recording_term_years: 70,
            review_buffer_years: 5,
        }
    }

    #[test]
    fn test_public_domain() {
        let (status, _) = estimate(Some(1940), &cfg());
        assert_eq!(status, CopyrightStatus::PublicDomain);
    }

    #[test]
    fn test_under_copyright() {
        let (status, _) = estimate(Some(2010), &cfg());
        assert_eq!(status, CopyrightStatus::UnderCopyright);
    }

    #[test]
    fn test_unknown() {
        let (status, _) = estimate(None, &cfg());
        assert_eq!(status, CopyrightStatus::Unknown);
    }
}
