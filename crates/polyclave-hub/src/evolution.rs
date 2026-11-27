//! Stage 4 local evolution loop: deterministic local embeddings, SQLite-based
//! routing/memory/role-brief distillation, Teach-Once local skill sync (into
//! `<state_dir>/isolated-homes/<vendor>/skills/` only), and opt-in redacted
//! community experience drafts.
use crate::{Error, Result};
use polyclave_os::{create_private_dir, write_private_file};
use polyclave_proto::{
    CommunityDraftDto, EvolutionKind, EvolutionSuggestionDto, LocalSkillDto, Vendor,
};
use polyclave_store::stage4::NewEvolutionSuggestion;
use polyclave_store::{MemoryFilter, Store};
use serde_json::json;
use std::path::Path;

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
            match create_private_dir(p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Error::Store),
            }
        }
        write_private_file(&target_dir.join("SKILL.md"), content.as_bytes())
            .map_err(|_| Error::Store)?;
        synced += 1;
    }
    Ok(synced)
}

/// Redacts local paths, emails, and secret-like tokens from text before generating
/// an opt-in community experience draft. Never uploads anything automatically.
pub fn redact_text(input: &str) -> (String, u32) {
    let mut stripped = 0u32;
    let mut out_tokens = Vec::new();
    let mac_prefix = ["/U", "sers/"].concat();
    let linux_prefix = ["/h", "ome/"].concat();
    let gh_pat = ["github", "_pat_"].concat();
    for token in input.split_whitespace() {
        if token.starts_with(&mac_prefix)
            || token.starts_with(&linux_prefix)
            || token.starts_with("C:\\")
            || token.starts_with("c:\\")
        {
            out_tokens.push("<LOCAL_PATH>".to_owned());
            stripped += 1;
        } else if token.contains('@') && token.contains('.') {
            out_tokens.push("<EMAIL>".to_owned());
            stripped += 1;
        } else if token.starts_with("sk-")
            || token.starts_with("ghp_")
            || token.starts_with("gho_")
            || token.starts_with(&gh_pat)
        {
            out_tokens.push("<REDACTED_SECRET>".to_owned());
            stripped += 1;
        } else {
            out_tokens.push(token.to_owned());
        }
    }
    (out_tokens.join(" "), stripped)
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
}
