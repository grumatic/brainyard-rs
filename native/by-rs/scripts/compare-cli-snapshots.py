#!/usr/bin/env python3
"""Compare Clojure and Rust CLI snapshot directories.

The snapshots intentionally contain separate isolated HOME directories and may
also be captured from different working directories.  The comparator normalizes
those manifest-declared roots before checking output equality so strict mode
reports semantic CLI gaps instead of harness-local path noise.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import pathlib
import re
import sys
from dataclasses import dataclass


@dataclass(frozen=True)
class SnapshotCase:
    name: str
    command: str
    stdout: str
    stderr: str
    exit_code: int


@dataclass(frozen=True)
class SnapshotRoot:
    root: pathlib.Path
    manifest: dict
    cases: dict[str, SnapshotCase]
    replacements: tuple[tuple[str, str], ...]


TOP_VERSION_RE = re.compile(r"^(by)\s+v\S+$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare Clojure and Rust CLI snapshot directories."
    )
    parser.add_argument("--clojure", required=True, type=pathlib.Path)
    parser.add_argument("--rust", required=True, type=pathlib.Path)
    parser.add_argument(
        "--case",
        action="append",
        dest="selected_cases",
        help="Only compare the named snapshot case. Can be passed more than once.",
    )
    parser.add_argument(
        "--ignore-case",
        action="append",
        default=[],
        help="Skip the named snapshot case. Can be passed more than once.",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Exit non-zero when exit-code or combined-output mismatches remain.",
    )
    parser.add_argument(
        "--ignore-version",
        action="store_true",
        help="Normalize by --version output before comparing snapshots.",
    )
    parser.add_argument("--diff", action="store_true", help="Print unified diffs for gaps.")
    parser.add_argument("--diff-lines", type=int, default=120)
    return parser.parse_args()


def load_manifest(root: pathlib.Path) -> dict:
    manifest = root / "manifest.json"
    with manifest.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def manifest_replacements(manifest: dict) -> tuple[tuple[str, str], ...]:
    replacements: list[tuple[str, str]] = []
    for key, token in (
        ("isolatedHome", "<isolated-home>"),
        ("workingDirectory", "<working-directory>"),
    ):
        value = str(manifest.get(key) or "")
        if not value:
            continue
        candidates = {value, os.path.abspath(value), os.path.normpath(value)}
        try:
            candidates.add(str(pathlib.Path(value).resolve()))
        except OSError:
            pass
        for candidate in candidates:
            if candidate:
                replacements.append((candidate, token))

    replacements.sort(key=lambda pair: len(pair[0]), reverse=True)
    return tuple(replacements)


def load_snapshot_root(root: pathlib.Path) -> SnapshotRoot:
    manifest = load_manifest(root)
    cases: dict[str, SnapshotCase] = {}
    for raw in manifest.get("cases", []):
        case = SnapshotCase(
            name=str(raw["name"]),
            command=str(raw.get("command", "")),
            stdout=str(raw["stdout"]),
            stderr=str(raw["stderr"]),
            exit_code=int(raw["exitCode"]),
        )
        cases[case.name] = case
    return SnapshotRoot(
        root=root,
        manifest=manifest,
        cases=cases,
        replacements=manifest_replacements(manifest),
    )


def read_text(snapshot: SnapshotRoot, relative: str) -> str:
    path = snapshot.root / relative
    if not path.exists():
        return ""
    return path.read_text(encoding="utf-8", errors="replace")


def scrub_version_lines(lines: list[str]) -> list[str]:
    scrubbed: list[str] = []
    scrub_next_payload = False
    for line in lines:
        stripped = line.strip()
        if scrub_next_payload and stripped:
            indent = line[: len(line) - len(line.lstrip())]
            scrubbed.append(f"{indent}<version>")
            scrub_next_payload = False
            continue

        match = TOP_VERSION_RE.match(line)
        if match:
            scrubbed.append(f"{match.group(1)} <version>")
        else:
            scrubbed.append(line)

        scrub_next_payload = stripped == "VERSION:"
    return scrubbed


def normalize(
    text: str,
    *,
    ignore_version: bool = False,
    replacements: tuple[tuple[str, str], ...] = (),
) -> str:
    for old, new in replacements:
        text = text.replace(old, new)
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    if ignore_version:
        lines = scrub_version_lines(lines)
    while lines and lines[-1] == "":
        lines.pop()
    return "\n".join(lines)


def combined(snapshot: SnapshotRoot, case: SnapshotCase, *, ignore_version: bool = False) -> str:
    return normalize(
        read_text(snapshot, case.stdout) + read_text(snapshot, case.stderr),
        ignore_version=ignore_version,
        replacements=snapshot.replacements,
    )


def stream_exact(
    left_snapshot: SnapshotRoot,
    left: SnapshotCase,
    right_snapshot: SnapshotRoot,
    right: SnapshotCase,
    stream: str,
    *,
    ignore_version: bool = False,
) -> bool:
    left_file = left.stdout if stream == "stdout" else left.stderr
    right_file = right.stdout if stream == "stdout" else right.stderr
    return normalize(
        read_text(left_snapshot, left_file),
        ignore_version=ignore_version,
        replacements=left_snapshot.replacements,
    ) == normalize(
        read_text(right_snapshot, right_file),
        ignore_version=ignore_version,
        replacements=right_snapshot.replacements,
    )


def yes_no(value: bool) -> str:
    return "ok" if value else "diff"


def main() -> int:
    args = parse_args()
    clojure = load_snapshot_root(args.clojure)
    rust = load_snapshot_root(args.rust)

    if args.selected_cases:
        names = list(dict.fromkeys(args.selected_cases))
    else:
        names = sorted(set(clojure.cases) | set(rust.cases))
    ignored = set(args.ignore_case or [])

    failures = 0
    compared = 0
    print(f"{'case':<18} {'exit':<6} {'combined':<9} {'stdout':<7} {'stderr':<7} rust-command")
    print("-" * 88)
    for name in names:
        if name in ignored:
            continue

        clj = clojure.cases.get(name)
        rs = rust.cases.get(name)
        if clj is None or rs is None:
            failures += 1
            compared += 1
            side = "missing-clojure" if clj is None else "missing-rust"
            rust_command = rs.command if rs is not None else ""
            print(f"{name:<18} {side:<6} {'diff':<9} {'diff':<7} {'diff':<7} {rust_command}")
            continue

        compared += 1
        exit_ok = clj.exit_code == rs.exit_code
        combined_ok = combined(
            clojure, clj, ignore_version=args.ignore_version
        ) == combined(rust, rs, ignore_version=args.ignore_version)
        stdout_ok = stream_exact(
            clojure,
            clj,
            rust,
            rs,
            "stdout",
            ignore_version=args.ignore_version,
        )
        stderr_ok = stream_exact(
            clojure,
            clj,
            rust,
            rs,
            "stderr",
            ignore_version=args.ignore_version,
        )

        if not exit_ok or not combined_ok:
            failures += 1
        print(
            f"{name:<18} {yes_no(exit_ok):<6} {yes_no(combined_ok):<9} "
            f"{yes_no(stdout_ok):<7} {yes_no(stderr_ok):<7} {rs.command}"
        )

        if args.diff and not combined_ok:
            left = combined(clojure, clj, ignore_version=args.ignore_version).splitlines()
            right = combined(rust, rs, ignore_version=args.ignore_version).splitlines()
            diff = list(
                difflib.unified_diff(
                    left,
                    right,
                    fromfile=f"clojure/{name}",
                    tofile=f"rust/{name}",
                    lineterm="",
                )
            )
            for line in diff[: args.diff_lines]:
                print(line)
            if len(diff) > args.diff_lines:
                print(f"... truncated {len(diff) - args.diff_lines} diff lines ...")

    print("-" * 88)
    print(f"compared={compared} gaps={failures} strict={args.strict}")
    return 1 if args.strict and failures else 0


if __name__ == "__main__":
    sys.exit(main())
