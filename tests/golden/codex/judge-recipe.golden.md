Dispatch each judge task from judge-tasks.json with:

```bash
JOBS=${JOBS:-4}
jq -j '.tasks[] | [.dispatch_prompt_path, .response_path, (.model // "")] | @tsv + "\u0000"' judge-tasks.json | \
  xargs -0 -P "$JOBS" -I{} sh -c '
    prompt_path="$(printf "%s" "$1" | cut -f1)"
    response_path="$(printf "%s" "$1" | cut -f2)"
    model="$(printf "%s" "$1" | cut -f3)"
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="-m $model"
    codex --ask-for-approval never exec --cd "/work/iter-1" --sandbox workspace-write $model_arg --json \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.codex-events.jsonl" \
      2> "$response_base.codex-stderr.log"
  ' sh {}
```