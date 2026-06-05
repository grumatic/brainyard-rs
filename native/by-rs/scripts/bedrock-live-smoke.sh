#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
native_root="$repo_root/native/by-rs"
bin_path="${BY_RUST_BIN:-${BY_RS_BIN:-}}"
runner_command="${BY_RUST_RUNNER:-${BY_RS_RUNNER:-}}"
smoke_home="${BY_RS_BEDROCK_HOME:-}"
keep_home=0
mode="dry-run"
model="${BY_RS_BEDROCK_MODEL:-global.anthropic.claude-haiku-4-5-20251001-v1:0}"
region="${BY_RS_BEDROCK_REGION:-}"
profile="${BY_RS_BEDROCK_PROFILE:-}"
max_tokens="${BY_RS_BEDROCK_MAX_TOKENS:-32}"
question="${BY_RS_BEDROCK_QUESTION:-What is 2+2? Answer in one short sentence.}"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
host_home="${HOME:-}"

usage() {
  cat <<'USAGE'
Usage: bedrock-live-smoke.sh [--dry-run | --live] [--bin PATH | --runner COMMAND]
                             [--home DIR] [--keep-home] [--model MODEL]
                             [--region REGION] [--profile PROFILE]
                             [--max-tokens N] [--question TEXT]

Runs the Rust `by-rs ask -p bedrock` smoke path with an isolated HOME. The
script defaults to --dry-run so it never touches the network unless --live is
passed or BY_RS_BEDROCK_LIVE=1 is set.

Environment knobs:
  BY_RS_BEDROCK_LIVE=1          Opt into the live Bedrock Converse call.
  BY_RS_BEDROCK_MODEL=MODEL     Default: global.anthropic.claude-haiku-4-5-20251001-v1:0
  BY_RS_BEDROCK_REGION=REGION   Optional; otherwise by-rs resolves AWS env/defaults.
  BY_RS_BEDROCK_PROFILE=PROFILE Optional; otherwise by-rs resolves AWS env/defaults.
  BY_RS_BEDROCK_MAX_TOKENS=N    Default: 32
  BY_RS_BEDROCK_QUESTION=TEXT   Default: a short 2+2 prompt.
  BY_RS_BEDROCK_HOME=DIR        Reuse a specific isolated HOME.
  BY_RUST_BIN / BY_RS_BIN       Path to an existing by-rs binary.
  BY_RUST_RUNNER / BY_RS_RUNNER Runner command, e.g. "cargo run -q -p by-cli --bin by-rs --".
USAGE
}

truthy() {
  case "${1:-}" in
    1|true|TRUE|yes|YES|y|Y|on|ON) return 0 ;;
    *) return 1 ;;
  esac
}

configure_aws_env_for_isolated_home() {
  if [[ -z "${AWS_CONFIG_FILE:-}" && -n "$host_home" && -f "$host_home/.aws/config" ]]; then
    export AWS_CONFIG_FILE="$host_home/.aws/config"
  fi

  if [[ -z "${AWS_SHARED_CREDENTIALS_FILE:-}" && -n "$host_home" && -f "$host_home/.aws/credentials" ]]; then
    export AWS_SHARED_CREDENTIALS_FILE="$host_home/.aws/credentials"
  fi
}

abs_path() {
  python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$1"
}

if truthy "${BY_RS_BEDROCK_LIVE:-}"; then
  mode="live"
fi

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --dry-run)
      mode="dry-run"
      shift
      ;;
    --live)
      mode="live"
      shift
      ;;
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
    --home)
      [[ "$#" -ge 2 ]] || { echo "--home requires a directory" >&2; exit 2; }
      smoke_home="$2"
      keep_home=1
      shift 2
      ;;
    --keep-home)
      keep_home=1
      shift
      ;;
    --model)
      [[ "$#" -ge 2 ]] || { echo "--model requires a value" >&2; exit 2; }
      model="$2"
      shift 2
      ;;
    --region)
      [[ "$#" -ge 2 ]] || { echo "--region requires a value" >&2; exit 2; }
      region="$2"
      shift 2
      ;;
    --profile|--aws-profile)
      [[ "$#" -ge 2 ]] || { echo "$1 requires a value" >&2; exit 2; }
      profile="$2"
      shift 2
      ;;
    --max-tokens)
      [[ "$#" -ge 2 ]] || { echo "--max-tokens requires a value" >&2; exit 2; }
      max_tokens="$2"
      shift 2
      ;;
    --question)
      [[ "$#" -ge 2 ]] || { echo "--question requires text" >&2; exit 2; }
      question="$2"
      shift 2
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

if [[ -n "$bin_path" ]]; then
  bin_path="$(abs_path "$bin_path")"
  if [[ ! -x "$bin_path" ]]; then
    echo "Rust by-rs binary is not executable: $bin_path" >&2
    exit 2
  fi
fi

if [[ -z "$smoke_home" ]]; then
  smoke_home="$(mktemp -d "${TMPDIR:-/tmp}/by-rs-bedrock-home.XXXXXX")"
  if [[ "$keep_home" -eq 0 ]]; then
    trap 'rm -rf "$smoke_home"' EXIT
  fi
else
  smoke_home="$(abs_path "$smoke_home")"
  mkdir -p "$smoke_home"
fi

# bedrock-live-smoke is an internal parity/smoke harness. It intentionally
# opts into by-rs ask projection-only flags such as --max-tokens, --region,
# --aws-profile, and --dry-run, which are hidden from the production CLI.
export BY_RS_ALLOW_ASK_TEST_OPTIONS=1

args=("ask" "-p" "bedrock" "-m" "$model" "--max-tokens" "$max_tokens" "--no-prompt-cache")
if [[ -n "$region" ]]; then
  args+=("--region" "$region")
fi
if [[ -n "$profile" ]]; then
  args+=("--aws-profile" "$profile")
fi
case "$mode" in
  live) ;;
  dry-run) args+=("--dry-run") ;;
  *) echo "Unknown smoke mode: $mode" >&2; exit 2 ;;
esac
args+=("$question")

if [[ "$mode" == "live" ]]; then
  echo "Running live Bedrock smoke via by-rs ask (isolated HOME: $smoke_home)" >&2
else
  echo "Running Bedrock dry-run smoke only; pass --live or set BY_RS_BEDROCK_LIVE=1 for network." >&2
fi

if [[ -n "$runner_command" ]]; then
  command="$runner_command"
  for arg in "${args[@]}"; do
    printf -v escaped '%q' "$arg"
    command+=" $escaped"
  done
  (
    cd "$native_root"
    configure_aws_env_for_isolated_home
    HOME="$smoke_home" \
    CARGO_HOME="$cargo_home" \
    RUSTUP_HOME="$rustup_home" \
    NO_COLOR=1 \
      bash -lc "$command"
  )
else
  (
    cd "$native_root"
    configure_aws_env_for_isolated_home
    HOME="$smoke_home" \
    CARGO_HOME="$cargo_home" \
    RUSTUP_HOME="$rustup_home" \
    NO_COLOR=1 \
      "$bin_path" "${args[@]}"
  )
fi
