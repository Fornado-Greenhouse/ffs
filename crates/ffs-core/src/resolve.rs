//! Name normalization for entity resolution (task_45, ADR-030).
//!
//! The blocking key and the alias table both compare surface forms
//! after the same normalization, so "Dr. Sara Chen, Jr." and
//! "sara chen" block together. Kept deliberately simple: lowercase,
//! collapse whitespace, strip punctuation except spaces and hyphens,
//! and drop honorifics and generational suffixes at the edges.

const HONORIFICS: &[&str] = &["mr", "mrs", "ms", "dr", "mx", "prof"];
const SUFFIXES: &[&str] = &["jr", "sr", "ii", "iii", "iv"];

/// Normalized full-name key.
pub fn normalized_name_key(display: &str) -> String {
    let lowered = display.to_lowercase();
    let cleaned: String = lowered
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let mut tokens: Vec<&str> = cleaned.split_whitespace().collect();
    while let Some(first) = tokens.first()
        && HONORIFICS.contains(first)
        && tokens.len() > 1
    {
        tokens.remove(0);
    }
    while let Some(last) = tokens.last()
        && SUFFIXES.contains(last)
        && tokens.len() > 1
    {
        tokens.pop();
    }
    tokens.join(" ")
}

/// Last token of the normalized name (the surname for Western name
/// order; a weak signal by design, see `resolution.toml`).
pub fn surname_key(display: &str) -> String {
    normalized_name_key(display)
        .split(' ')
        .next_back()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_case_punctuation_honorifics_and_suffixes() {
        assert_eq!(normalized_name_key("Dr. Sara  Chen, Jr."), "sara chen");
        assert_eq!(normalized_name_key("  SARA CHEN "), "sara chen");
        assert_eq!(
            normalized_name_key("Mary-Kate O'Neil III"),
            "mary-kate o neil"
        );
        assert_eq!(
            normalized_name_key("Mr"),
            "mr",
            "a lone honorific is kept, not erased"
        );
        assert_eq!(normalized_name_key(""), "");
    }

    #[test]
    fn surname_is_the_last_normalized_token() {
        assert_eq!(surname_key("Dr. Sara Chen Jr."), "chen");
        assert_eq!(surname_key("Prince"), "prince");
        assert_eq!(surname_key(""), "");
    }
}
