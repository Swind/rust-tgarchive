use unicode_normalization::UnicodeNormalization;

/// NFKC + lowercase; applied identically to indexed text and queries.
pub fn normalize_text(input: &str) -> String {
    input.nfkc().collect::<String>().to_lowercase()
}

/// Whether a query still has something to match after normalization.
pub fn is_searchable(input: &str) -> bool {
    normalize_text(input).chars().any(char::is_alphanumeric)
}

/// Normalizes char by char, remembering which original char each output char came from, so
/// matches can be located in the original text (used for snippets).
pub(super) fn normalize_with_map(text: &str) -> (Vec<char>, Vec<usize>) {
    let mut normalized = Vec::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    for (index, ch) in text.chars().enumerate() {
        if ch.is_ascii() {
            normalized.push(ch.to_ascii_lowercase());
            origin.push(index);
        } else {
            for out in ch.nfkc().collect::<String>().to_lowercase().chars() {
                normalized.push(out);
                origin.push(index);
            }
        }
    }
    (normalized, origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_fullwidth_and_case() {
        assert_eq!(normalize_text("ＳＱＬｉｔｅ"), "sqlite");
        assert_eq!(normalize_text("GitLab Runner"), "gitlab runner");
    }

    #[test]
    fn searchable_requires_a_letter_or_digit() {
        assert!(is_searchable("台"));
        assert!(is_searchable(" ａ "));
        assert!(!is_searchable(" ！？\"*- "));
        assert!(!is_searchable(""));
    }
}
