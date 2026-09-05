> **Codex inside Codex:** If the same generated task command succeeds in an ordinary terminal
> with equivalent inputs and configuration, but fails inside the operator Codex session with
> `Operation not permitted`, the outer sandbox may be responsible. This error alone does not establish
> the cause; the inner sandbox cannot grant access denied by the outer process. Prefer running
> the generated `eval-magic dispatch` command from that ordinary terminal. Alternatively, approve
> or escalate the outer launch of `eval-magic dispatch` where the operator surface and policy support
> it, limited to the required workspace and process access. Keep the task's `--sandbox workspace-write`
> and eval guard enabled. See `eval-magic docs isolation` for diagnosis and limits on creating
> the inner sandbox.
