use super::*;

const ROOTS: [&str; 1] = ["/work/env"];

fn classify(command: &str) -> Option<BashClassification> {
    classify_output_targets(command, &ROOTS.map(str::to_string), Path::new("/work/env"))
}

fn tokens(command: &str) -> Vec<ShellToken> {
    lex_shell(command).tokens
}

fn word(value: &str) -> ShellToken {
    ShellToken::Word(ShellWord::literal(value))
}

#[test]
fn lexer_reads_fd_duplication_as_its_own_token() {
    assert_eq!(
        tokens("cmd 2>&1"),
        vec![word("cmd"), word("2"), ShellToken::FdDuplicate]
    );
    assert_eq!(
        tokens("cmd >&2"),
        vec![word("cmd"), ShellToken::FdDuplicate]
    );
    assert_eq!(
        tokens("cmd >&-"),
        vec![word("cmd"), ShellToken::FdDuplicate]
    );
}

#[test]
fn lexer_keeps_treating_ampersand_before_a_redirect_as_file_output() {
    // `&>file` redirects both streams *to a file* — the leading `&` is a
    // separator, not part of the redirect operator.
    assert_eq!(
        tokens("cmd &>out.txt"),
        vec![
            word("cmd"),
            ShellToken::Separator,
            ShellToken::OutputRedirect,
            word("out.txt")
        ]
    );
}

#[test]
fn lexer_keeps_appending_and_clobbering_redirects_distinct_from_duplication() {
    assert_eq!(
        tokens("cmd 2>>log.txt"),
        vec![
            word("cmd"),
            word("2"),
            ShellToken::OutputRedirect,
            word("log.txt")
        ]
    );
    assert_eq!(
        tokens("cmd >|out.txt"),
        vec![word("cmd"), ShellToken::OutputRedirect, word("out.txt")]
    );
}

#[test]
fn fd_duplication_alone_requests_no_file_output() {
    assert_eq!(classify("git status 2>&1 | head -20"), None);
    assert_eq!(classify("printf done >&2"), None);
}

#[test]
fn fd_duplication_does_not_poison_an_in_bounds_redirect() {
    assert_eq!(classify("printf done > out.txt 2>&1"), None);
}

#[test]
fn heredoc_delimiter_forms_hide_body_syntax_from_output_classification() {
    for command in [
        "cat > tmp/out <<EOF\nprintf done > \"$OUT\"\nEOF",
        "cat > tmp/out <<'EOF'\nprintf done > \"$OUT\"\nEOF",
        "cat > tmp/out <<\"EOF\"\nprintf done > \"$OUT\"\nEOF",
        "cat > tmp/out <<-EOF\n\tprintf done > \"$OUT\"\n\tEOF",
    ] {
        assert_eq!(classify(command), None, "{command}");
    }
}

#[test]
fn multiple_heredoc_bodies_are_skipped_in_declaration_order() {
    let command = "cat > tmp/first <<FIRST > tmp/second <<'SECOND'\n\
        printf done > \"$FIRST_TARGET\"\n\
        FIRST\n\
        printf done > \"$SECOND_TARGET\"\n\
        SECOND\n\
        printf done > tmp/final";

    assert_eq!(classify(command), None);
}

#[test]
fn redirects_outside_a_heredoc_body_are_still_classified() {
    for command in [
        "cat > /etc/out <<'EOF'\nprintf done > \"$BODY_TARGET\"\nEOF",
        "cat > tmp/out <<'EOF'\nprintf done > \"$BODY_TARGET\"\nEOF\nprintf done > /etc/out",
    ] {
        let denial = classify(command).expect("should deny the real outside redirect");

        assert_eq!(denial.reason, OUTPUT_REDIRECTION_REASON);
        assert!(denial.resolved_targets.contains(&"/etc/out".to_string()));
    }
}

#[test]
fn a_malformed_heredoc_with_file_output_stays_fail_closed() {
    for command in [
        "cat > tmp/out <<\nprintf done",
        "cat > tmp/out <<'EOF'\nprintf done",
    ] {
        let denial = classify(command).expect("should deny malformed output syntax");

        assert_eq!(denial.reason, OUTPUT_REDIRECTION_REASON);
    }
}

#[test]
fn non_file_devices_are_not_recorded_as_targets() {
    assert_eq!(classify("ls fixtures 2>/dev/null"), None);
    assert_eq!(classify("printf done >/dev/null 2>&1"), None);
    assert_eq!(classify("printf done | tee /dev/null"), None);
}

#[test]
fn an_out_of_bounds_target_still_denies_alongside_a_duplication() {
    let denial = classify("printf done > /etc/out.txt 2>&1").expect("should deny");
    assert_eq!(denial.reason, OUTPUT_REDIRECTION_REASON);
    assert_eq!(denial.resolved_targets, vec!["/etc/out.txt".to_string()]);
}

#[test]
fn a_dynamic_target_still_denies_with_no_resolved_target() {
    let denial = classify("printf done > \"$OUT\" 2>&1").expect("should deny");
    assert!(denial.resolved_targets.is_empty());
}

#[test]
fn a_dev_prefix_does_not_launder_an_out_of_bounds_target() {
    for command in [
        "printf done > /dev/nullx",
        "printf done > /dev/sda",
        "printf done > /dev/../etc/passwd",
    ] {
        assert!(classify(command).is_some(), "should deny: {command}");
    }
}
