# The blocks corpus (`blocks/1`)

The contract of `cards_spec::block` (B1, `docs/blocks.md` v1.3), as data.
`crates/cards-spec/tests/blocks.rs` checks every case against the crate's
public API. CI runs it in both of serde_json's map orders (`preserve_order`
on and off), so every byte pinned here is the same in both.

**Who writes what.** `scripts/oracle_blocks.py`, a second implementation in
Python, writes every golden: normalized ops, digests, results, generations,
tags, edit texts, a message's body and notice lines, and all of
`limits.json`. A refusal's text is written by hand. `scripts/oracle.py`
runs both oracles; `--check` (a CI step) fails if any golden is stale.
After changing a case, run `python3 scripts/oracle.py` to fill it.

Every case has an `about`. A case from one of the design's examples has a
`section` too (§1.1–§1.6, §2.1, §3.3, §3.5, §3.6), and a test checks that
every normative section is present (round 2 L2).

## `valid.json` and `invalid.json`

A `cards_block` call's input (`input`, `{"ops":[…]}`), and then either:

- `normalized`: the batch as `parse_call` normalizes it (keys in schema
  order, maps in byte order, defaults filled, empty optional strings absent,
  strings sanitized, numbers canonical), `digest` (the first 16 hex digits
  of SHA-256 over it, compact) and `result` (`ok N ops (blocks/1 #…)`); or
- `error`: the refusal, `path: message`, without the `cards_block: ` the
  extension puts in front.

Every op and every agent kind has valid and invalid cases. Two valid cases
spell one batch two ways, so they share a digest.

## `limits.json`

Each numeric bound twice: at N, accepted, then N+1, refused, with its
refusal. Bounds on bytes (a call's 8 KiB, each kind's body cap) are hit
exactly: the oracle sizes the strings, with 1- to 4-byte characters. The
bulkiest accepted cases keep their digest only, which still pins their
bytes.

## `tags.json`

A conversation's first message, then the generation (SHA-256 over its
`role` and `content`, keys sorted, `_synaps_*` keys left out, at every
level) and an incarnation tag (`blocks/1`, the generation, the id, the
creation number, the tool id and the op index, NUL-separated; the first 32
hex digits of its SHA-256), each with its preimage.

## `edits.json`

An edit message (§3.3) built from its parts: `id`, `base`, `of`, the
user's `parts`, the title and the summary pieces. `text` is the message;
`design` is the design's own text, where the case is one of its examples.
Each case reads back to its edit.

## `look_alikes.json`

Texts that are, or nearly are, an edit. `edit` is what `message::parse`
reads, or `null`: one text per edit, nothing else counts.

## `notices.json`

A user's text, then what `message::split` reads: `body`, the notice
`lines` (0 to 2, one per kind) and the ids `named_twice` (which apply
neither). Includes the bounds (8 ids, versions 1 to 999,999,999), the
wrappers that must stay whole (`<event>`, a compaction summary, an
extension's saved block) and cut, spaced and look-alike lines.
