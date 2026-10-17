use semver::Version;

use super::*;

const REPO: &str = "https://github.com/jamesainslie/camctl";

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

const FIRST: &str = "# Changelog

Intro text.

## [Unreleased]

### Added

- The app.
- The CLI.

[Unreleased]: https://github.com/jamesainslie/camctl/commits/main
";

const LATER: &str = "# Changelog

## [Unreleased]

### Fixed

- A crash.

## [0.1.0] - 2026-10-17

### Added

- The app.

[Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0
";

#[test]
fn parses_unreleased_and_released_sections() {
    let c = Changelog::parse(LATER).unwrap();
    assert_eq!(c.unreleased().unwrap().categories(), vec!["Fixed"]);
    assert_eq!(c.latest_release(), Some(v("0.1.0")));
    assert!(c.notes(&v("0.1.0")).unwrap().contains("- The app."));
}

#[test]
fn an_empty_unreleased_section_has_nothing_to_release() {
    let c = Changelog::parse(
        "# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-10-17\n\n### Added\n\n- x\n",
    )
    .unwrap();
    assert!(!c.unreleased().unwrap().has_entries());
}

#[test]
fn the_first_release_uses_the_current_version() {
    let c = Changelog::parse(FIRST).unwrap();
    assert_eq!(c.next_version(&v("0.1.0")).unwrap(), v("0.1.0"));
}

#[test]
fn zero_x_fixes_and_features_bump_the_patch() {
    let c = Changelog::parse(LATER).unwrap();
    assert_eq!(c.next_version(&v("0.1.0")).unwrap(), v("0.1.1"));
    let added = LATER.replace("### Fixed", "### Added");
    let c = Changelog::parse(&added).unwrap();
    assert_eq!(c.next_version(&v("0.1.0")).unwrap(), v("0.1.1"));
}

#[test]
fn zero_x_removals_and_breaking_changes_bump_the_minor() {
    let removed = LATER.replace("### Fixed", "### Removed");
    let c = Changelog::parse(&removed).unwrap();
    assert_eq!(c.next_version(&v("0.1.0")).unwrap(), v("0.2.0"));
    let breaking = LATER.replace("- A crash.", "- BREAKING: renamed the CLI.");
    let c = Changelog::parse(&breaking).unwrap();
    assert_eq!(c.next_version(&v("0.1.0")).unwrap(), v("0.2.0"));
}

#[test]
fn one_x_follows_semver_proper() {
    let later = LATER.replace("0.1.0", "1.2.3");
    let c = Changelog::parse(&later).unwrap();
    assert_eq!(c.next_version(&v("1.2.3")).unwrap(), v("1.2.4"));
    let c = Changelog::parse(&later.replace("### Fixed", "### Added")).unwrap();
    assert_eq!(c.next_version(&v("1.2.3")).unwrap(), v("1.3.0"));
    let c = Changelog::parse(&later.replace("### Fixed", "### Removed")).unwrap();
    assert_eq!(c.next_version(&v("1.2.3")).unwrap(), v("2.0.0"));
}

#[test]
fn the_manifest_and_changelog_must_agree() {
    let c = Changelog::parse(LATER).unwrap();
    assert!(
        c.next_version(&v("0.0.9")).is_err(),
        "manifest behind the changelog"
    );
}

#[test]
fn nothing_to_release_is_an_error() {
    let c = Changelog::parse("# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-10-17\n\n- x\n")
        .unwrap();
    assert!(c.next_version(&v("0.1.0")).is_err());
}

#[test]
fn cutting_the_first_release() {
    let out = Changelog::parse(FIRST)
        .unwrap()
        .cut(&v("0.1.0"), "2026-10-17", REPO)
        .unwrap();
    assert!(out.contains("## [Unreleased]\n\n## [0.1.0] - 2026-10-17\n\n### Added\n\n- The app."));
    assert!(out.contains("Intro text."));
    assert!(
        out.contains("[Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.0...HEAD")
    );
    assert!(out.contains("[0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0"));
    assert!(!out.contains("commits/main"));
    // The result parses, and has nothing left to release.
    let again = Changelog::parse(&out).unwrap();
    assert!(!again.unreleased().unwrap().has_entries());
    assert_eq!(again.latest_release(), Some(v("0.1.0")));
}

#[test]
fn cutting_a_later_release_links_the_comparison() {
    let out = Changelog::parse(LATER)
        .unwrap()
        .cut(&v("0.1.1"), "2026-11-01", REPO)
        .unwrap();
    assert!(out.contains("## [0.1.1] - 2026-11-01\n\n### Fixed\n\n- A crash."));
    assert!(
        out.contains("[Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.1...HEAD")
    );
    assert!(
        out.contains("[0.1.1]: https://github.com/jamesainslie/camctl/compare/v0.1.0...v0.1.1")
    );
    assert!(out.contains("[0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0"));
}

#[test]
fn release_notes_are_the_section_body() {
    let c = Changelog::parse(LATER).unwrap();
    let notes = c.notes(&v("0.1.0")).unwrap();
    assert_eq!(notes.trim(), "### Added\n\n- The app.");
    assert!(c.notes(&v("9.9.9")).is_none());
}

#[test]
fn a_changelog_without_unreleased_is_rejected() {
    assert!(Changelog::parse("# Changelog\n\n## [0.1.0] - 2026-10-17\n").is_err());
}

#[test]
fn setting_the_workspace_version() {
    let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace.dependencies]\nfoo = { version = \"0.1.0\" }\n";
    let out = set_workspace_version(toml, &v("0.2.0")).unwrap();
    assert!(out.contains("[workspace.package]\nversion = \"0.2.0\""));
    assert!(
        out.contains("foo = { version = \"0.1.0\" }"),
        "only the workspace version moves"
    );
    assert_eq!(workspace_version(&out).unwrap(), v("0.2.0"));
}

#[test]
fn a_manifest_without_a_workspace_version_is_an_error() {
    assert!(workspace_version("[workspace]\nmembers = []\n").is_err());
}
