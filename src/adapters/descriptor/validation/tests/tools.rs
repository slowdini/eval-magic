use super::{MINIMAL, err_of};

#[test]
fn rejects_tool_declared_in_more_than_one_role() {
    let err = err_of(&format!(
        "{MINIMAL}\n[tools]\nwrite = [\"Edit\"]\nshell = [\"Edit\"]\n"
    ));
    assert!(err.contains("more than one [tools] role"), "{err}");
    assert!(err.contains("Edit"), "{err}");
}
