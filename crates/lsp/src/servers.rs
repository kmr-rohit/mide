//! Which language server to run for a language, and finding it on this machine.
//!
//! A Mac app launched from the Dock has a bare `PATH` (no `~/.cargo/bin`, no Homebrew), so lookups
//! also search the usual install places, and the server is started with the same extended `PATH`.

use std::path::{Path, PathBuf};

/// One way to run a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub command: &'static str,
    pub args: &'static [&'static str],
}

/// Everything the editor needs to know about a language's server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// Files whose languages share a server (JavaScript and TypeScript; C and C++) share this key.
    pub key: &'static str,
    /// The `languageId` to announce in `didOpen`.
    pub language_id: &'static str,
    /// Tried in order; the first one installed is used.
    pub candidates: &'static [Candidate],
    /// How to install the first candidate, shown when none is found.
    pub hint: &'static str,
}

// A macro, not a function: struct literals of constants are promoted to `'static`; const fn calls are not.
macro_rules! c {
    ($command:expr, $args:expr) => {
        Candidate { command: $command, args: $args }
    };
}

/// The server for a syntax name as the editor shows it ("Rust", "JavaScript (Babel)", "C++", ...).
pub fn spec_for(language: &str) -> Option<Spec> {
    let l = language.to_ascii_lowercase();
    let s = |key, language_id, candidates, hint| Some(Spec { key, language_id, candidates, hint });
    match l.as_str() {
        "rust" => s("rust", "rust", &[c!("rust-analyzer", &[])], "rustup component add rust-analyzer"),
        "python" => s(
            "python",
            "python",
            &[c!("pyright-langserver", &["--stdio"]), c!("basedpyright-langserver", &["--stdio"]), c!("pylsp", &[]), c!("ruff", &["server"])],
            "npm install -g pyright",
        ),
        "go" => s("go", "go", &[c!("gopls", &[])], "go install golang.org/x/tools/gopls@latest"),
        "c" => s("clangd", "c", &[c!("clangd", &[])], "brew install llvm  (or install Xcode)"),
        "c++" => s("clangd", "cpp", &[c!("clangd", &[])], "brew install llvm  (or install Xcode)"),
        "zig" => s("zig", "zig", &[c!("zls", &[])], "brew install zls"),
        "swift" => s("swift", "swift", &[c!("sourcekit-lsp", &[])], "install Xcode"),
        "bash" | "shell-unix-generic" | "shell script (bash)" => {
            s("bash", "shellscript", &[c!("bash-language-server", &["start"])], "npm install -g bash-language-server")
        }
        _ if l.starts_with("typescript") => {
            let id = if l.contains("react") { "typescriptreact" } else { "typescript" };
            s("typescript", id, &[c!("typescript-language-server", &["--stdio"])], "npm install -g typescript typescript-language-server")
        }
        _ if l.starts_with("javascript") => {
            let id = if l.contains("react") || l.contains("jsx") { "javascriptreact" } else { "javascript" };
            s("typescript", id, &[c!("typescript-language-server", &["--stdio"])], "npm install -g typescript typescript-language-server")
        }
        _ => None,
    }
}

/// Directories searched for servers: `PATH`, then the places tools usually install to.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut extra: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/homebrew/opt/llvm/bin", "/usr/local/opt/llvm/bin"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(h) = &home {
        for rel in [".cargo/bin", ".local/bin", "go/bin", ".npm-global/bin", ".bun/bin", ".volta/bin"] {
            extra.push(h.join(rel));
        }
    }
    for d in extra {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// `PATH` to give a server so it can find its own helpers (cargo, node, ...).
pub fn extended_path() -> std::ffi::OsString {
    std::env::join_paths(search_dirs()).unwrap_or_default()
}

/// A runnable file called `name` in `dirs`.
pub fn find_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    dirs.iter().map(|d| d.join(name)).find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// The first installed candidate for `spec`, with its resolved path.
pub fn resolve(spec: &Spec) -> Option<(PathBuf, &'static [&'static str])> {
    let dirs = search_dirs();
    spec.candidates.iter().find_map(|c| find_in(c.command, &dirs).map(|p| (p, c.args)))
}

/// The project root for a file: the *outermost* folder (up to `workspace`) holding a project marker,
/// else `workspace` itself. Outermost, so every crate of a Cargo workspace (or package of a JS
/// monorepo) shares one server instead of each starting its own; and a folder full of unrelated
/// projects still gets one server per project, because nothing above them has a marker.
pub fn project_root(file: &Path, workspace: &Path) -> PathBuf {
    const MARKERS: &[&str] =
        &["Cargo.toml", "go.mod", "package.json", "tsconfig.json", "pyproject.toml", "setup.py", "compile_commands.json", "build.zig", "Package.swift"];
    let mut found: Option<PathBuf> = None;
    let mut dir = file.parent();
    while let Some(d) = dir {
        if !d.starts_with(workspace) {
            break;
        }
        if MARKERS.iter().any(|m| d.join(m).exists()) {
            found = Some(d.to_path_buf());
        }
        if d == workspace {
            break;
        }
        dir = d.parent();
    }
    found.unwrap_or_else(|| workspace.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_map_to_servers_and_share_them() {
        assert_eq!(spec_for("Rust").unwrap().key, "rust");
        assert_eq!(spec_for("JavaScript (Babel)").unwrap().key, "typescript");
        assert_eq!(spec_for("TypeScript").unwrap().language_id, "typescript");
        assert_eq!(spec_for("TypeScriptReact").unwrap().language_id, "typescriptreact");
        assert_eq!(spec_for("C").unwrap().key, spec_for("C++").unwrap().key);
        assert_eq!(spec_for("C++").unwrap().language_id, "cpp");
        assert!(spec_for("Plain Text").is_none());
        assert!(spec_for("Markdown").is_none());
    }

    #[test]
    fn finds_executables_only() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fake-server");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::write(dir.path().join("not-exec"), "x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        assert_eq!(find_in("fake-server", &dirs), Some(exe));
        assert!(find_in("not-exec", &dirs).is_none());
        assert!(find_in("missing", &dirs).is_none());
    }

    #[test]
    fn root_is_the_outermost_project_folder_inside_the_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let crate_dir = ws.path().join("crates/a");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(crate_dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(ws.path().join("notes.txt"), "").unwrap();
        // Only the crate has a marker: it is the root.
        assert_eq!(project_root(&crate_dir.join("src/lib.rs"), ws.path()), crate_dir);
        assert_eq!(project_root(&ws.path().join("notes.txt"), ws.path()), ws.path());
        assert_eq!(project_root(Path::new("/elsewhere/x.rs"), ws.path()), ws.path());
        // A workspace-level Cargo.toml too: all crates share the workspace root.
        std::fs::write(ws.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(project_root(&crate_dir.join("src/lib.rs"), ws.path()), ws.path());
    }

    #[test]
    fn unrelated_projects_in_one_folder_keep_separate_roots() {
        let ws = tempfile::tempdir().unwrap();
        for name in ["one", "two"] {
            std::fs::create_dir_all(ws.path().join(name).join("src")).unwrap();
            std::fs::write(ws.path().join(name).join("Cargo.toml"), "").unwrap();
        }
        assert_eq!(project_root(&ws.path().join("one/src/a.rs"), ws.path()), ws.path().join("one"));
        assert_eq!(project_root(&ws.path().join("two/src/a.rs"), ws.path()), ws.path().join("two"));
    }
}
