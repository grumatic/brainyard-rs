# CLI Snapshot Fixtures

This directory is the default output target for Clojure `by` CLI snapshots used
by the Rust port parity harness. Generate snapshots with:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh \
  --bin projects/agent-tui-app/target/by
```

If the native/JVM `by` binary has not been built, capture through the Clojure
Babashka task instead:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh --runner "bb tui"
```

When no binary is found and `bb` is available, the script automatically falls
back to the `bb tui` runner.

The capture script uses an isolated temporary `HOME` by default so read-only
snapshots do not touch the developer's real `~/.brainyard` state.

For local parity checks, keep generated snapshots under `target/` and compare
the Clojure and Rust outputs:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh \
  --runner "bb tui" \
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
