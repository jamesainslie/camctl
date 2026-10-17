//! camctl's task runner: `cargo xtask <task>`.
//!
//! Local runs and CI run the same tasks, so a green `cargo xtask ci` here
//! means a green CI there.

mod changelog;

use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// Format check, clippy with warnings denied, and the tests: exactly
    /// what CI runs.
    Ci,
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
    /// Print the workspace version.
    Version,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    eprintln!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root())
        .status()
        .with_context(|| format!("could not run {program}"))?;
    if !status.success() {
        bail!("{program} {} failed ({status})", args.join(" "));
    }
    Ok(())
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

/// The `repository` URL from the workspace manifest.
fn repository() -> Result<String> {
    read("Cargo.toml")?
        .lines()
        .find_map(|l| {
            let t = l.trim();
            t.strip_prefix("repository")
                .and_then(|r| r.split_once('='))
                .map(|(_, v)| v.trim().trim_matches('"').trim_end_matches('/').to_owned())
        })
        .context("no repository in [workspace.package]")
}

/// Cargo parallelism: `CAMCTL_JOBS`, else unlimited on CI, else 3 (a shared
/// developer machine).
fn jobs() -> Option<String> {
    match std::env::var("CAMCTL_JOBS") {
        Ok(j) => Some(j),
        Err(_) if std::env::var_os("CI").is_some() => None,
        Err(_) => Some("3".to_owned()),
    }
}

fn cargo(args: &[&str]) -> Result<()> {
    let jobs = jobs();
    let mut all: Vec<&str> = args.to_vec();
    if let Some(j) = &jobs {
        // Insert after the subcommand: `cargo clippy -j 3 ...`.
        all.insert(1, j);
        all.insert(1, "-j");
    }
    run("cargo", &all)
}

fn ci() -> Result<()> {
    run("cargo", &["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&["test", "--workspace"])
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
    let cut = log.cut(&next, &date, &repository()?)?;
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

fn dist() -> Result<()> {
    let version = current_version()?;
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    let dist = root().join("dist");
    std::fs::create_dir_all(&dist)?;

    run("scripts/bundle.sh", &["release"])?;
    cargo(&["build", "--release", "--package", "camctl-cli"])?;

    let app_zip = format!("dist/camctl-{version}-macos-{arch}.zip");
    let zip_app = |to: &str| {
        run(
            "ditto",
            &["-c", "-k", "--keepParent", "dist/camctl.app", to],
        )
    };
    zip_app(&app_zip)?;

    // Notarize when Developer ID credentials are present (CI secrets).
    if let (Ok(id), Ok(team), Ok(password)) = (
        std::env::var("APPLE_ID"),
        std::env::var("APPLE_TEAM_ID"),
        std::env::var("APPLE_APP_PASSWORD"),
    ) {
        run(
            "xcrun",
            &[
                "notarytool",
                "submit",
                &app_zip,
                "--apple-id",
                &id,
                "--team-id",
                &team,
                "--password",
                &password,
                "--wait",
            ],
        )?;
        run("xcrun", &["stapler", "staple", "dist/camctl.app"])?;
        std::fs::remove_file(root().join(&app_zip))?;
        zip_app(&app_zip)?;
    } else {
        eprintln!("no notarization credentials; the app is signed but not notarized");
    }

    let cli_tar = format!("dist/camctl-cli-{version}-macos-{arch}.tar.gz");
    run("tar", &["-czf", &cli_tar, "-C", "target/release", "camctl"])?;

    let sums = output("shasum", &["-a", "256", &app_zip, &cli_tar])?;
    // Name the files without the dist/ prefix, as users will see them.
    let mut sums = sums.replace("dist/", "");
    sums.push('\n');
    std::fs::write(dist.join("SHA256SUMS"), sums)?;
    println!("{app_zip}\n{cli_tar}\ndist/SHA256SUMS");
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::Ci => ci(),
        Task::ChangelogCheck { base } => changelog_check(&base),
        Task::Notes { version } => notes(version),
        Task::Release { version, dry_run } => release(version, dry_run),
        Task::Tag => tag(),
        Task::VerifyTag { tag } => verify_tag(&tag),
        Task::Dist => dist(),
        Task::Version => {
            println!("{}", current_version()?);
            Ok(())
        }
    }
}
