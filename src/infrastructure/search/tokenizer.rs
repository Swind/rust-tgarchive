use std::sync::{Arc, OnceLock};

use jieba_rs::Jieba;

use super::{
    bigram::{cjk_runs, run_bigrams},
    normalize::normalize_text,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchDocument {
    pub words: String,
    pub bigrams: String,
}

/// Turns text into the two token streams stored in `messages_fts`. Implementations normalize
/// internally, so callers pass raw text for both indexing and querying.
pub trait SearchTokenizer: Send + Sync {
    fn tokenize_words(&self, text: &str) -> Vec<String>;
    fn tokenize_bigrams(&self, text: &str) -> Vec<String>;

    fn build_document(&self, text: &str) -> SearchDocument {
        SearchDocument {
            words: self.tokenize_words(text).join(" "),
            bigrams: self.tokenize_bigrams(text).join(" "),
        }
    }
}

/// jieba search-mode words plus CJK bigrams. Holds one shared [`Jieba`] (building the
/// dictionary is expensive, so it is created once per process, on first use).
pub struct HybridChineseTokenizer {
    jieba: Arc<Jieba>,
}

impl HybridChineseTokenizer {
    pub fn new(jieba: Arc<Jieba>) -> Self {
        Self { jieba }
    }
}

impl SearchTokenizer for HybridChineseTokenizer {
    fn tokenize_words(&self, text: &str) -> Vec<String> {
        let normalized = normalize_text(text);
        self.jieba
            .cut_for_search(&normalized, true)
            .into_iter()
            .map(|token| token.word.trim())
            // Punctuation-only tokens produce no FTS tokens (and would make an empty phrase).
            .filter(|token| token.chars().any(char::is_alphanumeric))
            .map(ToOwned::to_owned)
            .collect()
    }

    fn tokenize_bigrams(&self, text: &str) -> Vec<String> {
        cjk_runs(&normalize_text(text))
            .iter()
            .flat_map(|run| run_bigrams(run))
            .collect()
    }
}

fn shared() -> &'static HybridChineseTokenizer {
    static SHARED: OnceLock<HybridChineseTokenizer> = OnceLock::new();
    SHARED.get_or_init(|| HybridChineseTokenizer::new(Arc::new(Jieba::new())))
}

/// Handle to the process-wide tokenizer. Creating the handle is free: the jieba dictionary
/// (hundreds of ms) is loaded once, on the first tokenization, so commands and servers that
/// never index or search do not pay for it.
struct SharedTokenizer;

impl SearchTokenizer for SharedTokenizer {
    fn tokenize_words(&self, text: &str) -> Vec<String> {
        shared().tokenize_words(text)
    }

    fn tokenize_bigrams(&self, text: &str) -> Vec<String> {
        shared().tokenize_bigrams(text)
    }
}

pub fn default_tokenizer() -> Arc<dyn SearchTokenizer> {
    Arc::new(SharedTokenizer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenizer() -> Arc<dyn SearchTokenizer> {
        default_tokenizer()
    }

    #[test]
    fn traditional_chinese_words_and_bigrams() {
        let t = tokenizer();
        let words = t.tokenize_words("今天下午要去台北喝咖啡");
        assert!(words.contains(&"台北".to_owned()), "{words:?}");
        assert!(words.contains(&"咖啡".to_owned()), "{words:?}");
        let bigrams = t.tokenize_bigrams("今天下午要去台北喝咖啡");
        for expected in ["台北", "喝咖", "咖啡"] {
            assert!(bigrams.contains(&expected.to_owned()), "{bigrams:?}");
        }
    }

    #[test]
    fn mixed_language_keeps_ascii_terms() {
        let t = tokenizer();
        let words = t.tokenize_words("GitLab Runner 今天沒有回應");
        assert!(words.contains(&"gitlab".to_owned()), "{words:?}");
        assert!(words.contains(&"runner".to_owned()), "{words:?}");
        assert_eq!(t.tokenize_bigrams("GitLab Runner 今天沒有回應")[0], "今天");
    }

    #[test]
    fn fullwidth_text_is_normalized() {
        assert!(
            tokenizer()
                .tokenize_words("ＳＱＬｉｔｅ")
                .contains(&"sqlite".to_owned())
        );
    }

    #[test]
    fn punctuation_and_whitespace_are_dropped() {
        let doc = tokenizer().build_document("，。 ！ ");
        assert_eq!(doc, SearchDocument::default());
    }

    #[test]
    fn repeated_words_keep_term_frequency() {
        let words = tokenizer().tokenize_words("咖啡 咖啡 咖啡");
        assert_eq!(words.iter().filter(|w| *w == "咖啡").count(), 3);
    }

    #[test]
    fn jieba_is_created_once_and_shared() {
        assert!(std::ptr::eq(shared(), shared()));
        let _ = default_tokenizer().tokenize_words("台北");
        assert!(std::ptr::eq(shared(), shared()));
    }
}
