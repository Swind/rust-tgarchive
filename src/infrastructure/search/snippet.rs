use super::normalize::normalize_with_map;

const WINDOW: usize = 120;
const LEAD: usize = 30;

/// Excerpt of at most [`WINDOW`] characters (plus `…` markers) starting shortly before the
/// earliest occurrence of any needle. Works on `char`s, never on byte offsets. `None` when no
/// needle occurs in the text.
pub fn make_snippet(text: &str, needles: &[String]) -> Option<String> {
    let (normalized, origin) = normalize_with_map(text);
    let start = needles
        .iter()
        .filter_map(|needle| {
            let needle: Vec<char> = needle.chars().collect();
            if needle.is_empty() || needle.len() > normalized.len() {
                return None;
            }
            normalized
                .windows(needle.len())
                .position(|window| window == needle.as_slice())
        })
        .min()?;
    let chars: Vec<char> = text.chars().collect();
    let first = origin[start];
    let begin = first.saturating_sub(LEAD);
    let end = (begin + WINDOW).min(chars.len());
    let mut snippet = String::new();
    if begin > 0 {
        snippet.push('…');
    }
    snippet.extend(&chars[begin..end]);
    if end < chars.len() {
        snippet.push('…');
    }
    Some(snippet)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Vec<String> {
        vec![s.to_owned()]
    }

    #[test]
    fn short_text_is_returned_whole() {
        assert_eq!(
            make_snippet("今天去台北喝咖啡", &n("台北")).unwrap(),
            "今天去台北喝咖啡"
        );
    }

    #[test]
    fn long_text_is_windowed_on_char_boundaries() {
        let text = format!("{}台北{}", "字".repeat(100), "尾".repeat(200));
        let snippet = make_snippet(&text, &n("台北")).unwrap();
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
        assert!(snippet.contains("台北"));
        assert!(snippet.chars().count() <= WINDOW + 2);
    }

    #[test]
    fn matches_through_normalization_and_maps_back_to_original() {
        let snippet = make_snippet("前綴 ＳＱＬｉｔｅ 後綴", &n("sqlite")).unwrap();
        assert!(snippet.contains("ＳＱＬｉｔｅ"));
    }

    #[test]
    fn earliest_needle_wins_and_no_match_is_none() {
        let needles = vec!["咖啡".to_owned(), "台北".to_owned()];
        assert!(make_snippet("台北有咖啡", &needles).is_some());
        assert_eq!(make_snippet("高雄", &needles), None);
    }
}
