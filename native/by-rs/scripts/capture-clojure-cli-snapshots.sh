#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
out_dir="$repo_root/native/by-rs/target/cli-snapshots/clojure"
bin_path="${BY_CLOJURE_BIN:-}"
runner_command="${BY_CLOJURE_RUNNER:-}"
fixture_home=""
keep_home=0

usage() {
  cat <<'USAGE'
Usage: capture-clojure-cli-snapshots [--bin PATH | --runner COMMAND] [--out DIR] [--home DIR] [--keep-home]

Captures read-only Clojure `by` CLI stdout/stderr/exit-code snapshots for the
Rust native port parity harness. The script uses a temporary HOME unless --home
is provided, avoiding accidental writes to the developer's real ~/.brainyard.

When a native/JVM `by` binary has not been built yet, the script falls back to
the project-local Clojure CLI runner automatically. You can still pass a custom
--runner command such as "bb tui" when needed.
USAGE
}

abs_path() {
  python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$1"
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --bin)
      [[ "$#" -ge 2 ]] || { echo "--bin requires a path" >&2; exit 2; }
      bin_path="$2"
      shift 2
      ;;
    --runner)
      [[ "$#" -ge 2 ]] || { echo "--runner requires a command" >&2; exit 2; }
      runner_command="$2"
      shift 2
      ;;
    --out)
      [[ "$#" -ge 2 ]] || { echo "--out requires a directory" >&2; exit 2; }
      out_dir="$2"
      shift 2
      ;;
    --home)
      [[ "$#" -ge 2 ]] || { echo "--home requires a directory" >&2; exit 2; }
      fixture_home="$2"
      keep_home=1
      shift 2
      ;;
    --keep-home)
      keep_home=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

out_dir="$(abs_path "$out_dir")"
if [[ -n "$fixture_home" ]]; then
  fixture_home="$(abs_path "$fixture_home")"
fi
if [[ -n "$bin_path" ]]; then
  bin_path="$(abs_path "$bin_path")"
fi

if [[ -n "$bin_path" && -n "$runner_command" ]]; then
  echo "--bin and --runner are mutually exclusive" >&2
  exit 2
fi

if [[ -z "$bin_path" ]]; then
  if [[ -n "$runner_command" ]]; then
    :
  elif [[ -x "$repo_root/projects/agent-tui-app/target/by" ]]; then
    bin_path="$repo_root/projects/agent-tui-app/target/by"
  elif command -v clojure >/dev/null 2>&1; then
    runner_command="set -a && [ -f .env ] && source .env || true; cd projects/agent-tui-app && exec clojure -J-Duser.home=\$HOME -J--enable-native-access=ALL-UNNAMED -M -m ai.brainyard.agent-tui-app.main"
  elif command -v by >/dev/null 2>&1; then
    bin_path="$(command -v by)"
  elif [[ -f "$repo_root/bb.edn" ]] && command -v bb >/dev/null 2>&1; then
    runner_command="bb tui"
  else
    echo "Could not find Clojure by binary or runner. Pass --bin PATH, --runner COMMAND, or set BY_CLOJURE_BIN/BY_CLOJURE_RUNNER." >&2
    exit 2
  fi
fi

if [[ -n "$bin_path" && ! -x "$bin_path" ]]; then
  echo "Clojure by binary is not executable: $bin_path" >&2
  exit 2
fi

if [[ -z "$fixture_home" ]]; then
  fixture_home="$(mktemp -d "${TMPDIR:-/tmp}/by-clojure-cli-home.XXXXXX")"
  if [[ "$keep_home" -eq 0 ]]; then
    trap 'rm -rf "$fixture_home"' EXIT
  fi
else
  mkdir -p "$fixture_home"
fi

mkdir -p "$out_dir"
project_dir="$fixture_home/project"
fixture_bin="$fixture_home/bin"
mkdir -p "$project_dir"
mkdir -p "$fixture_bin"
cat >"$fixture_bin/tmux" <<'TMUX'
#!/bin/sh
exit 0
TMUX
chmod +x "$fixture_bin/tmux"

write_select_resume_fixture() {
  local root="$fixture_home/.brainyard/sessions"
  rm -rf "$root"
  mkdir -p "$root/older" "$root/newer"
  cat >"$root/older/meta.edn" <<'EDN'
{:id "older"
 :label "Old"
 :defagent-id :coact-agent
 :started-at 1000
 :last-attached-at 2000}
EDN
  cat >"$root/newer/meta.edn" <<'EDN'
{:id "newer"
 :label "New"
 :agent-id :main-agent
 :started-at 1000
 :last-attached-at 3000}
EDN
}

clear_select_resume_fixture() {
  rm -rf "$fixture_home/.brainyard/sessions"
}

declare -a cases=(
  "top_help|--help"
  "top_help_short|-h"
  "top_help_question|-?"
  "top_version_long|--version"
  "top_version_short|-V"
  "run_help|run --help"
  "run_short_h|run -h"
  "run_resume_missing|run --resume missing"
  "run_with_tmux_need_session|run --with-tmux"
  "run_select_resume_with_tmux_need_session|run --select-resume --with-tmux"
  "ask_help|ask --help"
  "ask_short_h|ask -h"
  "ask_missing_question|ask"
  "ask_missing_question_bedrock|ask --provider bedrock --model amazon.nova-lite-v1:0"
  "ask_missing_question_bedrock_short|ask -p bedrock -m amazon.nova-lite-v1:0"
  "agents_help|agents --help"
  "agents_short_h|agents -h"
  "agents|agents"
  "models_help|models --help"
  "models_short_h|models -h"
  "models|models"
  "models_bedrock|models --provider bedrock"
  "models_bedrock_short|models -p bedrock"
  "models_claude_code|models --provider claude-code"
  "models_unknown_provider|models --provider bogus"
  "config_help|config --help"
  "config_short_h|config -h"
  "sessions_help|sessions --help"
  "sessions_short_h|sessions -h"
  "sessions_list_help|sessions list --help"
  "sessions_list_short_h|sessions list -h"
  "sessions_list|sessions list"
  "sessions_prune_help|sessions prune --help"
  "sessions_prune_short_h|sessions prune -h"
  "sessions_prune_missing|sessions prune -s missing"
  "sessions_prune_missing_positional|sessions prune missing"
)

json_escape() {
  python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'
}


snapshot_timeout_seconds="${BY_CLI_SNAPSHOT_TIMEOUT_SECONDS:-60}"

terminate_tree() {
  local pid="$1"
  local signal="${2:-TERM}"
  local child=""

  if command -v pgrep >/dev/null 2>&1; then
    while IFS= read -r child; do
      [[ -n "$child" ]] || continue
      terminate_tree "$child" "$signal"
    done < <(pgrep -P "$pid" 2>/dev/null || true)
  fi

  kill -"$signal" "$pid" 2>/dev/null || true
}

capture_case() {
  local name="$1"
  shift
  local stdout_file="$out_dir/$name.stdout.txt"
  local stderr_file="$out_dir/$name.stderr.txt"
  local status_file="$out_dir/$name.exitcode"
  local status=0
  local command=""
  local env_prefix=""
  local stdin_file="/dev/null"
  local arg escaped

  printf -v env_prefix 'export PATH=%q:"$PATH"; unset TMUX; ' "$fixture_bin"

  if [[ "$name" == "run_select_resume_with_tmux_need_session" ]]; then
    write_select_resume_fixture
    stdin_file="$fixture_home/$name.stdin"
    printf 'N\n' >"$stdin_file"
  fi

  local timeout_file="$fixture_home/$name.timeout"
  local command_pid=""
  local timeout_pid=""
  rm -f "$timeout_file"

  if [[ -n "$runner_command" ]]; then
    command="$runner_command"
    for arg in "$@"; do
      printf -v escaped '%q' "$arg"
      command+=" $escaped"
    done
    (
      cd "$repo_root"
      HOME="$fixture_home" \
      BY_NO_DOTENV=1 \
      BY_RS_ORACLE_USER_HOME="$fixture_home" \
      PATH="$fixture_bin:$PATH" \
      TMUX= \
      BRAINYARD_PROJECT_DIR="$project_dir" \
      NO_COLOR=1 \
        bash -lc "$env_prefix$command" <"$stdin_file"
    ) >"$stdout_file" 2>"$stderr_file" &
  else
    (
      cd "$repo_root"
      HOME="$fixture_home" \
      BY_NO_DOTENV=1 \
      BY_RS_ORACLE_USER_HOME="$fixture_home" \
      PATH="$fixture_bin:$PATH" \
      TMUX= \
      BRAINYARD_PROJECT_DIR="$project_dir" \
      NO_COLOR=1 \
        "$bin_path" "$@" <"$stdin_file"
    ) >"$stdout_file" 2>"$stderr_file" &
  fi

  command_pid="$!"
  (
    elapsed=0
    while [[ "$elapsed" -lt "$snapshot_timeout_seconds" ]]; do
      sleep 1
      if ! kill -0 "$command_pid" 2>/dev/null; then
        exit 0
      fi
      elapsed=$((elapsed + 1))
    done
    if kill -0 "$command_pid" 2>/dev/null; then
      printf 'snapshot command timed out after %s seconds\n' "$snapshot_timeout_seconds" >"$timeout_file"
      terminate_tree "$command_pid" TERM
      sleep 1
      terminate_tree "$command_pid" KILL
    fi
  ) &
  timeout_pid="$!"

  wait "$command_pid" || status=$?
  kill "$timeout_pid" 2>/dev/null || true
  wait "$timeout_pid" 2>/dev/null || true
  if [[ -f "$timeout_file" ]]; then
    cat "$timeout_file" >>"$stderr_file"
    rm -f "$timeout_file"
    status=124
  fi
  if [[ "$name" == "run_select_resume_with_tmux_need_session" ]]; then
    clear_select_resume_fixture
    rm -f "$stdin_file"
  fi
  printf '%s\n' "$status" >"$status_file"
}

for case_spec in "${cases[@]}"; do
  name="${case_spec%%|*}"
  command_string="${case_spec#*|}"
  # shellcheck disable=SC2206
  args=( $command_string )
  capture_case "$name" "${args[@]}"
done

{
  printf '{\n'
  printf '  "schemaVersion": 2,\n'
  printf '  "source": "clojure",\n'
  if [[ -n "$bin_path" ]]; then
    printf '  "binary": %s,\n' "$(printf '%s' "$bin_path" | json_escape)"
  else
    printf '  "binary": null,\n'
  fi
  if [[ -n "$runner_command" ]]; then
    printf '  "runner": %s,\n' "$(printf '%s' "$runner_command" | json_escape)"
  else
    printf '  "runner": null,\n'
  fi
  printf '  "workingDirectory": %s,\n' "$(printf '%s' "$repo_root" | json_escape)"
  printf '  "isolatedHome": %s,\n' "$(printf '%s' "$fixture_home" | json_escape)"
  printf '  "cases": [\n'
  for i in "${!cases[@]}"; do
    name="${cases[$i]%%|*}"
    command_string="${cases[$i]#*|}"
    comma=','
    [[ "$i" -eq "$((${#cases[@]} - 1))" ]] && comma=''
    printf '    {"name": %s, "command": %s, "stdout": %s, "stderr": %s, "exitCode": %s}%s\n' \
      "$(printf '%s' "$name" | json_escape)" \
      "$(printf '%s' "$command_string" | json_escape)" \
      "$(printf '%s' "$name.stdout.txt" | json_escape)" \
      "$(printf '%s' "$name.stderr.txt" | json_escape)" \
      "$(cat "$out_dir/$name.exitcode")" \
      "$comma"
  done
  printf '  ]\n'
  printf '}\n'
} >"$out_dir/manifest.json"

if [[ "$keep_home" -eq 1 ]]; then
  printf 'Kept isolated HOME at %s\n' "$fixture_home"
fi
printf 'Wrote Clojure CLI snapshots to %s\n' "$out_dir"
