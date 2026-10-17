use super::*;

const ROOT: &str = r#"[workspace]
members = ["crates/a", "xtask"]

[workspace.package]
version = "0.1.0"
rust-version = "1.98"
"#;

const TOOLCHAIN: &str = "[toolchain]\nchannel = \"1.98.1\"\n";

const MEMBER: &str = r#"[package]
name = "a"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[lints]
workspace = true
"#;

fn dirs(names: &[&str]) -> Vec<String> {
    names.iter().map(|&n| n.to_owned()).collect()
}

#[test]
fn reads_the_member_list() {
    assert_eq!(members(ROOT).unwrap(), dirs(&["crates/a", "xtask"]));
}

#[test]
fn every_crate_directory_is_a_member() {
    let members = dirs(&["crates/a"]);
    assert!(unlisted(&members, &dirs(&["crates/a"])).is_empty());
    let p = unlisted(&members, &dirs(&["crates/a", "crates/b"]));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("crates/b"), "{p:?}");
}

#[test]
fn rust_version_matches_the_pinned_toolchain() {
    assert!(toolchain_problems(ROOT, TOOLCHAIN).unwrap().is_empty());
    let exact = ROOT.replace("\"1.98\"", "\"1.98.1\"");
    assert!(toolchain_problems(&exact, TOOLCHAIN).unwrap().is_empty());

    let behind = ROOT.replace("1.98", "1.90");
    let p = toolchain_problems(&behind, TOOLCHAIN).unwrap();
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("1.90") && p[0].contains("1.98.1"), "{p:?}");

    let ahead = ROOT.replace("1.98", "1.99");
    assert_eq!(toolchain_problems(&ahead, TOOLCHAIN).unwrap().len(), 1);
}

#[test]
fn the_toolchain_channel_is_an_exact_version() {
    for channel in ["stable", "1.98", "nightly-2026-10-01"] {
        let toolchain = TOOLCHAIN.replace("1.98.1", channel);
        let p = toolchain_problems(ROOT, &toolchain).unwrap();
        assert!(p.iter().any(|m| m.contains("exact")), "{channel}: {p:?}");
    }
}

#[test]
fn members_inherit_the_workspace_package_settings() {
    assert!(member_problems("crates/a", MEMBER).unwrap().is_empty());
    for key in ["version", "edition", "rust-version", "license"] {
        let prefix = format!("{key}.workspace");
        let manifest = MEMBER
            .lines()
            .filter(|l| !l.starts_with(&prefix))
            .collect::<Vec<_>>()
            .join("\n");
        let p = member_problems("crates/a", &manifest).unwrap();
        assert_eq!(p.len(), 1, "{key}: {p:?}");
        assert!(
            p[0].contains(&format!(": set {prefix} = true")),
            "{key}: {p:?}"
        );
    }
    let own = MEMBER.replace("edition.workspace = true", "edition = \"2021\"");
    let p = member_problems("crates/a", &own).unwrap();
    assert!(p.iter().any(|m| m.contains("edition.workspace")), "{p:?}");
}

#[test]
fn members_use_the_workspace_lints() {
    let manifest = MEMBER.replace("[lints]\nworkspace = true\n", "");
    let p = member_problems("crates/a", &manifest).unwrap();
    assert!(p.iter().any(|m| m.contains("[lints]")), "{p:?}");
}

#[test]
fn members_are_not_published() {
    for manifest in [
        MEMBER.replace("publish = false\n", ""),
        MEMBER.replace("publish = false", "publish = true"),
    ] {
        let p = member_problems("crates/a", &manifest).unwrap();
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("publish = false"), "{p:?}");
    }
}

#[test]
fn rust_version_cannot_outrun_the_pinned_patch() {
    let ahead = ROOT.replace("\"1.98\"", "\"1.98.2\"");
    let p = toolchain_problems(&ahead, TOOLCHAIN).unwrap();
    assert_eq!(p.len(), 1, "{p:?}");
}

#[test]
fn rust_version_must_be_a_version() {
    let bad = ROOT.replace("\"1.98\"", "\"latest\"");
    let p = toolchain_problems(&bad, TOOLCHAIN).unwrap();
    assert!(p.iter().any(|m| m.contains("is not a version")), "{p:?}");
}

#[test]
fn problems_name_the_crate() {
    let p = member_problems("crates/a", &MEMBER.replace("publish = false\n", "")).unwrap();
    assert!(p[0].starts_with("crates/a/Cargo.toml"), "{p:?}");
}
