use semver::Version;

use super::*;

const MISE: &str = "# Pinned tools.\n[tools]\nactionlint = \"1.7.12\"\ncargo-deny = \"0.20.2\"\nshellcheck = \"0.11.0\"\n";

#[test]
fn reads_the_pins() {
    let p = pins(MISE).unwrap();
    let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["actionlint", "cargo-deny", "shellcheck"]);
    assert_eq!(p[1].version, Version::new(0, 20, 2));
}

#[test]
fn pins_are_exact_versions() {
    for loose in ["latest", "1.7", "^1.7.12", "1", "lts"] {
        assert!(pins(&MISE.replace("1.7.12", loose)).is_err(), "{loose}");
    }
}

#[test]
fn a_table_pin_is_rejected() {
    let table = MISE.replace(
        "cargo-deny = \"0.20.2\"",
        "cargo-deny = { version = \"0.20.2\" }",
    );
    assert!(pins(&table).is_err());
}

#[test]
fn a_config_without_tools_is_an_error() {
    assert!(pins("[env]\nX = \"1\"\n").is_err());
    assert!(pins("[tools]\n").is_err());
}

#[test]
fn reads_versions_from_tool_output() {
    assert_eq!(
        reported_version("cargo-deny 0.20.2\n"),
        Some(Version::new(0, 20, 2))
    );
    assert_eq!(
        reported_version(
            "1.7.12\ninstalled by downloading from release page\nbuilt with go1.26.1 compiler for darwin/arm64\n"
        ),
        Some(Version::new(1, 7, 12))
    );
    assert_eq!(reported_version("v1.7.12\n"), Some(Version::new(1, 7, 12)));
    assert_eq!(
        reported_version(
            "ShellCheck - shell script analysis tool\nversion: 0.11.0\nlicense: GNU General Public License, version 3\n"
        ),
        Some(Version::new(0, 11, 0))
    );
    assert_eq!(reported_version("no version here\n"), None);
}
