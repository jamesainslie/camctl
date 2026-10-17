//! The workspace's manifest rules, checked by `cargo xtask lint`.
//!
//! Every crate is a workspace member (otherwise nothing builds or tests
//! it), inherits the workspace's package settings and lints, and stays
//! unpublished. The declared `rust-version` is the pinned toolchain's,
//! because that is the only compiler CI proves the code against.

use anyhow::{Context, Result};
use semver::Version;
use toml::{Table, Value};

fn parse(text: &str, file: &str) -> Result<Table> {
    text.parse::<Table>()
        .with_context(|| format!("could not parse {file}"))
}

/// The `[workspace] members` list of the root manifest.
pub fn members(root: &str) -> Result<Vec<String>> {
    parse(root, "Cargo.toml")?
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(Value::as_array)
        .context("Cargo.toml has no [workspace] members")?
        .iter()
        .map(|m| {
            m.as_str()
                .map(str::to_owned)
                .context("Cargo.toml lists a workspace member that is not a string")
        })
        .collect()
}

/// A problem for each crate directory that is not a workspace member.
#[must_use]
pub fn unlisted(members: &[String], crate_dirs: &[String]) -> Vec<String> {
    crate_dirs
        .iter()
        .filter(|dir| !members.contains(dir))
        .map(|dir| format!("{dir} is not a workspace member, so nothing builds or tests it"))
        .collect()
}

/// Problems with the root manifest's `rust-version` against the channel
/// pinned in `rust-toolchain.toml`.
pub fn toolchain_problems(root: &str, toolchain: &str) -> Result<Vec<String>> {
    let declared = parse(root, "Cargo.toml")?
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("rust-version"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("Cargo.toml has no [workspace.package] rust-version")?;
    let channel = parse(toolchain, "rust-toolchain.toml")?
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("rust-toolchain.toml has no [toolchain] channel")?;

    let Ok(pinned) = Version::parse(&channel) else {
        return Ok(vec![format!(
            "rust-toolchain.toml pins channel {channel:?}; pin an exact version, such as 1.98.1"
        )]);
    };
    let Some(minimum) = rust_version(&declared) else {
        return Ok(vec![format!(
            "Cargo.toml rust-version {declared:?} is not a version"
        )]);
    };
    if (minimum.major, minimum.minor) != (pinned.major, pinned.minor) || minimum > pinned {
        return Ok(vec![format!(
            "Cargo.toml rust-version is {declared}, but rust-toolchain.toml pins {pinned}; \
             CI proves the code against {pinned} only, so declare {}.{}",
            pinned.major, pinned.minor
        )]);
    }
    Ok(Vec::new())
}

/// A `rust-version` (`1.98` or `1.98.1`) as a version.
fn rust_version(declared: &str) -> Option<Version> {
    Version::parse(declared)
        .or_else(|_| Version::parse(&format!("{declared}.0")))
        .ok()
}

/// Problems with the manifest of the member crate at `path`.
pub fn member_problems(path: &str, manifest: &str) -> Result<Vec<String>> {
    let file = format!("{path}/Cargo.toml");
    let manifest = parse(manifest, &file)?;
    let package = manifest
        .get("package")
        .and_then(Value::as_table)
        .with_context(|| format!("{file} has no [package]"))?;

    let mut problems = Vec::new();
    for key in ["version", "edition", "rust-version", "license"] {
        if !inherits(package.get(key)) {
            problems.push(format!(
                "{file}: set {key}.workspace = true, so the crate shares the workspace's {key}"
            ));
        }
    }
    if package.get("publish").and_then(Value::as_bool) != Some(false) {
        problems.push(format!(
            "{file}: set publish = false; camctl publishes no crates"
        ));
    }
    if !inherits(manifest.get("lints")) {
        problems.push(format!(
            "{file}: add [lints] workspace = true, so the workspace lints apply"
        ));
    }
    Ok(problems)
}

/// Whether a manifest value is `{ workspace = true }`.
fn inherits(value: Option<&Value>) -> bool {
    value
        .and_then(|v| v.get("workspace"))
        .and_then(Value::as_bool)
        == Some(true)
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
