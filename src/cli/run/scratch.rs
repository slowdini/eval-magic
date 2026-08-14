//! Task-local scratch guidance shared by dispatch prompt assembly.

use std::path::Path;

use crate::core::fs::artifact_path;
use crate::sandbox::TASK_SCRATCH_DIR;

pub(super) fn context(eval_root: &str) -> String {
    format!(
        "Task environment: {eval_root}\nTask-local scratch directory: {}",
        artifact_path(&Path::new(eval_root).join(TASK_SCRATCH_DIR))
    )
}

pub(super) fn push_instruction(lines: &mut Vec<String>, eval_root: Option<&str>) {
    if eval_root.is_some() {
        lines.push(
            "- Keep temporary and scratch files in the task-local scratch directory, not in a host temp directory."
                .to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_task_local_scratch_guidance() {
        let mut lines = Vec::new();

        let context = context("/work/task");
        push_instruction(&mut lines, Some("/work/task"));

        assert_eq!(
            context,
            "Task environment: /work/task\nTask-local scratch directory: /work/task/tmp"
        );
        assert_eq!(
            lines,
            [
                "- Keep temporary and scratch files in the task-local scratch directory, not in a host temp directory."
            ]
        );
    }

    #[test]
    fn omits_instruction_without_an_eval_root() {
        let mut lines = Vec::new();
        push_instruction(&mut lines, None);
        assert!(lines.is_empty());
    }
}
