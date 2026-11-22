# Releasing mide

## One-time setup

1. **GitHub Pages**: repository Settings, Pages, *Build and deployment*, Source *Deploy from a branch*,
   Branch `main`, folder `/docs`. The site appears at `https://<user>.github.io/<repo>/`.
2. Edit `site-src/config.json`: set `"repo"` to `<user>/<repo>`, then `python3 site-src/build.py` and commit `docs/`.
3. (Recommended) **Apple Developer ID** so the installer opens without a warning. It needs a paid Apple Developer
   account. Create two certificates, *Developer ID Application* and *Developer ID Installer*, and an app-specific
   password for notarization. Add these repository secrets:

   | Secret | Value |
   |---|---|
   | `MACOS_CERTIFICATE` | the two certificates exported together as a `.p12`, base64-encoded |
   | `MACOS_CERTIFICATE_PWD` | the `.p12` password |
   | `DEVELOPER_ID_APP` | `Developer ID Application: Your Name (TEAMID)` |
   | `DEVELOPER_ID_INSTALLER` | `Developer ID Installer: Your Name (TEAMID)` |
   | `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | for `notarytool` |

   Without them the release workflow still works; the packages are just unsigned.

## Each release

1. Bump `version` in `crates/app/Cargo.toml`; commit.
2. Refresh the benchmark data if performance changed: `scripts/perf.sh --json`, then `python3 site-src/build.py`,
   commit `site-src/data` and `docs`.
3. `git tag v0.1.0 && git push --tags`. The *Release* workflow tests, builds the arm64 and universal packages,
   signs and notarizes them if the secrets exist, and attaches them (with `.sha256` files) to a GitHub release.

## Building a package by hand

```sh
scripts/pkg.sh                  # this Mac's architecture, unsigned
scripts/pkg.sh --universal      # needs the x86_64-apple-darwin Rust target

DEVELOPER_ID_APP="Developer ID Application: ..." \
DEVELOPER_ID_INSTALLER="Developer ID Installer: ..." \
NOTARY_PROFILE=mide-notary \
scripts/pkg.sh                  # signed, notarized, stapled
```

Create the notary profile once with `xcrun notarytool store-credentials mide-notary`.

## Before publishing the repository

* `reference/` holds screenshots of private projects. It is in `.gitignore`; keep it that way.
* `.cargo/config.toml` is machine-specific and also ignored.
* Run `python3 scripts/licenses.py --check` (CI does too) after adding dependencies.
