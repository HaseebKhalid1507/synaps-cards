# synaps-cards v2: the editable transcript (blocks)

**Author:** Zero · **Status:** design only, no code · **Builds on** `docs/design.md` v1.2, the cards design, which stays in force. **Hard rule, unchanged:** zero changes to SynapsCLI. Everything below is the cards extension plus the desk.

**Why this exists.** Haseeb: "the cards feel underwhelming… treat the entire transcript as something that can be edited / taken control of by the agent. They should also be able to mold the right rail too." He picked two mocks, `interact/widgets/` and `interact/wild2/` (`interact/PICK.md`). This document turns their union into a contract that the shipped stack can actually carry. Where the mocks assumed things the stack can't do, §0.2 says so.

**Read at:**
- SynapsCLI `origin/dev` `0a71ea0d`, cited as `CLI:`. `ae/` is `crates/agent-engine/src/`.
- synaps-desktop `origin/main` `cbae9df`, cited as `DESK:`. `sc/` is `crates/synaps-client/src/` and `sd/` is `crates/synaps-desktop/src/`.
- synaps-cards `origin/main` `335e456`, cited as `SPEC:`. `cs/` is `crates/cards-spec/src/` and `cx/` is `crates/cards-ext/src/`. `design §n` is `docs/design.md`.

---

## TL;DR

- **A sibling tool.** The `cards` plugin gains a second tool, `block`, which the model sees as `cards_block`. Its input is `{"ops":[…]}`: up to 16 ops per call, 8 KiB in all. It validates the batch, returns `ok 3 ops (blocks/1 #<digest>)` at once, and keeps no state. It needs `tools.register` only, as `cards_ui` does.
- **History is the board.** The desk folds every bound `cards_block` call, every card, and every user edit message into one board per conversation (`cards_spec::board`), the way it folds cards today. Reattach, restart and a second desk all rebuild the same board. Compaction empties it, as it empties cards.
- **Blocks.** A block has an id the agent chooses, a kind, and a target: `transcript` (default) or `rail`. The v1 kinds are plan, status, diff, note, metric and file. A question is a card, addressable as `card:<id>`. Items inside a block (plan steps, diff hunks) have ids, and patches address them by id, never by index.
- **Ops:** `put`, `patch`, `replace`, `collapse`, `merge`, `move`, `remove`. Every op in a batch lands in the same frame, so one burst can change the transcript and the rail together.
- **Cheap by construction:**
  - Ops ride the agent's real tool calls in the same message, so they cost no round trip.
  - The desk works out for itself, at zero agent cost, what is running now, which files changed and their diffs, and how tool rows group.
  - The common tick is about 28 output tokens.
  - A typical 8-round turn spends about 450 output tokens on ops, and at most one short extra round (§2).
- **The user's side:**
  - **Content edits go to the agent.** Take a block, edit it, hand it back. The agent gets an ordinary user message: `[block plan edited r4] …`, with a patch on the next line in the agent's own op vocabulary. The patch applies at its place in history, path by path, over whatever the agent did meanwhile.
  - **Presentation stays local.** Hold, collapse, dismiss, pin, unpack and viewing a revision never reach the agent. Dismissed stays dismissed: it is persisted beside `settings.json`.
- **The rail.** Agent sections go in an AGENT zone between the desk's Prompt cache and Files. At most 4 are expanded, they collapse and dismiss, and a "NOW" line the desk derives sits on top (§4).
- **Motion.** Heights depend on a block's structure, never its content. Nothing resizes under a resting pointer. A change glows once (480 ms) and then the window draws 0 frames. Corners are square, the colour is the theme accent, and it works at 320 px (§6).

---

## 0. What carries over, and what the mocks assumed

### 0.1 Reused unchanged from cards v1.2

| Piece | Where | Used for |
|---|---|---|
| The extension never blocks and stores nothing | design §1.2, §4 | `cards_block` returns at once. Holds and dismissals are never its business |
| The result binds the call by a digest of the normalized input | `SPEC:cs/digest.rs:93-145`; design §2.2 | A hook-rewritten batch draws as a plain row, never as ops |
| The fold is pure, over Call, Result and User events | `SPEC:cs/fold.rs:43-53`, `:168-176` | The board wraps the card fold and reads the same stream in one pass |
| The return path is a user message in a two-line grammar | design §3.3; `SPEC:cs/message.rs:78`, `:127` | A block edit is `[block …]` plus JSON, like `[card …]` plus JSON |
| Delivery and the memo | design §3.4–§3.5; `DESK:sd/deliver.rs:1-40`, `:337` | A hand-back takes typed text's paths, and the memo shows it if it's lost (B4 generalizes the memo) |
| `sanitize`, the input gate, and keys that never authorize | `SPEC:cs/sanitize.rs`; `DESK:sd/live.rs:179`; design §5.1 | Every agent string is sanitized. Only an input-holder edits. No block key is an authorize key |
| Workers are refused, and the rate limiter | `SPEC:cx/tool.rs:31-37`, `:104-118` | The same `chat_of` rule, with blocks' own limits |
| Desk-local persistence | `DESK:sd/recall.rs:44-63`, `:97-113` | `blocks.json` uses the same atomic, private, swept writer |

### 0.2 What the mocks assumed that the stack can't do, and what replaces it

| Mock | Assumed | Why not | Here |
|---|---|---|---|
| point: `held: plan is being edited… Queued` result; wild2: "agent writes against a user-owned block fail" | The extension knows about holds | The extension can't reach the desk, and the desk can't reach it. Its only inbound methods are `memory.*` and `config.*` (design §1.2) | Holds are desk-local drafts. Agent ops still apply in the fold; the holding desk defers drawing them, and the hand-back patch wins path by path (§3.4) |
| point: "the lock… published as a `hold` line in history" | A history line that every desk sees | A user message `Submit`s, which wakes an idle agent, or `Steer`s, which the model reads and pays for. A steer can also take the one `queued_message` slot from the user's own steer (design §3.4 d3) | No cross-desk lock. The history orders the hand-backs (§5.4) |
| wild2: `if_rev` and `owner` on agent ops | The agent knows revisions | A stateless extension can't tell it the current revision, and the agent learns of user changes only from edit messages | Dropped. Item ids and path-level application make stale writes harmless (§1.4) |
| widgets: "parked ops are applied in order on hand-back" | Ops wait off the record | Ops are in history when they're made, and every desk folds them | They apply at their history position. Only the holding desk's *drawing* of them waits (§3.4) |
| widgets #4: a range card with samples | A new card kind | That would be a cards catalog change, cards/2 | Later. v1's question kind is today's card (§1.5) |

**Borrowed from the unpicked mocks, as the pick allows:**
- from point: item ids (§1.4), the "N blocks changed above ↑" pill, block walk keys, the rail cap of 4 sections × 12 lines, the `rN` chip with `[` `]`, and the op log ("WIRE tape");
- from workspace: restore as a new revision, and a tombstone receipt for a dismissed block.

---

## 1. The model

### 1.1 The tool and its gate

- **Registration.** `initialize` declares two tools: today's `ui` and the new `block`, in `capabilities.tools` (`SPEC:cx/tool.rs:60-66`; `CLI:docs/extensions/protocol.md` §initialize). The runtime name is `cards:block` and the API name is `cards_block`.
- **Gate.** A manifest config entry `blocks`, default `false`. The host resolves it into `initialize`'s `params.config`: env `SYNAPS_EXTENSION_CARDS_BLOCKS`, or user config `extension.cards.blocks` (`CLI:docs/extensions/protocol.md:149-197`). With it off, `block` isn't declared at all, so its description costs nothing.
  - It stays off until the desk renders blocks (D5).
  - A TUI-only user leaves it off: the TUI shows ops as JSON panels (§5.5).
- **Permissions.** `tools.register` only. There are no hooks, no conversation access and no interception (`SPEC:.synaps-plugin/plugin.json`).
- **Workers.** `chat_of` refuses a worker's call (`SPEC:cx/tool.rs:104-118`) with `cards_block: only the main chat has blocks`. A worker's transcript is not the user's document.

**Why a sibling tool, not `ops` inside `cards_ui`:**
1. **Compatibility.** `cards_ui`'s schema is closed (`additionalProperties: false`, design §1.4). Adding ops would make it cards/2, and every shipped desk would draw ops calls as plain rows anyway, so nothing would be gained.
2. **Rate limits.** Cards allow 3 per minute (`SPEC:cx/tool.rs:34-35`). Blocks need about 10 times that.
3. **Two decisions, two descriptions.** "Ask the user" and "show your work" are different choices. B5 and S4 tune their descriptions separately, and the gate can turn blocks off while cards stay on.

The fold is still one: the board wraps `cards_spec::fold::Fold` (§1.7).

### 1.2 Blocks, ids, targets, places

- **The block id** is the agent's choice, matching `^[a-z][a-z0-9_-]{0,23}$`. It is unique within its conversation's board. `card:<card id>` names a card (§1.5): the `:` can't appear in a block id, so the two can never collide.
- **`target`** is `"transcript"` (the default) or `"rail"`. It is placement, not identity: `move` changes it, and the block keeps its id, revisions and fields.
- **Its place in the transcript is an anchor**, the history position of the op that last placed it (a `put` that created it, or a `move`):
  - The block draws where that call's tool row would be, after the text that precedes the call in its message.
  - Like a card, it is never inside a tool run. A block anchored mid-run draws right after the run, so the run stays one card (`DESK:sd/turns.rs:328-332`, the `Ask` rule).
  - A later `patch` changes the block in place, wherever it is, and its own call row draws nothing.
- **`at`**, on `put` and `move`:
  - absent: on create, here; on a `put` over an existing block, it stays where it is;
  - `{"after":"<id>"}`: right after that block, in the transcript or in the rail;
  - `"top"`: the transcript's top strip. At most 1 block goes there, and only a note or a status (§6.4).
  - `move` with neither `target` nor `at` means "here": re-anchored at this call. This is the mocks' "the plan moves down beside the summary".

### 1.3 Ops

```jsonc
{"op":"put",     "id":"plan", "kind":"plan", "title":"Close the refresh race", "target":"rail", "at":{"after":"cause"}, "body":{…}, "why":"…"}
{"op":"patch",   "id":"plan", "set":{"steps.fix.note":"token_store.rs:118"}, "append":{"steps":[{…}]},
                              "remove":["steps.bandaid"], "order":{"steps":["fix","test","deploy"]}, "state":{"fix":"done","test":"doing"}}
{"op":"replace", "id":"hyp",  "kind":"note", "title":"Root cause", "body":{…}, "why":"first guess was wrong"}
{"op":"collapse","id":"diff"}  ·  {"op":"collapse","ids":["t1","t2"],"open":true}  ·  {"op":"collapse","run":true,"summary":"Explored relay"}
{"op":"merge",   "into":"research", "title":"Why the cutoff belongs to the session family", "ids":["schema","trace","note"]}
{"op":"move",    "id":"plan", "target":"rail"}  ·  {"op":"move","id":"plan"}            // no target/at: here
{"op":"remove",  "id":"tests", "why":"passed"}
```

| Op | Does | On an id that's absent or removed |
|---|---|---|
| `put` | Creates a block. On a live id it is a `replace` that keeps the block's place unless `at`/`target` is given, which makes a retried put harmless. A put identical to the current body is a no-op, so the revision doesn't move | Creates it. A removed id can be put again, with its revisions continuing. A **dismissed** id stays hidden on that desk (§3.5) |
| `patch` | Changes fields by path (§1.4). `state` is sugar for setting `state` on items of the kind's main list: `{"fix":"done"}` is `set {"steps.fix.state":"done"}` | Ignored, and marked |
| `replace` | A new body, and kind if given, wholesale. The header shows `rN · replaced`, with `why` on hover. The old revision stays one `[` away | Ignored, and marked |
| `collapse` | Folds a block (`open: true` unfolds it), one or several. `run: true` folds **the tool run this call sits in** under `summary`. That's the desk's own run grouping (`DESK:sd/turns.rs:1-5`), titled by the agent: tool rows merge with no tool ids named | Ignored, and marked |
| `merge` | Groups 2–8 blocks into a `group` block `into`, at the first member's place, with the members folded inside. The user can unpack it locally | Missing members are skipped. Fewer than 2 left: ignored |
| `move` | Re-places a block (§1.2): `target`, `at`, or here | Ignored, and marked |
| `remove` | A tombstone: the block leaves the view, and `rN · removed · why` stays in the op log | No-op |

**Batching:**
- A call carries 1 to 16 ops.
- **The extension** validates the whole batch or refuses it with one teaching line: syntax, ids, paths, kinds, bodies and bounds.
- **The fold** applies ops in array order, and each one stands alone. An op that doesn't apply (an unknown id, an item that isn't there, a broken cap) is skipped with its reason, and the rest still land.
- **All ops of a call land in one frame.** That is wild2's "one agent burst changing transcript and rail together".

### 1.4 Paths and item ids (borrowed from point)

- **Items have ids.** A plan's steps and a diff's hunks are lists of items with ids matching `^[a-z0-9][a-z0-9_-]{0,23}$`, unique in their list. `u1`, `u2`, … (`^u[0-9]{1,4}$`) are reserved for items the user adds, and an agent item with such an id is refused.
- **The path grammar** is `field` | `list.item` | `list.item.field`, as `^[a-z_]{1,16}(\.[a-z0-9][a-z0-9_-]{0,23}(\.[a-z_]{1,16})?)?$`. There are no indices.
- **Why ids, not indices.** A user who reorders or strikes steps can't make a later agent patch land on the wrong step. A patch to a step the user cut finds no step, and is skipped and marked.
- **What each part of a patch does:**
  - `set` replaces scalars;
  - `append` adds items at the end of a list, or lines to a hunk (`"hunks.h1.lines"`), or numbers to a metric's `series`;
  - `remove` deletes items;
  - `order` lists item ids in order. Ids it doesn't name keep their relative order after the named ones, so an item the agent added that the user never saw goes last, not lost.
- **Path by path.** A patch applies path by path, in that order. A path that doesn't resolve, or that would break its kind's rule (a 21st step, an unknown state), is skipped and marked. The other paths still apply.

### 1.5 Kinds v1

A block's `title` (≤ 80) is top-level, and the `body` is its kind's. All strings go through `cards_spec::sanitize` (design §5.2). Each kind's draw height depends on its structure only (§6.2).

| Kind | Body (sketch) | Limits | Main list | The user may edit |
|---|---|---|---|---|
| **plan** | `{"steps":[{"id":"fix","label":"Serialize rotate() per session","state":"todo","note":"token_store.rs:118","stop":false}]}` | 1–20 steps; label ≤ 100, note ≤ 80; state `todo·doing·done·failed·skipped`; ≤ 4 KiB | `steps` | order, `skipped`↔`todo` (not done or doing steps), `stop`, label, add `u` steps |
| **status** | `{"text":"Bisecting the cutoff","detail":"3 of 7 commits left","progress":{"n":3,"of":7},"state":"working"}` | text ≤ 80, detail ≤ 120; n ≤ of ≤ 10⁶; state `working·waiting·blocked·done`; ≤ 512 B | — | nothing (it's the agent's voice) |
| **diff** | `{"file":"crates/relay/src/token_store.rs","state":"editing","from":"edits"}`, or a proposal: `{"file":…,"state":"proposed","hunks":[{"id":"h1","at":"rotate()","lines":["- let t = …","+ let _g = slot.lock().await;"],"state":"open"}]}` | file ≤ 200; state `editing·proposed·written·reverted`; 1–6 hunks, ≤ 120 lines in all, a line ≤ 160; hunk state `open·kept·dropped`; ≤ 6 KiB | `hunks` | a proposal's hunks: kept or dropped |
| **note** | `{"text":"Two refreshes read the same token before either rotates it…"}` | ≤ 1500 chars, ≤ 30 lines, markdown-lite as a card's detail (design §5.2); ≤ 2 KiB | — | `text`, `title` |
| **metric** | `{"label":"Upstream rotations per burst","value":"1.0","was":"2.0","unit":"","series":[2,2,1.5,1],"state":"ok"}` | label ≤ 60; value, was and unit ≤ 16 (the agent formats them); series ≤ 32 numbers, and an append rolls off the oldest; state `ok·warn·bad·neutral`; ≤ 1 KiB | — | nothing |
| **file** | `{"path":"crates/relay/src/token_store.rs","line":84,"note":"TokenStore::rotate","state":"editing"}` | path ≤ 200, note ≤ 80; state `reading·editing·written·done`; ≤ 512 B | — | nothing |
| **question** | **A card.** It is made only by `cards_ui` (design §1.4), and named `card:c4f9x2a` in ops | the card's own: 8 KiB | — | answered as a card (design §3) |
| *group* | from `merge` only: `{"members":["schema","trace"]}` | 2–8 members | — | unpack (local) |

**A diff from the edits themselves.** With `"from":"edits"`, the agent sends no lines. The desk draws the hunks of this session's successful `edit`/`write`/`multi_edit` calls on `file` from its own ledger (`DESK:sd/files.rs:1-18`), exactly as the tool rows' inline diffs do. That is "a diff that grows while the agent edits" at the cost of one put. Agent-written `hunks` are for proposals: a change shown but not yet made, which the user may keep or drop (§3.2).

**Questions stay cards.** The asking machinery is already built and reviewed: the memo and delivery reconciliation (D2a), Ctrl+J entry and batches (D2b), presence and notifications (D3), recall (D4). A second asking path would fork all of it.

So "a question that becomes its answer" works like this:
1. `cards_ui` asks.
2. The answer comes back as today.
3. `{"op":"replace","id":"card:c4f9x2a","kind":"note","body":{"text":"Lock wait set to 900 ms"}}` turns the question into its outcome. The card's receipt stays revision 1.

Ops on a card apply only once it is resolved (answered, skipped, closed or replaced in the card fold). On an open card they are ignored: the agent closes it first (design §1.4 `close`). Cards don't go to the rail in v1.

### 1.6 The result, the binding, refusals

- **Success** is one line of ≤ 64 bytes: `ok 3 ops (blocks/1 #3f9a1c2e4b5d6f70)`.
  - `blocks/1` is the op and kind catalog version.
  - `#…` is the first 16 hex digits of SHA-256 over the normalized batch, as `cards_spec::block` writes it (keys in schema order, defaults filled in), the same construction as cards' digest (`SPEC:cs/digest.rs:65`).
  - There is no echo of the ops. The agent wrote them, and every echoed token would be re-read on every later round.
- **Binding.** A call binds only when line 1 parses and its digest matches the call's own input, normalized by the same `cards_spec` (design §2.2 step 3). Unbound (a hook rewrote the input, a refusal, an unknown catalog, a cut line) means a plain tool row, and **none** of its ops apply: the fail-safe.
- **Refusal** is a JSON-RPC error with one teaching line, stored as plain text, so it draws as a plain row:
  - `cards_block: ops[0].body.steps[2].state: "finished" is not a plan state (todo, doing, done, failed, skipped)`
  - `cards_block: 31 calls in the last minute; batch your ops or patch less`
- **What the agent is told: nothing about application.** The extension is stateless, so `ok` means "valid", not "applied". An op that won't apply is visible on the desk (§1.7), not to the agent. Making it otherwise would take extension state that compaction, restarts and user edits would make wrong. An honest `ok` beats a confident wrong one.

### 1.7 The board fold (`cards_spec::board`)

**Input.** The card fold's events (`SPEC:cs/fold.rs:43-53`), with three changes:
- `events()` keeps a `cards_block` call's input too. Today only card calls keep theirs (`:326-328`).
- It keeps that call's result.
- A genuine user message is also read as a block edit (§3.3).

**On the desk,** as for cards, the board is built from the history message by message, plus the live turn's rows on top (`DESK:sc/cards.rs:96-260`). A live call draws its ops as soon as its input is whole: `ToolUse` comes during the stream, before any tool runs (`CLI:ae/runtime/api.rs:452-468`). They are confirmed when the result binds, and reverted if it doesn't, which happens only on a refusal or a rewrite. So where the call sits in the tool batch never delays what the user sees.

**State per conversation:**
- **blocks by id:** kind, target, anchor, title, body, `rev`, and the revisions;
- **revisions:** for each, the history position, who made it (agent or user), the op, and `why`. The desk materializes the latest 16 bodies per block and re-folds older ones on demand, which is pure and cheap;
- **for each field, its last writer**, for the "the agent changed your edit" mark (§3.4);
- **run titles:** tool id → summary, from `collapse run`;
- **an op log:** the latest 256 ops, each with its result: applied, skipped and why, or an unbound call. It feeds the desk's mutation stream (§3.8);
- **the cards:** the embedded card `Fold`, unchanged. `card:<id>` resolves through `Fold::bound`.

**Caps the fold enforces** (the extension can't, as it has no state):
- ≤ 32 live blocks per conversation. A put beyond that is skipped and marked.
- ≤ 1 at `top`.
- The rail draws ≤ 4 expanded (§4), but takes any number.

**Compaction, `/new`, a resume:**
- The new conversation's history is the summary, plus pending events and the queued message (`CLI:crates/agent-core/src/core/session.rs:292`; `CLI:ae/runtime/compaction.rs:830-840`). So the board is empty, as every card is gone (design §2.3).
- §5.2 says what the desk keeps meanwhile.

---

## 2. Cost

### 2.1 What one call costs

| Part | Tokens | Paid |
|---|---|---|
| The call, `{"ops":[…]}`, wrapper and name | ~10 + the ops | output, once |
| A tick: `{"op":"patch","id":"plan","state":{"fix":"done","test":"doing"}}` | ~18 | output, once |
| A full-path set, `set {"steps.fix.state":"done"}` | ~22 | output, once |
| The result `ok 1 ops (blocks/1 #…)` | ~20 | input, re-read every later round (cache read) |
| The tool definition, schema plus description | ~750 | input, cached prefix, every request |
| **A call alone in the final message** | the whole context | **one more model round**, read from cache, plus ~1–2 s |

The last row is the expensive one. A call to tick, on its own, costs a round trip that re-reads the context: at 60 k tokens of context, five such ticks in a turn are 300 k cache-read tokens. Everything below exists to avoid it.

### 2.2 Ride the work: ops share a message with the real calls

- **No extra round.** A model message may carry several tool calls. The engine runs them in one batch and returns all their results in one round (`CLI:ae/runtime/stream.rs:2549-2552`). An op that shares a message with a `read` or a `bash` costs only its own tokens.
- **The lane rule.** Extension tools inherit the default effect `NonIdempotent` (`CLI:ae/tools/mod.rs:530-531`), so `cards_block` runs in the serial lane (`stream.rs:2636-2650`). Once a serial call is queued, **every later call joins the serial lane behind it** (`stream.rs:2670-2680`). So `[cards_block, read, read]` serializes both reads.
- **The fix is in the description: put it after your reads and edits.** Reads are `ReadOnly` and concurrent, and edits are keyed by path (`CLI:ae/tools/grep.rs:22-23`, `CLI:ae/tools/edit.rs:9-10`). Calls before the first serial call keep their lanes, so `cards_block` placed after them runs alongside them.
- **Visibility doesn't depend on position.** The desk draws ops at `ToolUse` time (§1.7), and each call's result is reported as it finishes (`stream.rs:2881-2887`).
- **The extension process** serializes its calls under one lock (`CLI:ae/extensions/runtime/process.rs:710`, `:1836`). A block call is microseconds of validation, so this is never felt.

### 2.3 Batch

- One call per message at most. Ops on the transcript and the rail go together.
- `state` sugar covers the common tick.
- `put` with `from: "edits"` replaces every `append` of diff lines.
- `collapse run` replaces listing tool ids.

### 2.4 What the desk derives, at zero agent cost

| Derived | From | Where it shows |
|---|---|---|
| **NOW**, what's happening: the running tool's verb and target, thinking, writing, waiting for you, idle | The live `ToolUse`/`ToolStarted`/status the desk already draws, the open wait cards (`DESK:sd/waiting.rs`) | The AGENT zone's first line (§4). It replaces a status op per round |
| **Files changed** and each file's diff | The files ledger (`DESK:sd/files.rs:1-18`), the right panel's Files section (`DESK:sd/chrome.rs:2504-2629`) | Files, and `diff` blocks with `from:"edits"` |
| **Tool-row merging** | The run fold: a finished run of ≥ 4 calls folds to one summary row (`DESK:sd/turns.rs:31`) | The agent only names it (`collapse run`) |
| **Tokens, context, cache, keep-warm** | The desk's own sections (`chrome.rs:2330-2503`) | Unchanged. The description says not to restate them |
| **A step still "doing" when the turn ends** | `Idle` with the board unchanged | Drawn "doing · turn ended" (static, honest), never "done" |

### 2.5 The closing round

The last step's outcome often comes in the last tool result, after which the agent answers in text. There are three choices:
1. tick in the final message, which costs one round;
2. leave it, so the desk draws "doing · turn ended";
3. tick optimistically before the result is in, which is a lie when it fails.

**Rule: at most one closing call per turn, and only when the end state matters**, e.g. the plan done or the diff folded. Otherwise leave it, and tick at the start of the next turn's first batch. No optimistic ticks. B5 measures how often closing rounds happen (decision 4).

### 2.6 The description (B3 ships it; B5 tunes it)

> Keep a few living blocks in the user's desktop app instead of narrating progress: a plan that ticks, a status line, a diff, a note, a metric, a file, in the transcript or the right rail ("target":"rail"). One call takes {"ops":[…]} (up to 16) and returns at once.
> Ops: put (id, kind, title, body; at "top" or {"after":id}), patch (set/append/remove/order by path such as "steps.fix.label"; items are named by id, never by index; "state":{"fix":"done"} ticks steps), replace (new body, give why), collapse (id, ids, or run:true with a summary to fold the tool calls of this run), merge (ids into one), move (no target: here), remove. A finished question card can be replaced: id "card:<id>".
> Ride your work: put ops in a message that already calls other tools, after its reads and edits. Never call this just to tick: a call in your final message costs one more round, so make at most one closing call, only when the end state matters. Patch on real transitions, not progress chatter. Keep ≤ 6 blocks, ≤ 4 in the rail. Don't restate what the app shows itself: the running tool, changed files and their diffs (use a diff with "from":"edits"), tokens. Never start a step marked stop: end your turn and say why.
> The user may take a block and edit it. You then get a user message "[block <id> edited r<N>] …" with a patch in this same vocabulary on the next line, and their edit wins. They may fold or dismiss blocks; you aren't told, and ops on a dismissed block are dropped. After a compaction your blocks are gone: put again only what's still useful. To ask the user something, use cards_ui.

About 330 tokens. The schema stays shallow, about 420 tokens: `ops` is an array of objects with `op` and `kind` enums, plus `id`, `target` and `at`. The kind bodies are described rather than spelled out per kind, so the definition stays near cards' 3.5 KB (`SPEC:cs/schema.json` 2.6 KB plus `cx/description.md` 0.9 KB).

### 2.7 An estimate for a typical turn

The turn: a bug fix in 8 rounds (read → plan → three edits → test → fix → test → answer).

| What | Rides | Output tokens | Into history |
|---|---|---|---|
| `put plan` (5 steps) | round 2's reads | ~150 | ~20 |
| `put diff from:"edits"` | round 3's edit | ~35 | (same call) |
| 5 ticks (`state`, 1–2 steps each) | rounds 3–7 | ~5 × 28 = 140 | ~4 × 20 |
| rail `put metric` and 2 patches | rounds 5–7 | ~80 | (same calls) |
| Closing: last tick, `collapse diff`, `collapse run` | **1 extra round** | ~50 | ~20 |
| **Total** | 7 calls, 1 extra round | **~455** | **~120** |

- **Plus:** the 750-token definition in each request's cached prefix, and the closing round's cache read of the context.
- **Minus:** most per-round narration ("Now I'll run the tests…", ~30–40 tokens a round), which the description asks blocks to replace.
- **Latency:** ~455 output tokens at 60–80 tok/s is ~6–7 s spread over the turn, about 0.8 s a round, plus ~1–2 s for the closing round.
- **The mocks' naive pattern** for comparison: a status `replace` every round, ticks with full paths, diff lines appended by hand, and a call per tick. That is ~1.4 k output tokens and 5 extra rounds.
- **B5's budget:** ≤ 400 output tokens of ops per 8-round turn, and ≤ 1 closing round. It is measured, not assumed.

---

## 3. The user's side

### 3.1 The principle: content is shared, presentation is local

| Action | What it is | Reaches the agent | Other desks | Persisted |
|---|---|---|---|---|
| **Hand back** an edit | content | yes: a user message (§3.3) | yes, by history | in the journal |
| **Restore** a revision | content | yes: `[block … restored rN]` | yes | in the journal |
| Answer a question | content | yes: today's card answer | yes | in the journal |
| **Take** (hold) and edit a draft | a local lease | no | no | no (in memory) |
| **Dismiss** | presentation | no | no | `blocks.json` |
| **Pin** to the rail, and back | presentation | no | no | `blocks.json` |
| Collapse or expand, unpack a group | presentation | no | no | no |
| View revision N | presentation | no | no | no |

**Why.** Every message to the agent costs tokens, wakes an idle agent (a `Submit` starts a turn, `CLI:ae/session/actor.rs:5440-5447`), or competes for the one steer slot (design §3.4 d3). The desk **never sends on its own**: only a deliberate hand-back, restore or answer sends, which is design §3.5's rule kept. Who may send is today's `input_gate()` (`DESK:sd/live.rs:179`). Without it, the block draws read-only, with "edit from <owner> · /takeover" (design §3.2).

### 3.2 Take, edit, hand back

- **Taking a block:**
  - Press `e` in block walk (§3.8), or click an editable part: a step's label, its strike or stop gutter, a note's text, or a proposed hunk's keep/drop.
  - The block is then **held** on this desk: a local draft over the base revision `B` it showed when taken.
  - The header reads `EDITING · r4`, in the theme accent, with the same focus ring an entered card has (`DESK:sd/ask_keys.rs:17-22`).
  - From the first agent change after `B`, it adds `· 2 agent changes held`.
- **Editing, by kind:**
  - **plan:** move a step (drag its grip, or `Shift+J`/`Shift+K`); strike or unstrike it (`x`); toggle a stop before it (`b`, or a gutter click); rename it (`r`, an inline field); add a step after it (`a`, which gets a `u` id).
    - Finished steps (done or failed) and the doing step can't be moved or struck. They say "already ran" (widgets).
  - **note:** its text, and its title.
  - **diff** (proposed): keep or drop each hunk (`y`/`n`, widgets and point).
  - **status, metric, file** aren't editable. Pin, fold and dismiss still work.
- **Hand back:**
  - Press `Enter` on the block, or click **Hand back**. In a text field, `Tab` leaves the field and `Enter` then hands back; in a one-line field `Enter` hands back, as a card form's does (`ask_keys.rs:44-47`).
  - **Ctrl/Cmd+Enter is not used**: it is the authorize card's Allow (design §5.1).
- **Cancel:**
  - With no changes, `Esc` leaves.
  - With changes, the first `Esc` shows "Esc again discards · Enter hands back" in the footer, and the second `Esc` discards the draft. There's no dialog.
- **A hold doesn't pause the agent.** It is local, and the agent isn't told (§0.2). To stop the agent, Cancel or HALT as today. The hold header says "the agent keeps working".

### 3.3 The edit message (built and parsed by `cards_spec::block::message`)

```
[block plan edited r4] Close the refresh race → order fix, test, deploy, bandaid · skip bandaid · stop before deploy
{"order":{"steps":["fix","test","deploy","bandaid"]},"set":{"steps.bandaid.state":"skipped","steps.deploy.stop":true}}

[block plan edited r6] Close the refresh race → add “Check staging logs” after test
{"append":{"steps":[{"id":"u1","label":"Check staging logs","state":"todo"}]},"order":{"steps":["fix","test","u1","deploy"]}}

[block cause edited r2] Root cause → text: “Two refreshes read the same token before either rotates…”
{"set":{"text":"Two refreshes read the same token before either rotates it, so both call upstream."}}

[block auth-diff edited r3] Fix: serialize rotation → keep h1, h2 · drop h3
{"set":{"hunks.h1.state":"kept","hunks.h2.state":"kept","hunks.h3.state":"dropped"}}

[block cause restored r1] Root cause → back to r1 (“First guess: the retry loop”)
{"restore":1}
```

- **Line 1, for people.**
  - It matches `^\[block ([a-z][a-z0-9_-]{0,23}) (edited|restored) r([1-9][0-9]{0,5})\] `, then the block's title (or its id), then ` → `.
  - After that comes a summary of pieces joined by ` · `, each piece cut at 40 characters, with the line at most 200 characters, sanitized (design §3.3's rules).
  - `rN` is the **base**: the revision the user edited from, or the one restored. It tells the agent what the user saw. The patch applies at the message's own place in history, whatever the base was.
- **Line 2, for machines.** It is a patch in the agent's own vocabulary (§1.4): `set`, `append`, `remove` and `order`, with no `op` or `id`, since line 1 names the block. Or it is `{"restore":N}`. It is ≤ 8 KiB, written in schema key order.
- **Self-describing.** The readable line states the result, not just the diff, so a compaction summary that sees only user text keeps the substance (§5.2).
- **Parsed exactly like a card answer.** All of a genuine user message must be the grammar (design §2.3). The id must name a live block, and every path in it must be **user-editable** for that block's kind (§1.5, last column). Anything else is ordinary text. An admitted edit then applies path by path (§1.4): a path whose item is gone, cut or replaced since the base is skipped, as in §3.4.
  - A typed look-alike from an input-holder counts. That is design §4's limit 7, unchanged.
- **What the agent never sees:** drafts, keystrokes, the hold, how long it took, which desk, dismissals, folds, pins, or which revision was viewed (design §5.6).

### 3.4 Agent ops that arrive while a block is held

1. **The fold applies them.** They are in history, and every desk agrees. **The holding desk draws the draft** over `B`, and counts the agent's changes since `B` in the header. Other desks draw the fold.
2. **On hand-back** the draft becomes a patch: the user's changes from `B`, by item id and path, never a whole body.
   - At its place in history, after the agent's ops, it applies path by path.
   - The agent's changes to paths the user didn't touch survive. A user reorder keeps the agent's `fix → done`.
   - On a path both touched, the user's wins, because it's later. This is wild2's "rebase", as a pure fold.
3. **An agent `replace` or `remove` while held.** The draft's items may be gone. The header turns to `the agent rewrote this block (r7)`, and the user chooses:
   - **Hand back anyway:** paths that no longer resolve are skipped, and line 1 still tells the agent what the user wanted;
   - **Discard.**
   
   Nothing sends on its own.
4. **An agent `move` or `collapse` while held** waits until the hold ends: a held block never moves or folds under the user.
5. **On cancel,** the view shows the fold: every held change at once, with one glow.
6. **Afterwards.** Once the agent overwrites a path whose last writer was the user (§1.7), that block's header carries `changed your edit` (with the path on hover) until the user next focuses it. That is honest, and silent otherwise.

### 3.5 Dismiss, pin, collapse

- **Dismiss** (`x`, or `×` in the header's hover actions):
  - **In the transcript,** the block shrinks to a fixed 24 px tombstone: `◇ tests · dismissed · u restore` (workspace).
  - **In the rail,** the section goes, and the zone's footer line counts `1 dismissed · show`.
  - **Later agent ops on the id still fold** (the history is the history), but this desk doesn't draw the block, so dismissed stays dismissed, across reattach and restart. Only the user's **restore** brings it back, drawing the fold's current state.
  - **Persisted** in `blocks.json`, beside `settings.json`, with recall's writer: atomic, mode 0600, swept (`DESK:sd/recall.rs:44-63`). It is keyed by conversation id (the transcript's journal id), with ≤ 256 conversations, the least recently touched dropped first.
  - **Carried across a compaction** to the successor. The desk sees both ids at `CompactionApplied` (`DESK:sc/transcript.rs:1798-1803`), and on attach the successor's header names its predecessor (`DESK:sd/favourites.rs:600`).
- **Pin** (`p`) moves a transcript block to the rail, and `p` again brings it back.
  - It is local, persisted with dismissals.
  - The transcript keeps a 24 px stub: `◇ plan · in the rail · p`.
  - A user pin outranks the agent's `move`. The stub then adds "the agent moved it", and unpinning takes the agent's placement.
- **Collapse** (`z`/`c`, or the chevron) is local, and **the user's choice wins until the agent's next `collapse` op on that block** (widgets). It isn't persisted: it's cheap to redo, and stale folds after a restart would hide news.
- **Unpack** a group shows its members in place, locally (wild2).
- **Undo.** `Ctrl/Cmd+Z` in block walk undoes the last local action: a dismiss, pin, collapse or unpack, one level per block. Sent content can't be undone. Hand back another edit, or restore a revision.

### 3.6 Revisions

- **The header chip** `rN` reads `r5`, or `r5 · replaced` (with `why` on hover).
- **Walking.** `[` and `]` step through the revisions. The header reads `r3 of 5 · agent · 07:41`: who, and when, from the message's `_synaps_at` (`DESK:sc/transcript.rs:69-77`). A live row uses its event time.
- **No yanking.** An agent op landing while an old revision is shown doesn't move the view. A dot `r6 new` appears, and `]` or `Esc` returns to the latest.
- **Restore** (`R`, or a button on a past revision) sends `[block … restored rN]` as a hand-back: content, which the agent reads (workspace's "restore as new revision"). It goes through the input gate.

### 3.7 Delivery and who may edit

- **Sending.** A hand-back takes the card path exactly: `Steer` when busy, `Submit` when idle, on the block's Live attachment (`client.on(seq)`), through D2a's send (`DESK:sd/answer.rs:415`). It is followed by the memo: sending, queued, held, checking, in the chat, unconfirmed or not sent, with Resend only by a click (design §3.5).
- **B4** generalizes `cards_spec::memo` from a card to a **subject**: a card answer, or a block edit. Its binding is (block id, base). A **superseding** edit is a later user edit message to the same block.
  - The rules carry over, including `forget` (`SPEC:cs/memo.rs:464`).
  - One edit is in flight per block. While it is, the block shows "Sending…" and takes no new hold.
- **Two split panes on one session** share the Live, its memo and its holds (`DESK:sd/live.rs`; design §3.6). A block held in one pane shows `EDITING` in the other.

### 3.8 Keys and mouse

- **Block walk:**
  - **Entering.** `Ctrl/Cmd+G` enters block walk in the focused pane. It is unbound on `main`, as is `Ctrl/Cmd+O`. Ctrl+J stays "what's waiting for you" (design §6.1).
  - **Walking.** It starts on the newest block in view. `j`/`k` (or `↓`/`↑`) walk this pane's blocks in reading order, then the rail's agent sections.
  - **Acting** on the block under the walk:
    - `e` takes it;
    - `z` or `c` folds it;
    - `x` dismisses it;
    - `p` pins it;
    - `[` and `]` walk its revisions;
    - `R` restores a revision;
    - `u` restores a tombstone.
  - **Leaving.** `Esc` returns to the composer, never to the window, so a second `Esc` can't deny a prompt (design §5.1).
  - **Activation** is the entered-card rule: keys act from the first frame that paints the walk ring, and only fresh bare presses count (`DESK:sd/ask_keys.rs:17-40`).
- **A status line** shows the current mode's keys, as point's does.
- **Mouse:** hovering a header shows `rev · edit · rail · fold · ×`. These **overlay the header's right end and never reflow it** (point). A chevron folds, and a drag grip moves a plan step.
- **`Ctrl/Cmd+O` opens the op log,** the mutation stream (wild2, point's WIRE). It is an overlay, `DESK:sd/overlay.rs`'s frame, listing the board's latest 256 ops: time, who, op, id, and applied or skipped with the reason. Clicking one jumps to its block. It is derived and read-only. It is also where an agent's skipped op, which the agent itself never learns of, becomes visible.

---

## 4. The rail

- **Where agent sections sit.** The right panel today is Context, Tokens, Prompt cache, then Files (`DESK:sd/chrome.rs:2330-2659`; assembled at `:2630-2659`). The AGENT zone goes **after Prompt cache and before Files**:
  - Context, Tokens and Cache have fixed heights.
  - Files is the one open-ended section: up to 40 files plus the shell list (`chrome.rs:34`, `:2541-2629`). Under Files, agent sections would be pushed off screen.
  - The panel already scrolls (`chrome.rs:2635`).
  - This is decision 1.
- **The zone's look:**
  - It has a `section(...)` header in the desk's own style: `AGENT`, the theme accent dot, and `N sections · M dismissed`.
  - Then comes **NOW** (§2.4): one fixed 22 px mono line, desk-derived. It beats on the 8 Hz heartbeat only while the turn streams.
  - Then the sections:
    - user pins first, then agent sections in placement order (`at` may reorder them);
    - each has a 28 px header with the kind glyph, the title, an `AGENT` chip, `rN`, the chevron and `×`;
    - each has a body of ≤ 12 lines in its kind's fixed layout (§6.2).
- **The cap.** 4 sections expanded. The 5th and later draw as one `+2 more · plan, tests` row, and clicking it lists their headers. Nothing is ever dropped.
- **Fit.** Sections fit the panel's floor of 296 px (`DESK:sd/app.rs:679`), so 248 px of content inside the 24 px padding. Values ellipsize, and a metric's sparkline is ≤ 32 quads.
- **Collapse and dismiss** each section (§3.5). A collapsed section is its header plus one summary line: plan `3/5 done`, metric `1.0 · was 2.0`, status its text.
- **A folded panel, or a window too narrow for it** (`Side::Strip`, `DESK:sd/app.rs:744-748`; the centre keeps 480 px, `:299-300`):
  - The strip (`chrome.rs:1457-1459`) gains a `◆3` count under its context gauge.
  - Clicking it opens the panel.
  - Rail sections are also reachable from block walk, and `p` brings one into the transcript.
- **Split panes.** The panel describes the focused pane's session (`chrome.rs:2331`), and so does its zone.
- **The desk's own sections are out of reach.** They aren't in the board, no id names them, and an agent section always carries the `AGENT` chip and the agent frame. A block titled "PROMPT CACHE" still reads as the agent's.

---

## 5. History, replay, compaction, several desks, the TUI

### 5.1 Replay and restart

- **One fold builds the board, from the Full history.** The desk runs Full (design §2.1). It keeps block calls' input whole, as it keeps card inputs past the 4 KiB preview cut (`DESK:sc/transcript.rs:63`, `:336-342`).
- **A mid-turn attach** gets the calls from the replay ring, as any row does. **A late desk** draws exactly what the others draw, minus their local presentation.
- **A daemon restart** reloads the journal. An op or an edit that wasn't saved is lost as any message is (design §4). A hand-back the desk can't confirm shows Resend while the memo holds it (§3.7).
- **A desk restart** keeps dismissals and pins (`blocks.json`), and loses drafts, folds and the memo, as cards lose the memo (design §4).

### 5.2 Compaction

- **What survives: the summary, and nothing else on the board.** The successor's history is the summary, plus pending events and the queued message (§1.7). So every block and card is gone, and the rail's agent zone empties.
  - The agent learns of compaction from the `<context-summary>`. The description tells it to put again only what's still useful, **with the same ids**.
  - The desk carries dismissals and pins across the compaction link (§3.5), so a re-put block the user had dismissed stays dismissed.
- **An edited block after compaction.** The user's hand-backs were user messages, and the summary reads user text (`CLI:ae/runtime/compaction.rs`, the summarization prompt). Their line 1 states the result in words (§3.3), so the substance reaches the summary.
  - Until the agent puts the block again, there is no block.
  - **While the desk process lives,** it keeps the pre-compaction board in memory, read-only. A block put again with the same id then shows that last state as `r0 · before compaction`: one `[` away, comparable, and restorable as a user edit. That restore sends `{"set":…}` with r0's editable fields, because `restore` names only revisions in the history.
  - After a desk restart that r0 is gone, and the board is the history's, as for cards.

### 5.3 Daemon reload, crash, cancel

- The extension holds nothing in flight (design §1.6), and neither do block calls.
- A Cancel mid-batch leaves the bound calls bound and the rest unbound. Unbound calls have no ops, so the board is never half of one call.
- A held answer or a dequeued hand-back is the memo's (design §3.4 d4–d5).

### 5.4 Two desks, one session

- **Content agrees.** Both desks fold the same history, so they draw the same blocks, revisions and edits.
- **Presentation doesn't.** Dismiss, pin and fold are each desk's own.
- **Holds are per desk, and nobody "holds" a block globally.** There is no core channel for one desk to tell another, and a history line would reach the agent (§0.2).
  - If desk B hands back while desk A holds the same block, A's header shows `edited on another desk · r7`. A's draft stays a patch over its base, so when A hands back, its paths apply after B's. Where both touched the same path, A's (later) wins, and A's header said so before A sent.
  - The agent reads both messages in order. Latest wins, as for card answers (design §2.3, §3.6).
- **Would need core:** a client-to-client presence frame, so other desks could show "held on another desk". It is listed beside design §8, not designed around.

### 5.5 The TUI, older desks, Digest clients

| Client | Sees |
|---|---|
| **The TUI** | The generic tool panel (`CLI:crates/agent-tui/src/tui/render.rs:552-560`): `cards_block`, its ops JSON, `ok 3 ops (blocks/1 #…)`. Desk hand-backs show as two-line user messages that read well. Nothing to answer, nothing lost: the agent's prose still carries the turn |
| **A desk before D5** | Ordinary tool rows, which fold in runs of ≥ 4 (`DESK:sd/turns.rs:31`). Harmless and noisy, which is one more reason for the off-by-default gate (§1.1) |
| **A Digest client** | Plain panels, inputs only (design §2.1) |

**A TUI-only user** should leave `blocks` off: every op would cost them tokens they never see drawn. The tool can't know who is attached (design §8 #2).

**Later, not v1:** a TUI HUD of the rail. The extension sees every rail op it validates, and `widget.upsert` frames reach daemon-mode TUIs per session (`CLI:ae/extensions/notify_router.rs:1-20`). It would be live only, never replayed, and blind to desk dismissals.

---

## 6. Motion and look

### 6.1 Look

- **The cards-D language** (`DESK:sd/cards.rs:1-10`): square corners everywhere, read by fill (`card_d`) with no outline, Sans for words and mono for targets and values.
- **A living block** marks itself with a **2 px spine in the theme accent** on its left edge. That isn't an outline, and it's the one sign that the agent can still change it. A removed or tombstoned block loses the spine.
- **The header** is 28 px: the kind glyph (the life glyph family, `DESK:sd/glyph.rs`), the title, then `rN`, a state word and the chevron, right-aligned.
- **A held block** turns the spine and the ring to the focus colour, and reads `EDITING`.
- **Never used:** the authorize card's warning colours and icon (design §5.2).
- **Subagent boxes** keep their own theme colour, and blocks use the accent, so the two never read as each other.

### 6.2 Heights come from structure, never from content

| Kind | Height |
|---|---|
| plan | header + steps × 28 px. A label ellipsizes, and its note sits in the same row, right, dim |
| status | header + 2 fixed lines (text, then detail and the progress meter) |
| diff | header + min(lines, 12) × 20 px. Past 12, the well scrolls inside and the box stops growing. Appends keep the well pinned to its newest line unless the user scrolled it |
| note | header + min(lines, 6). Past 6, `more` opens it, which is the user's own resize |
| metric | header + 44 px (the value, and the label with `was`), + 16 px with a series |
| file | header only (the path, `:line`, the note) |
| group, tombstone, stub, a collapsed block | 24–28 px, one line |

So an agent op can change a block's height only by changing its structure: adding a step, a hunk past nothing, unfolding. Then two rules hold:
- **Above the viewport,** the list keeps its anchor (`DESK:sd/app.rs:1312-1330`, the hold, or the follow it keeps). A pill, `2 blocks changed above ↑`, sits at the list's top edge (a layer, as sticky headers are, `DESK:sd/sticky.rs:1-9`). Clicking it walks to them.
- **Under a resting pointer, nothing resizes.** The block keeps its drawn height: new rows clip behind a `+1` marker, and removed rows leave padding. It snaps once the pointer leaves the block, or a key acts. A block in view that the pointer isn't over snaps at once, anchored at its top edge.

### 6.3 Motion (`DESK:sd/motion.rs`)

| Event | Full | Reduced | Off |
|---|---|---|---|
| A changed value or label | One afterglow: role → ink, decaying in three steps over 480 ms (`Motion::glow`, `motion.rs:2290-2298`, `GLOW` `:130`) | A 120 ms flash | — |
| A new block or row | `appear` (`motion.rs:324`) | Opacity only | At once |
| A tick todo → doing → done | The glyph changes. "doing" scans on the 8 Hz beat layer while the turn streams, repainting only that layer over the cached frame (`motion.rs:17-22`) | Still glyph | Still glyph |
| A move within view | One slide by the y delta (spatial) | At once | At once |
| A move from out of view | Gone at the source (anchored), `appear` at the destination | Same, opacity | At once |
| A rail section added or removed | A height change, deferred under a resting pointer (§6.2) | Same | Same |

**0 frames at idle.** Every afterglow ends within 480 ms. The beat runs only while the turn streams. A "doing · turn ended" step is static. The frame source then arms nothing, as today (`motion.rs:9-15`).

### 6.4 320 px, and the top strip

- **Width.** Every transcript block fits a 320 px pane. Narrow panes give way one order at a time, as the desk's do (#95): the plan's step notes go first, then `rN`, then the state word, keeping the glyph and the label.
  - A diff well scrolls sideways.
  - A metric stacks its value over its label.
  - A rail section's content width is ≥ 248 px (§4).
- **The top strip** (`at: "top"`, wild2's pinned mission):
  - It is one block, a note or a status, drawn as a fixed 2-line strip over the list in the sticky layer (`DESK:sd/sticky.rs`). It reflows nothing.
  - The user can unpin it locally (`p`), and dismiss it as any block.

---

## 7. Security and bounds

- **A block's only effect is to draw.**
  - No link opens, no file opens, no command runs, and no key is bound.
  - Diff lines are inert mono text. Paths are text: the desk never reads them from disk.
  - Notes are markdown-lite with links as plain text (design §5.4).
  - The only sends are the desk's own: hand-back, restore and card answers, all text that `cards_spec` built.
- **It never resolves a permission prompt.** Hand-back and restore never call `answer(prompt_id, …)`, and only `SessionCommand::Answer` resolves one (design §5.1). D8 repeats the tests: with a confirm pending, a hand-back whose text is `yes`/`always` leaves it pending, and `Esc` in block walk never denies.
- **Provenance.**
  - Every block has the agent frame and the spine, and rail sections have the `AGENT` chip.
  - Agent strings never appear in chrome: the `EDITING` and `AGENT` labels, the counts and the tombstones are the desk's.
  - No block can take a user message's style, a desk section's chrome, or the authorize card's colours.
  - Every string goes through `cards_spec::sanitize`: control and bidi characters are refused, invisibles dropped, and ZWJ and ZWNJ kept (design §5.2).
- **Bounds:**

| Bound | Limit |
|---|---|
| A call | 1–16 ops, ≤ 8 KiB compact, depth ≤ 6 |
| A block's body | the kind's cap (§1.5), ≤ 6 KiB |
| Blocks | ≤ 32 live per conversation, ≤ 1 at the top |
| The rail | ≤ 4 expanded, ≤ 12 lines each |
| An edit message | line 1 ≤ 200 characters, line 2 ≤ 8 KiB |
| Revisions materialized | 16 per block (older ones re-folded) |
| The op log | 256 per conversation |

- **Rate**, per chat (`chat_of`):
  - ≤ 30 calls a minute and ≤ 120 ops a minute;
  - ≤ 200 calls per 10 minutes.
  
  Past that, a teaching refusal. In memory: a restart resets it, which is acceptable for a brake (design §5.5).
- **What the agent sees of the user:** hand-back patches and restores (§3.3), and card answers. It never sees drafts, holds, presentation, timings or which desk (design §5.6). Hand-backs are user messages, so extensions with `privacy.llm_content` and memory extensions see them as they see typed text.

---

## 8. The PR plan

### 8.1 synaps-cards

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **B0** (this) | `docs/blocks.md` | It fixes the contract | — | ~680 lines of doc | — |
| **B1** `cards-spec::block` | The op and kind types; validation (`sanitize` reused); the path grammar and item ids; normalization; the `blocks/1` envelope and digest; the edit-message grammar (build and parse, §3.3); `schema-blocks.json` | The extension and the desk share one validator | A valid/invalid fixture per op and kind; every cap at ±1; paths (indices, `u` ids, depth); golden digests; byte-identical output with and without `preserve_order`; message round trips, look-alikes, cut lines | ~1.3k + ~1.1k | low–medium: a new grammar |
| **B2** `cards-spec::board` | The fold (§1.7): it wraps the card `Fold`; `events()` keeps block calls' input; binding; anchors and `at`; path-wise patches and `state` sugar; revisions; field last-writers; run titles; `card:` ops on resolved cards only; user-patch admission (the editable paths); caps; the op log | It replays real journals into boards. D5 builds on it | Every op on present, absent and removed ids; the bound vs unbound fail-safe; a hook rewrite; mid-run anchors; user reorder vs agent tick (both survive); the same path (user wins); `replace` under a user patch; a dismissed-agnostic fold; a compaction (empty); history split at any message equals the whole history (as `DESK:sc/cards.rs`'s test) | ~1k + ~1.3k | medium: application order and merge edges |
| **B3** `cards-ext` `block` | The second tool behind `extension.cards.blocks` (default off); batch validation; worker refusal; rate limits; the ≤ 64-byte result; the description (§2.6) | **Agents can keep blocks**, opt-in. The TUI shows JSON panels | Framing; golden results; every refusal; the rate windows; the gate on/off in `initialize`; a smoke run in a real daemon (`scripts/smoke.py`) | ~350 + ~350 | low |
| **B4** memo subjects | `cards_spec::memo` over a subject: a card answer or a block edit, with its binding and supersede rule | D8's hand-back gets D2a's delivery truth | The S3 memo suite run again for block edits; edits superseding each other; two desks; restore | ~300 + ~500 | medium: it touches a reviewed state machine |
| **B5** eval and tuning | Real tasks with blocks on: ops per turn, output tokens, closing rounds, skipped-op rate, ops on dismissed blocks; tune §2.6 | The evidence for the budget (§2.7) and for turning the default on | The report | ~200 + notes | low |

### 8.2 synaps-desk

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **D5** blocks, read-only | Bind block calls (drawn at `ToolUse`, confirmed at the result); keep their inputs whole; the board over history plus the live turn; block calls' rows hidden; the six kinds at their anchors; in-place updates with glow; collapse, merge and `collapse run` titles; `card:` replace; "doing · turn ended"; `diff from:"edits"` off the ledger | **The transcript becomes a living document** | A render per kind and state at 320 px and wide; binding and revert; a split history; list anchoring above the view and the pill; Reduced and Off; 0 frames after the last glow (`bench.rs`'s frame counter, `DESK:sd/bench.rs:42`) | ~1.6k + ~900 | medium: virtual-list anchoring |
| **D6** the rail | The AGENT zone between Cache and Files; NOW; the cap and `+N`; collapse and dismiss with tombstones; `blocks.json` (recall's writer); carried across compaction; pin to the rail and back; the strip count | **The agent molds the rail** | Zone order; the cap; dismissed stays dismissed across reattach, restart and compaction; pin outranks `move`; the file's crash sweep; the strip | ~800 + ~500 | low–medium |
| **D7** block walk and views | `Ctrl/Cmd+G` walk, `j k z c x p u`, `[ ]` revisions and `r0`, the op log (`Ctrl/Cmd+O`), the hover-action overlay, the hover freeze, local undo | **Keyboard-first navigation** | Activation on paint; bare presses only; `Esc` to the composer, never denying a prompt; no reflow on hover; the freeze under a resting pointer | ~700 + ~500 | medium: key precedence |
| **D8** take, edit, hand back | Holds and drafts; the plan, note and proposed-diff editors; the edit and restore messages through B4 and D2a's send; the agent-change count; rewrite and other-desk headers; `changed your edit`; read-only without input authority | **The user edits the agent's document** | Reorder vs tick; cut vs patch; replace mid-hold; two desks; a hand-back refused, dequeued or held; the never-resolves-a-prompt tests (click and key); IME in label fields | ~1.4k + ~1.1k | medium–high: editors, focus, delivery |
| **D9** the top strip, polish | `at:"top"` in the sticky layer; pending-changes polish from use | wild2's pinned mission | Layer tests; the cap of 1 | ~400 + ~200 | low |

**Order:** B0 → B1 → B2 → D5 → B3 (opt-in; Haseeb turns it on) → D6 → D7 → B4 → D8 → D9 → B5 → the default turns on.
- B3 comes after D5 so the first agent to keep blocks has a desk that draws them.
- D8 waits for B4.
- #56 and the cards PRs are untouched.

**SynapsCLI:** no PRs.

---

## 9. Decisions for Haseeb

1. **Where agent sections sit in the rail.** **Recommendation: after Prompt cache, before Files.** Files is the only section that grows without bound (§4). If you want them literally under everything, D6 can cap Files at 8 rows plus "more" when agent sections are present.
2. **Tell the agent about dismissals?** A message costs tokens and wakes an idle agent. **Recommendation: no, in v1.** Dismissal is silent and local, and ops on a dismissed id are dropped. B5 counts the ops wasted on dismissed blocks, and if it's material, a later step adds "dismissed: tests" to the user's next hand-back or answer, never as a message of its own.
3. **One way to ask.** **Recommendation: questions stay `cards_ui` cards.** Blocks address them as `card:<id>` once resolved (§1.5), rather than adding a second asking path that would fork the memo, Ctrl+J, presence and recall.
4. **The closing round.** **Recommendation: allow at most one closing call per turn, when the end state matters.** Otherwise steps rest at "doing · turn ended" and are ticked next turn, and optimistic ticks are never allowed (§2.5). B5 checks the frequency.
5. **The gate.** **Recommendation: `extension.cards.blocks` off by default.** You turn it on after D5. The default flips after D8 and B5, and TUI-only setups keep it off (§1.1, §5.5).
