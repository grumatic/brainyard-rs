#!/usr/bin/env python3
"""Compare Clojure and Rust CLI snapshot directories.

The Rust port is not expected to be byte-for-byte compatible yet. By default
this script reports parity gaps and exits successfully so it can be used during
exploration. Use --strict to turn exit-code or combined-output mismatches into a
non-zero result.
"""
from __future__ import annotations

import argparse
import difflib
import json
import pathlib
import sys
from dataclasses import dataclass


@dataclass(frozen=True)
class SnapshotCase:
    name: str
    command: str
    stdout: str
    stderr: str
    exit_code: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clojure", required=True, type=pathlib.Path, help="Clojure snapshot directory")
    parser.add_argument("--rust", required=True, type=pathlib.Path, help="Rust snapshot directory")
    parser.add_argument("--strict", action="store_true", help="Exit non-zero when compared cases differ")
    parser.add_argument("--diff", action="store_true", help="Print combined-output unified diffs")
    parser.add_argument("--diff-lines", type=int, default=120, help="Maximum diff lines per case")
    return parser.parse_args()


def load_manifest(root: pathlib.Path) -> dict:
    manifest = root / "manifest.json"
    with manifest.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def load_cases(root: pathlib.Path) -> dict[str, SnapshotCase]:
    manifest = load_manifest(root)
    cases = {}
    for raw in manifest.get("cases", []):
        case = SnapshotCase(
            name=str(raw["name"]),
            command=str(raw.get("command", "")),
            stdout=str(raw["stdout"]),
            stderr=str(raw["stderr"]),
            exit_code=int(raw["exitCode"]),
        )
        cases[case.name] = case
    return cases


def read_text(root: pathlib.Path, relative: str) -> str:
    path = root / relative
    if not path.exists():
        return ""
    return path.read_text(encoding="utf-8", errors="replace")


def normalize(text: str) -> str:
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    while lines and lines[-1] == "":
        lines.pop()
    return "\n".join(lines)


def combined(root: pathlib.Path, case: SnapshotCase) -> str:
    return normalize(read_text(root, case.stdout) + read_text(root, case.stderr))


def stream_exact(root_left: pathlib.Path, left: SnapshotCase, root_right: pathlib.Path, right: SnapshotCase, stream: str) -> bool:
    left_file = left.stdout if stream == "stdout" else left.stderr
    right_file = right.stdout if stream == "stdout" else right.stderr
    return normalize(read_text(root_left, left_file)) == normalize(read_text(root_right, right_file))


def yes_no(value: bool) -> str:
    return "ok" if value else "diff"


def main() -> int:
    args = parse_args()
    clj_cases = load_cases(args.clojure)
    rust_cases = load_cases(args.rust)
    names = sorted(set(clj_cases) | set(rust_cases))

    failures = 0
    compared = 0
    print(f"{'case':<18} {'exit':<6} {'combined':<9} {'stdout':<7} {'stderr':<7} rust-command")
    print("-" * 88)
    for name in names:
        clj = clj_cases.get(name)
        rust = rust_cases.get(name)
        if clj is None or rust is None:
            failures += 1
            side = "missing-clojure" if clj is None else "missing-rust"
            print(f"{name:<18} {side:<6}")
            continue

        compared += 1
        exit_ok = clj.exit_code == rust.exit_code
        combined_ok = combined(args.clojure, clj) == combined(args.rust, rust)
        stdout_ok = stream_exact(args.clojure, clj, args.rust, rust, "stdout")
        stderr_ok = stream_exact(args.clojure, clj, args.rust, rust, "stderr")
        if not exit_ok or not combined_ok:
            failures += 1
        print(
            f"{name:<18} {yes_no(exit_ok):<6} {yes_no(combined_ok):<9} "
            f"{yes_no(stdout_ok):<7} {yes_no(stderr_ok):<7} {rust.command}"
        )

        if args.diff and not combined_ok:
            left = combined(args.clojure, clj).splitlines()
            right = combined(args.rust, rust).splitlines()
            diff = list(difflib.unified_diff(
                left,
                right,
                fromfile=f"clojure/{name}",
                tofile=f"rust/{name}",
                lineterm="",
            ))
            for line in diff[: args.diff_lines]:
                print(line)
            if len(diff) > args.diff_lines:
                print(f"... truncated {len(diff) - args.diff_lines} diff lines ...")

    print("-" * 88)
    print(f"compared={compared} gaps={failures} strict={args.strict}")
    return 1 if args.strict and failures else 0


if __name__ == "__main__":
    sys.exit(main())
