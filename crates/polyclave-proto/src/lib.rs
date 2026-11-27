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
            TurnDto,
            MemoryItemDto,
            ProviderProfileDto,
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
}
