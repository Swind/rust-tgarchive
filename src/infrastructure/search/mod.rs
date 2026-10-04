//! Chinese-friendly full-text search: NFKC normalization, jieba word segmentation and CJK
//! bigrams feed a contentless FTS5 table (see `rust-jieba-bigram-sqlite-fts5.md`).
//!
//! Index and query use the identical pipeline; any change to normalization, segmentation or
//! bigram rules must bump [`SEARCH_INDEX_VERSION`] so existing indexes are rebuilt.

mod bigram;
mod normalize;
mod query;
mod snippet;
mod tokenizer;

pub use bigram::{cjk_bigrams, is_cjk};
pub use normalize::{is_searchable, normalize_text};
pub use query::{SearchPlan, plan_query};
pub use snippet::make_snippet;
pub use tokenizer::{HybridChineseTokenizer, SearchDocument, SearchTokenizer, default_tokenizer};

/// Version of the tokenization semantics stored in `app_metadata.search_index_version`.
pub const SEARCH_INDEX_VERSION: u32 = 1;
