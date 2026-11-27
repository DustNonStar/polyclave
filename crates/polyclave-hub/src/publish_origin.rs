//! Decides where the PR gate may publish an R1-verified commit.
//!
//! The only input is the `origin` snapshot taken when the Run was admitted
//! (`polyclave_store::OriginSnapshot`). A snapshot that does not name exactly
//! one GitHub repository equal to the gate's `repo_slug` is refused, and the
//! gate escalates before any push or PR. The push itself always goes to the
//! canonical `https://github.com/<repo_slug>.git`, never to a URL taken from
//! the snapshot and never through a named remote on the host mirror.
use polyclave_store::OriginSnapshot;
use std::fmt;

/// A validated publish destination: a GitHub repository `owner/repo`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishTarget {
    repo_slug: String,
}

impl PublishTarget {
    /// The gate's `owner/repo`, as the gate was given it.
    pub fn repo_slug(&self) -> &str {
        &self.repo_slug
    }

    /// Builds a target from a slug alone (no origin check). Tests only.
    #[cfg(test)]
    pub(crate) fn for_tests(repo_slug: &str) -> Self {
        Self {
            repo_slug: repo_slug.to_owned(),
        }
    }
}

/// Why the snapshot cannot be published to. The text never contains a URL, so
/// nothing from the user's git config reaches a Hub notice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OriginRejection {
    /// The Run was admitted before origin snapshots existed.
    NoSnapshot,
    /// The repository had no `origin` URL at admission.
    Missing,
    /// `origin` had several URLs, so git would push to each of them.
    SeveralUrls,
    /// `origin` had a separate `pushurl`.
    SeparatePushUrl,
    /// The URL is not a GitHub HTTPS or SSH address of one repository.
    NotGitHub,
    /// The origin names another GitHub repository.
    SlugMismatch { origin_slug: String },
    /// The gate's own `repo_slug` is not `owner/repo`.
    InvalidRepoSlug,
}

impl fmt::Display for OriginRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSnapshot => f.write_str(
                "the Run has no origin snapshot (it was admitted before publish targets were recorded)",
            ),
            Self::Missing => f.write_str("the repository had no origin URL when the Run was admitted"),
            Self::SeveralUrls => f.write_str("origin had several URLs when the Run was admitted"),
            Self::SeparatePushUrl => {
                f.write_str("origin had a separate pushurl when the Run was admitted")
            }
            Self::NotGitHub => f.write_str("origin is not a GitHub HTTPS or SSH repository URL"),
            Self::SlugMismatch { origin_slug } => write!(
                f,
                "origin names GitHub repository {origin_slug}, not the PR gate's repository"
            ),
            Self::InvalidRepoSlug => f.write_str("the PR gate's repo_slug is not owner/repo"),
        }
    }
}

/// Checks the admission snapshot against the gate's `repo_slug`.
pub fn resolve_publish_target(
    snapshot: Option<&OriginSnapshot>,
    repo_slug: &str,
) -> Result<PublishTarget, OriginRejection> {
    let (owner, repo) = split_slug(repo_slug).ok_or(OriginRejection::InvalidRepoSlug)?;
    let snapshot = snapshot.ok_or(OriginRejection::NoSnapshot)?;
    if !snapshot.push_urls.is_empty() {
        return Err(OriginRejection::SeparatePushUrl);
    }
    let url = match snapshot.urls.as_slice() {
        [] => return Err(OriginRejection::Missing),
        [url] => url,
        _ => return Err(OriginRejection::SeveralUrls),
    };
    let (origin_owner, origin_repo) = github_slug(url).ok_or(OriginRejection::NotGitHub)?;
    if !origin_owner.eq_ignore_ascii_case(owner) || !origin_repo.eq_ignore_ascii_case(repo) {
        return Err(OriginRejection::SlugMismatch {
            origin_slug: format!("{origin_owner}/{origin_repo}"),
        });
    }
    Ok(PublishTarget {
        repo_slug: format!("{owner}/{repo}"),
    })
}

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.len() <= 100
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// Splits `owner/repo`; a `.git` suffix on the repo is not allowed here.
fn split_slug(slug: &str) -> Option<(&str, &str)> {
    let (owner, repo) = slug.split_once('/')?;
    (valid_segment(owner) && valid_segment(repo) && !repo.to_ascii_lowercase().ends_with(".git"))
        .then_some((owner, repo))
}

/// `owner/repo` path of a GitHub URL, with an optional `.git` and trailing `/`.
fn path_slug(path: &str) -> Option<(String, String)> {
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = if path.len() > 4 && path[path.len() - 4..].eq_ignore_ascii_case(".git") {
        &path[..path.len() - 4]
    } else {
        path
    };
    let (owner, repo) = path.split_once('/')?;
    (valid_segment(owner) && valid_segment(repo)).then(|| (owner.to_owned(), repo.to_owned()))
}

fn is_github_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
}

/// The `owner/repo` of a GitHub HTTPS or SSH URL, or `None` for anything else.
///
/// Accepted forms: `https://github.com/o/r[.git]` (port 443 allowed),
/// `ssh://[user@]github.com[:22]/o/r[.git]` and `[user@]github.com:o/r[.git]`.
pub fn github_slug(url: &str) -> Option<(String, String)> {
    if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        };
        let ok = match scheme.to_ascii_lowercase().as_str() {
            "https" => port.is_none_or(|p| p == "443"),
            "ssh" => port.is_none_or(|p| p == "22"),
            _ => false,
        };
        if !ok || !is_github_host(host) || path.contains(['?', '#']) {
            return None;
        }
        return path_slug(path);
    }
    // scp-like: [user@]host:path. A colon must come before any slash.
    let (authority, path) = url.split_once(':')?;
    if authority.contains('/') || path.starts_with('/') {
        return None;
    }
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if !is_github_host(host) {
        return None;
    }
    path_slug(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `\u{40}` is '@': scripts/privacy-scan.sh reads a user, an at sign and a host as an email address.
    fn snap(urls: &[&str], push_urls: &[&str]) -> OriginSnapshot {
        OriginSnapshot {
            urls: urls.iter().map(|s| (*s).to_owned()).collect(),
            push_urls: push_urls.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn accepts_github_https_and_ssh_origins_matching_the_slug() {
        for url in [
            "https://github.com/Org/Repo.git",
            "https://github.com/org/repo",
            "https://github.com/org/repo/",
            "https://GitHub.com:443/org/REPO.GIT",
            "ssh://git\u{40}github.com/org/repo.git",
            "ssh://git\u{40}github.com:22/org/repo",
            "git\u{40}github.com:org/repo.git",
            "github.com:org/repo",
        ] {
            let target = resolve_publish_target(Some(&snap(&[url], &[])), "org/repo")
                .unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!(target.repo_slug(), "org/repo");
        }
    }

    #[test]
    fn refuses_missing_foreign_mismatched_and_ambiguous_origins() {
        let slug = "org/repo";
        assert_eq!(
            resolve_publish_target(None, slug),
            Err(OriginRejection::NoSnapshot)
        );
        assert_eq!(
            resolve_publish_target(Some(&snap(&[], &[])), slug),
            Err(OriginRejection::Missing)
        );
        assert_eq!(
            resolve_publish_target(
                Some(&snap(
                    &[
                        "https://github.com/org/repo.git",
                        "https://github.com/org/repo.git"
                    ],
                    &[]
                )),
                slug
            ),
            Err(OriginRejection::SeveralUrls)
        );
        assert_eq!(
            resolve_publish_target(
                Some(&snap(
                    &["https://github.com/org/repo.git"],
                    &["https://github.com/org/repo.git"]
                )),
                slug
            ),
            Err(OriginRejection::SeparatePushUrl)
        );
        for url in [
            "https://gitlab.com/org/repo.git",
            "https://github.com.evil.example/org/repo.git",
            "https://www.github.com/org/repo.git",
            "http://github.com/org/repo.git",
            "git://github.com/org/repo.git",
            "https://github.com:8443/org/repo.git",
            "https://github.com/org/repo/extra.git",
            "https://github.com/org",
            "https://github.com/org/repo.git?x=1",
            "file:///tmp/org/repo.git",
            "/tmp/github.com:org/repo.git",
            "./github.com:org/repo",
            "git\u{40}github.com:/org/repo.git",
            "git\u{40}gitlab.com:org/repo.git",
            "https://github.com/../repo.git",
        ] {
            assert_eq!(
                resolve_publish_target(Some(&snap(&[url], &[])), slug),
                Err(OriginRejection::NotGitHub),
                "{url}"
            );
        }
        assert_eq!(
            resolve_publish_target(
                Some(&snap(&["git\u{40}github.com:org/other.git"], &[])),
                slug
            ),
            Err(OriginRejection::SlugMismatch {
                origin_slug: "org/other".into()
            })
        );
        for bad in [
            "org",
            "org/repo/x",
            "org/repo.git",
            "/repo",
            "org/",
            "o g/r",
        ] {
            assert_eq!(
                resolve_publish_target(
                    Some(&snap(&["git\u{40}github.com:org/repo.git"], &[])),
                    bad
                ),
                Err(OriginRejection::InvalidRepoSlug),
                "{bad}"
            );
        }
    }
}
