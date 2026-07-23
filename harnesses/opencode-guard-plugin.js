// slow-powers eval write guard — staged by `eval-magic` into this env's
// project plugins; removed by `eval-magic teardown-guard` (or the next run).
// Do not edit: re-staging overwrites, and teardown restores the original.
//
// Dumb forwarder by design: every tool call goes to
// `eval-magic guard-hook --harness opencode <marker>` on stdin and the shared
// arbiter inside the binary classifies it. Empty stdout allows; non-empty
// stdout is the deny verdict JSON whose reason blocks the call.
import { spawnSync } from "node:child_process";

const EXE = {exe};
const MARKER = {marker};

export const SlowPowersEvalGuard = async () => {
  return {
    "tool.execute.before": async (input, output) => {
      const payload = JSON.stringify({
        tool_name: input.tool,
        tool_input: output?.args ?? {},
      });
      const result = spawnSync(EXE, ["guard-hook", "--harness", "opencode", MARKER], {
        input: payload,
        encoding: "utf8",
        timeout: 10000,
        stdio: ["pipe", "pipe", "ignore"],
      });
      const stdout = (result.stdout ?? "").trim();
      if (!stdout) {
        return; // allow — also the fail-open path on spawn error or timeout
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
      throw new Error(reason);
    },
  };
};
