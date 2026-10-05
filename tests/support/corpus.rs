//! Deterministic, realistic Traditional Chinese chat corpus with planted needles.
//!
//! The same `(messages, seed)` always yields the same corpus. Text is built from a Zipf-sampled
//! vocabulary (see `vocab.rs`): common words, function characters, Taiwanese places (+compounds),
//! zh/en tech terms, person-like names, slang, emoji, URLs and numbers, with variable message
//! length, mixed zh/en, fullwidth characters and ~3% Simplified messages. ~5% of messages are
//! edited (created with other text, then updated), ~2% soft-deleted, spread over several chats,
//! senders and one year. Needles are planted *after* edits, so their counts are exact.

use chrono::{DateTime, Duration, TimeZone, Utc};
use tgarchive::{
    application::{ArchiveWriter, IngestBatch, IngestRecord, MessageSource},
    domain::{
        Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, Sender, SenderId, SenderKind,
    },
    infrastructure::persistence::sqlite::SqliteStore,
};

use super::vocab::*;

pub const SEED: u64 = 0x7467_6172_6368_6976;
const CHAT_COUNT: usize = 6;
const SENDER_COUNT: usize = 40;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    pub fn range(&mut self, low: usize, high: usize) -> usize {
        low + self.below(high - low + 1)
    }

    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// Zipf sampler over ranks `0..n` (rank 0 most frequent), exponent `s`.
struct Zipf {
    cumulative: Vec<f64>,
}

impl Zipf {
    fn new(n: usize, s: f64) -> Self {
        let mut total = 0.0;
        let cumulative = (0..n)
            .map(|rank| {
                total += 1.0 / ((rank + 1) as f64).powf(s);
                total
            })
            .collect();
        Self { cumulative }
    }

    fn sample(&self, rng: &mut Rng) -> usize {
        let target = rng.unit() * self.cumulative.last().copied().unwrap_or(1.0);
        self.cumulative
            .partition_point(|&c| c < target)
            .min(self.cumulative.len() - 1)
    }
}

struct Category {
    weight: f64,
    words: Vec<String>,
    zipf: Zipf,
}

pub struct Vocabulary {
    categories: Vec<Category>,
    total_weight: f64,
}

impl Vocabulary {
    pub fn build(rng: &mut Rng) -> Self {
        let split = |list: &str| -> Vec<String> {
            list.split_whitespace().map(ToOwned::to_owned).collect()
        };
        let places: Vec<String> = split(PLACES);
        let suffixes = split(PLACE_SUFFIX);
        let mut place_words = places.clone();
        for place in &places {
            for suffix in &suffixes {
                place_words.push(format!("{place}{suffix}"));
            }
        }
        let surnames: Vec<char> = SURNAMES
            .split_whitespace()
            .filter_map(|s| s.chars().next())
            .collect();
        let given: Vec<char> = GIVEN_CHARS
            .split_whitespace()
            .filter_map(|s| s.chars().next())
            .collect();
        let mut names = Vec::new();
        for s in &surnames {
            for a in &given {
                names.push(format!("{s}{a}"));
                for b in given.iter().take(12) {
                    names.push(format!("{s}{a}{b}"));
                }
            }
        }
        rng.shuffle(&mut names);
        names.truncate(1400);
        for prefix in NICK_PREFIX.split_whitespace() {
            for c in NICK_CHARS.split_whitespace() {
                names.push(format!("{prefix}{c}"));
            }
        }
        names.extend(split(EN_NAMES));

        let domains = split(DOMAINS);
        let url_words = split(URL_WORDS);
        let mut urls = Vec::new();
        for i in 0..1500 {
            let domain = &domains[rng.below(domains.len())];
            let word = &url_words[rng.below(url_words.len())];
            urls.push(format!(
                "https://{domain}/{word}/{}",
                1000 + rng.below(90_000) + i
            ));
        }

        let mut numbers = Vec::new();
        for _ in 0..800 {
            numbers.push(match rng.below(6) {
                0 => rng.below(100).to_string(),
                1 => (100 + rng.below(9900)).to_string(),
                2 => format!("{:02}:{:02}", rng.below(24), rng.below(60)),
                3 => format!("NT${}", 20 + rng.below(3000)),
                4 => format!("{}.{}", rng.below(20), rng.below(100)),
                _ => format!("2025/{}/{}", 1 + rng.below(12), 1 + rng.below(28)),
            });
        }

        let spec: Vec<(f64, Vec<String>)> = vec![
            (30.0, split(COMMON_ZH)),
            (18.0, split(FUNCTION)),
            (6.0, place_words),
            (6.0, split(TECH_ZH)),
            (7.0, split(TECH_EN)),
            (6.0, split(EN_WORDS)),
            (5.0, split(SLANG)),
            (3.0, split(EMOJI)),
            (4.0, names),
            (1.5, urls),
            (3.0, numbers),
        ];
        let total_weight = spec.iter().map(|(w, _)| w).sum();
        let categories = spec
            .into_iter()
            .map(|(weight, words)| Category {
                weight,
                zipf: Zipf::new(words.len(), 1.0),
                words,
            })
            .collect();
        Self {
            categories,
            total_weight,
        }
    }

    pub fn size(&self) -> usize {
        self.categories.iter().map(|c| c.words.len()).sum()
    }

    fn token(&self, rng: &mut Rng) -> &str {
        let mut target = rng.unit() * self.total_weight;
        for category in &self.categories {
            if target < category.weight {
                return &category.words[category.zipf.sample(rng)];
            }
            target -= category.weight;
        }
        let last = self.categories.last().unwrap();
        &last.words[0]
    }
}

fn is_ascii_alnum_edge(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric())
}

fn to_fullwidth(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '!'..='~' => char::from_u32(c as u32 - 0x20 + 0xFF00).unwrap_or(c),
            _ => c,
        })
        .collect()
}

fn to_simplified(text: &str) -> String {
    text.chars()
        .map(|c| {
            SIMPLIFIED
                .iter()
                .find(|(t, _)| *t == c)
                .map_or(c, |(_, s)| *s)
        })
        .collect()
}

fn sentence(vocab: &Vocabulary, rng: &mut Rng, tokens: usize) -> String {
    let mut out = String::new();
    for _ in 0..tokens {
        let mut token = vocab.token(rng).to_owned();
        if token.is_ascii() && token.chars().any(|c| c.is_ascii_alphabetic()) && rng.chance(0.03) {
            token = to_fullwidth(&token);
        }
        if rng.chance(0.015) {
            token = format!("（{token}）");
        }
        let space = match (out.chars().last(), token.chars().next()) {
            (Some(a), Some(b)) if a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric() => true,
            (last, first) if is_ascii_alnum_edge(last) || is_ascii_alnum_edge(first) => {
                !out.is_empty() && rng.chance(0.4)
            }
            _ => false,
        };
        if space {
            out.push(' ');
        }
        out.push_str(&token);
    }
    out
}

fn message_text(vocab: &Vocabulary, rng: &mut Rng) -> String {
    let class = rng.unit();
    let (sentences, tokens) = if class < 0.30 {
        (1, rng.range(1, 2))
    } else if class < 0.72 {
        (1, rng.range(3, 8))
    } else if class < 0.95 {
        (rng.range(2, 3), rng.range(4, 9))
    } else {
        (rng.range(4, 8), rng.range(5, 12))
    };
    let zh_marks: Vec<&str> = MARKS_ZH.split_whitespace().collect();
    let ascii_marks: Vec<&str> = MARKS_ASCII.split_whitespace().collect();
    let mut text = String::new();
    for index in 0..sentences {
        if index > 0 {
            text.push(if rng.chance(0.1) { '\n' } else { ' ' });
        }
        text.push_str(&sentence(vocab, rng, tokens));
        let r = rng.unit();
        if r < 0.5 {
            text.push_str(zh_marks[rng.below(zh_marks.len())]);
        } else if r < 0.6 {
            text.push_str(ascii_marks[rng.below(ascii_marks.len())]);
        }
    }
    if rng.chance(0.03) {
        text = to_simplified(&text);
    }
    text
}

#[derive(Debug, Clone)]
pub struct GenMessage {
    pub chat: i64,
    pub id: i64,
    pub sender: i64,
    pub ts: i64,
    pub text: String,
    /// Text the message was created with when it was later edited to `text`.
    pub original: Option<String>,
    pub deleted: bool,
}

/// A planted substring with the exact number of messages (any deletion state) containing it.
#[derive(Debug, Clone)]
pub struct Needle {
    pub substring: String,
    pub planted: usize,
    /// Cannot occur by chance in the random text, so the corpus count must equal `planted`.
    pub exact: bool,
}

pub struct Corpus {
    pub messages: Vec<GenMessage>,
    pub chats: Vec<i64>,
    pub senders: Vec<i64>,
    pub needles: Vec<Needle>,
    pub vocabulary_size: usize,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

pub struct CorpusStats {
    pub messages: usize,
    pub deleted: usize,
    pub edited: usize,
    pub total_chars: usize,
    pub avg_chars: f64,
    pub max_chars: usize,
    pub vocabulary_size: usize,
}

impl Corpus {
    pub fn stats(&self) -> CorpusStats {
        let total_chars: usize = self.messages.iter().map(|m| m.text.chars().count()).sum();
        CorpusStats {
            messages: self.messages.len(),
            deleted: self.messages.iter().filter(|m| m.deleted).count(),
            edited: self
                .messages
                .iter()
                .filter(|m| m.original.is_some())
                .count(),
            total_chars,
            avg_chars: total_chars as f64 / self.messages.len().max(1) as f64,
            max_chars: self
                .messages
                .iter()
                .map(|m| m.text.chars().count())
                .max()
                .unwrap_or(0),
            vocabulary_size: self.vocabulary_size,
        }
    }
}

struct Plant {
    texts: &'static [&'static str],
    count: usize,
    deleted: usize,
}

/// Planted messages: each entry is a group of templates; the i-th planted message of a plant uses
/// template `i % len`; the first `deleted` of them are soft-deleted.
const PLANTS: &[Plant] = &[
    Plant {
        texts: &["今天去吃了一頓饕餮大餐，超滿足", "饕餮大餐真的太豐盛了"],
        count: 6,
        deleted: 2,
    },
    Plant {
        texts: &["他是個饕餮客"],
        count: 3,
        deleted: 0,
    },
    Plant {
        texts: &["龘這個字怎麼唸", "名字裡有個龘字"],
        count: 3,
        deleted: 1,
    },
    Plant {
        texts: &["犇是三個牛", "那家店叫犇"],
        count: 5,
        deleted: 0,
    },
    Plant {
        texts: &["鱻很少見"],
        count: 1,
        deleted: 0,
    },
    Plant {
        texts: &["霹靂嬌娃重播了", "我最愛看霹靂嬌娃"],
        count: 4,
        deleted: 0,
    },
    // contiguous 台北咖啡
    Plant {
        texts: &["約在台北咖啡廳見面", "台北咖啡好喝嗎", "這家台北咖啡店很讚"],
        count: 30,
        deleted: 2,
    },
    // scattered: both words present, never adjacent
    Plant {
        texts: &[
            "台北的朋友推薦這家咖啡",
            "咖啡豆是我在台北的朋友買的",
            "我在台北的朋友家喝了好多咖啡",
        ],
        count: 30,
        deleted: 2,
    },
    Plant {
        texts: &[
            "run the benchmark on the new server",
            "this benchmark is flaky",
        ],
        count: 12,
        deleted: 1,
    },
    Plant {
        texts: &["the benchmarks look good", "two benchmarks failed today"],
        count: 8,
        deleted: 0,
    },
    Plant {
        texts: &["benchmarking takes forever"],
        count: 4,
        deleted: 0,
    },
    Plant {
        texts: &["benchmark效能測試結果出爐", "新的benchmark 效能提升兩倍"],
        count: 5,
        deleted: 0,
    },
    Plant {
        texts: &["zxqwidget is broken again", "who owns zxqwidget?"],
        count: 4,
        deleted: 2,
    },
    Plant {
        texts: &["zxqwidgets everywhere"],
        count: 2,
        deleted: 0,
    },
];

/// Substrings whose planted count is verified by the quality test; `exact` ones cannot occur in
/// the random text, the others may also occur by chance (counted as `>=`).
const NEEDLES: &[(&str, bool)] = &[
    ("饕餮", true),
    ("龘", true),
    ("犇", true),
    ("鱻", true),
    ("霹靂嬌娃", true),
    ("台北咖啡", false),
    ("台北的朋友", false),
    ("benchmark", true),
    ("benchmarks", true),
    ("zxqwidget", true),
    ("zxqwidgets", true),
    ("benchmark效能", true),
];

pub fn generate(count: usize, seed: u64) -> Corpus {
    assert!(
        count >= 1000,
        "needle planting needs at least 1000 messages"
    );
    let mut rng = Rng::new(seed);
    let vocab = Vocabulary::build(&mut rng);

    let chats: Vec<i64> = (0..CHAT_COUNT)
        .map(|i| {
            ChatId::from_telegram(ChatKind::Group, 1000 + i as i64)
                .unwrap()
                .get()
        })
        .collect();
    let senders: Vec<i64> = (0..SENDER_COUNT)
        .map(|i| {
            SenderId::from_telegram(SenderKind::User, 5000 + i as i64)
                .unwrap()
                .get()
        })
        .collect();
    let chat_zipf = Zipf::new(CHAT_COUNT, 0.8);
    let sender_zipf = Zipf::new(SENDER_COUNT, 0.9);

    let start = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
    let span = 365 * 86_400;
    let end = start + Duration::seconds(span);
    let mut messages: Vec<GenMessage> = (0..count)
        .map(|i| {
            let text = message_text(&vocab, &mut rng);
            let edited = rng.chance(0.05);
            GenMessage {
                chat: chats[chat_zipf.sample(&mut rng)],
                id: i as i64 + 1,
                sender: senders[sender_zipf.sample(&mut rng)],
                ts: start.timestamp() + (i as i64 * span) / count as i64 + rng.below(30) as i64,
                original: edited.then(|| message_text(&vocab, &mut rng)),
                text,
                deleted: rng.chance(0.02),
            }
        })
        .collect();

    let mut used = std::collections::HashSet::new();
    let mut planted_texts: Vec<String> = Vec::new();
    for plant in PLANTS {
        for n in 0..plant.count {
            let mut index = rng.below(count);
            while !used.insert(index) {
                index = rng.below(count);
            }
            let message = &mut messages[index];
            message.text = plant.texts[n % plant.texts.len()].to_owned();
            message.original = None;
            message.deleted = n < plant.deleted;
            planted_texts.push(message.text.to_lowercase());
        }
    }
    let needles = NEEDLES
        .iter()
        .map(|(substring, exact)| Needle {
            substring: (*substring).to_owned(),
            planted: planted_texts
                .iter()
                .filter(|t| t.contains(substring))
                .count(),
            exact: *exact,
        })
        .collect();

    Corpus {
        messages,
        chats,
        senders,
        needles,
        vocabulary_size: vocab.size(),
        start,
        end,
    }
}

fn to_message(m: &GenMessage, text: &str, edited_at: Option<i64>) -> Message {
    let at = DateTime::from_timestamp(m.ts, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(m.id).unwrap(),
        chat_id: ChatId::from_marked(m.chat).unwrap(),
        sender_id: Some(SenderId::from_marked(m.sender).unwrap()),
        timestamp: at,
        edited_at: edited_at.map(|s| DateTime::from_timestamp(s, 0).unwrap()),
        collected_at: at,
        text: Some(text.to_owned()),
        reply_to: None,
        attachments: Vec::new(),
    }
}

/// Writes the corpus through the real ingest path: all creations first (history), then edits
/// (realtime updates) and soft deletes, so the index is maintained incrementally.
pub async fn ingest(store: &SqliteStore, corpus: &Corpus) {
    const BATCH: usize = 1000;
    let record = |event, source| IngestRecord { event, source };
    let chats = corpus
        .chats
        .iter()
        .enumerate()
        .map(|(i, id)| Chat {
            id: ChatId::from_marked(*id).unwrap(),
            kind: ChatKind::Group,
            title: Some(format!("chat {i}")),
            username: None,
            tracked: false,
        })
        .collect();
    let senders = corpus
        .senders
        .iter()
        .enumerate()
        .map(|(i, id)| Sender {
            id: SenderId::from_marked(*id).unwrap(),
            kind: SenderKind::User,
            display_name: Some(format!("user {i}")),
            username: None,
            is_bot: None,
        })
        .collect();
    let mut first = Some((chats, senders));
    for chunk in corpus.messages.chunks(BATCH) {
        let (chats, senders) = first.take().unwrap_or_default();
        store
            .write_batch(IngestBatch {
                chats,
                senders,
                records: chunk
                    .iter()
                    .map(|m| {
                        let text = m.original.as_deref().unwrap_or(&m.text);
                        record(
                            MessageEvent::Created(to_message(m, text, None)),
                            MessageSource::History,
                        )
                    })
                    .collect(),
                ..IngestBatch::default()
            })
            .await
            .unwrap();
    }
    let follow_ups: Vec<&GenMessage> = corpus
        .messages
        .iter()
        .filter(|m| m.original.is_some() || m.deleted)
        .collect();
    for chunk in follow_ups.chunks(BATCH) {
        let mut records = Vec::new();
        for m in chunk {
            if m.original.is_some() {
                records.push(record(
                    MessageEvent::Updated(to_message(m, &m.text, Some(m.ts + 60))),
                    MessageSource::Realtime,
                ));
            }
            if m.deleted {
                records.push(record(
                    MessageEvent::Deleted {
                        chat_id: ChatId::from_marked(m.chat).unwrap(),
                        message_id: MessageId::new(m.id).unwrap(),
                        deleted_at: DateTime::from_timestamp(m.ts + 120, 0).unwrap(),
                    },
                    MessageSource::Realtime,
                ));
            }
        }
        store
            .write_batch(IngestBatch {
                records,
                ..IngestBatch::default()
            })
            .await
            .unwrap();
    }
}
