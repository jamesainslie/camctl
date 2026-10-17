//! Changelog-driven versioning.
//!
//! `CHANGELOG.md` (Keep a Changelog) is the source of truth for what a
//! release contains and therefore for its version. The next version is
//! derived from the categories in `## [Unreleased]`:
//!
//! - 0.x: a `Removed` entry or a `BREAKING` note bumps the minor version
//!   (0.x minor versions are Cargo's breaking boundary); anything else
//!   bumps the patch.
//! - 1.0 and later: breaking bumps the major, `Added`, `Changed` or
//!   `Deprecated` the minor, and `Fixed` or `Security` the patch.
//!
//! The very first release takes the version already in `Cargo.toml`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use semver::Version;

/// The Keep a Changelog categories. The version bump reads these names, so
/// any other spelling is an error rather than a silently smaller bump.
const CATEGORIES: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];

/// One `## [...]` section.
#[derive(Debug, Clone)]
pub struct Section {
    pub heading: Heading,
    /// Everything between this heading and the next, without the link
    /// reference lines at the end of the file.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heading {
    Unreleased,
    Release { version: Version, date: String },
}

impl Section {
    /// The `### Category` names in this section.
    #[must_use]
    pub fn categories(&self) -> Vec<&str> {
        self.body
            .lines()
            .filter_map(|l| l.strip_prefix("### "))
            .map(str::trim)
            .collect()
    }

    /// Whether any list entries are present.
    #[must_use]
    pub fn has_entries(&self) -> bool {
        self.body.lines().any(|l| {
            let t = l.trim_start();
            t.starts_with("- ") || t.starts_with("* ")
        })
    }

    fn is_breaking(&self) -> bool {
        self.categories().contains(&"Removed") || self.body.contains("BREAKING")
    }

    fn adds_features(&self) -> bool {
        self.categories()
            .iter()
            .any(|c| matches!(*c, "Added" | "Changed" | "Deprecated"))
    }
}

/// A parsed changelog.
#[derive(Debug, Clone)]
pub struct Changelog {
    preamble: String,
    sections: Vec<Section>,
    /// `[label]: url` reference lines, in file order.
    links: Vec<(String, String)>,
}

fn link_ref(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let (label, url) = rest.split_once("]: ")?;
    Some((label.to_owned(), url.trim().to_owned()))
}

fn heading(line: &str) -> Result<Option<Heading>> {
    let Some(rest) = line.strip_prefix("## [") else {
        return Ok(None);
    };
    let (label, tail) = rest
        .split_once(']')
        .with_context(|| format!("malformed heading: {line}"))?;
    if label == "Unreleased" {
        return Ok(Some(Heading::Unreleased));
    }
    let version =
        Version::parse(label).with_context(|| format!("bad version in heading: {line}"))?;
    let date = tail.trim().trim_start_matches('-').trim().to_owned();
    Ok(Some(Heading::Release { version, date }))
}

impl Changelog {
    /// Parse a Keep a Changelog file. It must have an `[Unreleased]` section.
    pub fn parse(text: &str) -> Result<Changelog> {
        let mut preamble = String::new();
        let mut sections: Vec<Section> = Vec::new();
        let mut links = Vec::new();
        for line in text.lines() {
            if let Some(link) = link_ref(line) {
                links.push(link);
                continue;
            }
            if let Some(h) = heading(line)? {
                sections.push(Section {
                    heading: h,
                    body: String::new(),
                });
                continue;
            }
            let target = match sections.last_mut() {
                Some(section) => &mut section.body,
                None => &mut preamble,
            };
            target.push_str(line);
            target.push('\n');
        }
        match sections
            .iter()
            .filter(|s| s.heading == Heading::Unreleased)
            .count()
        {
            0 => bail!("CHANGELOG.md has no ## [Unreleased] section"),
            1 => {}
            _ => {
                bail!("CHANGELOG.md has more than one ## [Unreleased] section; merge them into one")
            }
        }
        Ok(Changelog {
            preamble,
            sections,
            links,
        })
    }

    #[must_use]
    pub fn unreleased(&self) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.heading == Heading::Unreleased)
    }

    /// The newest released version.
    #[must_use]
    pub fn latest_release(&self) -> Option<Version> {
        self.sections
            .iter()
            .filter_map(|s| match &s.heading {
                Heading::Release { version, .. } => Some(version.clone()),
                Heading::Unreleased => None,
            })
            .max()
    }

    /// The body of the section for `version`, for release notes.
    #[must_use]
    pub fn notes(&self, version: &Version) -> Option<String> {
        self.sections.iter().find_map(|s| match &s.heading {
            Heading::Release { version: v, .. } if v == version => {
                Some(format!("{}\n", s.body.trim()))
            }
            _ => None,
        })
    }

    /// The version the unreleased changes should ship as, given the
    /// version in `Cargo.toml`.
    pub fn next_version(&self, current: &Version) -> Result<Version> {
        let unreleased = self
            .unreleased()
            .context("CHANGELOG.md has no ## [Unreleased] section")?;
        if !unreleased.has_entries() {
            bail!("nothing to release: ## [Unreleased] has no entries");
        }
        let Some(latest) = self.latest_release() else {
            return Ok(current.clone());
        };
        if *current != latest {
            bail!(
                "Cargo.toml says {current} but the changelog's latest release is {latest}; \
                 they must agree before a release"
            );
        }
        let mut next = latest.clone();
        let breaking = unreleased.is_breaking();
        if latest.major == 0 {
            if breaking {
                next.minor += 1;
                next.patch = 0;
            } else {
                next.patch += 1;
            }
        } else if breaking {
            next.major += 1;
            next.minor = 0;
            next.patch = 0;
        } else if unreleased.adds_features() {
            next.minor += 1;
            next.patch = 0;
        } else {
            next.patch += 1;
        }
        Ok(next)
    }

    /// What is inconsistent within the changelog, or between it, the
    /// workspace `version` and `repo`. Empty when all is well.
    #[must_use]
    pub fn problems(&self, version: &Version, repo: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if self.sections.first().map(|s| &s.heading) != Some(&Heading::Unreleased) {
            problems.push("## [Unreleased] must be the first section".to_owned());
        }
        let releases: Vec<(&Version, &str)> = self
            .sections
            .iter()
            .filter_map(|s| match &s.heading {
                Heading::Release { version, date } => Some((version, date.as_str())),
                Heading::Unreleased => None,
            })
            .collect();
        for section in &self.sections {
            let label = match &section.heading {
                Heading::Unreleased => "Unreleased".to_owned(),
                Heading::Release { version, .. } => version.to_string(),
            };
            for category in section.categories() {
                if !CATEGORIES.contains(&category) {
                    problems.push(format!(
                        "## [{label}] has an unknown category ### {category}; use one of {}",
                        CATEGORIES.join(", ")
                    ));
                }
            }
        }
        let mut seen = BTreeSet::new();
        for (v, date) in &releases {
            if !seen.insert(*v) {
                problems.push(format!("## [{v}] appears more than once"));
            }
            // Keep a Changelog marks a withdrawn release `[YANKED]`.
            let day = date.strip_suffix("[YANKED]").map_or(*date, str::trim_end);
            if !is_iso_date(day) {
                problems.push(format!("## [{v}] is dated {date:?}; use YYYY-MM-DD"));
            }
        }
        for pair in releases.windows(2) {
            let (newer, older) = (pair[0].0, pair[1].0);
            if newer < older {
                problems.push(format!(
                    "releases must run newest first, but [{newer}] comes before [{older}]"
                ));
            }
        }
        let latest = self.latest_release();
        if let Some(latest) = &latest
            && latest != version
        {
            problems.push(format!(
                "the latest release in CHANGELOG.md is {latest}, but Cargo.toml says {version}"
            ));
        }
        let unreleased = match &latest {
            Some(latest) => format!("{repo}/compare/v{latest}...HEAD"),
            None => format!("{repo}/commits/main"),
        };
        match self.link("Unreleased") {
            None => problems.push(format!(
                "[Unreleased] has no link; add [Unreleased]: {unreleased}"
            )),
            Some(url) if url != unreleased => problems.push(format!(
                "[Unreleased] links to {url}; it should be {unreleased}"
            )),
            Some(_) => {}
        }
        for (i, (v, _)) in releases.iter().enumerate() {
            let expected = match releases.get(i + 1) {
                Some((older, _)) => format!("{repo}/compare/v{older}...v{v}"),
                None => format!("{repo}/releases/tag/v{v}"),
            };
            match self.link(&v.to_string()) {
                None => problems.push(format!("[{v}] has no link; add [{v}]: {expected}")),
                Some(url) if url != expected => {
                    problems.push(format!("[{v}] links to {url}; it should be {expected}"));
                }
                Some(_) => {}
            }
        }
        problems
    }

    fn link(&self, label: &str) -> Option<&str> {
        self.links
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, url)| url.as_str())
    }

    /// The changelog with the unreleased entries released as `version`
    /// on `date`, a fresh empty `[Unreleased]`, and compare links for
    /// `repo` (e.g. `https://github.com/owner/name`).
    pub fn cut(&self, version: &Version, date: &str, repo: &str) -> Result<String> {
        let unreleased = self
            .unreleased()
            .context("CHANGELOG.md has no ## [Unreleased] section")?;
        let previous = self.latest_release();
        let mut out = self.preamble.clone();
        writeln!(out, "## [Unreleased]\n")?;
        writeln!(out, "## [{version}] - {date}\n")?;
        writeln!(out, "{}\n", unreleased.body.trim())?;
        for section in &self.sections {
            if let Heading::Release { version: v, date } = &section.heading {
                writeln!(out, "## [{v}] - {date}\n")?;
                writeln!(out, "{}\n", section.body.trim())?;
            }
        }
        writeln!(out, "[Unreleased]: {repo}/compare/v{version}...HEAD")?;
        match &previous {
            Some(prev) => writeln!(out, "[{version}]: {repo}/compare/v{prev}...v{version}")?,
            None => writeln!(out, "[{version}]: {repo}/releases/tag/v{version}")?,
        }
        for (label, url) in &self.links {
            if label != "Unreleased" && *label != version.to_string() {
                writeln!(out, "[{label}]: {url}")?;
            }
        }
        Ok(out)
    }
}

/// The `version` under `[workspace.package]` in a workspace manifest.
pub fn workspace_version(toml: &str) -> Result<Version> {
    let (_, line) = workspace_version_line(toml)?;
    let raw = line
        .split_once('=')
        .map(|(_, v)| v.trim().trim_matches('"'))
        .context("malformed version line")?;
    Version::parse(raw).with_context(|| format!("bad workspace version {raw}"))
}

/// The manifest with `[workspace.package] version` set to `version`.
pub fn set_workspace_version(toml: &str, version: &Version) -> Result<String> {
    let (index, _) = workspace_version_line(toml)?;
    let mut lines: Vec<String> = toml.lines().map(str::to_owned).collect();
    lines[index] = format!("version = \"{version}\"");
    let mut out = lines.join("\n");
    if toml.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn workspace_version_line(toml: &str) -> Result<(usize, &str)> {
    let mut in_package = false;
    for (i, line) in toml.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[workspace.package]";
            continue;
        }
        if in_package && t.starts_with("version") && t.contains('=') {
            return Ok((i, t));
        }
    }
    bail!("no version under [workspace.package] in Cargo.toml")
}

/// Whether `s` is a real calendar date written YYYY-MM-DD.
fn is_iso_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    let digits = |p: &str, n: usize| p.len() == n && p.bytes().all(|b| b.is_ascii_digit());
    if !(digits(year, 4) && digits(month, 2) && digits(day, 2)) {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

#[cfg(test)]
#[path = "changelog_tests.rs"]
mod tests;
