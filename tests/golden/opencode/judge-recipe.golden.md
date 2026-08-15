Dispatch each judge task from judge-tasks.json with:
Existing nonempty response files are skipped; delete one to dispatch that judge again.
The final `N/M verdicts present` summary exits nonzero until every task has one.

```bash
JOBS=${JOBS:-4}
jq -r '.tasks[] | .dispatch_prompt_path, .response_path, ("model=" + (.model // ""))' judge-tasks.json \
  | tr -d '\r' \
  | tr '\n' '\0' \
  | xargs -0 -P "$JOBS" -n 3 sh -c '
    prompt_path="$1"
    response_path="$2"
    model="${3#model=}"
    if [ -s "$response_path" ]; then exit 0; fi
    response_base="${response_path%.json}"
    mkdir -p "$(dirname "$response_path")"
    model_arg=""; [ -n "$model" ] && model_arg="-m $model"
    opencode run --dir "/work/iter-1" --format json --auto $model_arg \
      "Read the file at $prompt_path and follow it exactly. You are a judge worker only: write the JSON verdict to $response_path, then reply with one sentence. Do not run eval-magic. Do not dispatch other judge tasks. Do not wait for other workers." \
      </dev/null \
      > "$response_base.opencode-events.jsonl" \
      2> "$response_base.opencode-stderr.log"
  ' sh
judge_dispatch_status=$?
judge_total=$(jq '.tasks | length' judge-tasks.json | tr -d '\r')
judge_present=$(
  jq -r '.tasks[].response_path' judge-tasks.json \
    | tr -d '\r' \
    | while IFS= read -r response_path; do
        if [ -s "$response_path" ]; then printf '%s\n' "$response_path"; fi
      done \
    | wc -l \
    | tr -d '[:space:]'
)
printf '%s/%s verdicts present\n' "$judge_present" "$judge_total"
[ "$judge_dispatch_status" -eq 0 ] && [ "$judge_present" -eq "$judge_total" ]
```