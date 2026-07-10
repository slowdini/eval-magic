//! Codex harness support.
//!
//! The declarative half of this harness lives in `harnesses/codex.toml`; this
//! module tree keeps only the code-backed capabilities the descriptor
//! references: `item.completed` event-stream parsing ([`transcript`]) and the
//! write-guard hook ([`guard`]).

pub(crate) mod guard;
pub mod transcript;

#[cfg(test)]
mod tests {
    use crate::adapters::adapter_for;
    use crate::core::Harness;

    #[test]
    fn codex_parse_cli_events_delegates_to_events_parser() {
        use serde_json::json;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("codex-events.jsonl");
        let line = json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "command": "bun test", "output": "ok"}});
        std::fs::write(&path, format!("{line}\n")).unwrap();

        let inv = adapter_for(Harness::Codex).parse_cli_events(&path).unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].name, "command_execution");
    }
}
