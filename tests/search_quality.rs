//! Search quality evaluation over a generated chat corpus (`tests/support/corpus.rs`).
//!
//! Ground truth is computed independently in Rust from the generated texts (not from the index).
//! Run with `cargo test --test search_quality -- --nocapture` to see the summary table.
//!
//! # Ground-truth definitions (the engine's contract)
//!
//! A query is NFKC-normalized + lowercased and split into
//! * CJK runs of >= 2 characters — the document must contain the run *contiguously* (substring
//!   of its normalized text; the bigram phrase guarantees this),
//! * single CJK characters — the *raw* text must contain the character (`LIKE` scan; there are
//!   no unigrams in the index),
//! * ASCII alphanumeric terms — some document token (maximal ASCII alphanumeric run) must equal
//!   the term when it has < 3 chars, or start with it when it has >= 3 chars (prefix query).
//!
//! The expected set `T` is the AND of all constraints over non-deleted messages (all messages
//! with `include_deleted`) that also satisfy the chat/sender/time filters (`from` inclusive,
//! `to` exclusive). The engine additionally ORs a *words path* (all jieba words of the query
//! present, not necessarily adjacent), so for queries longer than one jieba word the hit set `H`
//! may legitimately contain "scattered" extras. Therefore:
//! * recall `|T∩H|/|T|` must be 100% everywhere;
//! * extras (`H\T`) must be *explained* (contain every ASCII term as a prefix token, every
//!   single character and every non-function CJK character of the query); queries marked
//!   `exact` (single words, English, single chars, filters on those, no-result) must have no
//!   extras at all (precision 100%);
//! * ranking sanity (relevance sort): when `T` is non-empty and extras exist, the top-1 hit must
//!   be in `T`, and the share of `T` among the top-10 is reported (`rank@10`, bar 0.8 per query).
//!   Both sort modes must return the identical set; time sort must be strictly newest-first.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use support::corpus::{self, Corpus, GenMessage};
use tgarchive::{
    application::{
        MessageFilters, MessageRepository, PageSize, SearchIndexState, SearchMessagesQuery,
        SearchSort, TimeRange,
    },
    domain::{ChatId, SenderId},
    infrastructure::persistence::sqlite::SqliteStore,
};
use unicode_normalization::UnicodeNormalization;

const MESSAGES: usize = 8_000;
const FUNCTION_CHARS: &str = "的了在是和與与及也就都而或跟";
const RANK_BAR: f64 = 0.8;

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2EBEF | 0x30000..=0x3134F)
}

fn normalize(text: &str) -> String {
    text.nfkc().collect::<String>().to_lowercase()
}

#[derive(Default)]
struct Parsed {
    runs: Vec<String>,
    singles: Vec<char>,
    ascii: Vec<String>,
}

fn parse(query: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let (mut run, mut token) = (String::new(), String::new());
    let flush_run = |run: &mut String, parsed: &mut Parsed| {
        match run.chars().count() {
            0 => {}
            1 => parsed.singles.push(run.chars().next().unwrap()),
            _ => parsed.runs.push(run.clone()),
        }
        run.clear();
    };
    for c in normalize(query).chars() {
        if is_cjk(c) {
            if !token.is_empty() {
                parsed.ascii.push(std::mem::take(&mut token));
            }
            run.push(c);
        } else {
            flush_run(&mut run, &mut parsed);
            if c.is_ascii_alphanumeric() {
                token.push(c);
            } else if !token.is_empty() {
                parsed.ascii.push(std::mem::take(&mut token));
            }
        }
    }
    flush_run(&mut run, &mut parsed);
    if !token.is_empty() {
        parsed.ascii.push(token);
    }
    parsed
}

fn ascii_tokens(normalized: &str) -> Vec<&str> {
    normalized
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect()
}

fn term_matches(tokens: &[&str], term: &str) -> bool {
    tokens.iter().any(|t| {
        if term.len() >= 3 {
            t.starts_with(term)
        } else {
            *t == term
        }
    })
}

/// Strict ground truth (the AND of all constraints).
fn matches_truth(p: &Parsed, text: &str) -> bool {
    let normalized = normalize(text);
    let tokens = ascii_tokens(&normalized);
    p.runs.iter().all(|r| normalized.contains(r.as_str()))
        && p.singles.iter().all(|c| text.contains(*c))
        && p.ascii.iter().all(|t| term_matches(&tokens, t))
}

/// Weaker condition every hit must satisfy (explains words-path extras).
fn explains_extra(p: &Parsed, text: &str) -> bool {
    let normalized = normalize(text);
    let tokens = ascii_tokens(&normalized);
    p.ascii.iter().all(|t| term_matches(&tokens, t))
        && p.singles.iter().all(|c| text.contains(*c))
        && p.runs
            .iter()
            .flat_map(|r| r.chars())
            .filter(|c| !FUNCTION_CHARS.contains(*c))
            .all(|c| normalized.contains(c))
}

#[derive(Clone, Default)]
struct Filter {
    chat: Option<i64>,
    sender: Option<i64>,
    from: Option<i64>,
    to: Option<i64>,
    deleted: bool,
}

impl Filter {
    fn admits(&self, m: &GenMessage) -> bool {
        (self.deleted || !m.deleted)
            && self.chat.is_none_or(|c| c == m.chat)
            && self.sender.is_none_or(|s| s == m.sender)
            && self.from.is_none_or(|f| m.ts >= f)
            && self.to.is_none_or(|t| m.ts < t)
    }

    fn engine(&self) -> MessageFilters {
        let time = |s: Option<i64>| s.map(|s| DateTime::<Utc>::from_timestamp(s, 0).unwrap());
        MessageFilters {
            chat_id: self.chat.map(|c| ChatId::from_marked(c).unwrap()),
            sender_id: self.sender.map(|s| SenderId::from_marked(s).unwrap()),
            time_range: TimeRange::new(time(self.from), time(self.to)).unwrap(),
            include_deleted: self.deleted,
        }
    }
}

struct Q {
    category: &'static str,
    text: &'static str,
    filter: Filter,
    /// Hit set must equal the truth (no scattered extras expected).
    exact: bool,
}

fn queries(corpus: &Corpus) -> Vec<Q> {
    let q = |category, text, exact| Q {
        category,
        text,
        filter: Filter::default(),
        exact,
    };
    let chat = Some(corpus.chats[0]);
    let sender = Some(corpus.senders[1]);
    let from = Some(corpus.start.timestamp() + 100 * 86_400);
    let to = Some((corpus.start + Duration::days(160)).timestamp());
    let f = |category, text, exact, filter| Q {
        category,
        text,
        filter,
        exact,
    };
    vec![
        q("zh common word", "今天", true),
        q("zh common word", "朋友", true),
        q("zh common word", "工作", true),
        q("zh common word", "咖啡", true),
        q("zh rare word", "饕餮", true),
        q("zh rare word", "饕餮大餐", false),
        q("zh rare word", "霹靂嬌娃", false),
        q("zh 2-char", "台北", true),
        q("zh 2-char", "高雄", true),
        q("zh 2-char", "新竹", true),
        q("zh substring 3+", "北咖啡", false),
        q("zh substring 3+", "靂嬌娃", false),
        q("zh substring 3+", "靂嬌", true),
        q("zh substring 3+", "料庫", true),
        q("zh substring 3+", "運站", true),
        q("zh substring 3+", "咖啡廳", false),
        q("single char common", "我", true),
        q("single char common", "的", true),
        q("single char common", "好", true),
        q("single char rare", "龘", true),
        q("single char rare", "犇", true),
        q("single char rare", "鱻", true),
        q("contiguous phrase", "台北咖啡", false),
        q("contiguous phrase", "重新啟動", false),
        q("contiguous phrase", "今天天氣", false),
        q("scattered words", "台北 咖啡", true),
        q("scattered words", "咖啡 台北", true),
        q("scattered words", "朋友 工作", true),
        q("english exact", "gitlab", true),
        q("english exact", "docker", true),
        q("english exact", "k8s", true),
        q("english exact", "ok", true),
        q("english prefix/plural", "benchmark", true),
        q("english prefix/plural", "benchmarks", true),
        q("english prefix/plural", "bench", true),
        q("english prefix/plural", "zxqwid", true),
        q("english prefix/plural", "zxqwidgets", true),
        q("mixed zh+en", "github 今天", true),
        q("mixed zh+en", "gitlab 咖啡", true),
        q("mixed zh+en", "benchmark 效能", true),
        q("fullwidth", "ＳＱＬｉｔｅ", true),
        q("fullwidth", "ＧＩＴＨＵＢ", true),
        q("fullwidth", "Ｄｏｃｋｅｒ 部署", true),
        q("no result", "魑魅魍魎", true),
        q("no result", "qqzzxxvv", true),
        q("no result", "鸞", true),
        q("no result", "饕餮 benchmark", true),
        f(
            "filter chat",
            "咖啡",
            true,
            Filter {
                chat,
                ..Filter::default()
            },
        ),
        f(
            "filter chat",
            "gitlab",
            true,
            Filter {
                chat,
                ..Filter::default()
            },
        ),
        f(
            "filter sender",
            "咖啡",
            true,
            Filter {
                sender,
                ..Filter::default()
            },
        ),
        f(
            "filter sender",
            "我",
            true,
            Filter {
                sender,
                ..Filter::default()
            },
        ),
        f(
            "filter time",
            "咖啡",
            true,
            Filter {
                from,
                to,
                ..Filter::default()
            },
        ),
        f(
            "filter time",
            "好 咖啡",
            true,
            Filter {
                from,
                to,
                ..Filter::default()
            },
        ),
        f(
            "filter time",
            "台北咖啡",
            false,
            Filter {
                from,
                to,
                ..Filter::default()
            },
        ),
        f(
            "filter combined",
            "台北",
            true,
            Filter {
                chat,
                sender,
                from,
                to,
                deleted: false,
            },
        ),
        f(
            "filter combined",
            "好",
            true,
            Filter {
                chat,
                from,
                to,
                ..Filter::default()
            },
        ),
        f(
            "include_deleted",
            "饕餮",
            true,
            Filter {
                deleted: true,
                ..Filter::default()
            },
        ),
        f(
            "include_deleted",
            "龘",
            true,
            Filter {
                deleted: true,
                ..Filter::default()
            },
        ),
        f(
            "include_deleted",
            "benchmark",
            true,
            Filter {
                deleted: true,
                ..Filter::default()
            },
        ),
        f(
            "include_deleted",
            "台北咖啡",
            false,
            Filter {
                deleted: true,
                ..Filter::default()
            },
        ),
        f(
            "include_deleted",
            "zxqwidget",
            true,
            Filter {
                deleted: true,
                chat: None,
                ..Filter::default()
            },
        ),
    ]
}

type Key = (i64, i64);

async fn fetch(store: &SqliteStore, q: &Q, sort: SearchSort) -> Vec<Key> {
    let mut hits = Vec::new();
    let mut offset = 0;
    let mut before = None;
    loop {
        let page = store
            .search(SearchMessagesQuery {
                text: q.text.to_owned(),
                filters: q.filter.engine(),
                before: before.take(),
                after: None,
                page_size: PageSize::new(1000).unwrap(),
                sort,
                offset,
            })
            .await
            .unwrap_or_else(|e| panic!("search {:?} failed: {e}", q.text));
        hits.extend(page.items.iter().map(|m| (m.chat_id.get(), m.id.get())));
        if !page.has_more {
            return hits;
        }
        match sort {
            SearchSort::Relevance => offset = page.next_offset.expect("next offset"),
            SearchSort::Time => before = Some(page.next_cursor.expect("next cursor")),
        }
    }
}

fn sample(corpus: &Corpus, keys: &[Key]) -> String {
    keys.iter()
        .take(3)
        .map(|k| {
            let m = corpus
                .messages
                .iter()
                .find(|m| (m.chat, m.id) == *k)
                .unwrap();
            let text: String = m.text.chars().take(50).collect();
            format!("    #{} {:?}", m.id, text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default)]
struct Totals {
    queries: usize,
    truth: usize,
    hits: usize,
    found: usize,
    rank_sum: f64,
    rank_n: usize,
    rank_min: f64,
    top1_ok: usize,
    top1_n: usize,
}

#[tokio::test]
async fn search_quality_against_ground_truth() {
    let corpus = corpus::generate(MESSAGES, corpus::SEED);
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("quality.db").display());
    let store = SqliteStore::connect(&url).await.unwrap();
    corpus::ingest(&store, &corpus).await;

    // Corpus sanity.
    let stats = corpus.stats();
    assert!(
        stats.vocabulary_size >= 3000,
        "vocabulary {}",
        stats.vocabulary_size
    );
    assert!(
        (0.03..0.08).contains(&(stats.edited as f64 / MESSAGES as f64)),
        "edited {}",
        stats.edited
    );
    assert!(
        (0.01..0.04).contains(&(stats.deleted as f64 / MESSAGES as f64)),
        "deleted {}",
        stats.deleted
    );
    for needle in &corpus.needles {
        let found = corpus
            .messages
            .iter()
            .filter(|m| normalize(&m.text).contains(&normalize(&needle.substring)))
            .count();
        if needle.exact {
            assert_eq!(found, needle.planted, "needle {:?}", needle.substring);
        } else {
            assert!(found >= needle.planted, "needle {:?}", needle.substring);
        }
    }
    let status = store.search_index_status().await.unwrap();
    assert_eq!(status.state, SearchIndexState::Ready);
    assert_eq!(status.indexed, status.total);

    let mut failures = Vec::new();
    let mut totals: BTreeMap<&str, Totals> = BTreeMap::new();
    for q in queries(&corpus) {
        let parsed = parse(q.text);
        let truth: BTreeSet<Key> = corpus
            .messages
            .iter()
            .filter(|m| q.filter.admits(m) && matches_truth(&parsed, &m.text))
            .map(|m| (m.chat, m.id))
            .collect();
        let relevance = fetch(&store, &q, SearchSort::Relevance).await;
        let by_time = fetch(&store, &q, SearchSort::Time).await;
        let hits: BTreeSet<Key> = relevance.iter().copied().collect();
        let label = format!("[{}] {:?} {}", q.category, q.text, describe(&q.filter));
        let mut problems = Vec::new();

        if hits.len() != relevance.len() {
            problems.push("duplicate hits in relevance paging".to_owned());
        }
        if by_time.iter().copied().collect::<BTreeSet<_>>() != hits
            || by_time.len() != relevance.len()
        {
            problems.push(format!(
                "sort modes disagree: relevance {} vs time {}",
                relevance.len(),
                by_time.len()
            ));
        }
        let ordered = by_time.windows(2).all(|w| {
            let key = |k: &Key| {
                let m = corpus
                    .messages
                    .iter()
                    .find(|m| (m.chat, m.id) == *k)
                    .unwrap();
                (m.ts, m.chat, m.id)
            };
            key(&w[0]) > key(&w[1])
        });
        if !ordered {
            problems.push("time sort is not strictly newest-first".to_owned());
        }
        if q.category != "no result" && truth.is_empty() {
            problems.push(
                "vacuous query: ground truth is empty (adjust the corpus or query)".to_owned(),
            );
        }
        let missing: Vec<Key> = truth.difference(&hits).copied().collect();
        if !missing.is_empty() {
            problems.push(format!(
                "MISSING {} of {}:\n{}",
                missing.len(),
                truth.len(),
                sample(&corpus, &missing)
            ));
        }
        let extras: Vec<Key> = hits.difference(&truth).copied().collect();
        let unexplained: Vec<Key> = extras
            .iter()
            .copied()
            .filter(|k| {
                let m = corpus
                    .messages
                    .iter()
                    .find(|m| (m.chat, m.id) == *k)
                    .unwrap();
                !q.filter.admits(m) || !explains_extra(&parsed, &m.text)
            })
            .collect();
        if !unexplained.is_empty() {
            problems.push(format!(
                "UNEXPLAINED EXTRAS {}:\n{}",
                unexplained.len(),
                sample(&corpus, &unexplained)
            ));
        }
        if q.exact && !extras.is_empty() {
            problems.push(format!(
                "EXTRAS {} on an exact query:\n{}",
                extras.len(),
                sample(&corpus, &extras)
            ));
        }

        let entry = totals.entry(q.category).or_default();
        entry.queries += 1;
        entry.truth += truth.len();
        entry.hits += hits.len();
        entry.found += truth.intersection(&hits).count();
        if !truth.is_empty() && !extras.is_empty() {
            let top1 = relevance.first().is_some_and(|k| truth.contains(k));
            entry.top1_n += 1;
            entry.top1_ok += usize::from(top1);
            let top10 = relevance
                .iter()
                .take(10)
                .filter(|k| truth.contains(*k))
                .count();
            let share = top10 as f64 / truth.len().min(10) as f64;
            entry.rank_sum += share;
            entry.rank_n += 1;
            entry.rank_min = if entry.rank_n == 1 {
                share
            } else {
                entry.rank_min.min(share)
            };
            if !top1 {
                problems.push(format!(
                    "RANK: top-1 is not an exact hit (extras {}, truth {})",
                    extras.len(),
                    truth.len()
                ));
            }
            if share < RANK_BAR {
                problems.push(format!(
                    "RANK: only {top10}/{} exact hits in top-10",
                    truth.len().min(10)
                ));
            }
        }
        if !problems.is_empty() {
            failures.push(format!("{label}\n  {}", problems.join("\n  ")));
        }
    }

    println!(
        "\nsearch quality on {} messages ({} deleted, {} edited, vocabulary {})",
        stats.messages, stats.deleted, stats.edited, stats.vocabulary_size
    );
    println!(
        "{:<24} {:>3} {:>7} {:>7} {:>8} {:>10} {:>8}",
        "category", "q", "truth", "hits", "recall", "precision", "rank@10"
    );
    for (category, t) in &totals {
        let ratio = |a: usize, b: usize| {
            if b == 0 {
                "n/a".to_owned()
            } else {
                format!("{:.1}%", 100.0 * a as f64 / b as f64)
            }
        };
        let rank = if t.rank_n == 0 {
            "n/a".to_owned()
        } else {
            format!("{:.2}", t.rank_sum / t.rank_n as f64)
        };
        println!(
            "{category:<24} {:>3} {:>7} {:>7} {:>8} {:>10} {:>8}",
            t.queries,
            t.truth,
            t.hits,
            ratio(t.found, t.truth),
            ratio(t.found, t.hits),
            rank,
        );
    }
    assert!(
        failures.is_empty(),
        "\n{} quality failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn describe(f: &Filter) -> String {
    let mut parts = Vec::new();
    if f.chat.is_some() {
        parts.push("chat");
    }
    if f.sender.is_some() {
        parts.push("sender");
    }
    if f.from.is_some() || f.to.is_some() {
        parts.push("time");
    }
    if f.deleted {
        parts.push("include_deleted");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("(+{})", parts.join(","))
    }
}
