use super::{
    bigram::{cjk_runs, is_cjk, run_bigrams},
    normalize::normalize_text,
    tokenizer::SearchTokenizer,
};

/// Single-character function words dropped from the *word* query only (the bigram path keeps
/// everything): they match nearly every message and carry no meaning on their own.
const FUNCTION_WORDS: [char; 14] = [
    '的', '了', '在', '是', '和', '與', '与', '及', '也', '就', '都', '而', '或', '跟',
];

/// How to run a plain-text user query. User text never reaches `MATCH` unescaped: every term
/// is a quoted FTS5 string.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchPlan {
    /// FTS5 `MATCH` expression; `None` when nothing in the query can use the index.
    pub fts: Option<String>,
    /// Substrings that must appear in the text (single CJK characters: the index stores no
    /// unigrams). ANDed with `fts`.
    pub like_terms: Vec<String>,
    /// Whitespace-separated terms used when the index is not ready.
    pub fallback_terms: Vec<String>,
    /// Strings to look for when building a snippet.
    pub highlight: Vec<String>,
}

impl SearchPlan {
    pub fn is_empty(&self) -> bool {
        self.fts.is_none() && self.like_terms.is_empty()
    }
}

fn quote(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// Quoted term; plain ASCII alphanumeric terms of 3+ characters become prefix queries
/// (`"term"*`) so `benchmark` finds `benchmarks`.
fn quote_term(term: &str) -> String {
    let prefix = term.len() >= 3 && term.chars().all(|c| c.is_ascii_alphanumeric());
    format!("{}{}", quote(term), if prefix { "*" } else { "" })
}

fn has_alnum(term: &str) -> bool {
    term.chars().any(char::is_alphanumeric)
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// Builds `words:(w1 AND w2) OR (bigrams:"b1 b2" AND words:"ascii")`:
/// * words: jieba tokens (function words dropped), all required;
/// * bigrams: one phrase per CJK run of 2+ characters (adjacent, in order, so it is a real
///   substring match), ASCII terms of the query required in the words column;
/// * single-character CJK runs become `like_terms`.
pub fn plan_query(tokenizer: &dyn SearchTokenizer, text: &str) -> SearchPlan {
    let normalized = normalize_text(text);
    let runs = cjk_runs(&normalized);
    let mut plan = SearchPlan {
        fallback_terms: normalized
            .split_whitespace()
            .filter(|term| has_alnum(term))
            .map(ToOwned::to_owned)
            .collect(),
        ..SearchPlan::default()
    };

    let mut phrases = Vec::new();
    for run in &runs {
        if run.len() >= 2 {
            phrases.push(format!(
                "bigrams:{}",
                quote(&run_bigrams(run).collect::<Vec<_>>().join(" "))
            ));
            push_unique(&mut plan.highlight, run.iter().collect());
        } else {
            let single: String = run.iter().collect();
            push_unique(&mut plan.like_terms, single.clone());
            push_unique(&mut plan.highlight, single);
        }
    }

    let words: Vec<String> = tokenizer
        .tokenize_words(text)
        .into_iter()
        .filter(|token| {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if is_cjk(c) => {
                    !FUNCTION_WORDS.contains(&c) && !plan.like_terms.contains(token)
                }
                _ => true,
            }
        })
        .collect();
    let mut word_terms = Vec::new();
    for word in words {
        push_unique(&mut word_terms, word);
    }
    for term in &word_terms {
        push_unique(&mut plan.highlight, term.clone());
    }
    for term in &plan.fallback_terms {
        push_unique(&mut plan.highlight, term.clone());
    }

    let words_expr = (!word_terms.is_empty()).then(|| {
        format!(
            "words:({})",
            word_terms
                .iter()
                .map(|t| quote_term(t))
                .collect::<Vec<_>>()
                .join(" AND ")
        )
    });
    let bigram_expr = (!phrases.is_empty()).then(|| {
        let ascii: String = normalized
            .chars()
            .map(|c| if is_cjk(c) { ' ' } else { c })
            .collect();
        for term in ascii.split_whitespace().filter(|term| has_alnum(term)) {
            phrases.push(format!("words:{}", quote_term(term)));
        }
        format!("({})", phrases.join(" AND "))
    });
    plan.fts = match (words_expr, bigram_expr) {
        (Some(words), Some(bigrams)) => Some(format!("{words} OR {bigrams}")),
        (Some(expr), None) | (None, Some(expr)) => Some(expr),
        (None, None) => None,
    };
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::search::default_tokenizer;

    fn plan(text: &str) -> SearchPlan {
        plan_query(default_tokenizer().as_ref(), text)
    }

    #[test]
    fn bigram_path_is_a_phrase() {
        let plan = plan("台北咖啡");
        let fts = plan.fts.unwrap();
        assert!(fts.contains("bigrams:\"台北 北咖 咖啡\""), "{fts}");
        assert!(fts.starts_with("words:("), "{fts}");
        assert!(plan.like_terms.is_empty());
    }

    #[test]
    fn single_cjk_character_uses_like() {
        let plan = plan("北");
        assert_eq!(plan.fts, None);
        assert_eq!(plan.like_terms, ["北"]);
        assert!(!plan.is_empty());
    }

    #[test]
    fn function_words_are_dropped_from_words_only() {
        let fts = plan("咖啡的").fts.unwrap();
        let (words, bigrams) = fts.split_once(" OR ").unwrap();
        assert!(!words.contains("\"的\""), "{words}");
        assert!(bigrams.contains("咖啡 啡的"), "{bigrams}");
    }

    #[test]
    fn ascii_only_query_uses_words_column() {
        let fts = plan("GitLab Runner").fts.unwrap();
        assert_eq!(fts, "words:(\"gitlab\"* AND \"runner\"*)");
    }

    #[test]
    fn mixed_query_requires_ascii_terms_on_bigram_path() {
        let fts = plan("gitlab 咖啡").fts.unwrap();
        assert!(
            fts.contains("(bigrams:\"咖啡\" AND words:\"gitlab\"*)"),
            "{fts}"
        );
    }

    #[test]
    fn quotes_and_operators_are_neutralized() {
        let plan = plan("a\" OR \"b NOT *");
        let fts = plan.fts.unwrap();
        assert!(!fts.contains("\"\"\""), "{fts}");
        assert_eq!(fts.matches('"').count() % 2, 0, "{fts}");
        assert!(plan_query(default_tokenizer().as_ref(), "！？").is_empty());
    }

    #[test]
    fn short_ascii_terms_stay_exact_and_symbols_are_quoted() {
        assert_eq!(plan("ab").fts.unwrap(), "words:(\"ab\")");
        let fts = plan("foo*bar a:b").fts.unwrap();
        assert!(!fts.contains("foo*bar"), "{fts}");
    }

    #[test]
    fn fullwidth_query_is_normalized() {
        assert_eq!(plan("ＳＱＬｉｔｅ").fts.unwrap(), "words:(\"sqlite\"*)");
    }
}
