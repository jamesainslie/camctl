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

fn problems(text: &str, version: &str) -> Vec<String> {
    Changelog::parse(text).unwrap().problems(&v(version), REPO)
}

#[test]
fn a_consistent_changelog_has_no_problems() {
    assert_eq!(problems(FIRST, "0.1.0"), Vec::<String>::new());
    assert_eq!(problems(LATER, "0.1.0"), Vec::<String>::new());
}

#[test]
fn a_cut_changelog_is_consistent() {
    for (text, next) in [(FIRST, "0.1.0"), (LATER, "0.1.1")] {
        let out = Changelog::parse(text)
            .unwrap()
            .cut(&v(next), "2026-11-01", REPO)
            .unwrap();
        assert_eq!(problems(&out, next), Vec::<String>::new(), "{out}");
    }
}

#[test]
fn the_latest_release_must_match_the_manifest() {
    let p = problems(LATER, "0.2.0");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("0.1.0") && p[0].contains("0.2.0"), "{p:?}");
}

#[test]
fn every_section_needs_its_link() {
    let unlinked = LATER.replace(
        "[0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0\n",
        "",
    );
    let p = problems(&unlinked, "0.1.0");
    assert!(p.iter().any(|m| m.contains("[0.1.0]")), "{p:?}");
}

#[test]
fn the_unreleased_link_compares_against_the_latest_release() {
    let stale = LATER.replace("compare/v0.1.0...HEAD", "commits/main");
    let p = problems(&stale, "0.1.0");
    assert!(
        p.iter().any(|m| m.contains("compare/v0.1.0...HEAD")),
        "{p:?}"
    );
    let early = FIRST.replace("commits/main", "compare/v0.0.1...HEAD");
    let p = problems(&early, "0.1.0");
    assert!(p.iter().any(|m| m.contains("commits/main")), "{p:?}");
}

#[test]
fn releases_run_newest_first_without_repeats() {
    let older_first = LATER.replace(
        "## [0.1.0] - 2026-10-17\n\n### Added\n\n- The app.\n",
        "## [0.1.0] - 2026-10-17\n\n### Added\n\n- The app.\n\n## [0.2.0] - 2026-11-01\n\n- Later.\n",
    );
    let p = problems(&older_first, "0.2.0");
    assert!(p.iter().any(|m| m.contains("newest first")), "{p:?}");

    let repeated = LATER.replace(
        "## [0.1.0] - 2026-10-17\n\n### Added\n\n- The app.\n",
        "## [0.1.0] - 2026-10-17\n\n### Added\n\n- The app.\n\n## [0.1.0] - 2026-10-16\n\n- Again.\n",
    );
    let p = problems(&repeated, "0.1.0");
    assert!(p.iter().any(|m| m.contains("more than once")), "{p:?}");
}

#[test]
fn unreleased_comes_first() {
    let late = "# Changelog\n\n## [0.1.0] - 2026-10-17\n\n- The app.\n\n## [Unreleased]\n\n\
                [Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.0...HEAD\n\
                [0.1.0]: https://github.com/jamesainslie/camctl/releases/tag/v0.1.0\n";
    let p = problems(late, "0.1.0");
    assert!(p.iter().any(|m| m.contains("first section")), "{p:?}");
}

#[test]
fn release_dates_are_iso_dates() {
    for bad in ["17 Oct 2026", "2026-13-01", "2026-10-32", "2026-1-7", ""] {
        let text = LATER.replace("2026-10-17", bad);
        let p = problems(&text, "0.1.0");
        assert!(p.iter().any(|m| m.contains("YYYY-MM-DD")), "{bad:?}: {p:?}");
    }
}

#[test]
fn a_second_unreleased_section_is_rejected() {
    let twice = LATER.replace(
        "## [0.1.0] - 2026-10-17\n",
        "## [Unreleased]\n\n### Added\n\n- Lost in a rebase.\n\n## [0.1.0] - 2026-10-17\n",
    );
    let err = Changelog::parse(&twice).unwrap_err().to_string();
    assert!(err.contains("more than one"), "{err}");
}

#[test]
fn unknown_categories_are_problems() {
    for bad in ["### Breaking changes", "### Features", "### added"] {
        let text = LATER.replace("### Fixed", bad);
        let p = problems(&text, "0.1.0");
        assert!(
            p.iter().any(|m| m.contains(bad.trim_start_matches("### "))),
            "{bad}: {p:?}"
        );
    }
    for good in [
        "Added",
        "Changed",
        "Deprecated",
        "Removed",
        "Fixed",
        "Security",
    ] {
        let text = LATER.replace("### Fixed", &format!("### {good}"));
        assert_eq!(problems(&text, "0.1.0"), Vec::<String>::new(), "{good}");
    }
}

#[test]
fn a_wrong_release_link_is_a_problem() {
    let wrong = LATER.replace("releases/tag/v0.1.0", "releases/tag/v0.0.1");
    let p = problems(&wrong, "0.1.0");
    assert!(p.iter().any(|m| m.contains("[0.1.0] links to")), "{p:?}");
}

#[test]
fn a_missing_unreleased_link_is_a_problem() {
    let unlinked = LATER.replace(
        "[Unreleased]: https://github.com/jamesainslie/camctl/compare/v0.1.0...HEAD\n",
        "",
    );
    let p = problems(&unlinked, "0.1.0");
    assert!(
        p.iter().any(|m| m.contains("[Unreleased] has no link")),
        "{p:?}"
    );
}

#[test]
fn dates_are_real_calendar_dates() {
    for bad in [
        "2026-02-29",
        "2026-02-31",
        "2026-04-31",
        "2026-00-10",
        "2026-12-00",
    ] {
        let p = problems(&LATER.replace("2026-10-17", bad), "0.1.0");
        assert!(p.iter().any(|m| m.contains("YYYY-MM-DD")), "{bad}: {p:?}");
    }
    for good in ["2024-02-29", "2026-02-28", "2026-12-31", "2000-02-29"] {
        let p = problems(&LATER.replace("2026-10-17", good), "0.1.0");
        assert_eq!(p, Vec::<String>::new(), "{good}");
    }
}

#[test]
fn a_yanked_release_keeps_a_valid_date() {
    let yanked = LATER.replace("2026-10-17", "2026-10-17 [YANKED]");
    assert_eq!(problems(&yanked, "0.1.0"), Vec::<String>::new());
}
