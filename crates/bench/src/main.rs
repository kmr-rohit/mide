//! Performance benchmarks for mide's engine crates, with explicit budgets.
//!
//!     cargo run --release -p mide-bench                 # everything, 1M-line fixture
//!     cargo run --release -p mide-bench -- --quick      # 100k lines, ~15 s
//!     cargo run --release -p mide-bench -- --repo ~/code/big-repo   # include git numbers
//!     cargo run --release -p mide-bench -- --filter search          # only matching names
//!     cargo run --release -p mide-bench -- --strict     # exit 1 if any budget is missed
//!     cargo run --release -p mide-bench -- --json out.json   # machine-readable results
//!
//! Budgets come from what a 60 Hz editor can spend: a keystroke should cost far
//! less than one 16 ms frame, and nothing the user triggers should freeze the UI.

mod fixtures;
mod report;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mide_core::search::{self, Matcher, Query};
use mide_core::{Buffer, Selection};
use mide_files::FileTree;
use mide_fuzzy::{Fuzzy, Kind};
use mide_syntax::{DocHighlighter, Syntaxes, Theme};
use mide_term::{Size, SpawnOptions, Terminal};

use report::Report;

struct Opts {
    lines: usize,
    repo: Option<PathBuf>,
    filter: Option<String>,
    strict: bool,
    json: Option<PathBuf>,
}

fn parse_args() -> Opts {
    let mut o = Opts { lines: 1_000_000, repo: None, filter: None, strict: false, json: None };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--quick" => o.lines = 100_000,
            "--lines" => o.lines = args.next().and_then(|v| v.parse().ok()).unwrap_or(o.lines),
            "--repo" => o.repo = args.next().map(PathBuf::from),
            "--filter" => o.filter = args.next(),
            "--strict" => o.strict = true,
            "--json" => o.json = args.next().map(PathBuf::from),
            other => {
                eprintln!("unknown option {other}; see the header of crates/bench/src/main.rs");
                std::process::exit(2);
            }
        }
    }
    o
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Run `f` `iters` times and report median / p95 in milliseconds.
fn measure<R>(iters: usize, mut f: impl FnMut() -> R) -> (f64, f64) {
    let mut samples: Vec<f64> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            ms(t.elapsed())
        })
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| samples[((samples.len() as f64 - 1.0) * p).round() as usize];
    (pct(0.5), pct(0.95))
}

fn main() {
    let opts = parse_args();
    let mut rep = Report::new(opts.filter.clone());
    println!("mide-bench: {} line fixture ({} build)\n", opts.lines, if cfg!(debug_assertions) { "DEBUG - numbers are meaningless, use --release" } else { "release" });

    let dir = fixtures::dir();
    let big = fixtures::big_rust_file(&dir, opts.lines);
    let mb = std::fs::metadata(&big).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
    println!("fixture: {} ({mb:.0} MB)\n", big.display());

    core_benchmarks(&mut rep, &big, opts.lines);
    search_benchmarks(&mut rep, &big);
    syntax_benchmarks(&mut rep, &big, opts.lines);
    fuzzy_benchmarks(&mut rep, &dir);
    files_benchmarks(&mut rep, &dir);
    terminal_benchmarks(&mut rep);
    if let Some(repo) = &opts.repo {
        git_benchmarks(&mut rep, repo);
    } else {
        println!("(skipping git benchmarks: pass --repo <path to a big repository>)\n");
    }

    let missed = rep.finish();
    if let Some(path) = &opts.json {
        let sh = |cmd: &str, args: &[&str]| {
            std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
        };
        let mem_gb = sh("sysctl", &["-n", "hw.memsize"]).parse::<f64>().unwrap_or(0.0) / 1e9_f64.powi(0) / 1073741824.0;
        let machine = format!("{}, {:.0} GB, macOS {}", sh("sysctl", &["-n", "machdep.cpu.brand_string"]), mem_gb, sh("sw_vers", &["-productVersion"]));
        let q = |s: String| format!("\"{}\"", s.replace('"', "'"));
        let meta = [
            ("machine", q(machine)),
            ("date", q(sh("date", &["+%Y-%m-%d"]))),
            ("fixture_lines", opts.lines.to_string()),
            ("fixture_mb", format!("{mb:.1}")),
        ];
        std::fs::write(path, rep.to_json(&meta)).expect("write json");
        println!("wrote {}", path.display());
    }
    if opts.strict && missed > 0 {
        std::process::exit(1);
    }
}

// ---- core: the text buffer ---------------------------------------------------------------

fn core_benchmarks(rep: &mut Report, big: &std::path::Path, lines: usize) {
    let (open, _) = measure(1, || Buffer::open(big).unwrap());
    rep.record("core: open file", open, open, 1500.0);

    let mut b = Buffer::open(big).unwrap();
    let mid = b.offset_of(lines / 2, 10);
    b.set_cursor(mid);

    let (m, p) = measure(2000, || {
        // What the editor does per visible line, per frame.
        for i in 0..60 {
            std::hint::black_box(b.line(lines / 2 + i));
        }
    });
    rep.record("core: read 60 visible lines (per frame)", m, p, 0.5);

    let (m, p) = measure(1000, || {
        b.insert("x");
    });
    rep.record("core: type a character mid-document", m, p, 0.25);

    let (m, p) = measure(300, || {
        b.undo();
        b.redo();
    });
    rep.record("core: undo + redo a typed run", m, p, 1.0);

    let cursors: Vec<Selection> = (0..100).map(|i| Selection::cursor(b.offset_of(lines / 2 + i * 7, 3))).collect();
    b.set_selections(cursors.clone());
    let (m, p) = measure(200, || {
        b.insert("y");
        b.set_selections(cursors.clone());
    });
    rep.record("core: type with 100 cursors", m, p, 3.0);

    let mut b = Buffer::open(big).unwrap();
    let (m, p) = measure(5, || {
        b.set_cursor(b.offset_of(lines / 3, 0));
        b.duplicate_line();
        b.undo();
    });
    rep.record("core: duplicate line + undo (big doc)", m, p, 5.0);

    let b = Buffer::from_text(&format!("{}\n", "word ".repeat(200_000)));
    let (m, p) = measure(20, || b.line(0));
    rep.record("core: read one 1 MB line (every frame!)", m, p, 1.0);
}

// ---- search --------------------------------------------------------------------------------------

fn search_benchmarks(rep: &mut Report, big: &std::path::Path) {
    let b = Buffer::open(big).unwrap();
    let q = |text: &str, regex: bool| Matcher::new(&Query { text: text.into(), regex, ..Query::default() }).unwrap().unwrap();

    let lit = q("function_5000", false);
    let (m, p) = measure(3, || search::find_all(&b, &lit, 10_000));
    rep.record("search: literal, rare (full scan)", m, p, 250.0);

    let common = q("let", false);
    let (m, p) = measure(3, || search::find_all(&b, &common, 10_000));
    rep.record("search: literal, very common (capped 10k)", m, p, 60.0);

    let rx = q(r"fn function_\d+\(x: u32\)", true);
    let (m, p) = measure(3, || search::find_all(&b, &rx, 10_000));
    rep.record("search: regex (capped 10k)", m, p, 120.0);

    let nope = q("zzzz-not-present", false);
    let (m, p) = measure(3, || search::find_all(&b, &nope, 10_000));
    rep.record("search: no match (full scan of file)", m, p, 250.0);

    let (m, p) = measure(5, || search::find_next(&b, &lit, b.len_chars() / 2, true));
    rep.record("search: find next (wraps, rare term)", m, p, 250.0);

    // The question people actually ask: Find for something that only occurs at the very end of a
    // huge file, cursor at the top. `recompute_find` does exactly these two steps per keystroke.
    let last_fn = (b.len_lines().saturating_sub(8) / 8) * 8; // the last `fn function_N` line
    let tail = q(&format!("function_{last_fn}("), false);
    let (m, p) = measure(5, || search::find_next(&b, &tail, 0, true));
    rep.record("search: find next, match at the end (cursor at top)", m, p, 250.0);

    let (m, p) = measure(5, || {
        let hits = search::find_all(&b, &tail, 10_000);
        let first = search::find_next(&b, &tail, 0, true);
        (hits, first)
    });
    rep.record("search: type in Find bar, match at the end (highlight + jump)", m, p, 400.0);

    let (m, p) = measure(5, || search::find_prev(&b, &tail, 0, true));
    rep.record("search: find previous from top, match at the end (wraps)", m, p, 250.0);

    let mut b2 = Buffer::open(big).unwrap();
    let many = q("let y", false);
    let (m, _) = measure(1, || search::replace_all(&mut b2, &many, "let z"));
    rep.record("search: replace all (every 'let y')", m, m, 4000.0);

    let (m, p) = measure(2, || {
        b2.select_all_occurrences();
    });
    rep.record("core: select all occurrences", m, p, 1500.0);
}

// ---- syntax highlighting ----------------------------------------------------------------------------

fn syntax_benchmarks(rep: &mut Report, big: &std::path::Path, lines: usize) {
    let syntaxes = Syntaxes::load_default();
    let theme = Arc::new(Theme::default_dark());
    let (load, _) = measure(1, Syntaxes::load_default);
    rep.record("syntax: load all grammars (startup cost)", load, load, 80.0);

    let b = Buffer::open(big).unwrap();
    let window = |h: &mut DocHighlighter, first: usize| h.highlight_range(first, 60, |i| b.line(i));

    let (m, p) = measure(10, || {
        let mut h = DocHighlighter::new(syntaxes.clone(), theme.clone(), "Rust");
        window(&mut h, 0)
    });
    rep.record("syntax: first window (60 lines, top of file)", m, p, 6.0);

    let mut h = DocHighlighter::new(syntaxes.clone(), theme.clone(), "Rust");
    window(&mut h, 0);
    let (m, p) = measure(50, || {
        h.invalidate_from(10);
        window(&mut h, 0)
    });
    rep.record("syntax: re-highlight window after an edit", m, p, 6.0);

    // The nasty one: jumping far into a file nobody has scrolled through yet.
    let jump = lines.min(100_000);
    let (m, _) = measure(1, || {
        let mut h = DocHighlighter::new(syntaxes.clone(), theme.clone(), "Rust");
        window(&mut h, jump)
    });
    rep.record(&format!("syntax: cold jump to line {jump} (⌘↓ in a big file)"), m, m, 50.0);

    let (m, _) = measure(1, || {
        let mut h = DocHighlighter::new(syntaxes.clone(), theme.clone(), "Rust");
        h.highlight_range(0, 20_000, |i| b.line(i)).len()
    });
    rep.record("syntax: throughput, 20k lines of Rust", m, m, 1000.0); // >= 20k lines/s: 4x a hard scroll fling

    let json = format!("[{}]", (0..20_000).map(|i| format!("{{\"id\": {i}, \"name\": \"item {i}\", \"tags\": [\"a\", \"b\"]}}")).collect::<Vec<_>>().join(",\n"));
    let jb = Buffer::from_text(&json);
    let (m, _) = measure(1, || {
        let mut h = DocHighlighter::new(syntaxes.clone(), theme.clone(), "JSON");
        h.highlight_range(0, 20_000, |i| jb.line(i)).len()
    });
    rep.record("syntax: throughput, 20k lines of JSON", m, m, 600.0);
}

// ---- fuzzy -----------------------------------------------------------------------------------------------

fn fuzzy_benchmarks(rep: &mut Report, dir: &std::path::Path) {
    let paths = fixtures::paths(dir, 100_000);
    let mut f = Fuzzy::new(Kind::Path);
    for (label, query) in [("short query", "mn"), ("typical query", "srcmain"), ("long query", "components/button/index")] {
        let (m, p) = measure(10, || f.search(&paths, query, 300));
        rep.record(&format!("fuzzy: ⌘P over 100k paths, {label}"), m, p, 12.0);
    }
}

// ---- files ----------------------------------------------------------------------------------------------------

fn files_benchmarks(rep: &mut Report, dir: &std::path::Path) {
    let tree = fixtures::project_tree(dir, 20_000);
    let (m, p) = measure(3, || mide_files::list_files(&tree, 200_000).len());
    rep.record("files: list 20k files (⌘P index)", m, p, 400.0);
    let (m, p) = measure(5, || FileTree::open(&tree).unwrap().rows().len());
    rep.record("files: open the sidebar tree", m, p, 20.0);
}

// ---- terminal -----------------------------------------------------------------------------------------------------

fn terminal_benchmarks(rep: &mut Report) {
    let sh = |script: &str| SpawnOptions { program: Some(("/bin/sh".into(), vec!["-c".into(), script.into()])), ..Default::default() };

    // How fast mide takes in output from a program that writes in big blocks (a noisy
    // build or `cat` of a log). `seq`/`echo`-per-line producers are limited by the OS pty
    // (~2 MB/s on macOS regardless of the terminal), so they would not measure mide.
    let n: usize = std::env::var("MIDE_BENCH_TERM_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000);
    let script = format!(
        "{{ yes 'INFO  2026-11-22 12:00:00 service=api worker=7 status=ok latency=12ms path=/v1/items' | head -n {n}; echo MIDE_DONE; }}; sleep 30"
    );
    let (m, _) = measure(1, || {
        let mut t = Terminal::spawn(sh(&script), Size::new(120, 40)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            t.poll_events();
            if t.snapshot().text().contains("MIDE_DONE") || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let mb = n as f64 * 85.0 / 1e6;
    println!("    ({mb:.1} MB through a pty = {:.0} MB/s)", mb / (m / 1000.0));
    rep.record(&format!("terminal: take in {n} log lines (~{mb:.0} MB) from a pty"), m, m, 900.0);

    emulator_only(rep);

    let t = Terminal::spawn(sh("sleep 5"), Size::new(80, 24)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let (m, p) = measure(300, || t.snapshot());
    rep.record("terminal: snapshot 80x24 (per frame)", m, p, 0.5);
    let big = Terminal::spawn(sh("sleep 5"), Size::new(260, 80)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let (m, p) = measure(300, || big.snapshot());
    rep.record("terminal: snapshot 260x80 (per frame)", m, p, 2.0);

    let (m, _) = measure(5, || {
        let t = Terminal::spawn(sh("sleep 30"), Size::new(80, 24)).unwrap();
        drop(t);
    });
    rep.record("terminal: start + close a session", m, m, 40.0);
}

/// The terminal emulator on its own (no PTY, no UI): how fast it parses output.
fn emulator_only(rep: &mut Report) {
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct Quiet;
    impl EventListener for Quiet {
        fn send_event(&self, _: Event) {}
    }
    struct Dim;
    impl Dimensions for Dim {
        fn total_lines(&self) -> usize {
            40
        }
        fn screen_lines(&self) -> usize {
            40
        }
        fn columns(&self) -> usize {
            120
        }
    }
    let mut data = Vec::new();
    for i in 1..=1_000_000u32 {
        data.extend_from_slice(i.to_string().as_bytes());
        data.extend_from_slice(b"\r\n");
    }
    let mb = data.len() as f64 / 1e6;
    let (m, _) = measure(1, || {
        let mut term = Term::new(Config::default(), &Dim, Quiet);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, &data);
    });
    println!("    (emulator alone: {mb:.1} MB in {m:.0} ms = {:.0} MB/s)", mb / (m / 1000.0));
    rep.record("terminal: emulator parse speed, 1M lines (no PTY)", m, m, 600.0);
}

// ---- git ----------------------------------------------------------------------------------------------------------------

fn git_benchmarks(rep: &mut Report, repo: &std::path::Path) {
    use mide_git::Repo;
    let Ok(mut r) = Repo::discover(repo) else {
        println!("(not a git repository: {})\n", repo.display());
        return;
    };
    let (m, p) = measure(3, || r.log(0, 2000).unwrap().len());
    rep.record("git: commit log, first 2000 + graph", m, p, 300.0);
    let (m, p) = measure(3, || r.refs().unwrap().branches.len());
    rep.record("git: branches/tags/remotes", m, p, 100.0);
    let (m, p) = measure(3, || r.status().unwrap().len());
    rep.record("git: working-tree status", m, p, 300.0);
    let (m, p) = measure(3, || r.status_summary().unwrap().index_clean);
    rep.record("git: status summary (status bar)", m, p, 300.0);
    if let Some(first) = r.log(0, 1).ok().and_then(|l| l.first().map(|e| e.commit.id)) {
        let (m, p) = measure(5, || r.commit_detail(first).unwrap().files.len());
        rep.record("git: diff of the newest commit", m, p, 100.0);
    }
    let (m, p) = measure(3, || r.working_diff(false).map(|d| d.len()).unwrap_or(0));
    rep.record("git: unstaged diff (working copy view)", m, p, 300.0);
}
