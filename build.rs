//! Generates the table of embedded web UI files from `web/dist` (committed, so Node is not needed).
use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest.join("web/dist");
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    collect(&dist, &mut files);
    files.sort();
    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for file in files {
        let rel = file
            .strip_prefix(&dist)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let _ = writeln!(
            code,
            "    ({rel:?}, include_bytes!({:?})),",
            file.to_string_lossy()
        );
    }
    code.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("web_assets.rs"),
        code,
    )
    .unwrap();
}
