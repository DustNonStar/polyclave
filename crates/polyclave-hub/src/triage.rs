use crate::{Error, Result};
use polyclave_core::runs::{RunManager, RunRequest};
use polyclave_proto::RunDto;
use polyclave_store::{Store, TicketIntent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Component, Path};

#[derive(Clone)]
pub struct Triage {
    store: Store,
    runs: RunManager,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mandate {
    pub goal: String,
    pub allowed_paths: Vec<String>,
    pub forbidden_actions: Vec<String>,
    pub acceptance_criteria: Vec<String>,
}
impl Mandate {
    fn valid(&self) -> bool {
        !self.goal.trim().is_empty()
            && !self.allowed_paths.is_empty()
            && self.allowed_paths.iter().all(|p| {
                !p.is_empty()
                    && !p.contains(['\\', ':'])
                    && Path::new(p)
                        .components()
                        .all(|c| matches!(c, Component::Normal(_)))
            })
    }
}
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InferredAdmission {
    Ignored,
    Proposal,
    Eligible,
}
impl Triage {
    pub fn new(store: Store, runs: RunManager) -> Self {
        Self { store, runs }
    }
    pub fn explicit(
        &self,
        hub: &str,
        message: i64,
        title: &str,
        mandate: Mandate,
        max: u32,
    ) -> Result<TicketIntent> {
        if !mandate.valid() {
            return Err(Error::Policy);
        }
        self.store
            .hub_ticket(hub, message, title, json!(mandate), max)
            .map_err(|_| Error::Policy)
    }
    pub fn worker_receipt(&self, run: &str) -> Result<Option<String>> {
        self.store.hub_worker_receipt(run).map_err(|_| Error::Store)
    }
    /// Identity comes from a trusted observer, never from message text.
    pub fn inferred(
        &self,
        repo: &str,
        actor: &str,
        verified_human: bool,
        is_bot: bool,
    ) -> Result<InferredAdmission> {
        let (mode, bots, humans) = self
            .store
            .hub_inference_policy(repo)
            .map_err(|_| Error::Store)?;
        if mode == "off" || is_bot || bots.iter().any(|a| a.eq_ignore_ascii_case(actor)) {
            return Ok(InferredAdmission::Ignored);
        }
        if mode == "auto" && verified_human && humans.iter().any(|a| a.eq_ignore_ascii_case(actor))
        {
            return Ok(InferredAdmission::Eligible);
        }
        Ok(InferredAdmission::Proposal)
    }
    pub fn dispatch(&self, mut request: RunRequest) -> Result<(RunDto, bool)> {
        let ticket = request.ticket_id.as_ref().ok_or(Error::Policy)?.clone();
        let repo =
            polyclave_os::canonical_real_path(&request.repo_cwd).map_err(|_| Error::Policy)?;
        let repo = repo.to_str().ok_or(Error::Policy)?;
        let (saved, mandate) = self
            .store
            .hub_ticket_mandate(&ticket)
            .map_err(|_| Error::Policy)?;
        if saved.hub != request.hub_session_id {
            return Err(Error::Policy);
        }
        let mandate: Mandate = serde_json::from_value(mandate).map_err(|_| Error::Policy)?;
        if !mandate.valid() {
            return Err(Error::Policy);
        }
        self.store
            .hub_ticket_for_dispatch(&request.hub_session_id, &ticket, repo)
            .map_err(|_| Error::Policy)?;
        request.instructions = format!(
            "Owner Ticket mandate (host data):\n{}\n\nTask under this mandate:\n{}",
            json!(mandate),
            request.instructions
        );
        // RunManager reserves the budget with admission in the single SQLite
        // writer. A competing caller cannot spend a slot this check observed.
        match self.runs.accept(request) {
            Ok(r) => Ok(r),
            Err(polyclave_store::Error::BudgetExceededError) => {
                let _ = self.store.hub_close_ticket_budget(&ticket);
                Err(Error::Policy)
            }
            Err(_) => Err(Error::Policy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use polyclave_core::mirror::MirrorManager;
    use polyclave_proto::Vendor;
    use std::{process::Command, sync::Arc};
    fn fixture() -> (tempfile::TempDir, Store, Triage, RunRequest) {
        let t = tempfile::tempdir().unwrap();
        for dir in ["repo", "state", "other"] {
            std::fs::create_dir(t.path().join(dir)).unwrap();
        }
        let repo = polyclave_os::canonical_real_path(&t.path().join("repo")).unwrap();
        for args in [
            vec!["init", "-b", "main", "--template="],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        ] {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{:?}", out.stderr);
        }
        let store = Store::open(t.path().join("ledger")).unwrap();
        let path = repo.to_str().unwrap().to_owned();
        store.write(move|db|{
            db.execute_batch("INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                VALUES('p','test','codex_cli','[]','now');")?;
            db.execute("INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                VALUES('hub','test','hub',?1,'p','test','now')",[path])?;
            Ok(())
        }).unwrap();
        let git = Arc::new(MirrorManager::new(&t.path().join("state")).unwrap());
        let runs = RunManager::new(store.clone(), git);
        let triage = Triage::new(store.clone(), runs);
        let req = RunRequest {
            hub_session_id: "hub".into(),
            request_id: "dispatch-1".into(),
            vendor: Vendor::Codex,
            model: None,
            repo_cwd: repo,
            instructions: "change one file".into(),
            base_branch: "main".into(),
            ticket_id: None,
        };
        (t, store, triage, req)
    }
    fn mandate() -> Mandate {
        Mandate {
            goal: "edit specified file".into(),
            allowed_paths: vec!["src/lib.rs".into()],
            forbidden_actions: vec!["no merge".into()],
            acceptance_criteria: vec!["tests pass".into()],
        }
    }
    #[test]
    fn explicit_ticket_counts_budget_once_and_worker_receipts_cannot_self_trigger_twice() {
        let (_t, s, triage, mut a) = fixture();
        let id = s
            .hub_enqueue("hub", "ticket1", "approve one run")
            .unwrap()
            .id;
        let ticket = triage.explicit("hub", id, "title", mandate(), 1).unwrap();
        assert_eq!(
            triage
                .explicit("hub", id, "title", mandate(), 1)
                .unwrap()
                .id,
            ticket.id
        );
        assert!(triage.explicit("hub", id, "title", mandate(), 2).is_err());
        a.ticket_id = Some(ticket.id.clone());
        let (r, fresh) = triage.dispatch(a.clone()).unwrap();
        assert!(fresh);
        a.request_id = "different-request".into();
        assert!(triage.dispatch(a).is_err());
        let saved = s.hub_ticket_mandate(&ticket.id).unwrap().0;
        assert_eq!((saved.used, saved.status.as_str()), (1, "blocked_on_user"));
        assert!(triage.worker_receipt(&r.id).unwrap().is_none());
        let id = r.id.clone();
        s.write(move |db| {
            db.execute("UPDATE runs SET status='failed' WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            triage.worker_receipt(&r.id).unwrap().as_deref(),
            Some("hub")
        );
        assert!(triage.worker_receipt(&r.id).unwrap().is_none());
    }

    #[test]
    fn cross_repo_is_blocked_and_untrusted_actor_cannot_auto_admit() {
        let (t, s, triage, mut req) = fixture();
        let id = s.hub_enqueue("hub", "ticket", "approve").unwrap().id;
        let ticket = triage.explicit("hub", id, "title", mandate(), 4).unwrap();
        req.ticket_id = Some(ticket.id.clone());
        req.repo_cwd = t.path().join("other");
        assert!(triage.dispatch(req).is_err());
        let saved = s.hub_ticket_mandate(&ticket.id).unwrap().0;
        assert_eq!((saved.used, saved.status.as_str()), (0, "blocked_on_user"));
        let repo = saved.repo.clone();
        s.write(move|db|{db.execute("INSERT INTO admission_policies(repo_cwd,inferred_mode,bot_actors_json,human_actors_json,updated_at)
             VALUES(?1,'auto','[\"bot\"]','[\"human\"]','now')",[repo])?;Ok(())}).unwrap();
        for who in ["bot", "BOT"] {
            assert_eq!(
                triage.inferred(&saved.repo, who, true, false).unwrap(),
                InferredAdmission::Ignored
            );
        }
        assert_eq!(
            triage.inferred(&saved.repo, "human", false, false).unwrap(),
            InferredAdmission::Proposal
        );
        assert_eq!(
            triage.inferred(&saved.repo, "human", true, true).unwrap(),
            InferredAdmission::Ignored
        );
        assert_eq!(
            triage.inferred(&saved.repo, "human", true, false).unwrap(),
            InferredAdmission::Eligible
        );
    }
}
