# CLI Snapshot Fixtures

This directory is the default output target for Clojure `by` CLI snapshots used
by the Rust port parity harness. Generate snapshots with:

```bash
native/by-rs/scripts/capture-clojure-cli-snapshots.sh \
  --bin projects/agent-tui-app/target/by
```

The capture script uses an isolated temporary `HOME` by default so read-only
snapshots do not touch the developer's real `~/.brainyard` state.
