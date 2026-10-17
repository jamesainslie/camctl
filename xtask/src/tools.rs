//! The tools `cargo xtask lint` runs, pinned in `mise.toml`.
//!
//! CI installs exactly the pinned versions from that file. Locally, a
//! different version still runs, with a warning, since a tool's checks can
//! change between versions and CI's verdict is the one that counts.

use anyhow::{Context, Result, bail};
use semver::Version;
use toml::{Table, Value};

/// A tool and the exact version it is pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The tool's name, which is also its command.
    pub name: String,
    pub version: Version,
}

/// The `[tools]` pins in a `mise.toml`. Every pin must be an exact
/// version, so CI and local runs cannot drift apart silently.
pub fn pins(mise_toml: &str) -> Result<Vec<Pin>> {
    let config = mise_toml
        .parse::<Table>()
        .context("could not parse mise.toml")?;
    let Some(tools) = config.get("tools").and_then(Value::as_table) else {
        bail!("mise.toml has no [tools] table");
    };
    if tools.is_empty() {
        bail!("mise.toml pins no tools");
    }
    tools
        .iter()
        .map(|(name, value)| {
            let raw = value
                .as_str()
                .with_context(|| format!("mise.toml: {name} must be a version string"))?;
            let version = Version::parse(raw).with_context(|| {
                format!("mise.toml: pin {name} to an exact version such as 1.2.3, not {raw:?}")
            })?;
            Ok(Pin {
                name: name.clone(),
                version,
            })
        })
        .collect()
}

/// The first version number in a tool's `--version` output.
#[must_use]
pub fn reported_version(output: &str) -> Option<Version> {
    output
        .split(|c: char| c.is_whitespace() || c == ',')
        .find_map(|token| Version::parse(token.strip_prefix('v').unwrap_or(token)).ok())
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
