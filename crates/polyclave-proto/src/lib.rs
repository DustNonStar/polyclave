//! Transport-only types. This crate owns no platform or persistence primitives.
mod dto;
mod error;
mod events;

pub use dto::*;
pub use error::ApiError;
pub use events::*;

#[cfg(test)]
mod tests {
    use super::*;
    use ts_rs::TS;

    macro_rules! decls {
        ($($t:ty),+ $(,)?) => { [$(format!("export {}", <$t as TS>::decl())),+] };
    }

    #[test]
    fn generated_types_match_source_control() {
        let definitions = decls!(
            SessionRole,
            Vendor,
            MessageSource,
            ClientKind,
            TicketSource,
            TicketStatus,
            RunStatus,
            ResultSource,
            AttemptStatus,
            TurnMode,
            TurnStatus,
            TurnPhase,
            EndReason,
            WriterCheck,
            MemoryScope,
            MemoryKind,
            MemoryStatus,
            ProviderKind,
            SessionDto,
            MessageDto,
            TicketDto,
            RunDto,
            AttemptDto,
            RunEvidenceDto,
            TestResultDto,
            TestResultSource,
            TestStatus,
            TurnDto,
            MemoryItemDto,
            ProviderProfileDto,
            CouncilLevel,
            PlanningPreference,
            CouncilStatus,
            ProposalPayload,
            CouncilProposalDto,
            CouncilDto,
            PrGateStatus,
            ReviewVerdict,
            PrReviewDto,
            PrGateDto,
            PauseStateDto,
            PairingTicketDto,
            PairedDeviceDto,
            MeshNodeStatus,
            MeshNodeDto,
            MeshNodeLimitDto,
            NotificationChannelKind,
            NotificationChannelDto,
            CuaLockDto,
            EvolutionKind,
            EvolutionStatus,
            EvolutionSuggestionDto,
            LocalSkillDto,
            CommunityDraftDto,
            ChangeTopic,
            SseEnvelope,
            AgentEvent,
            ApiError,
        );
        let expected = format!(
            "// Type contract exported by polyclave-proto.
export type JsonValue = string | number | boolean | null | JsonValue[] | {{ [key: string]: JsonValue }};

{}
",
            definitions.join("

")
        );
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/polyclave-app/ui/src/generated/proto.ts");
        if std::env::var_os("POLYCLAVE_UPDATE_PROTO").is_some() {
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, &expected).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(target)
                .unwrap()
                .replace("\r\n", "\n"),
            expected,
            "Regenerate with POLYCLAVE_UPDATE_PROTO=1 cargo test -p polyclave-proto"
        );
    }

    #[test]
    fn public_turn_serialization_has_no_host_secret_or_process_identity() {
        let turn = TurnDto {
            id: 1,
            session_id: "s-fixture".into(),
            run_id: None,
            attempt_id: None,
            parent_turn_id: None,
            mode: TurnMode::Hub,
            status: TurnStatus::Running,
            phase: TurnPhase::Authorized,
            exit_code: None,
            end_reason: None,
            writer_check: None,
            started_at: "2026-11-26T00:00:00Z".into(),
            ended_at: None,
        };
        let value = serde_json::to_value(turn).unwrap();
        assert!(value.get("mcp_token").is_none());
        assert!(value.get("os_proc_id").is_none());
        assert_eq!(value["phase"], "authorized");
        assert!(!TurnDto::decl().contains("mcp_token"));
    }

    #[test]
    fn run_evidence_serializes_absent_evidence_as_null_and_empty_results() {
        let evidence = RunEvidenceDto {
            attempt_no: 2,
            receipt_sha: None,
            worker_claim: None,
            test_sha: None,
            receipt_tests: None,
            receipt_tests_status: None,
            receipt_not_done: None,
            test_results: Vec::new(),
        };
        let value = serde_json::to_value(&evidence).unwrap();
        // Exact equality: nothing is recorded, so every optional field is null and
        // the per-SHA list is empty rather than absent.
        assert_eq!(
            value,
            serde_json::json!({
                "attempt_no": 2,
                "receipt_sha": null,
                "worker_claim": null,
                "test_sha": null,
                "receipt_tests": null,
                "receipt_tests_status": null,
                "receipt_not_done": null,
                "test_results": []
            })
        );
        assert_eq!(
            serde_json::from_value::<RunEvidenceDto>(value).unwrap(),
            evidence
        );
    }

    #[test]
    fn test_result_serializes_as_the_ledger_spells_it() {
        for (source, raw) in [
            (TestResultSource::WorkerClaim, "worker_claim"),
            (TestResultSource::HostRun, "host_run"),
            (TestResultSource::Ci, "ci"),
        ] {
            assert_eq!(source.as_str(), raw);
            assert_eq!(
                serde_json::to_value(source).unwrap(),
                serde_json::json!(raw)
            );
            assert_eq!(TestResultSource::parse(raw), Some(source));
        }
        for (status, raw) in [
            (TestStatus::Passed, "passed"),
            (TestStatus::Failed, "failed"),
            (TestStatus::Pending, "pending"),
        ] {
            assert_eq!(status.as_str(), raw);
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(raw)
            );
            assert_eq!(TestStatus::parse(raw), Some(status));
        }
        assert_eq!(TestResultSource::parse("chat"), None);
        assert_eq!(TestStatus::parse("green"), None);
    }
}
