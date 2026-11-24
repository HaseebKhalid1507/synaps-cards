#!/usr/bin/env bash
# Build the cards extension (design §1.1). Synaps runs this as `bash scripts/setup.sh` from the
# plugin dir (manifest `extension.setup`) after a /plugins install or update; after a git clone
# into ~/.synaps-cli/plugins/cards, run it yourself, then `synaps daemon reload-extension cards`.
# Afterwards the manifest's `extension.command`, target/release/cards-ext, must exist.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if ! command -v cargo >/dev/null 2>&1; then
  echo "cards: cargo not found on PATH. Install the Rust toolchain (https://rustup.rs) and run scripts/setup.sh again." >&2
  exit 1
fi
if [ ! -f Cargo.lock ]; then
  echo "cards: Cargo.lock is missing; refusing an unlocked build." >&2
  exit 1
fi
# Pin the target dir: a user-level build.target-dir or CARGO_TARGET_DIR would otherwise put the
# binary somewhere the manifest's relative command can't find it.
cargo build --release --locked --target-dir target -p cards-ext
test -x target/release/cards-ext
echo "cards: built target/release/cards-ext"
