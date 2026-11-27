use crate::{Error, Result};
use polyclave_proto::{MemoryItemDto, MemoryKind};
use polyclave_store::{Curator, MemoryAction, Store};

pub struct MemoryCurator {
    store: Store,
    repo: String,
    actor: Curator,
}
impl MemoryCurator {
    pub fn for_host(store: Store, repo: String, actor: Curator) -> Self {
        Self { store, repo, actor }
    }
    pub fn draft_correction(
        &self,
        session: &str,
        message: i64,
        kind: MemoryKind,
        title: &str,
        content: &str,
    ) -> Result<MemoryItemDto> {
        if self.actor != Curator::User {
            return Err(Error::Policy);
        }
        self.store
            .draft_correction(session, message, kind, title, content)
            .map_err(|_| Error::Policy)
    }
    pub fn memory_commit(&self, id: &str) -> Result<MemoryItemDto> {
        self.apply(id, MemoryAction::Commit)
    }
    pub fn memory_supersede(&self, id: &str, prior: &str) -> Result<MemoryItemDto> {
        self.apply(id, MemoryAction::Supersede(prior.into()))
    }
    pub fn memory_archive(&self, id: &str) -> Result<MemoryItemDto> {
        self.apply(id, MemoryAction::Archive)
    }
    fn apply(&self, id: &str, action: MemoryAction) -> Result<MemoryItemDto> {
        self.store
            .curate_memory(&self.repo, id, self.actor, action)
            .map_err(|_| Error::Policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use polyclave_proto::MemoryStatus;
    #[test]
    fn user_confirmed_correction_and_version_chain_do_not_promote_candidates_implicitly() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("memory.db")).unwrap();
        store
            .write(|db| {
                db.execute_batch(
                    "
                INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                    VALUES('cli','CLI','codex_cli','[\"test\"]','now');
                INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                    VALUES('s','Hub','hub','project','cli','test','now');
            ",
                )?;
                Ok(())
            })
            .unwrap();
        let id = store
            .hub_enqueue("s", "first", "do not auto merge")
            .unwrap()
            .id;
        let user = MemoryCurator::for_host(store.clone(), "project".into(), Curator::User);
        let hub = MemoryCurator::for_host(store.clone(), "project".into(), Curator::Hub);
        let draft = user
            .draft_correction(
                "s",
                id,
                MemoryKind::Constraint,
                "Merge",
                "Do not auto merge",
            )
            .unwrap();
        assert_eq!(draft.status, MemoryStatus::Candidate);
        assert!(store.find_memory(Default::default()).unwrap().is_empty());
        assert!(hub.memory_commit(&draft.id).is_err());
        let first = user.memory_commit(&draft.id).unwrap();
        assert_eq!(first.status, MemoryStatus::Verified);
        let next = user
            .draft_correction(
                "s",
                id,
                MemoryKind::Constraint,
                "Merge",
                "Use owner review before merging",
            )
            .unwrap();
        let next = user.memory_supersede(&next.id, &first.id).unwrap();
        assert_eq!(next.supersedes_id, Some(first.id));
        assert_eq!(store.find_memory(Default::default()).unwrap().len(), 1);
        assert_eq!(
            user.memory_archive(&next.id).unwrap().status,
            MemoryStatus::Archived
        );
        assert!(store.find_memory(Default::default()).unwrap().is_empty());
        let advisor = store
            .register_manager("s", "cli", "test", "Advisor")
            .unwrap();
        assert_eq!(
            store
                .register_manager("s", "cli", "test", "Advisor")
                .unwrap(),
            advisor
        );
        assert!(store
            .register_manager("s", "cli", "other", "Advisor")
            .is_err());
    }
}
