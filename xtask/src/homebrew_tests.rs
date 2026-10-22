use semver::Version;

use super::*;

const APP_SHA: &str = "932493f665803e02e1e254447e2d0f64d58ab589fd519c8190ed721465f35d51";
const CLI_SHA: &str = "19bb3d98f607785537116cb298033d5989af7829d8c14b22e3a6c6ecc8dad5de";

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn sums() -> String {
    format!(
        "{APP_SHA}  camctl-0.1.1-macos-arm64.zip\n{CLI_SHA}  camctl-cli-0.1.1-macos-arm64.tar.gz\n"
    )
}

#[test]
fn archives_are_named_as_the_release_names_them() {
    assert_eq!(app_archive(&v("0.1.1")), "camctl-0.1.1-macos-arm64.zip");
    assert_eq!(
        cli_archive(&v("0.1.1")),
        "camctl-cli-0.1.1-macos-arm64.tar.gz"
    );
}

#[test]
fn checksums_are_found_by_file_name() {
    let sums = sums();
    assert_eq!(
        sha256_for(&sums, "camctl-0.1.1-macos-arm64.zip").unwrap(),
        APP_SHA
    );
    assert_eq!(
        sha256_for(&sums, "camctl-cli-0.1.1-macos-arm64.tar.gz").unwrap(),
        CLI_SHA
    );
}

#[test]
fn a_missing_checksum_is_an_error() {
    let err = sha256_for(&sums(), "camctl-0.2.0-macos-arm64.zip").unwrap_err();
    assert!(
        err.to_string().contains("camctl-0.2.0-macos-arm64.zip"),
        "{err}"
    );
}

#[test]
fn a_malformed_checksum_is_an_error() {
    let sums = "not-a-sha  camctl-0.1.1-macos-arm64.zip\n";
    assert!(sha256_for(sums, "camctl-0.1.1-macos-arm64.zip").is_err());
    let upper = format!("{}  camctl-0.1.1-macos-arm64.zip\n", APP_SHA.to_uppercase());
    assert!(sha256_for(&upper, "camctl-0.1.1-macos-arm64.zip").is_err());
}

#[test]
fn the_cask_installs_the_app_from_the_private_release() {
    let cask = cask(&v("0.1.1"), APP_SHA);
    for want in [
        "require_relative \"../custom_download_strategy\"",
        "cask \"camctl\" do",
        "version \"0.1.1\"",
        &format!("sha256 \"{APP_SHA}\""),
        "url \"https://github.com/jamesainslie/camctl/releases/download/v#{version}/camctl-#{version}-macos-arm64.zip\",",
        "using: GitHubPrivateRepositoryReleaseDownloadStrategy",
        "depends_on arch: :arm64",
        "depends_on macos: :sonoma",
        "depends_on formula: \"yaklabco/tap/camctl-cli\"",
        "app \"camctl.app\"",
        "uninstall quit: \"us.ainslies.camctl\"",
    ] {
        assert!(cask.contains(want), "missing {want:?} in:\n{cask}");
    }
    // Upgrades run uninstall directives, which would drop the login item.
    assert!(!cask.contains("login_item:"), "{cask}");
}

#[test]
fn the_cask_leaves_the_quarantine_to_gatekeeper() {
    // Releases are Developer ID signed and notarized, so Gatekeeper passes
    // them with the download quarantine set; clearing it would only switch
    // the check off.
    let cask = cask(&v("0.1.1"), APP_SHA);
    assert!(!cask.contains("quarantine"), "{cask}");
    assert!(!cask.contains("postflight"), "{cask}");
    assert!(
        cask.contains("zap trash: \"~/Library/Logs/camctl\""),
        "{cask}"
    );
}

#[test]
fn the_cask_url_names_the_archive_the_checksum_is_for() {
    let version = v("0.1.1");
    let cask = cask(&version, APP_SHA).replace("#{version}", "0.1.1");
    let url = cask
        .lines()
        .find_map(|l| l.trim().strip_prefix("url \""))
        .and_then(|l| l.strip_suffix("\","))
        .expect("a url line");
    assert!(
        url.ends_with(&format!("/{}", app_archive(&version))),
        "{url}"
    );
}

/// A fresh directory under the system temp dir, removed on drop.
struct TempTap(std::path::PathBuf);

impl TempTap {
    fn new(name: &str, with_strategy: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("camctl-xtask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if with_strategy {
            std::fs::write(dir.join("custom_download_strategy.rb"), "# stub\n").unwrap();
        }
        TempTap(dir)
    }
}

impl Drop for TempTap {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn writing_fills_in_the_casks_and_formula_directories() {
    let tap = TempTap::new("write", true);
    let [cask_path, formula_path] = write(&tap.0, &v("0.1.1"), &sums()).unwrap();
    assert_eq!(cask_path, tap.0.join("Casks/camctl.rb"));
    assert_eq!(formula_path, tap.0.join("Formula/camctl-cli.rb"));
    let cask = std::fs::read_to_string(cask_path).unwrap();
    let formula = std::fs::read_to_string(formula_path).unwrap();
    assert!(cask.contains(APP_SHA), "{cask}");
    assert!(formula.contains(CLI_SHA), "{formula}");
}

#[test]
fn writing_refuses_a_directory_that_is_not_the_tap() {
    let tap = TempTap::new("not-a-tap", false);
    let err = write(&tap.0, &v("0.1.1"), &sums()).unwrap_err();
    assert!(
        err.to_string().contains("custom_download_strategy.rb"),
        "{err}"
    );
}

#[test]
fn writing_changes_nothing_when_a_checksum_is_missing() {
    let tap = TempTap::new("partial", true);
    let only_app = format!("{APP_SHA}  camctl-0.1.1-macos-arm64.zip\n");
    assert!(write(&tap.0, &v("0.1.1"), &only_app).is_err());
    assert!(!tap.0.join("Casks").exists());
    assert!(!tap.0.join("Formula").exists());
}

#[test]
fn pre_releases_are_not_published_to_the_tap() {
    let tap = TempTap::new("pre", true);
    let sums = format!(
        "{APP_SHA}  camctl-0.2.0-rc.1-macos-arm64.zip\n{CLI_SHA}  camctl-cli-0.2.0-rc.1-macos-arm64.tar.gz\n"
    );
    let err = write(&tap.0, &v("0.2.0-rc.1"), &sums).unwrap_err();
    assert!(err.to_string().contains("pre-release"), "{err}");
    assert!(!tap.0.join("Casks").exists());
}

#[test]
fn the_formula_installs_the_cli_from_the_private_release() {
    let formula = formula(&v("0.1.1"), CLI_SHA);
    for want in [
        "require_relative \"../custom_download_strategy\"",
        "class CamctlCli < Formula",
        "version \"0.1.1\"",
        "license \"MIT\"",
        &format!("sha256 \"{CLI_SHA}\""),
        "url \"https://github.com/jamesainslie/camctl/releases/download/v0.1.1/camctl-cli-0.1.1-macos-arm64.tar.gz\",",
        "using: GitHubPrivateRepositoryReleaseDownloadStrategy",
        "bin.install \"camctl\"",
        "shell_output(\"#{bin}/camctl --version\")",
    ] {
        assert!(formula.contains(want), "missing {want:?} in:\n{formula}");
    }
}

#[test]
fn generated_files_say_where_they_come_from() {
    for text in [cask(&v("0.1.1"), APP_SHA), formula(&v("0.1.1"), CLI_SHA)] {
        assert!(
            text.contains("Generated by `cargo xtask homebrew`"),
            "{text}"
        );
        assert!(text.ends_with("end\n"), "{text}");
    }
}
