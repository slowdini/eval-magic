// slow-powers eval write guard — staged by `eval-magic` into this env's
// project plugins; removed by `eval-magic teardown-guard` (or the next run).
// Do not edit: re-staging overwrites, and teardown restores the original.
//
// Dumb forwarder by design: every tool call goes to
// `eval-magic guard-hook --harness cline <marker>` on stdin and the shared
// arbiter inside the binary classifies it. Empty stdout allows; non-empty
// stdout is the deny verdict JSON whose reason blocks the call.
import { spawnSync } from "node:child_process";

const EXE = {exe};
const MARKER = {marker};

// Cline's plugin hook surface (3.0.53): the runtime calls `beforeTool` with
// {snapshot, tool, toolCall, input}; returning {skip: true, reason} blocks
// the call and the reason reaches the agent (and the transcript).
const SlowPowersEvalGuard = {
  name: "slow-powers-eval-guard",
  manifest: { capabilities: ["hooks"] },
  hooks: {
    beforeTool(context) {
      const name = context?.toolCall?.toolName ?? "";
      const input = context?.toolCall?.input ?? context?.input ?? {};
      // run_commands nests its shell commands as an array; the shared arbiter
      // classifies one `command` string, so join before forwarding.
      let toolInput = input;
      if (name === "run_commands" && Array.isArray(input?.commands)) {
        const { commands, ...rest } = input;
        toolInput = { ...rest, command: commands.join("\n") };
      }
      const payload = JSON.stringify({ tool_name: name, tool_input: toolInput });
      const result = spawnSync(EXE, ["guard-hook", "--harness", "cline", MARKER], {
        input: payload,
        encoding: "utf8",
        // Under the runtime's 3000ms hook budget, so a hung arbiter fails
        // open here rather than erroring the hook.
        timeout: 2000,
        stdio: ["pipe", "pipe", "ignore"],
      });
      const stdout = (result.stdout ?? "").trim();
      if (!stdout) {
        return {}; // allow — also the fail-open path on spawn error or timeout
      }
      let reason = stdout;
      try {
        const verdict = JSON.parse(stdout);
        if (typeof verdict?.reason === "string") {
          reason = verdict.reason;
        }
      } catch {
        // Not the verdict shape — surface the raw stdout as the reason.
      }
      return { skip: true, reason };
    },
  },
};

export default SlowPowersEvalGuard;
