//! Plan-mode guidance shared by dispatch prompt assembly.
//!
//! A harness's own planning mode already tells the agent how it presents a
//! plan, and each one says something different — Claude Code points at
//! `~/.claude/plans`, OpenCode's `plan` agent says nothing of the kind. These
//! lines are the part eval-magic can rely on everywhere: the agent closes its
//! planning turn with the whole plan, so the driver has a plan to save as
//! `outputs/plan.md` whatever the harness does.

use crate::core::PlanMode;

pub(super) fn push_instructions(lines: &mut Vec<String>, plan_mode: PlanMode) {
    if plan_mode.is_off() {
        return;
    }
    lines.push(
        "- This session starts in the harness's planning mode: read and explore the task environment, but do not edit files."
            .to_string(),
    );
    lines.push(
        "- End this turn with your complete plan as your final message — the whole plan text, not a summary of it and not a pointer to where it lives."
            .to_string(),
    );
    lines.push(
        if plan_mode.implements_the_plan() {
            "- You will be told when the plan is approved; implement it in this same session then."
        } else {
            "- The plan is the deliverable: this task ends once you have presented it, and there is nothing to implement."
        }
        .to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two lines every planning round gets, whatever the harness and
    /// whatever follows the plan.
    #[test]
    fn both_plan_shapes_ask_for_the_whole_plan_in_the_final_message() {
        for plan_mode in [PlanMode::PlanThenAct, PlanMode::PlanOnly] {
            let mut lines = Vec::new();
            push_instructions(&mut lines, plan_mode);
            let rendered = lines.join("\n");
            assert!(
                rendered.contains("planning mode"),
                "{plan_mode:?}: {rendered}"
            );
            assert!(
                rendered.contains("do not edit files"),
                "{plan_mode:?}: {rendered}"
            );
            assert!(
                rendered.contains("complete plan as your final message"),
                "{plan_mode:?}: {rendered}"
            );
        }
    }

    /// What follows the plan is the one thing the two shapes disagree on, and
    /// the agent is told which it is so it does not sit waiting for an approval
    /// that never comes.
    #[test]
    fn each_shape_says_what_follows_the_plan() {
        let mut then_act = Vec::new();
        push_instructions(&mut then_act, PlanMode::PlanThenAct);
        let then_act = then_act.join("\n");
        assert!(then_act.contains("approved"), "{then_act}");
        assert!(!then_act.contains("nothing to implement"), "{then_act}");

        let mut plan_only = Vec::new();
        push_instructions(&mut plan_only, PlanMode::PlanOnly);
        let plan_only = plan_only.join("\n");
        assert!(plan_only.contains("nothing to implement"), "{plan_only}");
        assert!(!plan_only.contains("approved"), "{plan_only}");
    }

    /// An eval outside plan mode contributes nothing, so its prompt is
    /// byte-identical to what it was before this module existed.
    #[test]
    fn a_non_plan_mode_eval_contributes_nothing() {
        let mut lines = Vec::new();
        push_instructions(&mut lines, PlanMode::Off);
        assert!(lines.is_empty());
    }
}
