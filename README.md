# synaps-cards

Agent-drawn cards for Synaps: a Synaps extension (the `ui` tool, which the model sees as `cards_ui`) plus the card spec
synaps-desk renders. No core changes. The design is [`docs/design.md`](docs/design.md).

## What's here

| Path | What |
|---|---|
| `crates/cards-spec` | The spec both sides run (S1): the doc schema (`schema.json`), `parse_input` and `revalidate`, the digest and the result's envelope line, `parse_answer`, and the answer message, built and parsed. No I/O; serde, serde_json and sha2 only. |
| `crates/cards-ext` | The extension (S2): the process Synaps starts, speaking the extension protocol on stdio. It registers `ui` and answers each call at once with the card's envelope and result text, or a refusal. Needs only `tools.register`. |
| `.synaps-plugin/plugin.json`, `scripts/setup.sh` | The plugin: this repo is its root. Setup builds `target/release/cards-ext`. |
| `fixtures/` | The corpus the spec is tested against ([`fixtures/README.md`](fixtures/README.md)). |
| `scripts/oracle.py` | A second implementation of the digest, envelope, message and the extension's result text, in Python: it writes the corpus's goldens (`--check` verifies them). |
| `scripts/gen-casefold.py` | Generates the case-folding table, pinned to Unicode 16.0.0, and `fixtures/casefold.json`. |

## Installing the extension

Into a running daemon (design §1.1); it needs a Rust toolchain:

```
git clone https://github.com/HaseebKhalid1507/synaps-cards ~/.synaps-cli/plugins/cards
(cd ~/.synaps-cli/plugins/cards && bash scripts/setup.sh)
synaps daemon reload-extension cards
```

The model then sees `cards_ui`. With progressive tool disclosure on, extension tools stay deferred unless
`tools.always_loaded` lists `ext.cards:ui`. A card shows in synaps-desk; anywhere else the agent's question and its
numbered options are a plain tool row, and the user answers in text.

## Building

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p cards-spec --locked --features preserve_order   # serde_json maps in insertion order, as in synaps-desk
python3 scripts/oracle.py --check
python3 scripts/smoke.py --synaps "$(command -v synaps)"   # the built extension in a real Synaps, in a throwaway home
```

## License

MIT (see `LICENSE`). Parts are ported from SynapsCLI under the Apache License 2.0, and the case-folding table is generated from Unicode data under the Unicode License v3 (`LICENSE-UNICODE`); see `NOTICE`.
