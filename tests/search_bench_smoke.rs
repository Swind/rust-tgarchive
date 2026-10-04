//! Keeps `examples/search_bench.rs` from rotting: runs it on a small corpus.
//! `cargo test --release --test search_bench_smoke -- --ignored --nocapture`

use std::process::Command;

#[test]
#[ignore = "builds and runs the release benchmark example (about a minute)"]
fn search_bench_example_runs_on_a_small_corpus() {
    let out = std::env::temp_dir().join(format!("search-bench-smoke-{}.md", std::process::id()));
    let output = Command::new(env!("CARGO"))
        .args([
            "run",
            "--offline",
            "--release",
            "--example",
            "search_bench",
            "--",
        ])
        .args(["--messages", "20000", "--iterations", "3", "--out"])
        .arg(&out)
        .output()
        .expect("run cargo");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("## 20000 generated messages"), "{stdout}");
    assert!(stdout.contains("common word 今天 [rel]"), "{stdout}");
    assert!(std::fs::read_to_string(&out).unwrap().contains("Result:"));
    let _ = std::fs::remove_file(out);
}
