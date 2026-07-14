//! Claude Code harness support — the default harness.
//!
//! The declarative half of this harness lives in `harnesses/claude-code.toml`;
//! this module tree keeps only the code-backed capabilities the descriptor
//! references: `claude -p` stream-json transcript parsing ([`stream_json`] +
//! [`transcript`]), plugin-shadow detection ([`plugin_shadow`]), and the
//! write-guard hook ([`guard`]).

pub(crate) mod guard;
pub mod plugin_shadow;
pub mod stream_json;
pub mod transcript;

#[cfg(test)]
mod tests {
    use crate::adapters::adapter_for;
    use crate::core::Harness;

    #[test]
    fn claude_adapter_advertises_cli_events_file_and_model_flag() {
        let a = adapter_for(Harness::resolve("claude-code").unwrap());
        assert_eq!(
            a.cli_events_filename().as_deref(),
            Some("claude-events.jsonl")
        );
        assert_eq!(a.cli_model_flag().as_deref(), Some("--model"));
    }

    #[test]
    fn claude_parse_cli_events_full_reads_stream_json_result_event() {
        use serde_json::json;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("claude-events.jsonl");
        // No per-line timestamps; the result event is the only source of duration.
        let lines = [
            json!({"type": "assistant", "message": {"id": "msg_1", "role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
            ]}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done", "duration_ms": 5637, "usage": {"input_tokens": 1, "output_tokens": 2, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}),
        ];
        let body = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, format!("{body}\n")).unwrap();

        let a = adapter_for(Harness::resolve("claude-code").unwrap());
        let summary = a.parse_cli_events_full(&path).unwrap();
        assert_eq!(summary.final_text, Some("Done".into()));
        assert_eq!(summary.duration_ms, Some(5637));
        assert_eq!(summary.tool_invocations.len(), 1);
        assert_eq!(summary.tool_invocations[0].name, "Bash");
    }
}
