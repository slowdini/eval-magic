use super::{MINIMAL, TOOLED, err_of};

#[test]
fn rejects_transcript_without_write_and_shell_tools() {
    let err = err_of(&format!(
        "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n"
    ));
    assert!(err.contains("detect-stray-writes"), "{err}");
}

#[test]
fn rejects_extract_transcript_without_write_and_shell_tools() {
    let err = err_of(&format!(
        "{MINIMAL}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
         [transcript.extract.final_text]\nfield = \"text\"\n"
    ));
    assert!(err.contains("detect-stray-writes"), "{err}");
}

#[test]
fn rejects_transcript_with_parser_and_extract() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\nparser = \"codex-items\"\n\n\
         [transcript.extract.final_text]\nfield = \"text\"\n"
    ));
    assert!(err.contains("exactly one"), "{err}");
    assert!(err.contains("new label"), "{err}");
}

#[test]
fn rejects_transcript_with_neither_parser_nor_extract() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n"
    ));
    assert!(err.contains("exactly one"), "{err}");
    assert!(err.contains("llm_judge"), "{err}");
}

#[test]
fn rejects_empty_extract_block() {
    let err = err_of(&format!(
        "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n[transcript.extract]\n"
    ));
    assert!(err.contains("[transcript.extract]"), "{err}");
    assert!(err.contains("at least one"), "{err}");
}

#[test]
fn rejects_duration_with_both_variants_or_neither() {
    for duration_block in [
        "field = \"elapsed_ms\"\ntimestamp_spread = \"timestamp\"\n",
        "",
    ] {
        let err = err_of(&format!(
            "{TOOLED}\n[transcript]\nevents_filename = \"demo-events.jsonl\"\n\n\
             [transcript.extract.duration]\n{duration_block}"
        ));
        assert!(err.contains("duration"), "{err}");
        assert!(err.contains("exactly one"), "{err}");
    }
}
