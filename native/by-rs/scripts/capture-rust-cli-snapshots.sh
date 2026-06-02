#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
native_root="$repo_root/native/by-rs"
out_dir="$native_root/fixtures/cli-snapshots/rust"
bin_path="${BY_RUST_BIN:-}"
runner_command="${BY_RUST_RUNNER:-}"
fixture_home=""
keep_home=0
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"

usage() {
  cat <<'USAGE'
Usage: capture-rust-cli-snapshots [--bin PATH | --runner COMMAND] [--out DIR] [--home DIR] [--keep-home]

Captures read-only Rust `by-rs` CLI stdout/stderr/exit-code snapshots for the
native port parity harness. The script uses a temporary HOME unless --home is
provided, avoiding accidental writes to the developer's real ~/.brainyard.

When `target/debug/by-rs` has not been built yet, the script falls back to
`cargo run -q -p by-cli --bin by-rs --` if Cargo is available.
USAGE
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

if [[ -n "$bin_path" && -n "$runner_command" ]]; then
  echo "--bin and --runner are mutually exclusive" >&2
  exit 2
fi

if [[ -z "$bin_path" ]]; then
  if [[ -n "$runner_command" ]]; then
    :
  elif [[ -x "$native_root/target/debug/by-rs" ]]; then
    bin_path="$native_root/target/debug/by-rs"
  elif command -v cargo >/dev/null 2>&1; then
    runner_command="cargo run -q -p by-cli --bin by-rs --"
  else
    echo "Could not find Rust by-rs binary. Pass --bin PATH, --runner COMMAND, or set BY_RUST_BIN/BY_RUST_RUNNER." >&2
    exit 2
  fi
fi

if [[ -n "$bin_path" && ! -x "$bin_path" ]]; then
  echo "Rust by-rs binary is not executable: $bin_path" >&2
  exit 2
fi

if [[ -z "$fixture_home" ]]; then
  fixture_home="$(mktemp -d "${TMPDIR:-/tmp}/by-rust-cli-home.XXXXXX")"
  if [[ "$keep_home" -eq 0 ]]; then
    trap 'rm -rf "$fixture_home"' EXIT
  fi
else
  mkdir -p "$fixture_home"
fi

mkdir -p "$out_dir"
project_dir="$fixture_home/project"
mkdir -p "$project_dir"

declare -a cases=(
  "top_help|--help"
  "run_help|run --help"
  "ask_help|ask --help"
  "agents_help|agents --help"
  "agents|agents"
  "models_help|models --help"
  "models|models"
  "models_bedrock|models --provider bedrock"
  "models_claude_code|models --provider claude-code"
  "config_help|config --help"
  "sessions_help|sessions --help"
  "sessions_list_help|sessions list --help"
  "sessions_list|sessions list"
  "sessions_prune_help|sessions prune --help"
)

json_escape() {
  python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'
}

capture_case() {
  local name="$1"
  shift
  local stdout_file="$out_dir/$name.stdout.txt"
  local stderr_file="$out_dir/$name.stderr.txt"
  local status_file="$out_dir/$name.exitcode"
  local status=0
  local command=""
  local arg escaped

  if [[ -n "$runner_command" ]]; then
    command="$runner_command"
    for arg in "$@"; do
      printf -v escaped '%q' "$arg"
      command+=" $escaped"
    done
    (
      cd "$native_root"
      HOME="$fixture_home" \
      CARGO_HOME="$cargo_home" \
      RUSTUP_HOME="$rustup_home" \
      BRAINYARD_PROJECT_DIR="$project_dir" \
      NO_COLOR=1 \
        bash -lc "$command"
    ) >"$stdout_file" 2>"$stderr_file" || status=$?
  else
    (
      cd "$native_root"
      HOME="$fixture_home" \
      CARGO_HOME="$cargo_home" \
      RUSTUP_HOME="$rustup_home" \
      BRAINYARD_PROJECT_DIR="$project_dir" \
      NO_COLOR=1 \
        "$bin_path" "$@"
    ) >"$stdout_file" 2>"$stderr_file" || status=$?
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
  printf '  "schemaVersion": 1,\n'
  printf '  "source": "rust",\n'
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
  printf '  "workingDirectory": %s,\n' "$(printf '%s' "$native_root" | json_escape)"
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
printf 'Wrote Rust CLI snapshots to %s\n' "$out_dir"
