# CLI Snapshot Fixtures

This directory documents the CLI snapshot parity harness. Generated Clojure and
Rust snapshots default to `native/by-rs/target/cli-snapshots/...` so local
checks do not dirty the Git worktree. Generate Clojure snapshots with:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh \
  --bin projects/agent-tui-app/target/by
```

If the native/JVM `by` binary has not been built, the script automatically
falls back to the project-local Clojure CLI runner. You can still capture
through the Babashka task explicitly:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh --runner "bb tui"
```

The capture script uses an isolated temporary `HOME` by default so read-only
snapshots do not touch the developer's real `~/.brainyard` state.

For local parity checks, capture both Clojure and Rust outputs, then compare
them:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh \
  --out native/by-rs/target/cli-snapshots/clojure \
  --home native/by-rs/target/cli-home/clojure

native/by-rs/scripts/capture-rust-cli-snapshots.sh \
  --out native/by-rs/target/cli-snapshots/rust \
  --home native/by-rs/target/cli-home/rust

native/by-rs/scripts/compare-cli-snapshots.py \
  --clojure native/by-rs/target/cli-snapshots/clojure \
  --rust native/by-rs/target/cli-snapshots/rust \
  --ignore-version
```

`--ignore-version` is useful for local runner comparisons because Clojure and
Rust snapshots can be stamped from different build artifacts.

The comparison script reports current parity gaps without failing by default;
pass `--strict` when a case is ready to become a gated compatibility check.
Use repeated `--case` flags to gate only known-compatible cases while other
commands are still being ported:

```bash
native/by-rs/scripts/compare-cli-snapshots.py \
  --clojure native/by-rs/target/cli-snapshots/clojure \
  --rust native/by-rs/target/cli-snapshots/rust \
  --strict \
  --case agents \
  --case models \
  --case sessions_list
```
