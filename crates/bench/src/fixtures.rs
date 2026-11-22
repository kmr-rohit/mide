//! Deterministic test data, cached in `target/bench-data` between runs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bench-data");
    fs::create_dir_all(&d).expect("create bench-data");
    d.canonicalize().expect("bench-data path")
}

/// A tiny deterministic generator (no `rand` dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// `lines` lines of plausible Rust.
pub fn big_rust_file(dir: &Path, lines: usize) -> PathBuf {
    let path = dir.join(format!("big-{lines}.rs"));
    if fs::metadata(&path).is_ok() {
        return path;
    }
    let mut rng = Lcg(42);
    let mut out = std::io::BufWriter::new(fs::File::create(&path).expect("create fixture"));
    for i in 0..lines {
        let r = rng.next();
        let line = match i % 8 {
            0 => format!("fn function_{i}(x: u32) -> u32 {{"),
            1 => format!("    let y = x * {} + {};", r % 97, r % 1000),
            2 => format!("    // note {i}: this is a comment about the next step"),
            3 => format!("    let name = \"item-{}\".to_string();", r % 5000),
            4 => format!("    if y > {} && name.len() < {} {{ return y; }}", r % 500, r % 50),
            5 => format!("    for k in 0..{} {{ println!(\"{{}}\", k + y); }}", r % 20),
            6 => format!("    y.wrapping_add({})", r % 10_000),
            _ => "}\n".to_owned(),
        };
        writeln!(out, "{line}").expect("write fixture");
    }
    path
}

/// `n` realistic-looking relative paths.
pub fn paths(_dir: &Path, n: usize) -> Vec<String> {
    let mut rng = Lcg(7);
    let parts = ["src", "components", "button", "index", "main", "utils", "tests", "core", "ui", "lib", "models", "views", "api", "config", "node_modules", "target", "docs"];
    (0..n)
        .map(|i| {
            let depth = 2 + (rng.next() % 5) as usize;
            let mut p: Vec<String> = (0..depth).map(|_| parts[(rng.next() as usize) % parts.len()].to_owned()).collect();
            p.push(format!("file_{i}.{}", ["rs", "ts", "py", "md", "json"][(rng.next() % 5) as usize]));
            p.join("/")
        })
        .collect()
}

/// A directory tree with `n` small files, 40 per directory.
pub fn project_tree(dir: &Path, n: usize) -> PathBuf {
    let root = dir.join(format!("tree-{n}"));
    if root.is_dir() {
        return root;
    }
    for i in 0..n {
        let d = root.join(format!("pkg{}/mod{}", i / 400, (i / 40) % 10));
        fs::create_dir_all(&d).expect("mkdir");
        fs::write(d.join(format!("f{i}.txt")), "x").expect("write");
    }
    root
}
