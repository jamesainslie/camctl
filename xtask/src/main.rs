//! camctl's task runner: `cargo xtask <task>`.
//!
//! Local runs and CI run the same tasks, so a green `cargo xtask ci` here
//! means a green CI there.

mod changelog;
mod homebrew;
mod manifest;
mod tools;

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use semver::Version;

use crate::changelog::{Changelog, set_workspace_version, workspace_version};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "camctl's build, check and release tasks"
)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Every check CI runs: `lint`, then `check`.
    Ci,
    /// The checks that need no build: formatting, the workspace rules, the
    /// workflows, the shell scripts and the dependency policy.
    Lint,
    /// The checks that build, on macOS: Clippy, rustdoc and the tests with
    /// warnings denied, then smoke tests of the CLI and the app bundle.
    Check,
    /// Use the repository's git hooks: a format check before each commit
    /// and `cargo xtask ci` before each push.
    InstallHooks,
    /// Fail unless CHANGELOG.md changed relative to `--base` (for PRs).
    ChangelogCheck {
        #[arg(long, default_value = "origin/main")]
        base: String,
    },
    /// Print the release notes for a version (default: the current one).
    Notes { version: Option<String> },
    /// Prepare a release on the current branch: cut the changelog, set the
    /// workspace version, and commit. Opens no PR and pushes nothing.
    Release {
        /// Override the version derived from the changelog.
        #[arg(long)]
        version: Option<String>,
        /// Show what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Tag HEAD with the version in Cargo.toml, after the release PR has
    /// merged. Pushes nothing.
    Tag,
    /// Check that a pushed tag matches Cargo.toml and the changelog.
    VerifyTag { tag: String },
    /// Build the release artifacts into dist/.
    Dist,
    /// Write the Homebrew cask (the app) and formula (the CLI) for the
    /// current version into a clone of yaklabco/homebrew-tap.
    Homebrew {
        /// The tap clone to write into.
        #[arg(long)]
        tap: PathBuf,
        /// The release's checksums (default: dist/SHA256SUMS, as
        /// `cargo xtask dist` writes them).
        #[arg(long)]
        sums: Option<PathBuf>,
    },
    /// Print the workspace version.
    Version,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not a UTF-8 path", path.display()))
}

/// A script in `scripts/`, by absolute path.
fn script(name: &str) -> Result<String> {
    Ok(utf8(&root().join("scripts").join(name))?.to_owned())
}

/// Run `program` in the workspace root, with `envs` added to its
/// environment, and fail if it fails. Arguments listed in `secrets` show
/// as `***` in the echoed command and in errors.
fn run_masked(program: &str, args: &[&str], envs: &[(&str, &str)], secrets: &[&str]) -> Result<()> {
    let mut shown = String::new();
    for (k, v) in envs {
        write!(shown, "{k}='{v}' ")?;
    }
    let shown_args: Vec<&str> = args
        .iter()
        .map(|a| if secrets.contains(a) { "***" } else { a })
        .collect();
    let shown_args = shown_args.join(" ");
    eprintln!("$ {shown}{program} {shown_args}");
    let status = Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        .current_dir(root())
        .status()
        .with_context(|| format!("could not run {program}"))?;
    if !status.success() {
        bail!("{program} {shown_args} failed ({status})");
    }
    Ok(())
}

fn run_with(program: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    run_masked(program, args, envs, &[])
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    run_with(program, args, &[])
}

fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(root())
        .output()
        .with_context(|| format!("could not run {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn read(name: &str) -> Result<String> {
    std::fs::read_to_string(root().join(name)).with_context(|| format!("could not read {name}"))
}

fn write(name: &str, text: &str) -> Result<()> {
    std::fs::write(root().join(name), text).with_context(|| format!("could not write {name}"))
}

fn current_version() -> Result<Version> {
    workspace_version(&read("Cargo.toml")?)
}

/// The `[workspace.package] repository` URL, without a trailing slash.
fn repository() -> Result<String> {
    let manifest: toml::Table = read("Cargo.toml")?
        .parse()
        .context("could not parse Cargo.toml")?;
    manifest
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("repository"))
        .and_then(toml::Value::as_str)
        .map(|r| r.trim_end_matches('/').to_owned())
        .context("Cargo.toml has no [workspace.package] repository")
}

fn on_ci() -> bool {
    std::env::var_os("CI").is_some()
}

/// Cargo parallelism: `CAMCTL_JOBS`, else unlimited on CI, else 3 (a shared
/// developer machine).
fn jobs() -> Option<String> {
    match std::env::var("CAMCTL_JOBS") {
        Ok(j) => Some(j),
        Err(_) if on_ci() => None,
        Err(_) => Some("3".to_owned()),
    }
}

/// `args` with the job limit inserted after the subcommand:
/// `cargo clippy -j 3 ...`.
fn with_jobs(args: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = args.iter().map(|&a| a.to_owned()).collect();
    if let Some(j) = jobs() {
        all.splice(1..1, ["-j".to_owned(), j]);
    }
    all
}

fn cargo_with(args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    let all = with_jobs(args);
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    run_with("cargo", &all, envs)
}

fn cargo(args: &[&str]) -> Result<()> {
    cargo_with(args, &[])
}

/// Run `cargo build` with `args` and return the executable it built for
/// the binary `bin`, as cargo reports it. That way `CARGO_TARGET_DIR`,
/// `build.target-dir` and `--target` all lead to the right file.
fn cargo_build(args: &[&str], bin: &str) -> Result<PathBuf> {
    let mut build = vec!["build", "--message-format=json-render-diagnostics"];
    build.extend_from_slice(args);
    let all = with_jobs(&build);
    eprintln!("$ cargo {}", all.join(" "));
    let out = Command::new("cargo")
        .args(&all)
        .current_dir(root())
        .stderr(Stdio::inherit())
        .output()
        .context("could not run cargo")?;
    if !out.status.success() {
        bail!("cargo {} failed ({})", all.join(" "), out.status);
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == bin)
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
        .with_context(|| format!("cargo built no executable named {bin}"))
}

/// Cargo's target directory, as cargo itself resolves it.
fn target_dir() -> Result<PathBuf> {
    let metadata = output(
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps", "--locked"],
    )?;
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata).context("cargo metadata printed invalid JSON")?;
    metadata["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .context("cargo metadata has no target_directory")
}

/// A workflow command's message, escaped as GitHub Actions requires.
fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A workflow command's property value, which also may not hold `:` or `,`.
fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

/// Run one named step, as a collapsible group in GitHub Actions logs, and
/// report how long it took.
fn step(name: &str, f: impl FnOnce() -> Result<()>) -> Result<()> {
    let actions = std::env::var_os("GITHUB_ACTIONS").is_some();
    if actions {
        eprintln!("::group::{name}");
    } else {
        eprintln!("\n==> {name}");
    }
    let start = Instant::now();
    let result = f();
    let took = start.elapsed().as_secs_f64();
    if actions {
        eprintln!("::endgroup::");
    }
    match &result {
        Ok(()) => eprintln!("ok: {name} ({took:.1}s)"),
        Err(e) if actions => eprintln!(
            "::error title={}::{}",
            escape_property(&format!("{name} failed")),
            escape_data(&format!("{e:#}"))
        ),
        Err(_) => eprintln!("FAILED: {name} ({took:.1}s)"),
    }
    result.with_context(|| format!("{name} failed"))
}

/// Fail with every error in `results`, so independent steps all report.
fn all_passed(what: &str, results: impl IntoIterator<Item = Result<()>>) -> Result<()> {
    let failed: Vec<String> = results
        .into_iter()
        .filter_map(Result::err)
        .map(|e| format!("{e:#}"))
        .collect();
    if failed.is_empty() {
        return Ok(());
    }
    bail!(
        "{} {what} step(s) failed:\n  - {}",
        failed.len(),
        failed.join("\n  - ")
    )
}

fn ci() -> Result<()> {
    lint()?;
    check()
}

/// The command for a pinned tool. When mise is installed, that is mise's
/// install of the version mise.toml pins, regardless of what else is on
/// PATH (cargo and rustup put `~/.cargo/bin` first). Otherwise it is the
/// tool on PATH.
fn tool(name: &str) -> String {
    Command::new("mise")
        .args(["which", name])
        .current_dir(root())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

/// The lint steps are independent and quick, so all of them run and every
/// failure is reported at the end.
fn lint() -> Result<()> {
    let shellcheck = tool("shellcheck");
    all_passed(
        "lint",
        [
            step("rustfmt", || run("cargo", &["fmt", "--all", "--check"])),
            step("workspace rules", workspace_rules),
            step("pinned tools", require_tools),
            step("workflows: actionlint and shellcheck", || {
                run(&tool("actionlint"), &[&format!("-shellcheck={shellcheck}")])
            }),
            step("workflows: zizmor", || {
                run(
                    &tool("zizmor"),
                    &[
                        "--offline",
                        "--no-progress",
                        "--persona",
                        "pedantic",
                        ".github",
                    ],
                )
            }),
            step("shell scripts: shellcheck", || {
                let scripts = shell_scripts()?;
                let scripts: Vec<&str> = scripts.iter().map(String::as_str).collect();
                run(&shellcheck, &scripts)
            }),
            step("dependencies: cargo-deny", || {
                run(
                    &tool("cargo-deny"),
                    &["--locked", "check", "--deny", "warnings"],
                )
            }),
        ],
    )
}

fn check() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("cargo xtask check builds the macOS-only crates; run it on macOS");
    }
    step("clippy", || {
        cargo(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ])
    })?;
    step("rustdoc", || {
        cargo_with(
            &[
                "doc",
                "--workspace",
                "--no-deps",
                "--document-private-items",
                "--locked",
            ],
            &[("RUSTDOCFLAGS", "-D warnings")],
        )
    })?;
    step("tests", || {
        cargo(&[
            "test",
            "--workspace",
            "--all-features",
            "--locked",
            "--no-fail-fast",
        ])
    })?;
    step("smoke test: CLI", smoke_cli)?;
    step("smoke test: app bundle", smoke_bundle)
}

/// The workspace manifests, the toolchain pin and the changelog agree with
/// each other and with the project's rules.
fn workspace_rules() -> Result<()> {
    let manifest = read("Cargo.toml")?;
    let members = manifest::members(&manifest)?;
    let mut problems = manifest::unlisted(&members, &crate_dirs()?);
    problems.extend(manifest::toolchain_problems(
        &manifest,
        &read("rust-toolchain.toml")?,
    )?);
    for member in &members {
        problems.extend(manifest::member_problems(
            member,
            &read(&format!("{member}/Cargo.toml"))?,
        )?);
    }
    let log = Changelog::parse(&read("CHANGELOG.md")?)?;
    problems.extend(log.problems(&workspace_version(&manifest)?, &repository()?));
    if !problems.is_empty() {
        bail!(
            "{} workspace rule(s) broken:\n  - {}",
            problems.len(),
            problems.join("\n  - ")
        );
    }
    eprintln!(
        "{} members, rust-toolchain.toml and CHANGELOG.md agree",
        members.len()
    );
    Ok(())
}

/// Every directory under `crates/` with a manifest, as `crates/<name>`.
fn crate_dirs() -> Result<Vec<String>> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(root().join("crates")).context("could not list crates/")? {
        let entry = entry?;
        if entry.path().join("Cargo.toml").is_file() {
            dirs.push(format!("crates/{}", entry.file_name().to_string_lossy()));
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// The tools pinned in mise.toml are installed, at exactly those versions
/// on CI. Every problem is reported, not just the first.
fn require_tools() -> Result<()> {
    let pins = tools::pins(&read("mise.toml")?)?;
    let mut problems = Vec::new();
    for pin in &pins {
        let out = Command::new(tool(&pin.name))
            .arg("--version")
            .current_dir(root())
            .output();
        let found = match out {
            Ok(out) if out.status.success() => {
                tools::reported_version(&String::from_utf8_lossy(&out.stdout))
            }
            _ => {
                problems.push(format!(
                    "{} is not installed (mise.toml pins {})",
                    pin.name, pin.version
                ));
                continue;
            }
        };
        if found.as_ref() == Some(&pin.version) {
            eprintln!("{} {}", pin.name, pin.version);
            continue;
        }
        let found = found.map_or_else(|| "an unknown version".to_owned(), |v| v.to_string());
        let mismatch = format!(
            "{} is {found}, but mise.toml pins {}",
            pin.name, pin.version
        );
        if on_ci() {
            problems.push(mismatch);
        } else {
            eprintln!("warning: {mismatch}; run `mise install` so local checks match CI");
        }
    }
    if !problems.is_empty() {
        bail!(
            "{}; install the pinned tools with `mise install`",
            problems.join("; ")
        );
    }
    Ok(())
}

/// The shell scripts to lint: `scripts/*.sh` and the git hooks.
fn shell_scripts() -> Result<Vec<String>> {
    let mut scripts = Vec::new();
    for (dir, only_sh) in [("scripts", true), (".githooks", false)] {
        for entry in
            std::fs::read_dir(root().join(dir)).with_context(|| format!("could not list {dir}/"))?
        {
            let path = entry?.path();
            if path.is_file() && (!only_sh || path.extension().is_some_and(|e| e == "sh")) {
                let name = path.file_name().context("a script has no name")?;
                scripts.push(format!("{dir}/{}", name.to_string_lossy()));
            }
        }
    }
    scripts.sort();
    Ok(scripts)
}

/// The CLI builds, reports the workspace version, and its argument
/// definitions hold up for every subcommand.
fn smoke_cli() -> Result<()> {
    let camctl = cargo_build(&["--locked", "--package", "camctl-cli"], "camctl")?;
    let camctl = utf8(&camctl)?;
    let want = format!("camctl {}", current_version()?);
    let got = output(camctl, &["--version"])?;
    if got != want {
        bail!("camctl --version printed {got:?}, expected {want:?}");
    }
    for command in [
        "list", "controls", "formats", "get", "set", "point", "home", "sleep", "wake",
    ] {
        output(camctl, &[command, "--help"])?;
    }
    eprintln!("{got}; every subcommand's --help works");
    Ok(())
}

/// The app bundles and signs (ad hoc, under the target directory, never
/// touching dist/), and its Info.plist has what macOS needs to run it.
fn smoke_bundle() -> Result<()> {
    let dir = target_dir()?.join("smoke");
    run_with(
        &script("bundle.sh")?,
        &["debug"],
        &[
            ("CAMCTL_SIGN_IDENTITY", "-"),
            ("CAMCTL_BUNDLE_DIR", utf8(&dir)?),
        ],
    )?;
    let app = dir.join("camctl.app");
    run(
        "codesign",
        &["--verify", "--strict", "--verbose=2", utf8(&app)?],
    )?;
    let plist = app.join("Contents/Info.plist");
    let plist = utf8(&plist)?;
    run("plutil", &["-lint", plist])?;
    let extract = |key: &str| {
        output("plutil", &["-extract", key, "raw", "-o", "-", plist])
            .with_context(|| format!("Info.plist has no {key}"))
    };
    let version = current_version()?.to_string();
    for (key, want) in [
        ("CFBundleIdentifier", "us.ainslies.camctl"),
        ("CFBundleExecutable", "camctl"),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleShortVersionString", version.as_str()),
        ("CFBundleVersion", version.as_str()),
        ("LSUIElement", "true"),
    ] {
        let got = extract(key)?;
        if got != want {
            bail!("Info.plist {key} is {got:?}, expected {want:?}");
        }
    }
    if extract("NSCameraUsageDescription")?.is_empty() {
        bail!(
            "Info.plist needs an NSCameraUsageDescription, or macOS kills the app when it opens the camera"
        );
    }
    let exe = app.join("Contents/MacOS/camctl");
    let mode = std::fs::metadata(&exe)
        .with_context(|| format!("{} is missing", exe.display()))?
        .permissions()
        .mode();
    if mode & 0o111 == 0 {
        bail!("{} is not executable", exe.display());
    }
    eprintln!("{} is signed and its Info.plist is complete", app.display());
    Ok(())
}

/// Point this clone's `core.hooksPath` at `.githooks`. Each hook there ends
/// by running the global hook of the same name, so global hooks keep
/// working.
fn install_hooks() -> Result<()> {
    for hook in ["pre-commit", "pre-push", "commit-msg", "run-global"] {
        let path = root().join(".githooks").join(hook);
        let mode = std::fs::metadata(&path)
            .with_context(|| format!("{} is missing", path.display()))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            bail!("{} is not executable", path.display());
        }
    }
    let local = Command::new("git")
        .args(["config", "--local", "--get", "core.hooksPath"])
        .current_dir(root())
        .output()
        .context("could not run git")?;
    let local = String::from_utf8_lossy(&local.stdout).trim().to_owned();
    if !local.is_empty() && local != ".githooks" {
        bail!(
            "this clone already sets core.hooksPath to {local}; unset it first \
             (git config --local --unset core.hooksPath) if .githooks should replace it"
        );
    }
    run("git", &["config", "--local", "core.hooksPath", ".githooks"])?;
    println!(
        "git now runs .githooks/pre-commit (a format check) before each commit \
         and .githooks/pre-push (cargo xtask ci) before each push. Each then \
         runs the global hook of the same name, if there is one. Undo with \
         `git config --local --unset core.hooksPath`."
    );
    Ok(())
}

fn require_clean_tree() -> Result<()> {
    let status = output("git", &["status", "--porcelain"])?;
    if !status.is_empty() {
        bail!("the working tree has uncommitted changes:\n{status}");
    }
    Ok(())
}

fn tag_exists(tag: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")])
        .current_dir(root())
        .output()
        .is_ok_and(|o| o.status.success())
}

fn changelog_check(base: &str) -> Result<()> {
    let changed = output("git", &["diff", "--name-only", &format!("{base}...HEAD")])?;
    if !changed.lines().any(|f| f == "CHANGELOG.md") {
        bail!(
            "CHANGELOG.md was not updated. Add an entry under ## [Unreleased], \
             or label the PR skip-changelog if it changes nothing user-visible."
        );
    }
    let log = Changelog::parse(&read("CHANGELOG.md")?)?;
    eprintln!(
        "CHANGELOG.md updated; next release would be {}",
        log.next_version(&current_version()?)
            .map_or_else(|e| format!("(none: {e})"), |v| v.to_string())
    );
    Ok(())
}

fn notes(version: Option<String>) -> Result<()> {
    let version = match version {
        Some(v) => Version::parse(v.trim_start_matches('v'))?,
        None => current_version()?,
    };
    let log = Changelog::parse(&read("CHANGELOG.md")?)?;
    let notes = log
        .notes(&version)
        .with_context(|| format!("CHANGELOG.md has no ## [{version}] section"))?;
    print!("{notes}");
    Ok(())
}

fn release(version: Option<String>, dry_run: bool) -> Result<()> {
    if !dry_run {
        require_clean_tree()?;
    }
    let manifest = read("Cargo.toml")?;
    let current = workspace_version(&manifest)?;
    let log = Changelog::parse(&read("CHANGELOG.md")?)?;
    let next = match version {
        Some(v) => Version::parse(v.trim_start_matches('v'))?,
        None => log.next_version(&current)?,
    };
    if !log
        .unreleased()
        .is_some_and(changelog::Section::has_entries)
    {
        bail!("nothing to release: ## [Unreleased] has no entries");
    }
    let tag = format!("v{next}");
    if tag_exists(&tag) {
        bail!("tag {tag} already exists");
    }
    let date = output("date", &["+%Y-%m-%d"])?;
    let repo = repository()?;
    let cut = log.cut(&next, &date, &repo)?;
    // Refuse a release whose changelog `cargo xtask lint` would reject.
    let problems = Changelog::parse(&cut)?.problems(&next, &repo);
    if !problems.is_empty() {
        bail!(
            "releasing {tag} would leave CHANGELOG.md inconsistent:\n  - {}",
            problems.join("\n  - ")
        );
    }
    if dry_run {
        println!("would release {current} -> {next} ({tag}) on {date}");
        print!(
            "{}",
            Changelog::parse(&cut)?.notes(&next).unwrap_or_default()
        );
        return Ok(());
    }
    write("CHANGELOG.md", &cut)?;
    write("Cargo.toml", &set_workspace_version(&manifest, &next)?)?;
    // Bring the workspace packages' versions in Cargo.lock up to date.
    run("cargo", &["update", "--workspace", "--offline"])?;
    run("git", &["add", "CHANGELOG.md", "Cargo.toml", "Cargo.lock"])?;
    run("git", &["commit", "-m", &format!("chore(release): {tag}")])?;
    println!(
        "Prepared {tag}. Next: push this branch, open a PR, merge it, then run \
         `cargo xtask tag` on main and push the tag."
    );
    Ok(())
}

fn tag() -> Result<()> {
    require_clean_tree()?;
    let version = current_version()?;
    let log = Changelog::parse(&read("CHANGELOG.md")?)?;
    if log.notes(&version).is_none() {
        bail!("CHANGELOG.md has no ## [{version}] section; run `cargo xtask release` first");
    }
    let tag = format!("v{version}");
    if tag_exists(&tag) {
        bail!("tag {tag} already exists");
    }
    run(
        "git",
        &["tag", "-a", &tag, "-m", &format!("camctl {version}")],
    )?;
    println!("Tagged {tag}. Push it to publish: git push origin {tag}");
    Ok(())
}

fn verify_tag(tag: &str) -> Result<()> {
    let version = current_version()?;
    if tag != format!("v{version}") {
        bail!("tag {tag} does not match Cargo.toml version {version}");
    }
    let log = Changelog::parse(&read("CHANGELOG.md")?)?;
    if log.notes(&version).is_none() {
        bail!("CHANGELOG.md has no ## [{version}] section");
    }
    eprintln!("{tag} matches Cargo.toml and CHANGELOG.md");
    Ok(())
}

/// How `notarytool` signs in to Apple's notary service.
#[derive(Debug, PartialEq, Eq)]
enum Notary {
    /// A keychain profile saved by `xcrun notarytool store-credentials`,
    /// named by `CAMCTL_NOTARY_PROFILE`: for releases built locally, with
    /// no password in the environment.
    Profile(String),
    /// `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD`: for CI.
    AppleId {
        id: String,
        team: String,
        password: String,
    },
}

/// The notary credentials `var` provides, if complete. A profile wins;
/// an empty variable counts as unset.
fn notary(var: impl Fn(&str) -> Option<String>) -> Option<Notary> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    if let Some(profile) = set("CAMCTL_NOTARY_PROFILE") {
        return Some(Notary::Profile(profile));
    }
    Some(Notary::AppleId {
        id: set("APPLE_ID")?,
        team: set("APPLE_TEAM_ID")?,
        password: set("APPLE_APP_PASSWORD")?,
    })
}

fn dist() -> Result<()> {
    let version = current_version()?;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    let dist = root().join("dist");
    std::fs::create_dir_all(&dist)?;

    // Pinned to dist/, whatever CAMCTL_BUNDLE_DIR the caller has set.
    run_with(
        &script("bundle.sh")?,
        &["release"],
        &[("CAMCTL_BUNDLE_DIR", "dist")],
    )?;
    let cli = cargo_build(
        &["--release", "--locked", "--package", "camctl-cli"],
        "camctl",
    )?;
    let cli_dir = cli.parent().context("the CLI has no parent directory")?;

    let app_zip = format!("dist/camctl-{version}-macos-{arch}.zip");
    let zip_app = |to: &str| {
        run(
            "ditto",
            &["-c", "-k", "--keepParent", "dist/camctl.app", to],
        )
    };
    zip_app(&app_zip)?;

    // Notarize when credentials are present: a keychain profile locally,
    // the Apple ID secrets on CI.
    if let Some(credentials) = notary(|name| std::env::var(name).ok()) {
        match &credentials {
            Notary::Profile(profile) => run(
                "xcrun",
                &[
                    "notarytool",
                    "submit",
                    &app_zip,
                    "--keychain-profile",
                    profile,
                    "--wait",
                ],
            )?,
            Notary::AppleId { id, team, password } => run_masked(
                "xcrun",
                &[
                    "notarytool",
                    "submit",
                    &app_zip,
                    "--apple-id",
                    id,
                    "--team-id",
                    team,
                    "--password",
                    password,
                    "--wait",
                ],
                &[],
                &[password],
            )?,
        }
        run("xcrun", &["stapler", "staple", "dist/camctl.app"])?;
        std::fs::remove_file(root().join(&app_zip))?;
        zip_app(&app_zip)?;
    } else {
        eprintln!("no notarization credentials; the app is signed but not notarized");
    }

    let cli_tar = format!("dist/camctl-cli-{version}-macos-{arch}.tar.gz");
    run("tar", &["-czf", &cli_tar, "-C", utf8(cli_dir)?, "camctl"])?;

    let sums = output("shasum", &["-a", "256", &app_zip, &cli_tar])?;
    // Name the files without the dist/ prefix, as users will see them.
    let mut sums = sums.replace("dist/", "");
    sums.push('\n');
    std::fs::write(dist.join("SHA256SUMS"), sums)?;
    println!("{app_zip}\n{cli_tar}\ndist/SHA256SUMS");
    Ok(())
}

fn homebrew_task(tap: &Path, sums: &Path) -> Result<()> {
    let version = current_version()?;
    let sums =
        std::fs::read_to_string(sums).with_context(|| format!("reading {}", sums.display()))?;
    for path in homebrew::write(tap, &version, &sums)? {
        println!("{}", path.display());
    }
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Ci => ci(),
        Task::Lint => lint(),
        Task::Check => check(),
        Task::InstallHooks => install_hooks(),
        Task::ChangelogCheck { base } => changelog_check(&base),
        Task::Notes { version } => notes(version),
        Task::Release { version, dry_run } => release(version, dry_run),
        Task::Tag => tag(),
        Task::VerifyTag { tag } => verify_tag(&tag),
        Task::Dist => dist(),
        Task::Homebrew { tap, sums } => homebrew_task(
            &tap,
            &sums.unwrap_or_else(|| root().join("dist").join("SHA256SUMS")),
        ),
        Task::Version => {
            println!("{}", current_version()?);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::{Cli, Notary, all_passed, escape_data, escape_property, notary};

    #[test]
    fn the_command_line_definition_is_valid() {
        Cli::command().debug_assert();
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn a_keychain_profile_notarizes_without_a_password_in_the_environment() {
        let vars = [
            ("CAMCTL_NOTARY_PROFILE", "camctl-notary"),
            ("APPLE_ID", "me@example.com"),
        ];
        assert_eq!(
            notary(env(&vars)),
            Some(Notary::Profile("camctl-notary".into()))
        );
    }

    #[test]
    fn an_apple_id_needs_its_team_and_password() {
        let all = [
            ("APPLE_ID", "me@example.com"),
            ("APPLE_TEAM_ID", "TEAM123456"),
            ("APPLE_APP_PASSWORD", "abcd-efgh"),
        ];
        assert_eq!(
            notary(env(&all)),
            Some(Notary::AppleId {
                id: "me@example.com".into(),
                team: "TEAM123456".into(),
                password: "abcd-efgh".into(),
            })
        );
        let partial = [
            ("APPLE_ID", "me@example.com"),
            ("APPLE_TEAM_ID", "TEAM123456"),
        ];
        assert_eq!(notary(env(&partial)), None);
        let empty = [
            ("APPLE_ID", "me@example.com"),
            ("APPLE_TEAM_ID", "TEAM123456"),
            ("APPLE_APP_PASSWORD", ""),
        ];
        assert_eq!(notary(env(&empty)), None);
        assert_eq!(notary(env(&[("CAMCTL_NOTARY_PROFILE", "")])), None);
    }

    #[test]
    fn annotations_escape_their_delimiters() {
        assert_eq!(escape_data("50% done\r\nnext"), "50%25 done%0D%0Anext");
        assert_eq!(
            escape_property("lint: rustfmt, clippy"),
            "lint%3A rustfmt%2C clippy"
        );
    }

    #[test]
    fn every_failed_step_is_reported() {
        let err = all_passed(
            "lint",
            [Ok(()), Err(anyhow::anyhow!("a")), Err(anyhow::anyhow!("b"))],
        )
        .unwrap_err()
        .to_string();
        assert!(err.starts_with("2 lint step(s) failed"), "{err}");
        assert!(err.contains("- a") && err.contains("- b"), "{err}");
        assert!(all_passed("lint", [Ok(()), Ok(())]).is_ok());
    }
}
