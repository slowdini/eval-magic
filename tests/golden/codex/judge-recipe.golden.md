Dispatch each judge task from judge-tasks.json with:
Existing nonempty response files are skipped; delete one to dispatch that judge again.

```bash
JOBS=${JOBS:-4}
jq -j '.tasks[] | .dispatch_prompt_path, "\u0000", .response_path, "\u0000", ("model=" + (.model // "")), "\u0000"' judge-tasks.json | \
  xargs -0 -P "$JOBS" -n 3 sh -c '
    prompt_path="$1"
    response_path="$2"
    model="${3#model=}"
    if [ -s "$response_path" ]; then exit 0; fi
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="-m $model"
    codex --ask-for-approval never exec --cd "/work/iter-1" --sandbox workspace-write $model_arg --json \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.codex-events.jsonl" \
      2> "$response_base.codex-stderr.log"
  ' sh
```