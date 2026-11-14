//! The Borealis Connect release this server is: party/VERSION (party/release-version.sh). It is `X.Y.Z` for a full
//! release (the apps too) or `X.Y.Z.W` for a server and web release, where `X.Y.Z` is the crate's own version
//! (party/Cargo.toml), which stays 3-part semver for Cargo and the apps. `/api/server`, the admin page, the start-up
//! log and the backup names show this one.

const FILE: &str = include_str!("../../../VERSION");

/// The release version, such as `0.3.3` or `0.3.3.1`.
#[must_use]
pub fn version() -> &'static str {
    FILE.trim()
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn the_release_version_is_the_crate_version_or_one_more_number() {
        let crate_version = env!("CARGO_PKG_VERSION");
        let release = version();
        assert!(!release.is_empty(), "party/VERSION is empty");
        if release != crate_version {
            let build = release
                .strip_prefix(crate_version)
                .and_then(|rest| rest.strip_prefix('.'))
                .unwrap_or_else(|| {
                    panic!("party/VERSION is {release}, but party/Cargo.toml is {crate_version}")
                });
            assert!(
                !build.is_empty()
                    && !build.starts_with('0')
                    && build.chars().all(|c| c.is_ascii_digit()),
                "party/VERSION {release}: the 4th number must be 1 or more, without leading zeros"
            );
        }
    }
}
