//! Search speed benchmark (manual; use `--release`):
//!
//!     cargo run --release --example search_bench -- [--messages 100000] [--messages 1000000] \
//!         [--db PATH | SEARCH_BENCH_DB=PATH] [--iterations N] [--out target/search-bench.md] [--strict]
//!
//! Without `--db`, a deterministic generated corpus (`tests/support/corpus.rs`, the same one the
//! quality test uses) is ingested into a temp directory. With `--db` (or `SEARCH_BENCH_DB`) the
//! file is copied to a temp directory first; migrations and, if needed, an index rebuild run on
//! the COPY only, and the original is never opened. Soft budgets print PASS/WARN; the exit code
//! is non-zero for WARN only with `--strict`.

#[path = "../tests/support/mod.rs"]
mod support;

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::DateTime;
use sqlx::SqlitePool;
use tgarchive::{
    application::{
        MessageCursor, MessageFilters, MessageRepository, PageSize, SearchMessagesQuery,
        SearchSort, TimeRange,
    },
    domain::{ChatId, SenderId},
    infrastructure::persistence::sqlite::SqliteStore,
};

const PAGE: u16 = 30;

struct Args {
    sizes: Vec<usize>,
    db: Option<PathBuf>,
    iterations: usize,
    out: Option<PathBuf>,
    strict: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        sizes: Vec::new(),
        db: std::env::var_os("SEARCH_BENCH_DB").map(PathBuf::from),
        iterations: 15,
        out: None,
        strict: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .unwrap_or_else(|| panic!("{name} needs a value"))
        };
        match arg.as_str() {
            "--messages" => args.sizes.push(
                value("--messages")
                    .replace('_', "")
                    .parse()
                    .expect("--messages N"),
            ),
            "--db" => args.db = Some(PathBuf::from(value("--db"))),
            "--iterations" => {
                args.iterations = value("--iterations").parse().expect("--iterations N")
            }
            "--out" => args.out = Some(PathBuf::from(value("--out"))),
            "--strict" => args.strict = true,
            other => panic!("unknown argument {other}"),
        }
    }
    if args.sizes.is_empty() && args.db.is_none() {
        args.sizes.push(100_000);
    }
    args
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
    fn engine(&self) -> MessageFilters {
        let time = |s: Option<i64>| s.map(|s| DateTime::from_timestamp(s, 0).unwrap());
        MessageFilters {
            chat_id: self.chat.map(|c| ChatId::from_marked(c).unwrap()),
            sender_id: self.sender.map(|s| SenderId::from_marked(s).unwrap()),
            post_author: None,
            time_range: TimeRange::new(time(self.from), time(self.to)).unwrap(),
            include_deleted: self.deleted,
            exclude_bots: false,
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    First(SearchSort),
    /// Relevance page starting at this offset.
    Offset(u64),
    /// Time sort: walk this many 1000-item pages by cursor (untimed), then time the next page.
    TimeDeep(usize),
}

struct Work {
    name: String,
    text: &'static str,
    filter: Filter,
    kind: Kind,
    /// Unindexable single-character scan: documented worst case with a larger budget.
    like_scan: bool,
}

fn workload(filters: &Filter) -> Vec<Work> {
    let mut work = Vec::new();
    let mut both = |name: &str, text: &'static str, filter: Filter, like_scan: bool| {
        for (label, sort) in [("rel", SearchSort::Relevance), ("time", SearchSort::Time)] {
            work.push(Work {
                name: format!("{name} [{label}]"),
                text,
                filter: filter.clone(),
                kind: Kind::First(sort),
                like_scan,
            });
        }
    };
    let none = Filter::default();
    both("common word 今天", "今天", none.clone(), false);
    both("common word 咖啡", "咖啡", none.clone(), false);
    both("rare word 饕餮", "饕餮", none.clone(), false);
    both("2-char 台北", "台北", none.clone(), false);
    both("phrase 台北咖啡", "台北咖啡", none.clone(), false);
    both("substring 北咖啡", "北咖啡", none.clone(), false);
    both("scattered 台北 咖啡", "台北 咖啡", none.clone(), false);
    both("mixed github 今天", "github 今天", none.clone(), false);
    both("english gitlab", "gitlab", none.clone(), false);
    both("english prefix bench", "bench", none.clone(), false);
    both(
        "fullwidth ＳＱＬｉｔｅ",
        "ＳＱＬｉｔｅ",
        none.clone(),
        false,
    );
    both("no result 魑魅魍魎", "魑魅魍魎", none.clone(), false);
    both("no result english", "qqzzxxvv", none.clone(), false);
    both("single char common 我", "我", none.clone(), true);
    both(
        "single char rare 龘 (full LIKE scan)",
        "龘",
        none.clone(),
        true,
    );
    both(
        "single char no result 鸞 (full LIKE scan)",
        "鸞",
        none.clone(),
        true,
    );
    both(
        "filter chat 咖啡",
        "咖啡",
        Filter {
            chat: filters.chat,
            ..none.clone()
        },
        false,
    );
    both(
        "filter sender 咖啡",
        "咖啡",
        Filter {
            sender: filters.sender,
            ..none.clone()
        },
        false,
    );
    both(
        "filter time 咖啡",
        "咖啡",
        Filter {
            from: filters.from,
            to: filters.to,
            ..none.clone()
        },
        false,
    );
    both(
        "filter chat+sender+time 台北",
        "台北",
        filters.clone(),
        false,
    );
    both(
        "filter chat 龘 (LIKE + filter)",
        "龘",
        Filter {
            chat: filters.chat,
            ..none.clone()
        },
        true,
    );
    both(
        "include_deleted 饕餮",
        "饕餮",
        Filter {
            deleted: true,
            ..none.clone()
        },
        false,
    );
    both(
        "include_deleted 咖啡",
        "咖啡",
        Filter {
            deleted: true,
            ..none.clone()
        },
        false,
    );
    both(
        "include_deleted 龘 (full LIKE scan)",
        "龘",
        Filter {
            deleted: true,
            ..none.clone()
        },
        true,
    );
    for offset in [1_000, 20_000, 90_000] {
        work.push(Work {
            name: format!("deep relevance 今天 offset {offset}"),
            text: "今天",
            filter: none.clone(),
            kind: Kind::Offset(offset),
            like_scan: false,
        });
    }
    work.push(Work {
        name: "deep relevance 咖啡 offset 5000".into(),
        text: "咖啡",
        filter: none.clone(),
        kind: Kind::Offset(5_000),
        like_scan: false,
    });
    for pages in [10, 100] {
        work.push(Work {
            name: format!("deep time 今天 after {pages}k hits"),
            text: "今天",
            filter: none.clone(),
            kind: Kind::TimeDeep(pages),
            like_scan: false,
        });
    }
    work
}

fn query(
    w: &Work,
    sort: SearchSort,
    page: u16,
    offset: u64,
    before: Option<MessageCursor>,
) -> SearchMessagesQuery {
    SearchMessagesQuery {
        text: w.text.to_owned(),
        filters: w.filter.engine(),
        before,
        after: None,
        page_size: PageSize::new(page).unwrap(),
        sort,
        offset,
    }
}

/// Returns (items on the timed page, has_more).
async fn run_once(
    store: &SqliteStore,
    w: &Work,
    prepared: Option<&MessageCursor>,
) -> (Duration, usize, bool) {
    let q = match w.kind {
        Kind::First(sort) => query(w, sort, PAGE, 0, None),
        Kind::Offset(offset) => query(w, SearchSort::Relevance, PAGE, offset, None),
        Kind::TimeDeep(_) => query(w, SearchSort::Time, PAGE, 0, prepared.cloned()),
    };
    let started = Instant::now();
    let page = store.search(q).await.expect("search");
    (started.elapsed(), page.items.len(), page.has_more)
}

struct Row {
    name: String,
    items: usize,
    has_more: bool,
    min: f64,
    p50: f64,
    p95: f64,
    max: f64,
    budget: f64,
    pass: bool,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn budget(messages: i64, like_scan: bool) -> f64 {
    let normal = if messages <= 200_000 { 100.0 } else { 500.0 };
    if like_scan { normal * 10.0 } else { normal }
}

async fn measure(store: &SqliteStore, w: &Work, iterations: usize, messages: i64) -> Row {
    let mut cursor = None;
    if let Kind::TimeDeep(pages) = w.kind {
        for _ in 0..pages {
            let page = store
                .search(query(w, SearchSort::Time, 1000, 0, cursor.take()))
                .await
                .expect("search");
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
    }
    let (first, items, has_more) = run_once(store, w, cursor.as_ref()).await;
    let iterations = if first > Duration::from_secs(1) {
        iterations.min(5)
    } else {
        iterations
    };
    run_once(store, w, cursor.as_ref()).await;
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        samples.push(ms(run_once(store, w, cursor.as_ref()).await.0));
    }
    samples.sort_by(f64::total_cmp);
    let pick = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    let budget = budget(messages, w.like_scan);
    Row {
        name: w.name.clone(),
        items,
        has_more,
        min: samples[0],
        p50: pick(0.5),
        p95: pick(0.95),
        max: *samples.last().unwrap(),
        budget,
        pass: pick(0.95) < budget,
    }
}

async fn scalar(pool: &SqlitePool, sql: &str) -> Option<i64> {
    sqlx::query_scalar::<_, Option<i64>>(sql)
        .fetch_one(pool)
        .await
        .ok()
        .flatten()
}

fn human(bytes: i64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

fn machine() -> String {
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    let model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into());
    let mem = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<i64>().ok())
        })
        .map_or_else(
            || "unknown".into(),
            |kb| format!("{:.1} GiB", kb as f64 / 1_048_576.0),
        );
    let profile = if cfg!(debug_assertions) {
        "DEBUG build (numbers are not meaningful; use --release)"
    } else {
        "release"
    };
    format!("{cpus} CPUs ({model}), RAM {mem}, {profile}")
}

async fn benchmark(
    label: &str,
    store: &SqliteStore,
    db_file: &Path,
    ingest: Option<Duration>,
    iterations: usize,
    report: &mut String,
    warns: &mut usize,
) {
    let url = format!("sqlite://{}", db_file.display());
    eprintln!("[{label}] rebuilding index ...");
    let started = Instant::now();
    let rebuilt = store
        .rebuild_search_index(1000, |_| {})
        .await
        .expect("rebuild");
    let rebuild = started.elapsed();
    let pool = SqlitePool::connect(&url).await.unwrap();
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&pool)
        .await
        .unwrap();
    let size = std::fs::metadata(db_file).map_or(0, |m| m.len() as i64);
    let fts = scalar(
        &pool,
        "SELECT SUM(pgsize) FROM dbstat WHERE name LIKE 'messages_fts%'",
    )
    .await
    .unwrap_or(-1);
    let messages = scalar(&pool, "SELECT COUNT(*) FROM messages")
        .await
        .unwrap_or(0);
    let deleted = scalar(&pool, "SELECT COUNT(*) FROM messages WHERE is_deleted=1")
        .await
        .unwrap_or(0);
    let chars = scalar(&pool, "SELECT SUM(LENGTH(text)) FROM messages")
        .await
        .unwrap_or(0);
    let top = |col: &str| {
        format!(
            "SELECT {col} FROM messages WHERE {col} IS NOT NULL GROUP BY {col} ORDER BY COUNT(*) DESC LIMIT 1"
        )
    };
    let min_ts = scalar(&pool, "SELECT MIN(timestamp) FROM messages").await;
    let max_ts = scalar(&pool, "SELECT MAX(timestamp) FROM messages").await;
    let filters = Filter {
        chat: scalar(&pool, &top("chat_id")).await,
        sender: scalar(&pool, &top("sender_id")).await,
        from: min_ts.zip(max_ts).map(|(lo, hi)| lo + (hi - lo) * 2 / 5),
        to: min_ts
            .zip(max_ts)
            .map(|(lo, hi)| lo + (hi - lo) * 3 / 5 + 1),
        deleted: false,
    };
    pool.close().await;

    let _ = writeln!(report, "## {label}\n");
    let _ = writeln!(report, "| corpus | value |\n|---|---|");
    let _ = writeln!(
        report,
        "| messages | {messages} ({deleted} soft-deleted, {} indexed) |",
        rebuilt.indexed
    );
    let _ = writeln!(
        report,
        "| avg text length | {:.1} chars |",
        chars as f64 / messages.max(1) as f64
    );
    match ingest {
        Some(t) => {
            let _ = writeln!(
                report,
                "| ingest (incl. incremental indexing) | {:.1} s ({:.0} msg/s) |",
                t.as_secs_f64(),
                messages as f64 / t.as_secs_f64()
            );
        }
        None => {
            let _ = writeln!(report, "| ingest | n/a (existing DB copy) |");
        }
    }
    let _ = writeln!(
        report,
        "| full rebuild-index | {:.1} s |",
        rebuild.as_secs_f64()
    );
    let _ = writeln!(report, "| DB size (after checkpoint) | {} |", human(size));
    let _ = writeln!(
        report,
        "| FTS tables | {} ({:.0}% of DB) |\n",
        human(fts),
        100.0 * fts as f64 / size.max(1) as f64
    );

    let normal = budget(messages, false);
    let _ = writeln!(
        report,
        "Page size {PAGE}, {iterations} iterations after 2 warm-ups (5 if one run > 1 s). Soft budget p95 < {normal:.0} ms, < {:.0} ms for full single-character LIKE scans.\n",
        budget(messages, true)
    );
    let _ = writeln!(
        report,
        "| query | items | more | min ms | p50 ms | p95 ms | max ms | budget | |\n|---|---:|:-:|---:|---:|---:|---:|---:|:-:|"
    );
    for w in workload(&filters) {
        let row = measure(store, &w, iterations, messages).await;
        eprintln!(
            "[{label}] {:<55} p50 {:>8.2} ms p95 {:>8.2} ms",
            row.name, row.p50, row.p95
        );
        *warns += usize::from(!row.pass);
        let _ = writeln!(
            report,
            "| {} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.0} | {} |",
            row.name,
            row.items,
            if row.has_more { "y" } else { "n" },
            row.min,
            row.p50,
            row.p95,
            row.max,
            row.budget,
            if row.pass { "PASS" } else { "WARN" }
        );
    }
    let _ = writeln!(report);
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let mut report = String::new();
    let mut warns = 0;
    let _ = writeln!(report, "# Search benchmark\n\nMachine: {}\n", machine());

    for &size in &args.sizes {
        let dir = tempfile::tempdir().unwrap();
        let db_file = dir.path().join("bench.db");
        let url = format!("sqlite://{}", db_file.display());
        eprintln!("[{size}] generating corpus ...");
        let corpus = support::corpus::generate(size, support::corpus::SEED);
        let stats = corpus.stats();
        let store = SqliteStore::connect(&url).await.unwrap();
        eprintln!("[{size}] ingesting ...");
        let started = Instant::now();
        support::corpus::ingest(&store, &corpus).await;
        let ingest = started.elapsed();
        let _ = writeln!(
            report,
            "Generated corpus {size}: seed {:#x}, vocabulary {} entries, {} edited, {} soft-deleted, avg {:.1} chars (max {}), {} chats, {} senders, one year of timestamps.\n",
            support::corpus::SEED,
            stats.vocabulary_size,
            stats.edited,
            stats.deleted,
            stats.avg_chars,
            stats.max_chars,
            corpus.chats.len(),
            corpus.senders.len()
        );
        drop(corpus);
        benchmark(
            &format!("{size} generated messages"),
            &store,
            &db_file,
            Some(ingest),
            args.iterations,
            &mut report,
            &mut warns,
        )
        .await;
        store.close().await;
    }

    if let Some(source) = &args.db {
        let dir = tempfile::tempdir().unwrap();
        let db_file = dir.path().join("bench.db");
        std::fs::copy(source, &db_file).expect("copy --db to a temp dir");
        for ext in ["-wal", "-shm"] {
            let mut sidecar = source.as_os_str().to_owned();
            sidecar.push(ext);
            if Path::new(&sidecar).exists() {
                let mut target = db_file.as_os_str().to_owned();
                target.push(ext);
                std::fs::copy(&sidecar, target).expect("copy sidecar");
            }
        }
        let store = SqliteStore::connect(&format!("sqlite://{}", db_file.display()))
            .await
            .unwrap();
        benchmark(
            "copy of --db",
            &store,
            &db_file,
            None,
            args.iterations,
            &mut report,
            &mut warns,
        )
        .await;
        store.close().await;
    }

    let _ = writeln!(
        report,
        "Result: {}",
        if warns == 0 {
            "all budgets PASS".to_owned()
        } else {
            format!("{warns} WARN")
        }
    );
    print!("{report}");
    if let Some(out) = &args.out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(out, &report).unwrap();
    }
    if args.strict && warns > 0 {
        std::process::exit(1);
    }
}
