use crate::{
    mirror::{ensure_real, validate_oid},
    GitError,
};
use polyclave_os::resolve_cli_binary;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScopeCheckResult {
    pub allowed: bool,
    pub violations: Vec<String>,
}

/// Run `git diff --name-only <base_sha>..<head_sha>` against the authoritative
/// mirror and validate every changed path against `touched_paths` glob/prefix rules.
pub fn check_touched_paths(
    mirror: &Path,
    base_sha: &str,
    head_sha: &str,
    touched_paths: &[String],
) -> Result<ScopeCheckResult, GitError> {
    ensure_real(mirror)?;
    validate_oid(base_sha)?;
    validate_oid(head_sha)?;

    let git = resolve_cli_binary(Path::new("git"))?;
    let range = format!("{base_sha}..{head_sha}");
    let mut cmd = Command::new(&git);
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            cmd.env_remove(key);
        }
    }
    let output = cmd
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .arg("-C")
        .arg(mirror)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            &range,
            "--",
        ])
        .output()?;

    if !output.status.success() {
        return Err(GitError::Command("diff touched paths on mirror"));
    }

    let raw = std::str::from_utf8(&output.stdout)
        .map_err(|_| GitError::Invalid("non-Unicode path in diff"))?;
    let changed_paths: Vec<String> = raw
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();

    Ok(validate_touched_paths(&changed_paths, touched_paths))
}

/// Validate a list of changed paths against `touched_paths` glob/prefix rules.
pub fn validate_touched_paths(
    changed_paths: &[String],
    touched_paths: &[String],
) -> ScopeCheckResult {
    let mut violations = Vec::new();
    for path in changed_paths {
        let allowed = touched_paths
            .iter()
            .any(|rule| matches_touched_rule(path, rule));
        if !allowed {
            violations.push(path.clone());
        }
    }
    ScopeCheckResult {
        allowed: violations.is_empty(),
        violations,
    }
}

fn normalize_rel_path(raw: &str) -> String {
    let replaced = raw.replace('\\', "/");
    replaced
        .strip_prefix("./")
        .unwrap_or(&replaced)
        .trim_start_matches('/')
        .to_owned()
}

pub(crate) fn matches_touched_rule(path: &str, rule: &str) -> bool {
    let path = normalize_rel_path(path);
    let rule = normalize_rel_path(rule);
    if rule.is_empty() || path.is_empty() {
        return false;
    }

    if !rule.contains(['*', '?', '[']) {
        if rule.ends_with('/') {
            return path.starts_with(&rule);
        }
        return path == rule || path.starts_with(&format!("{rule}/"));
    }

    glob_match(rule.as_bytes(), path.as_bytes())
}

fn glob_match(pat: &[u8], text: &[u8]) -> bool {
    let mut memo = vec![None; (pat.len() + 1) * (text.len() + 1)];
    glob_match_dp(pat, text, 0, 0, text.len() + 1, &mut memo)
}

fn glob_match_dp(
    pat: &[u8],
    text: &[u8],
    p: usize,
    t: usize,
    stride: usize,
    memo: &mut [Option<bool>],
) -> bool {
    let idx = p * stride + t;
    if let Some(cached) = memo[idx] {
        return cached;
    }

    let res = if p == pat.len() {
        t == text.len()
    } else if pat[p] == b'*' {
        if p + 1 < pat.len() && pat[p + 1] == b'*' {
            // `**` matches zero or more characters including `/`
            let mut next_p = p + 2;
            if next_p < pat.len() && pat[next_p] == b'/' {
                next_p += 1;
            }
            // Either `**/` matches empty (skip `**/`) or `**` consumes one char of `text`
            glob_match_dp(pat, text, next_p, t, stride, memo)
                || (t < text.len() && glob_match_dp(pat, text, p, t + 1, stride, memo))
        } else {
            // Single `*` matches zero or more non-`/` characters
            glob_match_dp(pat, text, p + 1, t, stride, memo)
                || (t < text.len()
                    && text[t] != b'/'
                    && glob_match_dp(pat, text, p, t + 1, stride, memo))
        }
    } else if pat[p] == b'?' {
        t < text.len() && text[t] != b'/' && glob_match_dp(pat, text, p + 1, t + 1, stride, memo)
    } else if pat[p] == b'[' {
        if let Some((matched, next_p)) = t.checked_sub(0).and_then(|_| {
            if t < text.len() {
                match_char_class(&pat[p..], text[t])
            } else {
                None
            }
        }) {
            matched && glob_match_dp(pat, text, p + next_p, t + 1, stride, memo)
        } else {
            false
        }
    } else {
        t < text.len() && pat[p] == text[t] && glob_match_dp(pat, text, p + 1, t + 1, stride, memo)
    };

    memo[idx] = Some(res);
    res
}

fn match_char_class(pat: &[u8], ch: u8) -> Option<(bool, usize)> {
    if ch == b'/' || pat.len() < 2 || pat[0] != b'[' {
        return None;
    }
    let mut i = 1usize;
    let negated = if i < pat.len() && (pat[i] == b'!' || pat[i] == b'^') {
        i += 1;
        true
    } else {
        false
    };

    let mut matched = false;
    let mut first = true;
    while i < pat.len() {
        if pat[i] == b']' && !first {
            return Some((if negated { !matched } else { matched }, i + 1));
        }
        first = false;
        if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
            if ch >= pat[i] && ch <= pat[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if ch == pat[i] {
                matched = true;
            }
            i += 1;
        }
    }
    None
}
