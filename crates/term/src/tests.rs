use super::*;
use std::time::{Duration, Instant};

fn wait_for(t: &mut Terminal, pred: impl FnMut(&Snapshot) -> bool) -> Snapshot {
    wait_for_events(t, &mut Vec::new(), pred)
}

/// Like `wait_for`, but keeps every event it drains.
fn wait_for_events(
    t: &mut Terminal,
    events: &mut Vec<TermEvent>,
    mut pred: impl FnMut(&Snapshot) -> bool,
) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        events.extend(t.poll_events());
        let s = t.snapshot();
        if pred(&s) {
            return s;
        }
        assert!(Instant::now() < deadline, "timed out; screen was:\n{}", s.text());
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn sh(script: &str) -> SpawnOptions {
    SpawnOptions { program: Some(("/bin/sh".into(), vec!["-c".into(), script.into()])), ..Default::default() }
}

#[test]
fn runs_command_and_reports_exit() {
    let mut t = Terminal::spawn(sh("echo hello-term; exit 3"), Size::new(40, 5)).unwrap();
    let mut events = Vec::new();
    let s = wait_for_events(&mut t, &mut events, |s| s.text().contains("hello-term"));
    assert!(s.row_text(0).starts_with("hello-term"));

    let deadline = Instant::now() + Duration::from_secs(10);
    while !events.iter().any(|e| matches!(e, TermEvent::Exited(_))) && Instant::now() < deadline {
        events.extend(t.poll_events());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(events.contains(&TermEvent::Exited(Some(3))), "events: {events:?}");
}

#[test]
fn interactive_input_and_cwd() {
    let dir = std::env::temp_dir().canonicalize().unwrap();
    let opts = SpawnOptions {
        program: Some(("/bin/sh".into(), vec![])),
        cwd: Some(dir.clone()),
        ..Default::default()
    };
    let mut t = Terminal::spawn(opts, Size::new(80, 10)).unwrap();
    t.write(b"echo $((6*7)); pwd\n".to_vec());
    let s = wait_for(&mut t, |s| s.text().contains("42"));
    assert!(s.text().contains(dir.to_str().unwrap()));
}

#[test]
fn colors_and_resize() {
    let script = r"printf '\033[31mred\033[0m \033[1;34mbold\033[0m \033[38;2;1;2;3mrgb\033[0m'; sleep 5";
    let mut t = Terminal::spawn(sh(script), Size::new(40, 5)).unwrap();
    let s = wait_for(&mut t, |s| s.text().contains("rgb"));
    let p = Palette::default();
    assert_eq!(s.cell(0, 0).fg, p.ansi[1]);
    assert!(s.cell(0, 4).style.bold);
    assert_eq!(s.cell(0, 4).fg, p.ansi[12]); // bold blue brightens
    assert_eq!(s.cell(0, 9).fg, Rgb8 { r: 1, g: 2, b: 3 });
    assert_eq!(s.cell(0, 20).bg, p.bg);

    t.resize(Size::new(20, 3));
    let s = t.snapshot();
    assert_eq!((s.cols, s.rows, s.cells.len()), (20, 3, 60));
}

#[test]
fn indexed_palette_cube_and_grays() {
    let p = Palette::default();
    assert_eq!(indexed(16, &p), Rgb8 { r: 0, g: 0, b: 0 });
    assert_eq!(indexed(231, &p), Rgb8 { r: 255, g: 255, b: 255 });
    assert_eq!(indexed(232, &p), Rgb8 { r: 8, g: 8, b: 8 });
    assert_eq!(indexed(255, &p), Rgb8 { r: 238, g: 238, b: 238 });
}

#[test]
fn selection_copies_text_and_marks_cells() {
    let mut t = Terminal::spawn(sh("echo hello world; sleep 5"), Size::new(40, 5)).unwrap();
    wait_for(&mut t, |s| s.text().contains("hello world"));
    t.select(SelectKind::Simple, 0, 0);
    t.extend_selection(0, 4);
    let s = t.snapshot();
    assert!((0..5).all(|c| s.cell(0, c).selected));
    assert!(!s.cell(0, 6).selected);
    assert_eq!(t.selection_text().as_deref(), Some("hello"));
    // Double-click style selection grabs the whole word.
    t.select(SelectKind::Word, 0, 7);
    assert_eq!(t.selection_text().as_deref(), Some("world"));
    t.clear_selection();
    assert!(t.selection_text().is_none());
    assert!(!t.snapshot().cell(0, 0).selected);
}

#[test]
fn scrollback_option_is_accepted() {
    let opts = SpawnOptions { scrollback: Some(50), ..sh("echo hi; sleep 5") };
    let mut t = Terminal::spawn(opts, Size::new(20, 3)).unwrap();
    wait_for(&mut t, |s| s.text().contains("hi"));
    assert!(!t.app_cursor() && !t.alt_screen());
}

#[test]
fn waker_fires_on_output() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let mut t = Terminal::spawn_with_waker(sh("echo ping; sleep 5"), Size::new(20, 3), move || {
        h.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    wait_for(&mut t, |s| s.text().contains("ping"));
    assert!(hits.load(Ordering::SeqCst) > 0, "waker should have fired for the output");
}

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn dropping_a_session_that_ignores_hangup_never_blocks_and_still_ends_it() {
    // A program that ignores SIGHUP/SIGTERM, like a stuck `claude` or editor.
    let script = "trap '' HUP TERM; while :; do sleep 1; done";
    let mut t = Terminal::spawn(sh(script), Size::new(20, 3)).unwrap();
    let pid = t.pid();
    t.poll_events();
    std::thread::sleep(Duration::from_millis(300));
    assert!(pid_alive(pid));

    let started = Instant::now();
    drop(t);
    assert!(started.elapsed() < Duration::from_millis(250), "drop must not wait for the child: {:?}", started.elapsed());

    // It is force-killed shortly afterwards instead of lingering.
    let deadline = Instant::now() + Duration::from_secs(8);
    while pid_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!pid_alive(pid), "process {pid} should have been killed");
}

#[test]
fn dropping_a_well_behaved_session_ends_it_quickly() {
    let t = Terminal::spawn(sh("sleep 30"), Size::new(20, 3)).unwrap();
    let pid = t.pid();
    std::thread::sleep(Duration::from_millis(200));
    drop(t);
    let deadline = Instant::now() + Duration::from_secs(3);
    while pid_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!pid_alive(pid));
}

#[test]
fn wheel_scrolls_back_through_history_and_returns() {
    let mut t = Terminal::spawn(sh("seq 1 300; sleep 5"), Size::new(20, 5)).unwrap();
    wait_for(&mut t, |s| s.text().contains("300"));
    assert_eq!(t.display_offset(), 0);
    assert!(t.snapshot().text().contains("300"));

    t.scroll(10); // positive = up into history
    let up = t.snapshot();
    assert_eq!(up.display_offset, 10);
    assert!(!up.text().contains("300"), "scrolled view should no longer show the newest line");
    let top_line: i32 = up.row_text(0).trim().parse().expect("a number");
    assert!(top_line < 290, "top visible line should be older, got {top_line}");

    t.scroll(-4);
    assert_eq!(t.display_offset(), 6);
    t.scroll_page(true);
    assert!(t.display_offset() > 6);
    t.scroll_to_top();
    assert!(t.display_offset() > 200, "300 lines of history should be reachable");
    t.scroll_to_bottom();
    assert_eq!(t.display_offset(), 0);
    assert!(t.snapshot().text().contains("300"));
}

#[test]
fn programs_can_ask_for_mouse_reports_and_the_alternate_screen() {
    // Enable click reporting + SGR encoding, then switch to the alternate screen.
    let mut t = Terminal::spawn(sh(r"printf '[?1000h[?1006h[?1049h'; sleep 5"), Size::new(20, 5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !t.alt_screen() && Instant::now() < deadline {
        t.poll_events();
        std::thread::sleep(Duration::from_millis(10));
    }
    let m = t.mouse_mode();
    assert!(m.click && m.sgr && m.active());
    assert!(!m.drag && !m.motion);
    assert!(t.alt_screen());
    assert!(t.alternate_scroll(), "alternate-scroll is on by default");
}

#[test]
fn plain_shells_do_not_report_the_mouse() {
    let t = Terminal::spawn(sh("sleep 5"), Size::new(20, 5)).unwrap();
    assert!(!t.mouse_mode().active());
}
