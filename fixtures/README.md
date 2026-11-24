# The card corpus (`cards/1`)

The fixtures `cards-spec` is tested against (`crates/cards-spec/tests/corpus.rs`), and that any other implementation of the spec
(`docs/design.md` §1.4, §1.5, §3.3) can be checked against. Each file stands alone. Invisible, control, mark and private-use
characters are written as `\u` escapes so a reader can see them; they are ordinary JSON strings.

**Where it comes from.** `valid/`, `invalid/`, `raw/` and `sanitize.json` are SynapsCLI's C1 corpus
(`crates/agent-engine/tests/fixtures/ui/` at `6e895d27`), as synaps-desk #75 copied it (`crates/synaps-client/tests/fixtures/ui/`
at `3f61cff`). Every input, normalized doc, refusal and bad answer is unchanged. What v1.1 changed:

- **Card ids** are `c` and 6 of `[a-z0-9]` (`c4f9x2a`), not `s_` and 16 hex: the close fixtures use them, and
  `invalid/close_old_host_id.json` refuses the old form.
- **What only the core had is gone:** the contract digest, answer ids, revisions, the `<ui_answer>` message, the late capsule,
  `shown` and `wait_result`. In their place each card has its `digest` and `envelope` (§1.5), and each answer its `kind` and its
  answer `message` (§3.3).
- Two files whose subject was the capsule keep their cards under new names: `capsule_1024_1025` is
  `detail_multibyte_five_fields`, and `late_answer_capsule` is `form_with_empty_optional`. Answers that were repeats once the
  capsule went were dropped.
- `casefold.json` lists every fold, not only those that differ from a lowercase (see below).

**The goldens** (`digest`, `envelope`, every `message` and `text`) are written by `scripts/oracle.py`, a second implementation of
§1.5 and §3.3 in Python, from each fixture's own `doc`, `wait` and values. `python3 scripts/oracle.py --check` fails if any of them
is stale; CI runs it.

## `valid/*.json`

An input that is accepted, and everything made from it:

| key | meaning |
|---|---|
| `input` | the `cards_ui` input, as the agent sends it |
| `card` | the id the extension minted for it |
| `doc` | the normalized doc: sanitized, defaults materialized, string options as `{label}`, empty optional strings absent |
| `wait`, `mode` | the call's `wait`; `quick` or `form` (§1.4) |
| `digest` | the first 16 hex digits of SHA-256 over the doc's compact JSON, keys in schema order, with `"wait"` last (§1.5) |
| `envelope` | the result's line 1, at most 256 bytes of UTF-8 (only the question is cut, ending in `…`) |
| `answers[]` | `kind` (`answer`, `changed` or `skipped`), `values` (`{}` for a skip), and `message`, the exact answer message |
| `bad_answers[]` | `values` refused, with the exact `error` line |
| `input_bytes`, `doc_bytes` | (`max_size_*`) the input is exactly at its 8 KiB cap and normalizes to a doc of `doc_bytes`, over 8 KiB and within `DOC_MAX_BYTES` (9588) |

Every doc must also revalidate: read back, it is exactly what normalizing makes of it. A close (`a5_close.json`,
`close_with_defaults.json`) has `input`, `close` (the id) and its one-line `envelope` instead.

## `invalid/*.json`

`input` is refused with exactly `error` (`<path>: <one teaching line>`). The `a_*` files are C1's Appendix A invalid list. Every
refusal, here and in `bad_answers`, is one line of at most 322 characters with no control, bidi control or invisible character: a
key the agent or a client wrote appears escaped (`"\u001b[31m"`) and cut to 24 characters.

## `raw/*.json`

JSON text through the shared ingress (`decode_json`), then the model. A repeated key at any level is refused at decoding (never
last-wins), as are lone surrogates, numbers out of `f64` range and nesting past 128.

| key | meaning |
|---|---|
| `what` | `input` (a card input) or `answer` (values for the card `card`) |
| `text` | the raw JSON |
| `json_error` | the decoding refusal's prefix, or `null` when it decodes |
| `doc` / `values` / `error` | what the model makes of the decoded value |

## `messages/`

- `round_trips.json`: answer messages, each built from its case's `input` (normalized to `doc`), `kind` and `values`. The first four
  are §3.3's examples. An implementation must build exactly `text` and read it back as the same reply.
- `look_alikes.json`: texts that are not a card's answer (`parses: null`: ordinary text), or that read as one and then validate
  against `fixture`'s doc to `reply`, or fail with `error`.

## `casefold.json`

The key labels are compared by, for "labels must differ (ignoring case)" and for matching `default` to a label: Unicode full case
folding (`CaseFolding.txt` statuses C and F, Python's `str.casefold`) of the sanitized label, with no other normalization (no NFC).

**It is pinned to one Unicode version, `unicode`: 16.0.0.** `folds` lists every scalar value whose fold is not itself, at that
version; every other scalar value folds to itself. The file is written by Python's `str.casefold` on Unicode 16.0.0, and
`cards-spec` folds from its own table, generated from `CaseFolding-16.0.0.txt` (`scripts/gen-casefold.py`, which refuses to write
either unless the two agree at every scalar value). Neither depends on the Rust toolchain, so the extension and the desk fold alike
whatever built them (SynapsCLI #447). Moving to a newer Unicode changes which cards are valid, so it is a new catalog.

## `sanitize.json`

`ui_sanitize` (§5.2). `\r\n` and a lone `\r` are line endings: they become `\n` before anything else. For each `in`: `single` (a
one-line field: `\n` and `\t` become spaces), `multi` (`detail`, `long` values: `\t` becomes a space), and `refused`: the first
character validation refuses in an agent-written string (`{char, why}`), or `null`. A refused string never reaches sanitizing;
`single` and `multi` still show what display sanitizing does with it.
