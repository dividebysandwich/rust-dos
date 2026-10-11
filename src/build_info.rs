//! Which source the program was built from: the git commit build.rs found,
//! for `--version`, the debug server's `/api/version` and the journal's
//! `started` entry.

/// HEAD's full hash at build time, or None when the build had no git
/// checkout of its own to ask (a crate tarball, a copied tree) or no git.
pub const COMMIT: Option<&str> = nonempty(env!("RUST_DOS_GIT_COMMIT"));

/// Whether a tracked file differed from HEAD at build time; None when the
/// commit is unknown.
pub const DIRTY: Option<bool> = match env!("RUST_DOS_GIT_DIRTY").as_bytes() {
    b"true" => Some(true),
    b"false" => Some(false),
    _ => None,
};

/// HEAD's hash as `git rev-parse --short` gave it (7 hex digits or more,
/// as many as the repository needed to make it unique); None with `COMMIT`.
pub const COMMIT_SHORT: Option<&str> = nonempty(env!("RUST_DOS_GIT_COMMIT_SHORT"));

/// The commit for people: "1a2b3c4", "1a2b3c4-dirty" or "unknown".
pub const DESCRIBE: &str = env!("RUST_DOS_GIT_DESCRIBE");

/// The version with the commit, for `--version`: "1.6.0 (commit 1a2b3c4)".
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (commit ", env!("RUST_DOS_GIT_DESCRIBE"), ")");

/// `/api/version`'s and the journal's `build` object.
pub fn json() -> serde_json::Value {
    serde_json::json!({
        "commit": COMMIT,
        "commit_short": COMMIT_SHORT,
        "dirty": DIRTY,
    })
}

const fn nonempty(s: &str) -> Option<&str> {
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commit_is_a_full_hash_or_unknown() {
        match COMMIT {
            Some(c) => {
                assert_eq!(c.len(), 40);
                assert!(c.bytes().all(|b| b.is_ascii_hexdigit()));
                assert!(DIRTY.is_some());
                let short = COMMIT_SHORT.unwrap();
                assert!(short.len() >= 7 && c.starts_with(short));
                let dirty = if DIRTY == Some(true) { "-dirty" } else { "" };
                assert_eq!(DESCRIBE, format!("{}{}", short, dirty));
            }
            None => {
                assert_eq!((COMMIT_SHORT, DIRTY), (None, None));
                assert_eq!(DESCRIBE, "unknown");
            }
        }
        assert_eq!(VERSION, format!("{} (commit {})", env!("CARGO_PKG_VERSION"), DESCRIBE));
        assert_eq!(json()["commit"].as_str(), COMMIT);
    }
}
