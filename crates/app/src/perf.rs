//! Opt-in performance logging: `MIDE_PERF=1 mide` prints startup time and the
//! CPU time from an input event to the end of the paint it triggers.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// One scripted phase: its name, input-to-paint samples, and frame-time samples (ms).
type Phase = (String, Vec<f64>, Vec<f64>);

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();

thread_local! {
    static MARKS: RefCell<Vec<(String, f64)>> = const { RefCell::new(Vec::new()) };
    static PHASES: RefCell<Vec<Phase>> = const { RefCell::new(Vec::new()) };
    static INPUT_MS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
    static FRAME_MS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
    static QUIET: Cell<bool> = const { Cell::new(false) };
    static PREPAINT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static FIRST_PAINT_DONE: Cell<bool> = const { Cell::new(false) };
}

pub fn init() {
    let _ = START.set(Instant::now());
    let _ = ENABLED.set(std::env::var_os("MIDE_PERF").is_some() || std::env::var_os("MIDE_PERF_JSON").is_some());
}

fn enabled() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}

/// Log a labelled milestone relative to process start.
pub fn mark(label: &str) {
    if enabled() {
        if let Some(s) = START.get() {
            let ms = s.elapsed().as_secs_f64() * 1000.0;
            MARKS.with(|m| m.borrow_mut().push((label.to_owned(), ms)));
            eprintln!("[perf] {label}: {ms:.1} ms since launch");
        }
    }
}

pub fn note_prepaint(started: Instant) {
    PREPAINT.with(|p| p.set(started.elapsed()));
}

pub fn note_paint(started: Instant, input_at: Option<Instant>) {
    if !enabled() {
        return;
    }
    if !FIRST_PAINT_DONE.with(|f| f.replace(true)) {
        mark("first editor paint");
    }
    let pre = PREPAINT.with(Cell::get);
    let paint = started.elapsed();
    let frame_ms = (pre + paint).as_secs_f64() * 1000.0;
    FRAME_MS.with(|v| v.borrow_mut().push(frame_ms));
    let quiet = QUIET.with(Cell::get);
    if let Some(t) = input_at {
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        INPUT_MS.with(|v| v.borrow_mut().push(ms));
        if !quiet {
            eprintln!(
                "[perf] input -> end of paint: {ms:.2} ms (prepaint {:.2}, paint {:.2})",
                pre.as_secs_f64() * 1000.0,
                paint.as_secs_f64() * 1000.0,
            );
        }
    } else if !quiet && pre + paint > Duration::from_millis(8) {
        eprintln!(
            "[perf] slow frame: {frame_ms:.2} ms (prepaint {:.2}, paint {:.2})",
            pre.as_secs_f64() * 1000.0,
            paint.as_secs_f64() * 1000.0,
        );
    }
}

/// Stop printing every sample (used while a scripted run collects them).
pub fn set_quiet(q: bool) {
    QUIET.with(|c| c.set(q));
}

pub fn clear() {
    INPUT_MS.with(|v| v.borrow_mut().clear());
    FRAME_MS.with(|v| v.borrow_mut().clear());
}

fn stats(label: &str, samples: &[f64]) -> String {
    if samples.is_empty() {
        return format!("{label}: no samples");
    }
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| s[((s.len() as f64 - 1.0) * p).round() as usize];
    format!("{label}: n={} p50={:.2} ms p95={:.2} ms max={:.2} ms", s.len(), pct(0.5), pct(0.95), s[s.len() - 1])
}

/// Print and return the collected input-to-paint and frame-time statistics.
pub fn summary(phase: &str) {
    let input = INPUT_MS.with(|v| v.borrow().clone());
    let frames = FRAME_MS.with(|v| v.borrow().clone());
    eprintln!("[perf-summary] {phase}: {}", stats("input->paint", &input));
    eprintln!("[perf-summary] {phase}: {}", stats("frame cpu", &frames));
    PHASES.with(|p| p.borrow_mut().push((phase.to_owned(), input, frames)));
}

fn pct(samples: &[f64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() as f64 - 1.0) * p).round() as usize]
}

/// If `MIDE_PERF_JSON=<path>` is set, write the start-up marks and every phase summary
/// there (used to refresh the website's numbers; see `scripts/perf.sh --json`).
pub fn write_json() {
    let Some(path) = std::env::var_os("MIDE_PERF_JSON") else { return };
    let mark = |label: &str| MARKS.with(|m| m.borrow().iter().find(|(l, _)| l == label).map_or(0.0, |(_, v)| *v));
    let phase = |name: &str| {
        PHASES.with(|p| {
            p.borrow().iter().find(|(n, _, _)| n.starts_with(name)).map(|(_, i, f)| (i.clone(), f.clone())).unwrap_or_default()
        })
    };
    let (ti, tf) = phase("typing");
    let (si, sf) = phase("scrolling");
    let (ji, jf) = phase("jump");
    let json = format!(
        "{{\n  \"startup\": {{\"window_created_ms\": {:.1}, \"first_paint_ms\": {:.1}}},\n  \"typing\": {{\"samples\": {}, \"frame_p50_ms\": {:.2}, \"frame_p95_ms\": {:.2}, \"input_p50_ms\": {:.2}, \"input_p95_ms\": {:.2}}},\n  \"scrolling\": {{\"frame_p50_ms\": {:.2}, \"frame_p95_ms\": {:.2}, \"input_p50_ms\": {:.2}, \"input_p95_ms\": {:.2}}},\n  \"jump\": {{\"input_ms\": {:.2}, \"frame_ms\": {:.2}}}\n}}\n",
        mark("window created"),
        mark("first editor paint"),
        ti.len(),
        pct(&tf, 0.5), pct(&tf, 0.95), pct(&ti, 0.5), pct(&ti, 0.95),
        pct(&sf, 0.5), pct(&sf, 0.95), pct(&si, 0.5), pct(&si, 0.95),
        pct(&ji, 0.5), pct(&jf, 0.5),
    );
    let _ = std::fs::write(path, json);
}
