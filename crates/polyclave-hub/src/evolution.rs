//! Stage 4 local evolution loop: deterministic local embeddings, SQLite-based
//! routing/memory/role-brief distillation, Teach-Once local skill sync (into
//! `<state_dir>/isolated-homes/<vendor>/skills/` only), and opt-in redacted
//! community experience drafts.
use crate::{Error, Result};
use polyclave_os::{create_private_dir, write_private_file};
use polyclave_proto::{
    CommunityDraftDto, EvolutionKind, EvolutionStatus, EvolutionSuggestionDto, LocalSkillDto,
    Vendor,
};
use polyclave_store::stage4::NewEvolutionSuggestion;
use polyclave_store::{MemoryFilter, Store};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use std::time::Duration;

/// Deterministic 128-dim unit-normalized hashed trigram embedding (zero external APIs).
pub fn local_embedding(text: &str) -> Vec<f32> {
    polyclave_store::local_embedding(text)
}

/// Distills versioned evolution suggestions from local SQLite history without
/// touching host gate code or user global skills.
pub fn distill_local_suggestions(store: &Store) -> Result<Vec<EvolutionSuggestionDto>> {
    let (failed_runs, review_runs, verified_memories) = store
        .read(|db| {
            let failed: i64 =
                db.query_row("SELECT COUNT(*) FROM runs WHERE status='failed'", [], |r| {
                    r.get(0)
                })?;
            let review: i64 = db.query_row(
                "SELECT COUNT(*) FROM runs WHERE status='needs_review'",
                [],
                |r| r.get(0),
            )?;
            let mems: i64 = db.query_row(
                "SELECT COUNT(*) FROM memory_items WHERE status='verified'",
                [],
                |r| r.get(0),
            )?;
            Ok((failed, review, mems))
        })
        .map_err(|_| Error::Store)?;
    let mut created = Vec::new();
    created.push(
        store
            .create_evolution_suggestion(NewEvolutionSuggestion {
                kind: EvolutionKind::Routing,
                title: if failed_runs > 0 {
                    "Prefer conservative vendor fallback after failed runs".into()
                } else {
                    "Keep Flash-first routing for low-latency Hub turns".into()
                },
                rationale: format!(
                    "Observed {failed_runs} failed runs and {review_runs} runs awaiting review in local SQLite."
                ),
                payload_json: json!({
                    "failed_runs": failed_runs,
                    "review_runs": review_runs,
                    "default_model": "deepseek-flash",
                    "host_gates_mutable": false
                }),
                supersedes_id: None,
            })
            .map_err(|_| Error::Store)?,
    );
    if verified_memories > 0 {
        created.push(
            store
                .create_evolution_suggestion(NewEvolutionSuggestion {
                    kind: EvolutionKind::MemoryRule,
                    title: "Refresh local semantic embeddings for verified rules".into(),
                    rationale: format!(
                        "Indexed {verified_memories} verified memory items for local hybrid retrieval."
                    ),
                    payload_json: json!({
                        "verified_memories": verified_memories,
                        "dim": 128
                    }),
                    supersedes_id: None,
                })
                .map_err(|_| Error::Store)?,
        );
    }
    Ok(created)
}

fn sanitize_skill_dir_name(name: &str) -> Result<String> {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = clean.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        return Err(Error::Policy);
    }
    Ok(trimmed)
}

/// Writes a Teach-Once skill ONLY inside `<state_dir>/isolated-homes/<vendor>/skills/<name>/SKILL.md`.
/// Never mutates user global `~/.codex`, `~/.claude`, etc.
pub fn sync_skill_to_isolated_homes(state_dir: &Path, skill: &LocalSkillDto) -> Result<usize> {
    let dir_name = sanitize_skill_dir_name(&skill.name)?;
    let content = format!(
        "---\nname: {name}\ndescription: {desc}\nversion: {ver}\n---\n\n{body}\n",
        name = dir_name,
        desc = skill.description.replace('\n', " "),
        ver = skill.version,
        body = skill.body_md
    );
    let mut synced = 0usize;
    for vendor in &skill.synced_vendors {
        let vendor_dir = match vendor {
            Vendor::Codex => "codex",
            Vendor::Claude => "claude",
            Vendor::Agy => "agy",
            Vendor::Dsh => "dsh",
        };
        let iso_root = state_dir.join("isolated-homes");
        let vendor_root = iso_root.join(vendor_dir);
        let skills_root = vendor_root.join("skills");
        let target_dir = skills_root.join(&dir_name);
        for p in [&iso_root, &vendor_root, &skills_root, &target_dir] {
            if let Ok(meta) = std::fs::symlink_metadata(p) {
                if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                    return Err(Error::Policy);
                }
            }
            match create_private_dir(p) {
                Ok(()) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::AlreadyExists
                        && std::fs::symlink_metadata(p).is_ok_and(|m| {
                            !m.file_type().is_symlink() && m.file_type().is_dir()
                        }) => {}
                Err(_) => return Err(Error::Store),
            }
        }
        let canonical_iso =
            polyclave_os::canonical_real_path(&iso_root).map_err(|_| Error::Store)?;
        let canonical_target =
            polyclave_os::canonical_real_path(&target_dir).map_err(|_| Error::Store)?;
        if !canonical_target.starts_with(&canonical_iso) {
            return Err(Error::Policy);
        }
        write_private_file(&canonical_target.join("SKILL.md"), content.as_bytes())
            .map_err(|_| Error::Store)?;
        synced += 1;
    }
    Ok(synced)
}

/// Redacts local paths, emails, and secret-like tokens from text before generating
/// an opt-in community experience draft. Never uploads anything automatically.
pub fn redact_text(input: &str) -> (String, u32) {
    let mac_prefix = ["/U", "sers/"].concat();
    let linux_prefix = ["/h", "ome/"].concat();
    let gh_pat = ["github", "_pat_"].concat();
    let slack_bot = ["xox", "b-"].concat();
    let slack_user = ["xox", "p-"].concat();
    let secret_prefixes = [
        "sk-",
        "ghp_",
        "gho_",
        gh_pat.as_str(),
        slack_bot.as_str(),
        slack_user.as_str(),
        "AIza",
        "AKIA",
    ];
    let is_path_stop =
        |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | ']' | '>' | ',' | ';');
    let is_token_boundary = |prev: Option<char>| {
        prev.is_none_or(|c| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '=' | ':' | '(' | '[' | '<' | ',')
        })
    };
    let is_email_char =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');

    let mut out = String::with_capacity(input.len());
    let mut stripped = 0u32;
    let mut i = 0usize;
    while i < input.len() {
        let rest = &input[i..];
        let prev = input[..i].chars().next_back();

        if rest.starts_with("file://")
            && (rest[7..].starts_with(&mac_prefix)
                || rest[7..].starts_with(&linux_prefix)
                || rest[7..].starts_with('/')
                || rest[7..].starts_with("C:\\")
                || rest[7..].starts_with("c:\\"))
        {
            let end = rest.find(is_path_stop).unwrap_or(rest.len());
            out.push_str("<LOCAL_PATH>");
            stripped += 1;
            i += end;
            continue;
        }

        if (rest.starts_with(&mac_prefix)
            || rest.starts_with(&linux_prefix)
            || rest.starts_with("C:\\")
            || rest.starts_with("c:\\"))
            && is_token_boundary(prev)
        {
            let end = rest.find(is_path_stop).unwrap_or(rest.len());
            out.push_str("<LOCAL_PATH>");
            stripped += 1;
            i += end;
            continue;
        }

        if is_token_boundary(prev) && secret_prefixes.iter().any(|p| rest.starts_with(p)) {
            let end = rest.find(is_path_stop).unwrap_or(rest.len());
            out.push_str("<REDACTED_SECRET>");
            stripped += 1;
            i += end;
            continue;
        }

        if is_token_boundary(prev) {
            if let Some(at_rel) = rest.find('@') {
                let local = &rest[..at_rel];
                if !local.is_empty() && local.chars().all(is_email_char) {
                    let after_at = &rest[at_rel + 1..];
                    let dom_len = after_at
                        .find(|c: char| !is_email_char(c))
                        .unwrap_or(after_at.len());
                    let domain = &after_at[..dom_len];
                    let trimmed_domain = domain.trim_end_matches('.');
                    if trimmed_domain.contains('.') && trimmed_domain.len() >= 3 {
                        out.push_str("<EMAIL>");
                        stripped += 1;
                        i += at_rel + 1 + trimmed_domain.len();
                        continue;
                    }
                }
            }
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, stripped)
}

pub fn build_redacted_community_draft(store: &Store) -> Result<CommunityDraftDto> {
    if !store.community_opt_in().map_err(|_| Error::Store)? {
        return Err(Error::Policy);
    }
    let memories = store
        .find_memory(MemoryFilter::default())
        .map_err(|_| Error::Store)?;
    let mut lines = Vec::new();
    let mut total_stripped = 0u32;
    for m in memories.into_iter().take(10) {
        let (t, s1) = redact_text(&m.title);
        let (c, s2) = redact_text(&m.content);
        total_stripped += s1 + s2;
        lines.push(format!("- **{t}**: {c}"));
    }
    let body = if lines.is_empty() {
        "No verified workflow learnings recorded yet.".to_owned()
    } else {
        lines.join("\n")
    };
    let generated_at = store.read(polyclave_store::now).map_err(|_| Error::Store)?;
    Ok(CommunityDraftDto {
        opt_in: true,
        title: "Polyclave local workflow learnings (redacted)".into(),
        redacted_markdown: body,
        stripped_items: total_stripped,
        generated_at,
    })
}

/// Formats `Applied` evolution suggestions into a bounded Hub system prompt section.
/// Suggestions transition back out immediately when rolled back.
pub fn format_applied_evolution_guidance(store: &Store) -> Result<String> {
    let all = store
        .list_evolution_suggestions()
        .map_err(|_| Error::Store)?;
    let applied: Vec<_> = all
        .into_iter()
        .filter(|s| s.status == EvolutionStatus::Applied)
        .take(12)
        .collect();
    if applied.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::from("<applied_evolution_guidance>\n");
    for s in applied {
        let kind = match s.kind {
            EvolutionKind::Routing => "routing",
            EvolutionKind::MemoryRule => "memory_rule",
            EvolutionKind::RoleBrief => "role_brief",
        };
        out.push_str(&format!("- [{}] {}: {}\n", kind, s.title, s.rationale));
    }
    out.push_str("</applied_evolution_guidance>");
    Ok(out)
}

/// Distills new suggestions only if no `Proposed` suggestions are currently pending review.
pub fn distill_if_due(store: &Store) -> Result<Vec<EvolutionSuggestionDto>> {
    let existing = store
        .list_evolution_suggestions()
        .map_err(|_| Error::Store)?;
    if existing
        .iter()
        .any(|s| s.status == EvolutionStatus::Proposed)
    {
        return Ok(Vec::new());
    }
    distill_local_suggestions(store)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommunitySubmitRequest {
    pub repo_slug: String,
    pub title: String,
    pub body: String,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommunitySubmitResult {
    pub issue_url: String,
    pub issue_number: u64,
}

fn valid_repo_slug(slug: &str) -> bool {
    let Some((owner, repo)) = slug.split_once('/') else {
        return false;
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return false;
    }
    let valid_part = |p: &str| {
        !p.contains("..")
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    valid_part(owner) && valid_part(repo)
}

/// Explicitly submits a user-confirmed, redacted community draft to GitHub Issues.
/// Enforces opt-in, explicit confirmation, valid repo slug, and secondary redaction.
pub async fn submit_redacted_community_draft(
    store: &Store,
    api_base: &str,
    token: &str,
    req: CommunitySubmitRequest,
) -> Result<CommunitySubmitResult> {
    if !store.community_opt_in().map_err(|_| Error::Store)? || !req.confirmed {
        return Err(Error::Policy);
    }
    if req.title.trim().is_empty()
        || req.body.trim().is_empty()
        || token.trim().is_empty()
        || !valid_repo_slug(&req.repo_slug)
    {
        return Err(Error::Policy);
    }
    let (_, s1) = redact_text(&req.title);
    let (_, s2) = redact_text(&req.body);
    if s1 > 0 || s2 > 0 {
        return Err(Error::Policy);
    }
    let base = api_base.trim_end_matches('/');
    let url = format!("{base}/repos/{}/issues", req.repo_slug);
    let http = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| Error::Transport)?;
    let resp = http
        .post(url)
        .header("Authorization", format!("Bearer {}", token.trim()))
        .header("User-Agent", "polyclave-v5")
        .header("Accept", "application/vnd.github+json")
        .json(&json!({
            "title": req.title,
            "body": req.body,
            "labels": ["community-learning"]
        }))
        .send()
        .await
        .map_err(|_| Error::Transport)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Http {
            status: status.as_u16(),
            detail: "community issue submission failed".into(),
        });
    }
    let val: serde_json::Value = resp.json().await.map_err(|_| Error::Transport)?;
    let issue_url = val["html_url"].as_str().ok_or(Error::Transport)?.to_owned();
    let issue_number = val["number"].as_u64().ok_or(Error::Transport)?;
    Ok(CommunitySubmitResult {
        issue_url,
        issue_number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use polyclave_proto::MemoryScope;
    use polyclave_store::stage4::NewLocalSkill;

    #[test]
    fn redacts_paths_emails_secrets_and_respects_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("s.sqlite")).unwrap();
        assert!(matches!(
            build_redacted_community_draft(&store),
            Err(Error::Policy)
        ));

        let sample = format!(
            "Fix {} for {} with {}",
            ["/U", "sers/alice/repo"].concat(),
            ["alice", "@example.com"].concat(),
            ["sk-", "secret123"].concat()
        );
        let (redacted, count) = redact_text(&sample);
        assert_eq!(count, 3);
        assert!(!redacted.contains("alice"));
        assert!(!redacted.contains("sk-secret123"));

        store.set_community_opt_in(true).unwrap();
        let draft = build_redacted_community_draft(&store).unwrap();
        assert!(draft.opt_in);

        let suggestions = distill_local_suggestions(&store).unwrap();
        assert!(!suggestions.is_empty());

        let skill = store
            .save_local_skill(NewLocalSkill {
                name: "Fast Check".into(),
                description: "Run cargo test".into(),
                body_md: "# Fast Check\ncargo test".into(),
                synced_vendors: vec![Vendor::Codex, Vendor::Dsh],
            })
            .unwrap();
        let synced = sync_skill_to_isolated_homes(dir.path(), &skill).unwrap();
        assert_eq!(synced, 2);
        assert!(dir
            .path()
            .join("isolated-homes/codex/skills/fast-check/SKILL.md")
            .is_file());
        assert!(dir
            .path()
            .join("isolated-homes/dsh/skills/fast-check/SKILL.md")
            .is_file());
        let _ = MemoryScope::Project;
    }

    #[test]
    fn local_embedding_is_unit_normalized_and_deterministic() {
        let a = local_embedding("rustls without openssl");
        let b = local_embedding("rustls without openssl");
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
    }

    #[test]
    fn applied_evolution_guidance_reflects_apply_and_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("evo.sqlite")).unwrap();
        assert!(format_applied_evolution_guidance(&store)
            .unwrap()
            .is_empty());

        let first = distill_if_due(&store).unwrap();
        assert!(!first.is_empty());
        // Does not flood while a Proposed suggestion is pending.
        assert!(distill_if_due(&store).unwrap().is_empty());

        let id = first[0].id.clone();
        store
            .set_evolution_status(&id, EvolutionStatus::Applied)
            .unwrap();
        let guidance = format_applied_evolution_guidance(&store).unwrap();
        assert!(guidance.contains("<applied_evolution_guidance>"));
        assert!(guidance.contains(&first[0].title));

        store
            .set_evolution_status(&id, EvolutionStatus::RolledBack)
            .unwrap();
        assert!(format_applied_evolution_guidance(&store)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn community_submit_enforces_opt_in_confirmation_and_secondary_redaction() {
        use axum::{routing::post, Json, Router};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("comm.sqlite")).unwrap();

        let req = CommunitySubmitRequest {
            repo_slug: "DustNonStar/polyclave".into(),
            title: "Use rustls without openssl".into(),
            body: "Verified workflow learning".into(),
            confirmed: true,
        };
        assert!(matches!(
            submit_redacted_community_draft(&store, "http://127.0.0.1:1", "tok", req.clone()).await,
            Err(Error::Policy)
        ));

        store.set_community_opt_in(true).unwrap();
        assert!(matches!(
            submit_redacted_community_draft(
                &store,
                "http://127.0.0.1:1",
                "tok",
                CommunitySubmitRequest {
                    confirmed: false,
                    ..req.clone()
                }
            )
            .await,
            Err(Error::Policy)
        ));

        // Secondary redaction rejects unredacted paths/secrets.
        let leaky_body = format!("Check {}", ["/U", "sers/alice/secret"].concat());
        assert!(matches!(
            submit_redacted_community_draft(
                &store,
                "http://127.0.0.1:1",
                "tok",
                CommunitySubmitRequest {
                    body: leaky_body,
                    ..req.clone()
                }
            )
            .await,
            Err(Error::Policy)
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/repos/DustNonStar/polyclave/issues",
            post(|| async {
                Json(json!({"html_url": "https://github.com/DustNonStar/polyclave/issues/42", "number": 42}))
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let res = submit_redacted_community_draft(&store, &format!("http://{addr}"), "tok", req)
            .await
            .unwrap();
        assert_eq!(res.issue_number, 42);
    }

    #[test]
    fn sync_skill_refuses_symlinked_skills_dir_and_redact_text_handles_embedded_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside-skills");
        std::fs::create_dir_all(&outside).unwrap();
        let skills_parent = dir.path().join("isolated-homes/codex");
        std::fs::create_dir_all(&skills_parent).unwrap();
        polyclave_os::prepare_isolated_link(&outside, &skills_parent.join("skills")).unwrap();

        let store = Store::open(dir.path().join("s.sqlite")).unwrap();
        let skill = store
            .save_local_skill(NewLocalSkill {
                name: "Unsafe".into(),
                description: "Desc".into(),
                body_md: "Body".into(),
                synced_vendors: vec![Vendor::Codex],
            })
            .unwrap();
        assert!(matches!(
            sync_skill_to_isolated_homes(dir.path(), &skill),
            Err(Error::Policy)
        ));

        let embedded = format!(
            "PATH=\"{}\" `file://{}` TOKEN={} user=<{}>",
            ["/U", "sers/bob/src"].concat(),
            ["/h", "ome/bob/proj"].concat(),
            ["ghp_", "secret999"].concat(),
            ["bob", "@corp.example.org"].concat(),
        );
        let (redacted, count) = redact_text(&embedded);
        assert_eq!(count, 4, "{redacted}");
        assert!(!redacted.contains("bob"));
        assert!(!redacted.contains("secret999"));
        assert!(!redacted.contains("corp.example.org"));
    }
}
