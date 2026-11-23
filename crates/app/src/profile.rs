//! Work/personal profiles: one switch changes who you commit as, which SSH key git uses,
//! which kubeconfig `kubectl` reads, and any other environment variables, for every terminal
//! and every git action in the window.
//!
//! Nothing global is touched (no `git config --global`): a profile is a set of environment
//! variables. mide's own git commands get them directly; terminals get them through the
//! environment they start with, plus a small file their shell reads *after* your `.zshrc`,
//! so a `export OCI_CLI_PROFILE=...` in your rc file cannot undo the profile. Shells that
//! are already running and idle are switched in place (they `source` the file).
//!
//! ```toml
//! [profile.work]
//! git_name = "Rohit Kumar"
//! git_email = "me@work.example"
//! git_ssh_key = "~/.ssh/id_ed25519_work"      # optional
//! kubeconfig = "~/.kube/config-work"          # optional
//! color = "#e5484d"                           # optional: the status bar chip
//! folders = ["~/work"]                        # optional: opened automatically for these folders
//! env = { AWS_PROFILE = "work", OCI_CLI_PROFILE = "WORK" }
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProfileDef {
    #[serde(skip)]
    pub name: String,
    pub git_name: Option<String>,
    pub git_email: Option<String>,
    /// Private key git should use for SSH (`-i key -o IdentitiesOnly=yes`).
    pub git_ssh_key: Option<String>,
    pub kubeconfig: Option<String>,
    /// `#rrggbb` for the status bar chip.
    pub color: Option<String>,
    /// Folders that select this profile when opened (and everything inside them).
    pub folders: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// `~/x` and `$HOME/x` to an absolute path.
pub fn expand(path: &str, home: &Path) -> String {
    let p = path.trim();
    if p == "~" {
        return home.to_string_lossy().into_owned();
    }
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("$HOME/")) {
        return home.join(rest).to_string_lossy().into_owned();
    }
    p.to_owned()
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl ProfileDef {
    /// Everything this profile sets, in a stable order.
    pub fn env(&self, home: &Path) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        if let Some(n) = self.git_name.as_ref().filter(|s| !s.is_empty()) {
            v.push(("GIT_AUTHOR_NAME".into(), n.clone()));
            v.push(("GIT_COMMITTER_NAME".into(), n.clone()));
        }
        if let Some(e) = self.git_email.as_ref().filter(|s| !s.is_empty()) {
            v.push(("GIT_AUTHOR_EMAIL".into(), e.clone()));
            v.push(("GIT_COMMITTER_EMAIL".into(), e.clone()));
        }
        if let Some(k) = self.git_ssh_key.as_ref().filter(|s| !s.is_empty()) {
            let key = expand(k, home);
            v.push(("GIT_SSH_COMMAND".into(), format!("ssh -i {} -o IdentitiesOnly=yes", sh_quote(&key))));
        }
        if let Some(k) = self.kubeconfig.as_ref().filter(|s| !s.is_empty()) {
            v.push(("KUBECONFIG".into(), expand(k, home)));
        }
        for (k, val) in &self.env {
            v.retain(|(existing, _)| existing != k); // an explicit env entry wins over a shortcut
            v.push((k.clone(), expand(val, home)));
        }
        v.push(("MIDE_PROFILE".into(), self.name.clone()));
        v
    }

    /// The chip color as 0xRRGGBB.
    pub fn color_rgb(&self) -> Option<u32> {
        let c = self.color.as_deref()?.trim().trim_start_matches('#');
        (c.len() == 6).then(|| u32::from_str_radix(c, 16).ok()).flatten()
    }

    /// "Name <email>" for display, when the profile sets an identity.
    pub fn identity(&self) -> Option<String> {
        match (&self.git_name, &self.git_email) {
            (Some(n), Some(e)) => Some(format!("{n} <{e}>")),
            (None, Some(e)) => Some(e.clone()),
            (Some(n), None) => Some(n.clone()),
            _ => None,
        }
    }
}

/// Every variable any profile manages. Switching to a profile unsets the ones it does not set,
/// so nothing from the previous profile leaks through.
pub fn managed_keys(all: &[ProfileDef], home: &Path) -> BTreeSet<String> {
    all.iter().flat_map(|p| p.env(home)).map(|(k, _)| k).collect()
}

/// A script (`sh` syntax, valid for zsh and bash) that makes a shell match `active`.
pub fn env_script(active: Option<&ProfileDef>, all: &[ProfileDef], home: &Path) -> String {
    let set: BTreeMap<String, String> = active.map(|p| p.env(home).into_iter().collect()).unwrap_or_default();
    let mut out = String::from("# written by mide: the active profile's environment\n");
    for k in managed_keys(all, home) {
        match set.get(&k) {
            Some(v) => out.push_str(&format!("export {k}={}\n", sh_quote(v))),
            None => out.push_str(&format!("unset {k}\n")),
        }
    }
    out
}

/// The profile whose `folders` entry is the longest prefix of `folder`.
pub fn for_folder<'a>(all: &'a [ProfileDef], folder: &Path, home: &Path) -> Option<&'a ProfileDef> {
    all.iter()
        .flat_map(|p| {
            p.folders.iter().map(move |f| {
                let path = PathBuf::from(expand(f, home));
                // Follow symlinks (/var vs /private/var), as the opened folder's path does.
                (p, path.canonicalize().unwrap_or(path))
            })
        })
        .filter(|(_, f)| folder.starts_with(f))
        .max_by_key(|(_, f)| f.components().count())
        .map(|(p, _)| p)
}

/// Which profile a window should start with: what the project asks for, then the folder rules,
/// then what you last used in this folder, then your default, then what you last used anywhere.
pub fn choose(
    all: &[ProfileDef],
    folder: &Path,
    home: &Path,
    project: Option<&str>,
    default: Option<&str>,
    state: &State,
) -> Option<String> {
    let exists = |n: &str| all.iter().any(|p| p.name == n).then(|| n.to_owned());
    project
        .and_then(exists)
        .or_else(|| for_folder(all, folder, home).map(|p| p.name.clone()))
        .or_else(|| state.folders.get(&folder.to_string_lossy().into_owned()).and_then(|n| exists(n)))
        .or_else(|| default.and_then(exists))
        .or_else(|| state.last.as_deref().and_then(exists))
}

/// What is remembered between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub last: Option<String>,
    pub folders: BTreeMap<String, String>,
}

pub fn state_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/mide/profile_state.toml"))
}

impl State {
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(t) = toml::to_string(self) {
            let _ = std::fs::write(path, t);
        }
    }

    /// Remember a choice for this folder and as the latest overall.
    pub fn remember(&mut self, folder: &Path, name: Option<&str>) {
        let key = folder.to_string_lossy().into_owned();
        match name {
            Some(n) => {
                self.last = Some(n.to_owned());
                self.folders.insert(key, n.to_owned());
            }
            None => {
                self.folders.remove(&key);
            }
        }
    }
}

// ---- zsh integration ---------------------------------------------------------------------------

/// Wrapper startup files. zsh reads them from `$ZDOTDIR`; each one runs the user's real file of
/// the same name, and `.zshrc` then applies the profile so it wins over anything the rc files set.
pub fn zsh_wrapper_files() -> [(&'static str, &'static str); 4] {
    macro_rules! wrap {
        ($name:literal) => {
            concat!(
                "# mide: runs your own ",
                $name,
                "\n_mide_dir=\"$ZDOTDIR\"\nZDOTDIR=\"${MIDE_USER_ZDOTDIR:-$HOME}\"\n[[ -f \"$ZDOTDIR/",
                $name,
                "\" ]] && source \"$ZDOTDIR/",
                $name,
                "\"\n_mide_user=\"$ZDOTDIR\"\nZDOTDIR=\"$_mide_dir\"\n"
            )
        };
    }
    [
        (".zshenv", wrap!(".zshenv")),
        (".zprofile", wrap!(".zprofile")),
        (
            ".zshrc",
            concat!(
                "# mide: runs your own .zshrc, then applies the active profile\n",
                "_mide_dir=\"$ZDOTDIR\"\nZDOTDIR=\"${MIDE_USER_ZDOTDIR:-$HOME}\"\n",
                "[[ -f \"$ZDOTDIR/.zshrc\" ]] && source \"$ZDOTDIR/.zshrc\"\n",
                "[[ -n \"$MIDE_PROFILE_FILE\" && -f \"$MIDE_PROFILE_FILE\" ]] && source \"$MIDE_PROFILE_FILE\"\n",
                "# From here on this is just your shell: ZDOTDIR is what you had.\n",
                "if [[ -n \"$MIDE_USER_ZDOTDIR\" ]]; then ZDOTDIR=\"$MIDE_USER_ZDOTDIR\"; else unset ZDOTDIR; fi\n",
                "unset _mide_dir\n"
            ),
        ),
        (".zlogin", wrap!(".zlogin")),
    ]
}

/// Create (or refresh) the wrapper directory and return it.
pub fn install_zsh_wrapper(config_dir: &Path) -> Option<PathBuf> {
    let dir = config_dir.join("shell/zsh");
    std::fs::create_dir_all(&dir).ok()?;
    for (name, text) in zsh_wrapper_files() {
        let f = dir.join(name);
        if std::fs::read_to_string(&f).ok().as_deref() != Some(text) {
            std::fs::write(&f, text).ok()?;
        }
    }
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/me")
    }

    fn defs() -> Vec<ProfileDef> {
        let text = r##"
            [work]
            git_name = "Me"
            git_email = "me@work.example"
            kubeconfig = "~/.kube/config-work"
            color = "#e5484d"
            folders = ["~/work", "~/work/special"]
            env = { OCI_CLI_PROFILE = "WORK" }
            [personal]
            git_email = "me@home.example"
            git_ssh_key = "~/.ssh/id_personal"
            folders = ["~/play"]
        "##;
        let map: BTreeMap<String, ProfileDef> = toml::from_str(text).unwrap();
        map.into_iter().map(|(name, mut p)| { p.name = name; p }).collect()
    }

    #[test]
    fn env_has_git_identity_ssh_and_kube() {
        let all = defs();
        let work: std::collections::HashMap<_, _> = all[1].env(&home()).into_iter().collect();
        assert_eq!(work["GIT_AUTHOR_EMAIL"], "me@work.example");
        assert_eq!(work["GIT_COMMITTER_NAME"], "Me");
        assert_eq!(work["KUBECONFIG"], "/home/me/.kube/config-work");
        assert_eq!(work["OCI_CLI_PROFILE"], "WORK");
        assert_eq!(work["MIDE_PROFILE"], "work");
        assert!(!work.contains_key("GIT_SSH_COMMAND"));
        let personal: std::collections::HashMap<_, _> = all[0].env(&home()).into_iter().collect();
        assert_eq!(personal["GIT_SSH_COMMAND"], "ssh -i '/home/me/.ssh/id_personal' -o IdentitiesOnly=yes");
    }

    #[test]
    fn explicit_env_overrides_a_shortcut() {
        let mut p = defs()[1].clone();
        p.env.insert("KUBECONFIG".into(), "/other".into());
        let m: std::collections::HashMap<_, _> = p.env(&home()).into_iter().collect();
        assert_eq!(m["KUBECONFIG"], "/other");
    }

    #[test]
    fn script_sets_and_unsets_so_nothing_leaks_between_profiles() {
        let all = defs();
        let s = env_script(Some(&all[0]), &all, &home()); // personal
        assert!(s.contains("export GIT_SSH_COMMAND="));
        assert!(s.contains("unset KUBECONFIG\n"));
        assert!(s.contains("unset OCI_CLI_PROFILE\n"));
        assert!(s.contains("export MIDE_PROFILE='personal'"));
        let none = env_script(None, &all, &home());
        assert!(none.lines().filter(|l| !l.starts_with('#')).all(|l| l.starts_with("unset ")));
    }

    #[test]
    fn quoting_survives_apostrophes() {
        let mut p = ProfileDef { name: "x".into(), git_name: Some("O'Brien".into()), ..Default::default() };
        p.git_email = Some("a@b".into());
        let s = env_script(Some(&p), std::slice::from_ref(&p), &home());
        assert!(s.contains("export GIT_AUTHOR_NAME='O'\\''Brien'"));
    }

    #[test]
    fn folder_rules_prefer_the_longest_match() {
        let mut all = defs();
        all[1].folders = vec!["~/work".into()];
        all[0].folders = vec!["~/work/special".into()]; // personal claims a sub-folder of work
        assert_eq!(for_folder(&all, Path::new("/home/me/work/app"), &home()).unwrap().name, "work");
        assert_eq!(for_folder(&all, Path::new("/home/me/work/special/x"), &home()).unwrap().name, "personal");
        assert!(for_folder(&all, Path::new("/home/me/elsewhere"), &home()).is_none());
    }

    #[test]
    fn choose_follows_the_documented_order() {
        let all = defs();
        let mut st = State::default();
        let f = Path::new("/home/me/elsewhere");
        assert_eq!(choose(&all, f, &home(), None, None, &st), None);
        st.last = Some("personal".into());
        assert_eq!(choose(&all, f, &home(), None, None, &st).as_deref(), Some("personal"));
        assert_eq!(choose(&all, f, &home(), None, Some("work"), &st).as_deref(), Some("work"));
        st.folders.insert("/home/me/elsewhere".into(), "personal".into());
        assert_eq!(choose(&all, f, &home(), None, Some("work"), &st).as_deref(), Some("personal"));
        assert_eq!(choose(&all, Path::new("/home/me/work/a"), &home(), None, None, &st).as_deref(), Some("work"));
        assert_eq!(choose(&all, f, &home(), Some("work"), None, &st).as_deref(), Some("work"));
        // A project naming a profile you do not have is ignored.
        assert_eq!(choose(&all, f, &home(), Some("nope"), None, &st).as_deref(), Some("personal"));
    }

    #[test]
    fn state_round_trips() {
        let dir = std::env::temp_dir().join(format!("mide-profile-{}", std::process::id()));
        let path = dir.join("s.toml");
        let mut s = State::default();
        s.remember(Path::new("/a"), Some("work"));
        s.save_to(&path);
        assert_eq!(State::load_from(&path), s);
        s.remember(Path::new("/a"), None);
        assert!(s.folders.is_empty());
        assert_eq!(State::load_from(&dir.join("missing.toml")), State::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn color_and_identity() {
        let all = defs();
        assert_eq!(all[1].color_rgb(), Some(0xe5484d));
        assert_eq!(all[0].color_rgb(), None);
        assert_eq!(all[1].identity().as_deref(), Some("Me <me@work.example>"));
    }

    /// The wrapper really runs the user's rc files and then the profile (needs zsh installed).
    #[test]
    fn zsh_wrapper_applies_the_profile_after_the_users_rc() {
        let Some(zsh) = ["/bin/zsh", "/usr/bin/zsh"].into_iter().find(|p| Path::new(p).exists()) else { return };
        let base = std::env::temp_dir().join(format!("mide-zsh-{}", std::process::id()));
        let (home, cfg) = (base.join("home"), base.join("cfg"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".zshrc"), "export OCI_CLI_PROFILE=FROM_RC\nexport FROM_USER_RC=yes\n").unwrap();
        let wrapper = install_zsh_wrapper(&cfg).unwrap();
        let all = defs();
        let file = base.join("p.env");
        std::fs::write(&file, env_script(Some(&all[1]), &all, &home)).unwrap();
        let out = std::process::Command::new(zsh)
            .args(["-i", "-c", "echo $OCI_CLI_PROFILE:$FROM_USER_RC:$MIDE_PROFILE:${ZDOTDIR-unset}"])
            .env_clear()
            .env("HOME", &home)
            .env("ZDOTDIR", &wrapper)
            .env("MIDE_PROFILE_FILE", &file)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        // The profile beat the rc file's value, the rc file still ran, and ZDOTDIR is the user's again.
        assert!(text.contains("WORK:yes:work:unset"), "got: {text} / {}", String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::remove_dir_all(base);
    }
}
