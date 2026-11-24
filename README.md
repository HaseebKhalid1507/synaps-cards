# synaps-cards

Agent-drawn cards for Synaps: a Synaps extension (the `ui` tool, which the model sees as `cards_ui`) plus the card spec
synaps-desk renders. No core changes. The design is [`docs/design.md`](docs/design.md).

## What's here

| Path | What |
|---|---|
| `crates/cards-spec` | The spec both sides run (S1): the doc schema (`schema.json`), `parse_input` and `revalidate`, the digest and the result's envelope line, `parse_answer`, and the answer message, built and parsed. No I/O; serde, serde_json and sha2 only. |
| `fixtures/` | The corpus the spec is tested against ([`fixtures/README.md`](fixtures/README.md)). |
| `scripts/oracle.py` | A second implementation of the digest, envelope and message, in Python: it writes the corpus's goldens (`--check` verifies them). |
| `scripts/gen-casefold.py` | Generates the case-folding table, pinned to Unicode 16.0.0, and `fixtures/casefold.json`. |

The extension itself (`crates/cards-ext`, S2) comes next.

## Building

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p cards-spec --locked --features preserve_order   # serde_json maps in insertion order, as in synaps-desk
python3 scripts/oracle.py --check
```

## License

MIT (see `LICENSE`). Parts are ported from SynapsCLI under the Apache License 2.0, and the case-folding table is generated from Unicode data under the Unicode License v3 (`LICENSE-UNICODE`); see `NOTICE`.
