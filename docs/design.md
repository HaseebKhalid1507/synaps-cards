# synaps-cards: agent cards as an extension around Synaps (design v1.2)

**Author:** Zero · **Status:** design only, no code. It replaces `agent-ui/architecture.md` v3.4, whose core stack (SynapsCLI #439, #440, #444, #446) is dead. **Hard rule:** zero changes to SynapsCLI; everything below uses only what `dev` already ships. v1.1 answers Shady's review of v1 (`agent-ui/review-synaps-cards-design-shady.md`; PR #1 review `6032148226`). v1.2 answers round 2 (review `6032348822`): the delivery outcomes as a state machine, the limits of Resend, and D2 split in two.

**Read at:**
- SynapsCLI `origin/dev` `189a5a19`, cited as `CLI:`. `ae/` is `crates/agent-engine/src/`, and `docs/` is the CLI's docs.
- synaps-desktop `origin/main` `3f61cff` (#75 merged), cited as `DESK:`. `sc/` is `crates/synaps-client/src/`, and `sd/` is `crates/synaps-desktop/src/`. Every anchor was re-read at those commits.

---

## TL;DR

- **The extension.** A plugin named `cards` registers one tool, `ui`, which the model sees as `cards_ui`. It needs only the `tools.register` permission: no hooks, no conversation access, no tool interception.
- **The tool never blocks.** It validates the card, mints an id, and returns at once, naming the digest of the doc it validated. Blocking a `tool.call` is not viable on `dev` (§1.2).
- **The desk draws the card from the transcript**, but only when the call's input hashes to the digest in the result (§2.2). Anything else is a plain tool row, answered in text.
- **The answer is an ordinary user message** in a fixed two-line grammar: a readable line, then the values as JSON. The desk sends it on **the card's own attachment**, as the composer sends text. The daemon already enforces who may send input, already forwards over SSH, and already shows every attached client what was sent. So answering needs no new socket and no new protocol.
- **What happens on delivery:**
  - an idle agent wakes on the answer, because a `Submit` starts a turn;
  - a busy agent reads it at its next round, or as its next turn if no round comes.
  - "Wait" is advice in the result: end your turn now.
- **Delivery is best-effort, exactly as typed text.** The desk never re-sends on its own. When it can't confirm that an answer reached the chat (a drop, a Cancel, a replaced steer, a full driver queue, a lost save), the card checks the refreshed history, says so, keeps the values, and offers **Resend**: one click, by a human (§3.5).
  - That holds only while the desk still holds the values and the card is still in the history (§4).
  - Resend checks local evidence, not a server-side compare-and-set: it can repeat an answer that arrived unseen, or land after a newer one it couldn't see. That is a stated v1 limit.
- **History is the only store.** Card state is a pure fold over the session history: the call, its result, and the answer messages. The extension stores nothing.
- **Two crates, one validator.** `cards-spec` holds the doc schema, validation, the digest, the answer grammar, the fold and the card memo, ported from #75's pure logic. The extension and the desk both depend on it.
- **The TUI** gets a plain-text fallback: the tool panel shows the question with numbered options, and the user answers in words.

### Changes from v1

| # | Shady found | v1 | v1.1 | Dropped |
|---|---|---|---|---|
| H1 | Seen in history is not saved to disk | The outbox retired on "in history" or `queued_message`; "never lost" | Three states: accepted, in the chat, and durable, which `dev` doesn't let a client observe (§4). The memo keeps the values while the card lives, so a reattach that lost the answer offers Resend | "Never lost". Now: as durable as typed text, and a loss is visible while the card exists |
| H2 | An automatic resend can undo a Cancel, replay a stale choice, or start a turn | One automatic resend after a reconnect | No automatic resend. Resend is offered only if nothing newer reached the card, and a human clicks it (§3.5) | F17 = 0 (now +1); "a duplicate is harmless"; the fold's de-duplication |
| H3 | `Client::submit` goes to the newest attachment | `Client::submit` | `client.on(seq)` of the `Live` that holds the card's session: `Steer` when busy, `Submit` when idle (§3.5) | — |
| M1 | A hook can rewrite the input; the desk draws the original | The result bound only the id | Result line 1 carries `cards/1` and the digest of the normalized doc the extension validated. The desk binds only on a match (§1.5, §2.2) | The "not rewritten" claim: a hostile transformer can still forge it (§5.2) |
| M2 | Queue, dequeue and compaction states are missing | Shown on echo, then wait for history | The full table (§3.4): `Dequeued`, `Idle` reconciliation, no answering during compaction, and the desk decodes `queued_message` | Answering during a compaction |
| — | Eager vs leased retry | "The host retries" | v1 ships an eager manifest; a leased one never retries (§1.1, §1.6) | — |
| — | Soft-wait cost, install, PR owners | | §1.2, §1.1, §9 | "Same request count" |

### Changes from v1.1

| # | Shady found (round 2) | v1.1 | v1.2 |
|---|---|---|---|
| R2-M1.1 | A full driver FIFO drops the answer with only a notice, so the memo sits in "sending" for good | "or a notice" | "Sending" has an exit. After 10 s with no echo, or on a "steering queue full" notice, it moves to "checking" and settles as unconfirmed. The notice is a hint, never a receipt (§3.5) |
| R2-M1.2 | The driver's FIFO isn't the ordinary queue, and a busy `Steer` bypasses it | "Driver armed: FIFO" | Busy goes straight to the engine; idle goes to the FIFO (§3.2, §3.4 d7–d9). D2a decodes `DriverRevoked{undelivered_steering}`. An answer that went to the FIFO is uncertain on a reattach, and the card says so |
| R2-M1.3 | A busy steer can be replaced at the terminal edge, or held after an Error or EOF | "`Steered{delivered:true}`, then the next turn" | Rows d2, d3 and d5 cover `delivered:false` at the edge, the one replaceable slot, and a new **held** state. These are typed text's limits, now stated |
| R2-M1.4 | `Idle` arrives before the desk's history refresh | "Idle while queued, text absent: Resend" | A **checking** state. The settle rule runs only on a settled view (`history_settled()`, §3.5) |
| R2-M1.c | The compaction escapes weren't exhaustive | two cases | three, including a compaction the desk hasn't heard of, where a second click replaces the first. #8 narrowed (§3.4, §8) |
| R2-M2.1 | The memo cap had no eviction rule | 16, no policy | 32 per session, evicting the oldest "in the chat" entry first. The promise covers retained entries only (§3.5, §4) |
| R2-M2.2 | "Anyone's newer answer" also dropped the memo on our own first sighting | answer count > `rev` | Only a **superseding** answer drops the entry; our own first sighting keeps it (§3.5) |
| R2-M2.3 | A rollback below the recorded revision removed Resend | count == `rev` | `seen` is the list of answers as they were when sent, and any prefix of it allows Resend (§3.5) |
| R2-M2.4 | Resend's gate is local evidence, not compare-and-set | "only when nothing newer reached the card" | "No newer answer observed in the reconciled history", checked again on the click. B then A, and duplicates, are stated v1 limits (§3.5, §4) |
| R2-M2.5 | "A loss is visible" claimed too much | unconditional | §4 says when it holds and when it doesn't |
| D2 | One PR spans two failure domains | D2 | **D2a**: delivery, routing, reconciliation, mouse and form answers. **D2b**: keyboard, navigation and focus (§9.3) |
| — | The 1 KiB result promise had no byte rule; the digest's wording; the guard was an actor-id check | | §1.5 gets a UTF-8 byte cap and a truncation order, and the digest is "a 64-bit checksum, not an identity". §3.5 uses `input_gate()` plus the binding check |

### Amended by S3's review (synaps-cards #4)

| # | Shady found | Was | Now |
|---|---|---|---|
| P1 | A settled view showing our copy in the slot left an unconfirmed entry unconfirmed, and Resend sent it again | Rule 3 for checking entries only; the click checked the gate | Rule 3 moves an unconfirmed entry too, and the click runs the settle rules on its own settled view before it sends (§3.5) |
| P2 | A held answer whose slot the next turn's steer replaced stayed held for good | Held left only on the fold or `Dequeued` | An `Idle` while held has the next settled view check the slot. Still ours: held. Not ours: the settle rule, with `Idle`'s cause (§3.5) |
| P2 | An earlier card whose id was bound again stayed answerable, and answered the later card | Latest id wins, in the map only | The earlier card is **replaced**: it keeps its receipt and takes no answer (§2.3). An entry records its card's binding, and a new binding drops it (§3.5) |
| P2 | Another tool's call reusing a pending card's tool id let that call's result bind the card | Latest **card** call | A tool id belongs to its latest call of any tool, in a history and live alike (§2.2) |
| — | The S3 PR's seven decisions where §3.5 was silent | the PR body | §2.2, §2.3 and §3.5, "Where the table was silent" |

---

## 0. What carries over, what's dropped

| Carried over (from v3.4 and #75) | Dropped (it existed only because the core owned state) |
|---|---|
| The friction budget, now re-counted (§7) | The `surface`/`surface_result` wire, `Attached.surfaces` and capability gating |
| The card schema: one question per call, options, a small form (§1.4) | The sidecar store, the answer ledger, tombstones, and the commit pipeline |
| The keyboard flow: no auto-focus, Ctrl+J entry, digits, Enter on the default, Tab, Esc (§6.1) | The contract digest, CAS, epochs, and "Answer & resume" consent |
| "A card never resolves a permission prompt" (§5.1) | Exactly-once delivery and the reload fence |
| Privacy: the agent sees the values, never drafts or who answered (§5.6) | Admission ordering, typed refusals, and per-catalog presence counts |
| #75: `sanitize`, `doc` (parse and revalidate), and `answer` value checks | #75: `fold` by host marks, the `outbox` §3.8 transitions, and the C3 protocol mirror (rewritten or deleted, §9.3) |

---

## 1. The tool (Q1)

### 1.1 Registration

- **Manifest.** `.synaps-plugin/plugin.json` sits at the repo root: `name: "cards"`, `permissions: ["tools.register"]`, `hooks: []`.
  - A tools-only extension is legal: "at least one hook or request a registration permission" (`CLI:ae/extensions/manifest.rs:276-277`).
  - `web-tools` is a live precedent: tools only, `tools.register` only.
- **Declaring the tool.** It is declared in `initialize`'s `capabilities.tools` (`CLI:docs/extensions/protocol.md:117-143`).
- **Its names.** The runtime name is `cards:ui`. The API name is `cards_ui`, because `:` becomes `_` (`protocol.md:145-147`; `CLI:ae/tools/registry.rs:244-282`).
- **Progressive disclosure.** It is off by default (`CLI:crates/agent-core/src/core/config.rs:1468,1565`), and then every verified tool is loaded.
  - When it is on, extension tools stay deferred (`CLI:ae/tools/activation.rs:349-356`) unless the user lists `ext.cards:ui` in `tools.always_loaded` (`config.rs:1481-1487`; `activation.rs:363-395`).
  - The install note says so.
- **Building.** The process is a Rust binary built by `extension.setup`. `extension.command` points at the plugin-relative `target/release/cards-ext` (`CLI:docs/extensions/plugin-setup-scripts.md:5-30`).
- **Installing.** Discovery doesn't run setup. It tries to load the binary, and if it's missing it says to run the setup script and then reload (`CLI:ae/extensions/manager.rs:302-315`, `:2224-2243`). `reload-extension` loads a plugin installed after the daemon started, and leaves sessions alone (`CLI:docs/daemon-mode.md:482-512`):

  ```
  git clone https://github.com/HaseebKhalid1507/synaps-cards ~/.synaps-cli/plugins/cards
  (cd ~/.synaps-cli/plugins/cards && bash scripts/setup.sh)
  synaps daemon reload-extension cards
  ```
- **Lifecycle: eager.** The manifest has no `deferred` block, so it stays on the eager path even under progressive deferral (`manager.rs:649-667`): one process for the daemon, respawned and retried on a crash (§1.6).

### 1.2 Can `tool.call` block until the user answers? No. These are the facts on `dev`:

| Fact | Where |
|---|---|
| The host caps `tool.call` at **120 s**, with no configuration (30 s for other methods). A leased call is capped the same way. | `CLI:ae/extensions/runtime/process.rs:1937-1953`, `:1928-1933` |
| One `call_lock` **serializes every request to an extension process** for the whole exchange. | `process.rs:708-710`, `:1957-1958` |
| Hooks go through the same lock, and their 5 s budget includes waiting for it. A blocked card would make every hook of this extension in **every session** fail open after 5 s. | `process.rs:2434-2443`; fail-open at `protocol.md:1094-1105` |
| In daemon mode, one extension process serves every session. One blocked card queues all the others. | `protocol.md:236-240` |
| **Cancel** drops the tool future (`select!` on the cancel token). The protocol has no cancel message, so the extension is never told, and its late reply is discarded. | `CLI:ae/runtime/stream.rs:120-137`; the methods at `protocol.md:68-343` |
| **Crash** (eager, as v1 ships): the host respawns the process and **retries the in-flight request** once. A blocking card would be raised twice. A leased call never respawns or retries. | `process.rs:1993-2010`; `protocol.md:1114-1124`; leased: `process.rs:1882-1886`, `CLI:ae/extensions/lease.rs:757-766` |
| **Compaction:** a blocked call keeps the turn running, and `/compact` is refused while a turn runs. | `CLI:ae/session/actor.rs:4270-4275` |
| **Daemon reload** first waits for every session to go idle (≤ `--drain-secs`). It then cancels each running turn and stops the sidecars. A blocked card holds the reload, then dies. | `CLI:docs/daemon-mode.md:1079-1085`, `:1093` |
| The extension cannot push into a session, and has no request it can make that delivers an answer. Its inbound methods are `memory.*` and `config.*` only. | `process.rs:1254-1345`; "does not initiate calls", `protocol.md:70` |
| An answer sent as a user message while the tool blocks is only a steer, delivered after the tool round ends. Blocking on (b)'s path would deadlock. | `CLI:ae/runtime/helpers.rs:136-158`; `actor.rs:3282-3305` |

**What replaces blocking:**
- `ui` returns at once.
- The answer arrives later as a user message (§3).
- `wait: true` is advice in the result, not a host wait state: the agent should end its turn, and the answer starts the next.

**The cost.** One provider round more than a truly blocking call: the agent reads the result and ends its turn (a short request, its prefix cached), then the answer starts another. S4 measures it. In exchange nothing is ever in flight, so Cancel, compaction, reload and crash have nothing to lose (§1.6).

### 1.3 Modes

| Mode | When the agent uses it | What it does | What the user sees |
|---|---|---|---|
| **Live** (default) | It can keep working. | It keeps working. If the answer comes mid-turn, it is steered in at the next round. If it comes after the turn ended, it starts a turn. | An open card. |
| **Wait** (`wait: true`) | It can't proceed without the answer. | The result tells it to end its turn now (advice: the host doesn't enforce it). The answer starts the next one. | NEEDS YOU: the composer chip, a tab dot, the rail for attached sessions, and an OS notification (§6.1). |

### 1.4 Schema: v3.4 §5.1, with `wait` redefined and `close` taking the new id

```json
{"type":"object","additionalProperties":false,
 "properties":{
  "question":{"type":"string","minLength":1,"maxLength":120},
  "detail":{"type":"string","maxLength":1500},
  "options":{"type":"array","minItems":2,"maxItems":9,"items":{"oneOf":[
    {"type":"string","minLength":1,"maxLength":60},
    {"type":"object","additionalProperties":false,"required":["label"],"properties":{
      "label":{"type":"string","minLength":1,"maxLength":60},
      "hint":{"type":"string","maxLength":120},
      "note":{"type":"string","minLength":1,"maxLength":80}}}]}},
  "multi":{"type":"boolean","default":false},
  "default":{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"},"maxItems":9}]},
  "fields":{"type":"array","minItems":1,"maxItems":5,"items":{"type":"object","additionalProperties":false,
    "required":["id","label"],"properties":{
      "id":{"type":"string","pattern":"^[a-z][a-z0-9_]{0,23}$"},
      "label":{"type":"string","minLength":1,"maxLength":60},
      "type":{"enum":["text","long","check"],"default":"text"},
      "value":{"oneOf":[{"type":"string"},{"type":"boolean"}]},
      "placeholder":{"type":"string","maxLength":80},
      "optional":{"type":"boolean","default":false}}}},
  "wait":{"type":"boolean","default":false},
  "close":{"type":"string","pattern":"^c[a-z0-9]{6}$"}}}
```

**These rules carry over unchanged** from v3.4 §5.1, and #75's `doc.rs` already implements them with fixtures:
- **Create vs close.** A create needs `question` and options and/or fields. `close` comes alone.
- **Labels** must be unique, case-folded, after sanitizing.
- **`note`** goes on at most one option, and never with `multi`.
- **`default`** must be an option label, or a subset of labels with `multi`.
- **Field ids** must be unique.
- **`value`** must match the field's `type`: ≤200 characters for text, ≤2000 for long.
- **Normalizing** writes `\n` line endings, materializes defaults, and turns string options into `{label}`.
- **Bounds:**
  - the whole input is at most 8 KiB compact, and nested at most 4 deep;
  - the detail is at most 30 lines.

  Each limit refuses the card with one teaching line, e.g. `options: at most 9 (got 12); ask in plain text or narrow the choices`.

**Modes** come from the doc, so the agent can't get them wrong:
- **quick:** options, without `multi` and without fields. One click or digit answers.
- **form:** anything else. A Send button.

**Answer shapes** are v3.4 §5.3. The keys are reserved, so a field id can't collide with them:

| Card | Answer |
|---|---|
| options | `{"choice":"<label>"}` |
| options + `multi` | `{"choice":["<label>",…]}` |
| a `note` option picked | `{"choice":"<label>","note":"<1–500 chars>"}` |
| fields | `{"fields":{"<id>":"<text>"\|true\|false,…}}`, with every field present |
| options + fields | `{"choice":…,"fields":{…}}` |
| skip | `{}` |

### 1.5 Results and the tool description

**Results:**
- **Success** is plain text. Its **first line is the envelope** the desk binds to, `Card <id> is showing (cards/<v> #<digest>): <question>`. Then come numbered options, the field names, and the instruction:

  ```
  Card c4f9x2a is showing (cards/1 #3f9a1c2e4b5d6f70): Which approach?
    1. Refactor the parser (suggested)
    2. Rewrite from scratch
  Its answer will come as a user message starting "[card c4f9x2a]", values as JSON on the next line.
  The user may also just reply in text. Keep working meanwhile.
  ```

  With `wait: true`, the last line instead reads: `You can't continue without it: end your turn now; the answer starts your next turn.`
- **The envelope.** `cards/1` names the catalog and normalizer version. `#…` is the first 16 hex digits of SHA-256 over the normalized doc as `cards_spec` writes it, **with the call's `wait` last**: keys in schema order, `\n` line endings, defaults materialized (`wait` too, `false` when absent), string options as `{label}`. `wait` is the call's mode, not part of the doc, but it changes what the desk draws (NEEDS YOU) and what the result tells the agent, so a hook that flips it must not bind. It is the doc the extension validated, after any hook rewrote the input (§2.2). It is a 64-bit checksum against accidental divergence, not an identity or a signature (§5.2).
- **Size.** A result is at most 1024 bytes of UTF-8, below the result bounds and the default spill threshold (`tool_output_spill_bytes`, `CLI:ae/runtime/tool_spill.rs:71-95`). `cards_ext` builds it in this order and cuts only at character boundaries:
  1. Line 1, the envelope, is at most 256 bytes. Only the question in it is cut, ending in `…`, and the id, version and digest are never cut.
  2. The trailer, the "Its answer will come…" lines, is fixed text of under 200 bytes, and is always kept.
  3. The options fill what is left. Each label is cut to 60 characters. When the next option wouldn't fit, the list ends `  …and N more`. Field names follow under the same rule.

  S2 tests the cap on worst-case fixtures, such as nine 60-character labels in a 4-byte script. A result cut later, by a transformer or the spill, still binds if line 1 is whole, and a result whose line 1 doesn't parse is a plain row.
- **Close:** `Card c4f9x2a closed (cards/1).` The tool keeps no card state, so it can't say whether the card was open.
- **Refusal:** a JSON-RPC error, which the host stores as plain text with no envelope, so the desk shows a plain row (`protocol.md:296-298`; `CLI:ae/runtime/stream.rs:2392-2403`):
  - a validation line;
  - `cards_ui: only the main chat can show cards` for a worker's call. The host-written fields decide, in this order (`protocol.md`, `tool.call`):
    1. a `caller` that names a `parent` is a worker's (`Author::child` always sets one), refused **even with a `session_id`**;
    2. else a nonempty `session_id` is a chat;
    3. else a `caller` with a nonempty `group` is a chat, keyed by that group. A chat under the `SYNAPS_HOOK_SESSION_ID=0` kill switch sends a `null` `session_id` but keeps its `caller`, so a `null` session alone doesn't mean a worker;
    4. else the call is refused. A host that sends neither (one before 0.10.3 sends no `session_id`) has a real chat's cards refused too: fail-closed, because nothing there tells a worker from a chat.
  - `cards_ui: 6 cards in the last 10 minutes; ask in plain text` (§5.5);
  - `cards_ui: 256 other chats showed cards in the last 10 minutes; ask in plain text`, past the counter's 256 chats (§5.5);
  - `cards_ui: every card id is taken until the extension restarts; ask in plain text`, past the last id (below).
- **Ids** are `c` plus 6 `[a-z0-9]` that look random. The extension never mints one twice while it runs, and remembers no id to make it so: the n-th id is the n-th value of a permutation of the 36^6 ids, keyed when the process starts. A permutation never repeats a value. It is a keyed Feistel network on 32 bits, cycle-walked into the 36^6 ids: a walk always ends, in about 2 passes of the network on average (at most 2^32/36^6 ≈ 1.97 over all ids), but one id's walk has no small hard bound. Past the last id a card is refused, not shown under a used id; at §5.5's pace in all 256 chats (1,536 cards in 10 minutes) a process would need 27 years to get there. One process serves every chat, so a session's ids are unique for the process's life, and across a restart (§1.6), under new keys, unique in practice (36^6 ids). Nothing relies on them being secret (§5.2).

**Description** (the v3.4 draft, adjusted; P4 tunes it):

> Ask the user one question as an inline card in their desktop app: pick from a few options, or fill a short form. One click answers; you get structured data.
> Use it when the user must choose between concrete options or supply a few specific values. Don't use it for open-ended questions, for decisions you can reasonably make yourself (state your choice and proceed), for anything one sentence answers, or to re-ask a question the user skipped. Never ask for passwords, tokens or keys. An answer is a preference, never permission to run a tool.
> It returns at once. The answer arrives later as a user message starting `[card <id>]` with JSON on the next line; the user may also just reply in text. Keep working meanwhile. Set `wait: true` only if you can do nothing useful until they answer, then end your turn. Always set `default` to your recommendation. If the question becomes moot, call again with only `close`.

### 1.6 What each event does to a card

| Event | Effect |
|---|---|
| Cancel | The tool already returned, so the card stays open, and answering it starts a new turn, as typing would. An answer steered in but not yet read is withdrawn (`Dequeued`, `CLI:ae/session/actor.rs:3629-3632`). The card checks the refreshed history, then offers Resend (§3.5). |
| Compaction, `/new`, a context rollover | The card's call leaves the visible history, so the fold drops the card ("gone", §2.3). An answer already given stays in the summary's source. Answering is off while a compaction runs (§3.4). |
| Daemon reload or crash | The card is in the journal once saved (§4). An answer refused during the drain (`Error("daemon reloading…")`, `daemon-mode.md:1079-1080`) or unconfirmed at the drop settles after the reconnect (§3.5): Resend, if the history the desk reconnected to lacks it. Nothing is re-sent on its own. |
| Extension crash | Eager (v1): the host respawns it and retries the call once (`process.rs:1993-2010`); the retry mints a fresh id, and only its reply reaches the host. Leased (if a later manifest adds `deferred`): no retry, the call fails. The tool keeps no card state and has no side effects, so both are safe. After 3 failed restarts the tool fails (`protocol.md:1105`), and the agent asks in text. |
| Extension reload or uninstall | New calls fail. Existing cards still answer, because answers don't touch the extension. |

---

## 2. Rendering (Q2)

### 2.1 What a client receives (verified)

| Path | Tool input | Tool result | Answer messages |
|---|---|---|---|
| **Live, any mode** | `Stream(Llm(ToolUse{tool_name, tool_id, input: Value}))`: **full JSON**, emitted before the tool runs (`CLI:ae/runtime/api.rs:452-468`). `ToolUseStart`/`ToolUseDelta` come first and are partial. | `ToolResult{tool_id, result: String}`, no error flag | `TurnStarted{user_text}` when idle, `Steered{text, delivered}` when streaming, and then the history |
| **Attach, Full** (`CLI:docs/daemon-mode.md:742-774`) | `Attached.conversation.api_messages`: the stored `tool_use` block, in full | the stored `tool_result` block | user messages in the history |
| **Attach, Digest** (`daemon-mode.md:775-790`) | `display_tail` `ToolUse{input}`: the full input, as a JSON string | **absent**: tool-result arrays are not projected | `User` items |

**Sources and notes:**
- **Wire types.** `CLI:ae/session/wire.rs:982-983` (variants), `:1098-1099` (conversion). `forwarded()` drops only `MessageHistory` for Digest (`CLI:ae/daemon/conn.rs:949-957`).
- **The display tail.** `CLI:ae/session/display.rs:17-23` defines the items. Tool-use input is kept in full (`:111-116`). Event payloads and non-string user content are skipped (`:83-94`). The tail is capped at `tail_items` with an `omitted` count.
- **The desk runs Full.** That is `Identity`'s default (`DESK:sc/conn.rs:59-60,73-74`), and no desktop code calls `with_history`. So it has the input and the result, live and on attach.
- **One desk gap.** `synaps-client` cuts a tool's `input` to a 4 KiB preview (`DESK:sc/transcript.rs:63,1223`; live at `:1516`), but a card input may be up to 8 KiB. D1 keeps the full input for `cards_ui` calls; the spec's 8 KiB bound limits it. This is a desk-only change.
- **One more desk gap.** The typed `ConversationSnapshot` that the attach carries doesn't decode `queued_message` (`DESK:sc/protocol.rs:1356-1379`; the daemon sends it, `CLI:ae/session/types.rs:1722`). The live `ConversationDigest` already does (`DESK:sc/protocol.rs:1384-1396`). D2a adds the field to the snapshot.
- **Digest clients** (the TUI, and any future Digest desk) can't see the result. They show the call as a plain tool panel (§6.2). See "Would need core" #5.

### 2.2 Recognising a card

1. **The tool call.** A `tool_use` named `cards_ui`, or `cards:ui` (the runtime form), is a card candidate. Another name, including a collision suffix such as `cards_ui_2`, is drawn as an ordinary tool row: the fail-safe. A tool id belongs to its **latest call, of any tool**: a provider may use one again. A card or a close still waiting for its result when a later call takes its id never gets one (the card is plain), so another tool's output can't bind it.
2. **The input.** It goes through `cards_spec::parse_input`, the same function the extension ran, and is normalized. If it is invalid, or comes from a newer catalog this build doesn't know, the call is drawn as an ordinary tool row, and the user can still answer in text.
3. **The result.** Line 1 must parse as the envelope (§1.5), name a version this build knows, and carry the digest of step 2's normalized doc and `wait` (§1.5). That binds the id to the exact doc drawn, in the mode it was asked in. Anything else is a plain row:
   - **a mismatch:** a hook rewrote the input before the tool ran (`CLI:ae/runtime/stream.rs:2350-2390`), while the transcript keeps the model's original call. The extension validated one card, and the desk would draw another;
   - **an error:** the host stores it as text with no envelope, and the live `ToolResult` has no error flag (`stream.rs:2392-2403`; `CLI:ae/session/wire.rs:983`);
   - **a cut or virtualised line 1**, or an unknown version.

   A hook that rewrites the result and keeps line 1 still binds; that is the trust limit in §5.2.

### 2.3 State on attach: a pure fold over history (`cards_spec::fold`)

The fold's input is source-agnostic, so it works over Full history, live events, or a test fixture. It takes a sequence of three kinds of event:
- `Call{tool_id, name, input}`, from every assistant `tool_use` block, of any tool, because a call takes its tool id over (§2.2). Only a card call's input is read;
- `Result{tool_id, text}`, from a `tool_result` block's text;
- `User{text}`, from **a genuine outer user message only**:
  - `role: "user"`, its content with the trailing extension blocks removed by the host's own rule: only when `_synaps_hooked` counts them and each is a saved block, else all of it is the user's (`CLI:crates/agent-core/src/core/extension_context.rs:213-261`; `protocol.md:838-860`);
  - what remains must be one string, and the whole string must be the grammar (§3.3), with nothing before or after it;
  - never a `tool_result` container, an `<event …>` payload, a `<context-summary>`, a block inside an array, or quoted text.

| State | Rule |
|---|---|
| `pending` | A call without a result yet (milliseconds). It is drawn, but not answerable. |
| `open` | A bound create result (§2.2), and no answer yet. |
| `answered{values, rev, at}` | The latest **valid** answer message to that id that comes after the card (§3.3). `rev` counts the answers. Latest wins, always: A, B, A reads as A, as the model reads it. Nothing is de-duplicated. |
| `skipped` | The latest answer is a skip. |
| `closed` | A later `cards_ui {close: id}` call whose result reads `Card <id> closed (cards/1).` An answer already given keeps its receipt. |
| `plain` | The result didn't bind (§2.2), or a later call took its tool id before it came. It is drawn as a tool row. |
| `replaced` | A later card was bound to its id. Ids are unique while the extension runs, not across a restart (§1.5), so the latest binding is the card the id names: answers to the id are checked against its doc and are its own, as the model reads them. The earlier card keeps its receipt and takes no answer: a click on it would answer another question. |
| *(absent)* | The call is no longer in the visible history (compaction, `/new`, rollover): the card is gone. |

There is no clock-based expiry. A card is moot when the agent closes it, the user skips it, or the history drops it.

**Late-attach semantics:**
- A desk that attaches later computes exactly what every other desk shows, from the same history.
- A mid-turn attach gets the call and result from the attach replay ring (`Attached.replay`), as for any tool row.
- An answer still queued as a steer shows in `ConversationSnapshot.queued_message` (`CLI:ae/session/types.rs:1702-1722`), but only the newest steer does, and only once the desk decodes it (§2.1). The memo uses it (§3.5); the fold doesn't.

---

## 3. The answer path, desk → agent (Q3)

### 3.1 The options

| | (a) The extension's own socket, forwarded beside the daemon's | (b) A user message over the daemon protocol (**chosen**) | (c) (b) plus a `before_message` hook |
|---|---|---|---|
| Who may answer | Any same-uid process on the socket. The extension can't see input ownership, so an observer or a non-owner could answer. | The daemon's input rules, unchanged. `Submit` is input (`CLI:ae/session/actor.rs:366-387`), so the existing refusals apply: an exclusive non-owner, an observer, or the wrong client of an owned session is refused (`:1500-1514`, `:5318-5333`). | As (b) |
| Remote (bella → avante over SSH) | A second socket to forward, a `synaps-desktop-remote` change, and a second protocol | **Nothing new.** The desk speaks one socket (`DESK:sc/conn.rs:310-325`), and it is already forwarded. | As (b) |
| Several desks | The extension must push state to every desk: a subscription protocol of its own. | The daemon already tells every client: `TurnStarted{user_text}`, `Steered{text}` and the history (`types.rs:1495-1503,1518`). | As (b) |
| How the agent receives it | `synaps send --session` (`docs/extensions/session-id.md:88-93`) delivers it as `<event source=…>`. That is external-event authority, not the user's (`CLI:docs/events-reactor.md:13-25`), and it is **hidden** from the display tail (`display.rs:83-87`). | As the user's own message, which is what it is | As (b), plus injected context |
| What the extension must store | Every card's state and answered marks, made durable. This is the crash-consistency problem again. | **Nothing** | Seen-sets for deduplication |
| Permissions | `tools.register` | `tools.register` | **plus `privacy.llm_content`**: the extension reads every user message of every session. Only the newest of several steers is hooked (`protocol.md:758-762`; `CLI:ae/runtime/stream.rs:1292-1294`), so it can't be relied on. |
| Blocking | Possible within 120 s, minus §1.2's problems | Impossible, and not needed (§1.2) | Impossible |

**Decision: (b).**
- **Authority.** Who may answer is exactly who may type, and the core already enforces it.
- **Remote and multi-desk.** Both come for free.
- **The extension.** It stays stateless and privilege-light.
- **What it costs.** No host-side mark distinguishes a click from typed text (Would need core #4). Only someone who can already type into the session can produce either, so the gap stays inside the same trust boundary.

### 3.2 Who may answer, per session

| Session | Answers |
|---|---|
| `client_input = shared` | Mirror and Takeover clients. Observe clients can't. The answerer becomes the active client, as typing does (`actor.rs:1519-1530`). |
| `client_input = exclusive` | The input owner only. Other desks show the card read-only, with "answer from <owner> · /takeover". |
| Owned session | The owner only |
| Driver armed | As typed text, on two paths. **Busy:** a `Steer` goes straight to the engine like any steer, never through the driver's FIFO (`actor.rs:5441-5446` calls `steer`, `:3396-3409`). **Idle:** a `Submit` goes into the FIFO, which holds 16 messages or 256 KiB. Past that it is dropped, and the only sign is a broadcast `SystemNotice` (`:3307-3331`). |

### 3.3 The answer message (built and parsed by `cards_spec::message`)

```
[card c4f9x2a] Which approach? → Refactor the parser
{"choice":"Refactor the parser"}

[card c4f9x2a changed] Which approach? → Rewrite from scratch
{"choice":"Rewrite from scratch"}

[card c81kq0z] Release details → Target: prod · Notes: skip the changelog
{"fields":{"target":"prod","notes":"skip the changelog"}}

[card c4f9x2a skipped] Which approach? → (skipped)
{}
```

**Format:**
- **Line 1, for people.** The pattern is `^\[card (c[a-z0-9]{6})( changed| skipped)?\] `. After it come the question and a summary: labels joined with `, `, then `Label: value` for each field, each cut at 40 characters with `…`. The line is at most 200 characters, and each piece is sanitized.
- **Line 2, for machines.** The values as compact JSON. The spec's own writer emits the keys in doc order, so the bytes are the same with or without `preserve_order`.
- **Size.** Line 1 is at most 200 *characters* (so at most 800 bytes of UTF-8), and line 2 at most 8 KiB: with the newline between them, the message is at most 8 KiB + 801 bytes, far under any steering bound.

**Parsing (the fold):**
- A user text (§2.3) counts as an answer only if it is exactly two lines: line 1 matches in full, line 2 is one JSON object that passes `parse_answer(doc, values)`, and the id names a card bound earlier in the history. Anything else is ordinary text.
- The question and summary on line 1 are display only. The doc is the truth.

**What the model gets:**
- the question, the values, and whether this is a change or a skip;
- the readable line.

It is self-describing even after the call has been compacted away. It never names a desk, a person or timing.

### 3.4 Delivery

A card sends what the composer sends: `Steer` while the session is busy, `Submit` when it is idle (`DESK:sd/app.rs:4640-4641`; the actor treats a `Steer` to an idle session as a `Submit`, `actor.rs:5441-5447`). From there an answer takes typed text's paths exactly, including the places where typed text is lost today. The rows are the outcomes the memo must settle (§3.5).

| # | Session state | What the daemon does | What the desk sees |
|---|---|---|---|
| d1 | Idle | Starts a turn with the answer as its user message (`actor.rs:3350-3386`) | `TurnStarted{user_text}`, then the history |
| d2 | Streaming | Puts it in the steer channel and the one `queued_message` slot (`actor.rs:3396-3409`). The engine injects it at its next round (`:3840-3843`). If the turn ends normally first, the slot becomes the next turn (`:4119-4120`, `:4168-4180`) | `Steered{delivered}`. `true` means only that the channel took it, not that it was read. It is `false` at the terminal edge, when the turn's stream has already closed. `SteeringDelivered{message}` follows once the engine injects it |
| d3 | Streaming, with a later steer (anyone's) before the engine read ours | The slot holds only the newest steer. If the turn ends before the engine's next round, only that one carries over; ours is lost with no event (`:3409`, `:4119-4120`) | Another `Steered`, and no `SteeringDelivered` for ours |
| d4 | Cancelled before it was read | Withdrawn (`:3629-3632`; during a compaction, `:3512-3513`) | `Dequeued{text}`, `Conversation`, `Idle` (`:3630-3632`, `:3703-3704`), but no `MessageHistory`: the cancel path keeps the final history to itself (`:3792-3794`). The desk asks for the history again (`DESK:sc/client.rs:2291-2301`), and it arrives later (`:2240-2253`) |
| d5 | The turn fails, or its stream ends with no terminal event | The slot is **kept**, nothing sends it, and the session goes `Idle` (`:4136-4166`; `:5967-5995`). It goes in after the next turn ends, or a Cancel withdraws it | `Conversation{queued_message}` still ours, then `Idle` |
| d6 | Compacting | One replaceable slot (`:3333-3340`). On success it is appended to the compacted history and does **not** start a turn (`CLI:ae/runtime/compaction.rs:836-840`; `actor.rs:4374-4391`). On failure it is kept, as in d5 (`:4310-4321`, `:4389-4391`) | `Steered{delivered: false}`, then `CompactionApplied{queued_restored}` or `CompactionFailed` |
| d7 | Driver armed, idle | Into the driver's FIFO. The driver's next turn writes the FIFO into the history ahead of its own prompt (`CLI:ae/session/driver.rs:183-195`; `actor.rs:2228-2235`) | `Steered{delivered: false}`, and later the history |
| d8 | Driver armed, idle, FIFO full | Dropped. The only sign is a broadcast `SystemNotice("steering queue full … message dropped")`, with no input id, no echo and no `Idle` (`:3311-3322`) | The notice |
| d9 | Driver revoked with ours in its FIFO | `DriverRevoked{reason, undelivered_steering}` lists it, and nothing sends it (`:1640-1663`) | That event. The desk doesn't type it today (`DESK:sc/protocol.rs:1640-1798`); D2a adds it |
| d10 | Not the input owner, an observer, context blocked | Refused | `Refused{command, reason}` |
| d11 | Reload drain or fence | Refused at the connection (`daemon-mode.md:1079-1080`) or by the actor (`types.rs:1247-1262`) | `Error`, then `Reloading`/`Bye` |

**`delivered: true` is not "read".** Rows d2, d3 and d5 are where typed text is lost or held today. Cards inherit those paths and don't fix them. The memo makes each outcome visible instead (§3.5).

**No answering during a known compaction.** The desk decodes `CompactionStarted`, `Applied`, `Failed` and `Cancelled` today, but keeps no state (`DESK:sc/protocol.rs:1704-1719`). D2a tracks it, and meanwhile a card reads "Compacting · answer when it's done". A successful compaction removes the card anyway. Three cases slip past, and in each of them a second answer can replace the first:
- a desk that attaches mid-compaction (the attach snapshot has no flag, `CLI:ae/session/types.rs:1761-1781`);
- a compaction that started on the daemon before this desk got `CompactionStarted`: a click in that gap lands in the replaceable slot;
- a steer that an auto-compaction at turn end carries over (`actor.rs:4168-4176`).

The surviving answer lands in the compacted history as plain text, and the card is gone, so the memo drops its entry (§3.5). An attach flag (Would need core #8) would close only the first case.

**Drivers.** A card answer takes the driver's paths exactly as typed text: busy goes straight to the engine (d2), idle goes to the FIFO (d7–d9). The attach snapshot carries no driver state (`types.rs:1761-1781`), so after a reattach the desk can't see whether an answer is still in the FIFO. The memo treats that answer as uncertain and says so (§3.5, rule 5).

### 3.5 Sending, and the card memo (desk, in memory)

**Where it goes.**
- A card belongs to one `Live`: the store of its session's attachment, which every pane on that session shares (`DESK:sd/live.rs:36-38`). The card sends with `client.on(seq)`, where `seq` is that Live's `attachment()` (`live.rs:157-159`).
- **The gate.** `Live::input_gate()` must be `Ok` (`live.rs:179-196`). It adds the exclusive-owner rule to `can_send()`, and its error string is the read-only line (§3.2). The daemon stays the authority.
- **The binding.** The card must still be bound in the Live's current fold. Matching the actor id isn't enough: `active.session_id` is the actor's id (`DESK:sd/model.rs:1793-1801`), and it stays the same across `/new`, a resume or a compaction while the conversation changes (`CLI:ae/session/types.rs:476-480`).
- Never `Client::submit`, which goes to the newest attachment (`DESK:sc/client.rs:990-995`). With `on(seq)`, a gone attachment is answered `NotAttached` and the command goes nowhere (`client.rs:846-857`, `:1093-1115`). The card then shows Not sent · Retry.
- The text joins the Live's `InFlight` with a target, which is either a pane's composer or the card `(session_id, card_id)`. Every give-back path returns a card entry to its card, never to a composer: a refusal, a detach, and a left attachment's echo, refusal or end (`DESK:sd/model.rs:1387-1400`). An echo confirms it, as it does now (`model.rs:1862-1866`).

**The memo** is a pure state machine in `cards_spec::memo` (S3). D2a feeds it events.
- **The entry.** It records `{session_id, card_id, binding, text, values, seen, cause}`. `binding` is the card that was pressed: its call's tool id and its doc's digest, with the id. Once the id names another card (replaced, §2.3) the entry no longer answers it. `seen` is the card's answers in the fold at the moment of sending: their texts, in order. It is a list rather than a count, so a rollback can be recognised (rule 4).
- **One answer per card in flight.** While a card's entry is sending, queued, held or checking, another press does nothing.
- **Bounds.** It lives in the desk process, keyed by session, with at most 32 entries per session. Admission never fails. A new entry evicts the oldest entry that is "in the chat" first, then the oldest unconfirmed one, then the oldest of any state. An evicted card shows the fold's state, as it would on any other desk, with no Resend.
- **Matching** is by exact text. Two identical answers count as one, which is harmless because they say the same thing.
- **What drops an entry.** The card closes, is replaced or leaves the fold, a **superseding** answer appears, or the entry is evicted. A superseding answer is one later than ours in the fold, or, with ours absent, an answer that isn't in `seen` (rules 2 and 4). Our own answer appearing never drops the entry; it moves it to "in the chat", so the values survive a daemon crash before the save. The desk may also drop one entry itself (`Memo::forget`), when it knows more than `seen`. For example, a change's `seen` may count an answer the desk knows a turn's end lost, so another desk's copy of it reads as that one. No other entry moves.

| State | Entered on | The card shows | Leaves on |
|---|---|---|---|
| sending | The press | The values, "Sending…" | An echo (`TurnStarted`, `Steered`): queued. `Refused`, `NotAttached` or a reload `Error`: not sent. No echo within 10 s, or a "steering queue full" notice: checking |
| queued | The echo, or rule 3 while streaming | "Queued · the agent reads it at its next step" | `SteeringDelivered` with our text, or the fold: in the chat. `Dequeued` with our text, a `DriverRevoked` that lists it, `Idle`, `Attached`: checking |
| held | Rule 3 while idle (d5, or a failed compaction) | "Held · it goes in after the agent's next turn" | The fold: in the chat. `Dequeued`, `Attached`: checking. After an `Idle`, the next settled view: Q still ours keeps it held; otherwise checking, settled at once with `Idle` as the trigger. No Resend, because the held copy would follow it |
| checking | `Idle`, `Attached` (every reconnect bumps the epoch, `DESK:sd/model.rs:98-104`), `Dequeued`, `DriverRevoked`, a notice, the 10 s timeout | The values, "Checking…" | The settle rule, on the first settled view. With no settled view in 10 s: unconfirmed, "Couldn't check whether it arrived". A late echo or `SteeringDelivered` still moves it on (a parked session can take seconds to wake) |
| in the chat | `SteeringDelivered` with our text, or rule 2 | The receipt | A superseding answer, a close or gone: dropped. A later settled history without it (a crash before the save): checking |
| unconfirmed | The settle rule | The values, one line of cause, and **Resend** if the gate below holds | Resend: sending. On a settled view, rules 1–4: closed, replaced or gone: dropped; ours in F: in the chat; Q ours: queued or held; superseded: dropped. Rules 5 and 6 don't run again, so the cause stays |
| not sent | `Refused`, `NotAttached`, a reload `Error` | "Not sent: <reason> · Retry" | Retry: sending |
| dropped | A close, replaced, gone, superseded, evicted, or forgotten by the desk | The fold's state, with no Resend | — |

**The notice is a hint, not a receipt.** A "steering queue full" notice names no input and goes to every client. So it moves each of that Live's entries in sending to checking, and does nothing more.

**The settle rule** runs only on a **settled view**: the history the desk holds matches the latest `Conversation` digest (`messages_len` and `messages_hash`, the check at `DESK:sc/history.rs:150`). After a mismatch, the client asks for the history again and applies the reply when it comes (`sc/client.rs:2291-2301`, `:2240-2253`). Until then the view is unsettled, so an `Idle` that arrives first (d4) can't produce a false Resend. D2a exposes the condition as `Live::history_settled()`. Let F be the card's answers in that history, and Q the digest's `queued_message`. The first rule that matches decides:
1. The card is closed, replaced, or absent from the history: **dropped**. Closed and replaced drop on any view; absent only on a settled one.
2. Ours is in F after the `seen` prefix: **in the chat**, unless an answer follows ours, in which case it is **dropped** as superseded.
3. Q is ours: **queued** if the session is streaming, **held** if it is idle. This holds for an unconfirmed entry too: whatever made the desk unsure, the slot shows it on its way.
4. F is not a prefix of `seen`, so an answer we didn't know about is there: **dropped** as superseded. A **shorter** F is still a prefix. That is a rollback below what we had seen. For example, the card was saved with no answers, A was in the chat but never saved, B went out with `seen = [A]`, and the daemon crashed back to no answers. That falls through to rule 6 and offers Resend.
5. Our echo was a driver echo (`Steered{delivered: false}` while idle and not compacting), and no `DriverRevoked` has arrived since: **unconfirmed**, "It may still be in the driver's queue".
6. Otherwise **unconfirmed**, with the cause of what triggered the check:

| Trigger | Cause line |
|---|---|
| `Dequeued` | "The agent was stopped before reading it" |
| `DriverRevoked` | "The driver stopped before reading it" |
| The notice or the timeout | "No reply from the daemon. A driver's queue may be full" |
| `Attached` | "The connection dropped or the daemon restarted. It may not have arrived" |
| `Idle` | "It didn't reach the chat. Another message may have replaced it" |

**Held, after an `Idle`.** A held copy waits in the slot, outside any turn's steer channel. The next turn carries it over when it ends normally, and in that case no `Idle` comes: the host empties the slot, sends a `Conversation`, and then appends the copy (`actor.rs:4119-4125`, `:4180-4184`). So a settled view without ours means nothing until an `Idle` has come. After one, the next settled view decides. Q is still ours (another failed turn, d5): held. Q is something else or nothing: another message replaced the slot (d3 in a later turn), or nothing is left to carry it, so the entry settles by rules 4–6 with `Idle`'s cause. There is no timer: a held entry that never sees a settled view stays held. One gap remains: an `Idle` whose settled view comes only after a later turn has emptied the slot to carry ours shows unconfirmed until ours lands a moment later (rule 2).

**Where the table was silent** (S3's review settled these):
- `Attached` while sending or held moves the entry to checking, as it does a queued one: a reconnect may have lost either.
- A late echo moves only a checking entry. An unconfirmed one moves only on a settled view (rules 2 and 3).
- "Bound and open" in the Resend gate means the same binding, still answerable: open, answered or skipped, so a changed answer can be resent.
- Q being ours outranks an answer we didn't know about on every settled view (rule 3 before rule 4). Rules 1 and 2 still come first.
- In the chat clears the driver flag: once in the history it has left the driver's FIFO, and a later loss is the reconnect's.
- A result binds on its text alone, never on a stored error flag, because the live `ToolResult` has none (§2.1).
- A reused id names its latest card (§2.3), and a reused tool id its latest call (§2.2). An entry for the earlier card is dropped, and no press, Retry or Resend sends to the later one on its behalf.

**Resend.**
- **The gate.** Resend appears only in the unconfirmed state, and only while all of these hold:
  - the card is still bound and open;
  - F is a prefix of `seen`: no newer answer has been observed in the reconciled history;
  - `input_gate()` is `Ok`.
- **The click.** The click runs the settle rules on a settled view, then checks the gate again, not only when the button is painted. If the view isn't settled, the click shows "Checking…" and sends nothing. If Q is ours, the entry is queued or held and nothing is sent: the copy in the slot would follow a resend.
- **What it does.** Resend is a human action. It sends the same text by the same rules, on the same attachment. To an idle session it starts a turn, and the footer says so (§4). Under the button, one quiet line: "If it did arrive, the agent reads it twice."
- **The v1 limit.** The gate is local evidence, not a server-side compare-and-set:
  - It can't see an answer that is queued, in transit, or being sent by another desk. If newer answer B is still unseen and A is resent, the result is B then A: A wins in the fold, and the model reads it last.
  - It can repeat an answer that did arrive unseen, such as one still in a driver's FIFO or one hidden behind a newer steer.
  - Closing this needs a precondition on `Submit` (Would need core #6).
- **Why nothing is automatic.** A `Submit` isn't idempotent, because an idle one starts a turn. An automatic resend could undo a Cancel (the steer was `Dequeued`), replay a stale choice over another desk's newer one, or wake the agent with no user action. A memo and a button cost one click on a rare path, and avoid all three.

**What the memo can't see.** These limits are §4's qualification:
- **An evicted entry:** the card is open again, with no values kept.
- **A dead desk process:** the memo dies with it. On reopening, the desk shows the fold, and nothing more.
- **A card that was never saved** before a daemon crash: it is gone, and there is nothing to answer.
- **An open card after a restart** proves only that the answer is absent from the recovered history. A model turn may already have acted on it before the crash.

### 3.6 Several desks, split panes, changing an answer

- **Same history.** Every desk folds the same history, so all of them show the same state.
- **Split panes.** Two panes on one session share one `Live`, one `InFlight` and one memo, so they can't both send one card's answer. A pane that switches tabs leaves its entries with their Live, routed by session id (§3.5).
- **Concurrent answers.** Two desks answering at once produce two user messages. The later one wins in the fold, and the model reads them in order, like a user correcting themselves. A desk that sees a superseding answer drops its entry, so it never offers to send it again. It can't see one still in flight, so it may offer a Resend that lands after it (the limit in §3.5).
- **Change an answer.** It reopens the card with the old values, and the new answer is sent with `changed`. It is allowed while the card is in history and not closed.

---

## 4. Durability (Q4)

**The extension stores nothing.** Its only state is in memory, and a restart resets it: the per-chat rate counter (§5.5), acceptable for a spam brake, and how many ids it has minted (§1.5). Both are bounded: the counter keeps at most 256 chats, and an id costs nothing to remember. The protocol's "write state to disk promptly" advice (`protocol.md:1124`) has nothing to apply to.

**The session journal is the only store.** The card is the stored `tool_use` plus its `tool_result`, and the answer is a stored user message.

**Three states, two of them observable:**

| State | The desk's signal | What it means |
|---|---|---|
| Accepted | The echo: `TurnStarted` or `Steered` | The daemon has it in memory. |
| In the chat | The history shows it, or `SteeringDelivered` names it | The model will read it, or has. |
| Durable | **None on `dev`** | The daemon queues a save each round without waiting, and flushes before `Idle` within a budget. A failed or slow save still announces `Idle`, and is logged on the daemon only (`CLI:ae/session/actor.rs:1214-1273`). `Save` replies with nothing (`:5505-5507`). |

So an answer is exactly as durable as a typed message, and the desk never says "saved".

**When a loss is visible, and when it isn't.** A lost answer shows as Resend only while three things hold: the desk process lives, the memo still holds the entry (at most 32 per session, §3.5), and the card is still in the recovered history. Then every loss settles to unconfirmed once the history is checked:
- a dropped connection or a Cancel;
- a slot replaced at the terminal edge (d3);
- a full driver FIFO, or a revoked driver (d8, d9);
- a daemon crash that rolled the card's answers back below what the desk had seen (rule 4).

Outside those conditions the card falls back to the fold. The fold tells the truth about the recovered history, and nothing more:
- **after an eviction or a desk restart,** the card is simply open again, with no values kept;
- **a card that was never saved** is gone with the crash;
- **an open card after a restart** doesn't mean the agent never saw the answer. A turn may have acted on it before the crash.

Resend itself is checked on local evidence only (§3.5). It can repeat an answer that arrived unseen, or land after a newer one it couldn't see.

**What v1.2 trades away, plainly:**
1. **No delivery guarantee beyond typed text's:** no durable receipt, no idempotency, no exactly-once (Would need core #6).
2. **No automatic resend.** An unconfirmed answer costs one click (F17, F20, F22), and only while the memo holds it.
3. **Open cards don't survive compaction, `/new` or a rollover.** Answering is off during a compaction, and an answer carried into one waits for the next turn (§3.4).
4. **A card or an answer can vanish in a daemon crash before its save,** as any message can.
5. **No blocking.** `wait` is advice.
6. **Presence only for attached sessions.** The rail can't badge a session the desk hasn't attached (Would need core #1).
7. **No host mark on answers.** A typed look-alike counts as an answer. Only an input-holder can type one.
8. **Consent isn't gated.** Answering after a Cancel starts a turn, exactly as typing does. There is no "Answer & resume" gate. The footer says "answering continues the agent" when it is idle.
9. **Unbound cards are plain rows.** That happens in Digest mode, after a hook rewrote the input, or when the result didn't bind (§2.2). The user answers in text.
10. **Resend is not a compare-and-set.** It can duplicate an answer, or follow a newer answer from another desk that it couldn't see (§3.5). Typed text has the same gap.
11. **Typed text's queue losses carry over.** A steer replaced in the one slot at the terminal edge, a full driver FIFO, and a slot held after a failed turn all behave as they do for typed text (§3.4 d3, d5, d8). The card makes each one visible rather than fixing it.

---

## 5. Security

### 5.1 A card never resolves a permission prompt

- **Structurally:**
  - The desk's card code has one effect: a `Steer` or `Submit` of text that `cards_spec::message` built, on the card's attachment. It never calls `answer(prompt_id, …)`, and never touches the authorize card's state.
  - Only `SessionCommand::Answer` resolves a confirm and its allow-all (`CLI:ae/session/actor.rs:4495-4527`), so a submitted `yes`, `always` or JSON can't.
  - D2a pins this for clicks, and D2b for keys. With a confirm prompt pending, answering a card `"yes"`/`"always"` leaves the prompt pending: by mouse, then by keyboard with focus moving between the card and the authorize card.
- **The extension** has `tools.register` only. It can't block, modify or confirm a tool call, which needs `tools.intercept` (`protocol.md:1064-1072`). It sees no conversation.
- **What it doesn't promise.** An answer has a typed message's authority, no more and no less.
  - Agent-written labels and defaults copied into it, and whatever the user types in a field, reach the model as text and can steer it.
  - An already-authorized tool may run in response, as after a typed reply.
  - Tool gates still apply. The promise is "a card never resolves a host permission prompt", not "an answer never leads to a tool call".
- **Keys.** Agent cards bind none of the authorize card's keys: Ctrl/Cmd+Enter is Allow, bare Esc is Deny, and arrows and PgUp/PgDn scroll (`DESK:sd/app.rs:1737-1749`).
  - Leaving card mode returns focus to the composer, never to the window, so a second Esc can't deny a prompt (v3.4 §4.1).
  - Ctrl+J visits authorize prompts first.

### 5.2 Provenance and sanitizing

- **Chrome.** The desk draws it: "◇ The agent asks", the session, and the card frame. Agent strings never appear in the chrome. Cards never use the authorize card's warning colours or icon.
- **Sanitizing.** `cards_spec::sanitize` is #75's `sanitize.rs`. In an agent string it refuses, with a teaching line, any C0/C1 control (other than `\n`, `\t` and a line-ending `\r`) and any bidi embedding, override or isolate. It drops the other invisible characters (zero-width space, word joiner, BOM, soft hyphen, variation selectors, …), but **keeps ZWJ and ZWNJ** (U+200D, U+200C), which join emoji and shape scripts. It sanitizes the summary line again on the way out.
- **Detail** is markdown-lite: bold, italic, inline code and lists. A link shows as plain text.
- **Ids are not credentials.** Authority comes from the daemon's input rules (§3.2). A guessed id lets you do only what typing already lets you do.
- **Names aren't identities.** A tool name, a plugin name and the envelope don't authenticate anything.
  - A project-local plugin named `cards` overrides the user's (`docs/extensions/README.md:53`), and any same-uid plugin runs unsandboxed. But cards carry no power, so a spoof gets what any message gets.
  - An output transformer can rewrite a result and forge the envelope (`CLI:ae/runtime/stream.rs:2403-2419`). The digest binding catches an unaware rewrite, not a hostile one. Host-attested provenance would need core (#7).

### 5.3 Bounds

- **Per card:** §1.4.
- **Per answer:** 8 KiB, nested at most 3 deep. A text field is ≤200 characters, a long field ≤2000, and a note 1–500.
- **Per session:** at most 3 open cards are drawn expanded, and older open ones collapse to "+2 more questions".
- **Everything is validated twice:** in the extension, and in the desk's fold, and the digest says both saw the same doc.

### 5.4 No URLs, code or webviews

- Cards are text.
- No media, no links you can click, no HTML and no scripts.
- The deferred `code`/`diff` blocks (v3.4 §5.5) are inert monospace.

### 5.5 Rate limits

- **The extension:** 3 creates per chat per minute, and 6 per 10 minutes. A chat is its `session_id`, or its `group` when that is `null` (§1.5). Past that, it returns a teaching error.
  - The counter is bounded across chats, not only in each: it keeps at most 256 chats. A chat leaves when its last card is 10 minutes old, so the chats idle longest leave first, and never sooner. No chat with a card in its window is evicted, so other chats can't buy it a fresh budget. While all 256 have one, a new chat's card is refused until the chat idle longest leaves (§1.5's line). The memory a crowd took is given back once the table is a quarter full.
- **The desk:** one answer in flight per card, and repeat presses are ignored.
- **Skips.** A skip is a message the agent reads. The description tells it not to re-ask.

### 5.6 What the agent sees of user input

- **It sees:** the submitted values, the labels and its own field ids, and whether the answer is a change or a skip.
- **It never sees:**
  - drafts, keystrokes, focus or hover;
  - which desk answered;
  - timings (`_synaps_at` is stripped from requests, `protocol.md:855-858`).
- **Other extensions.** An answer is a user message, so every extension with `privacy.llm_content` sees it, exactly as it sees typed text. Memory extensions may keep it.
- **No secret fields.** A label heuristic (`pass|token|secret|api key`) adds the footnote "Sent to the agent and saved in the chat. Don't type secrets here". Secrets keep using the secret prompt.

---

## 6. Clients

### 6.1 Desk (GPUI)

- **Placement.** The card replaces the generic tool row at the call's place, in the cards-D language:
  - the chrome line, then the question and the detail;
  - options as rows with digit keycaps, the default marked "suggested";
  - fields;
  - the footer.

  An answered card collapses to a receipt: "Which approach? → Refactor the parser · change". The answer message itself draws as a slim row, "↳ answered «Which approach?»", and clicking it scrolls to the card.
- **Focus.** A card arriving never takes focus. The composer keeps it, and shows the chip "◇ 1 question · Ctrl+J".
- **Keys:**

  | Key | Does |
  |---|---|
  | Ctrl+J | Enters the card. It already exists as "go to what's waiting": `DESK:sd/app.rs:10033`; `DESK:sd/split_tests.rs:2179-2216`. Its order is authorize prompts, then wait cards, then open cards, this pane first. |
  | 1–9 | Pick an option |
  | Enter | The highlighted option, which starts on the default |
  | 0 | Skip |
  | Tab / Shift+Tab | Move between fields |
  | Space | Toggle a check |
  | Esc | Back to the composer |
  | Ctrl+Shift+J | The oldest waiting card in any open session |
  | Ctrl+P | Its "Waiting" group |

  Batches are frozen at entry (v3.4 §4.8). Only new key presses act. Inside a field, digits and Enter edit, and IME composition owns every key.
- **Clicks.** One click answers a quick card. A click acts only once the button is armed (the existing `Arm` rule).
- **Presence:**
  - for attached sessions (tabs and panes): a tab dot, rail NEEDS YOU for wait cards, ASKED for open ones;
  - for a wait card when the window or session isn't focused: an OS notification.
- **Motion.** Cards are static rows: 0 frames idle, no timers but `ARM`. They respect Full/Reduced/Off (`DESK:sd/motion.rs:37-43`).

### 6.2 TUI

The TUI needs no change.
- **The card.** The generic tool panel (`CLI:crates/agent-tui/src/tui/render.rs:552-560`) shows `cards_ui` with its input and, live, the result text: the question with numbered options, plus "the user may also just reply in text" (§1.5). A TUI attaching later runs Digest, so it shows the input only (§2.1).
- **Answering.** The user types "2" or "the parser one", and the model maps the reply from the result it wrote.
- **Desk answers** show as two-line user messages, so the record reads well in both clients.
- **A possible later step: a TUI widget.** `widget.upsert` frames reach daemon-mode TUIs (`CLI:ae/extensions/notify_router.rs:1-20`; `CLI:crates/agent-tui/src/tui/stream_handler.rs:632-652`). That allows a HUD for open cards, with two limits:
  - It is live only, never replayed.
  - Clearing it on answer needs `before_message`, and so `privacy.llm_content`.

  It is not in v1 (decision 3).

### 6.3 Older or missing pieces

| Situation | Result |
|---|---|
| The desk is older than the card renderer | A generic tool row and the result text. The user answers by typing. |
| The extension is not installed | No tool, so no cards. Nothing breaks. |
| The extension is newer than the desk's `cards-spec` (a new param) | `parse_input` fails closed: a generic row and text answers. A new catalog version means a major bump in the crate. |
| A Digest client | Plain panel (§2.1) |

---

## 7. Friction budget

The starting states are v3.4 §0.2's:
- **A:** the card is in view and the composer has focus;
- **B:** the card is scrolled out of view;
- **C:** the card is in another pane;
- **D:** the card is in another open session;
- **D′:** the session is in the rail only;
- **E:** the window is inactive.

A count runs from the moment the card appears until the agent is unblocked: the `Submit` or `Steer` was accepted.

| # | Flow | Start | Mouse | Keys | Note |
|---|---|---|---|---|---|
| F1 | Take the suggested option | A | **1** | **2** (Ctrl+J, Enter) | |
| F2 | Another option | A | **1** | **2** (Ctrl+J, digit) | |
| F3 | Option with a note | A | 2 + typing | 3 + typing | typing-bound |
| F4 | Form, prefilled and valid | A | **1** (Send) | **2** | |
| F5 | Form, m fields to fill | A | m + 1 + typing | 1 + (m−1) + 1 + typing | typing-bound |
| F6 | Multi-select, k of n | A | k + 1 | k + 2 | count-bound |
| F7 | Skip | A | **1** | **2** (Ctrl+J, 0) | |
| F8 | Scrolled out of view | B | **2** (chip, option) | **2** | |
| F9 | In another pane | C | **1** | **2** | |
| F10 | In another open session | D | **2** (tab dot, option) | **2** (Ctrl+Shift+J, key) | |
| F11 | Session in the rail only | D′ | 2 (open it, option) | 3+ | **no badge leads you there** (Would need core #1) |
| F12 | Window inactive, wait card | E | **2** (notification, option) | — | attached sessions only |
| F13 | N cards open at Ctrl+J | A | N | N + 1 | batch |
| F14 | Answered on another desk | any | 0 | 0 | |
| F15 | Change an answer | A | 2 | 3 | |
| F16 | Refused (not owner, context blocked) | A | +1 (Retry) | +1 | rare |
| F17 | Connection, reload or crash mid-send | any | 0 if confirmed, else +1 (Resend) | +1 | never automatic; checked first (§3.5) |
| F18 | TUI | — | — | typing + Enter | plain text |
| F19 | Desk without input authority | A | +1 (`/takeover`) | +1 | |
| F20 | Cancelled before the agent read it | A | +1 (Resend) | +1 | `Dequeued` (§3.4 d4) |
| F21 | The turn failed with the answer queued | any | 0 | 0 | held: it goes in after the next turn (d5) |
| F22 | Driver FIFO full, or driver revoked | any | +1 (Resend) | +1 | a Resend into a still-full FIFO drops again (d8, d9) |

**Targets:**
- ≤1 click for a quick card in view;
- ≤2 keys anywhere in the window;
- +1 (Resend or Retry) for any delivery the desk can't confirm while the memo holds it, and never an automatic send.

**Enforcement:**
- The PR that owns a flow adds its test (§9.3): build the start state, play the inputs, and assert exactly one `Submit` or `Steer` with the expected text, on the card's attachment.
- The negative tests from v3.4 §0.5 must each send nothing. D2a owns a click before the button is armed. D2b owns:
  - digits typed in a field;
  - IME composition;
  - key repeat;
  - a key pressed before the card was painted;
  - a mouse answer inside a batch.

---

## 8. Would need core (listed, not designed around)

| # | Gap | Smallest generic capability that would close it | v1 without it |
|---|---|---|---|
| 1 | Rail badges for sessions the desk hasn't attached | A per-session extension status line: a `status.set{session_id, count, label}` notification, routed like `widget.*` (`docs/extensions/session-id.md:59-63`) and carried, bounded, in `SessionMeta` | Badges for attached sessions only (F11) |
| 2 | Knowing whether any attached client can answer cards, so the tool returns "ask in text" to TUI-only users | `tool.call` `params.clients: [{kind, features}]`, the attached clients' Hello kinds | The card is always shown. The text fallback (§6.2) covers it. |
| 3 | Blocking beyond 120 s without stalling the extension | Concurrent `tool.call` (the inbox already routes replies by id: `process.rs:575-576`), a `$/cancelRequest` on drop, and a per-tool timeout declared at registration | Not needed: wait means end-turn (§1.2) |
| 4 | Telling a desk answer from typed text | `Submit.meta`, persisted as a `_synaps_*` sibling key the way `_synaps_hooked` is (`protocol.md:838-860`), and visible to clients. It marks the client; it wouldn't authenticate one or prove a person clicked. | The grammar plus input authority (§3.1) |
| 5 | Card state for Digest clients | A `DisplayItem::ToolResult{tool_id, preview}` in `display_tail` | The desk stays Full. Digest clients get the plain panel. |
| 6 | A durable receipt, and idempotent input | `Submit.input_id`, chosen by the client: persisted as a `_synaps_*` sibling key, de-duplicated by the actor, echoed on `Steered`/`Dequeued`, and confirmed by a `Persisted{input_ids}` event once the save lands | Best-effort, with a human Resend (§3.5, §4). This is a sketch only: atomic persist-and-dedup, receipts for queued input, and an `expect` precondition (the card's answers as the client saw them, refused if stale: the compare-and-set that Resend lacks) need their own design |
| 7 | Tool-result provenance | A host mark on the stored `tool_result` saying whether a hook changed the input or the output, visible to clients | The digest binding (§2.2). A hostile transformer can forge it. |
| 8 | Compaction state on attach | `AttachSnapshot.compacting` | A desk that attaches mid-compaction may answer into it (§3.4). The flag closes only that blind spot. A compaction that starts between a click and the desk's `CompactionStarted` stays a race |

None of these blocks v1. Revisit #1 first if F11 hurts in use, and #6 if unconfirmed answers turn out to be common.

---

## 9. The repo and the PR plan (Q6)

### 9.1 Layout

```
synaps-cards/                       (also the plugin root: clone as ~/.synaps-cli/plugins/cards)
  .synaps-plugin/plugin.json        name "cards", tools.register, setup → scripts/setup.sh
  scripts/setup.sh                  cargo build --release -p cards-ext
  Cargo.toml                        workspace
  crates/cards-spec/                lib, no I/O: serde, serde_json, sha2, unicode tables; no tokio, no gpui
    src/{types,sanitize,doc,digest,answer,message,fold,memo}.rs
    schema.json                     §1.4, which is the tool's input_schema
  crates/cards-ext/                 bin: Content-Length JSON-RPC stdio, initialize, tool.call, shutdown
    src/{main,rpc,tool}.rs, description.md
  fixtures/{valid,invalid,raw,sanitize.json,casefold.json}/   ported from DESK:crates/synaps-client/tests/fixtures/ui/
  fixtures/{messages,transcripts}/  new: answer-message round trips, fold scenarios
  docs/design.md
```

**Why it's laid out this way:**
- **One validator.** The extension and the desk run the same `parse_input`, so the hand-mirror parity work of #75 disappears.
- **The desk depends on `cards-spec`** through a git dependency pinned to a revision.

### 9.2 synaps-cards PRs, smallest first. Each one works alone.

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **S0** (this) | `docs/design.md` v1.2 | It fixes the contract. | — | ~730 lines of doc | — |
| **S1** `cards-spec` doc | `types`, `sanitize`, `doc` (`parse_input`, `revalidate`), `digest` and the envelope (§1.5), `answer` (`parse_answer`), and `message` (build and parse §3.3). Ports #75's `sanitize.rs`, `doc.rs` and `answer.rs`, minus the contract, ids and capsule. | The desk can depend on it, and both sides validate and hash identically. | The ported fixture corpus, `message` round trips, golden digests, and byte-identical output with and without `preserve_order` | ~1.7k src, ~1.1k tests, fixtures copied | low: a port |
| **S2** `cards-ext` and the plugin | The stdio loop, `initialize` with the tool, `tool.call` → `parse_input` → id, digest and result text (§1.5), worker refusal, the rate counter, `shutdown`, the eager manifest and the setup script | **Agents can ask cards now.** TUI and older desk users see the text and answer by typing. | Framing tests (split and joined frames), golden results per fixture, the worker path, rate limits, and a smoke run in a real daemon, installed by §1.1's three commands | ~600 src, ~450 tests | low. Watch the description's token cost. |
| **S3** `cards-spec` fold and memo | `fold` (§2.3) over `Call`/`Result`/`User`, and the memo (§3.5) as pure transitions: the states, the settle rule over a settled view, `seen` prefixes, eviction, and the Resend gate. Infrastructure: no user-facing feature on its own. | D1, D2a and D2b build on it. It replays against real journals. | Fold: every state; A, B, A; input rewritten (digest mismatch); result rewritten keeping line 1 (binds, the documented limit); an error, a cut result, an unknown version; look-alikes in tool results, events, summaries, quoted JSON, arrays and malformed `_synaps_hooked`. Memo: Cancel before the drain; `Idle` before the settled view (no Resend until it settles); reattach with and without the answer; `queued_message`-only reattach; held after an Error and after a failed compaction; a slot replaced at the terminal edge; FIFO overflow by notice and by timeout; `DriverRevoked` listing ours; a driver echo across a reattach (rule 5); a daemon-only crash after the answer was in the chat; a rollback below `seen` (Resend offered); our own first sighting keeps the entry while a superseding answer drops it; no Resend once the first copy is in the chat; two desks A, then B, then reattach; eviction at 32; the gate checked again on the click; compaction or `/new` mid-queue. From S3's review: a settled Q after the check timed out, and Q at the click; held, then the slot replaced in the next turn, and the held copy that survives; a card id bound again (a press on the earlier row, its entry queued, unconfirmed and not sent); a tool id taken over by another tool, live and in a history | ~800 src, ~1.2k tests | medium: the grammar and state edges |
| **S4** description tuning | A small eval script: N real tasks, measuring when agents call `cards_ui` and when they shouldn't, the extra end-turn round of `wait`, then tuning §1.5 | It keeps agents from spamming cards. | The eval's report | ~200 + notes | low |

### 9.3 synaps-desk PRs

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **D0** | Delete #75's dead core mirror: the `capabilities.surfaces` declaration, the `surface`/`surface_result` events, `Attached.surfaces`, `surface_action`, and `sc/surface/*`. Add the `cards-spec` dependency. Independent of #56. | It removes protocol code no daemon will ever speak. | The existing suite; the parity tests move to S1. | about −6.5k, +100 | low |
| **D1** | Read-only cards: recognise and bind them (§2.2), keep the full input for `cards_ui`, fold the state, draw the card and its receipts. No interaction. | The transcript reads as cards. | Render tests per state, the fold over live and history paths, the binding cases, a fallback per §6.3, and F14 | ~1k + ~600 tests | low |
| **D2a** | Answering by mouse, and the delivery adapter: option and Send clicks with pointer arming; `input_gate()` and the binding check; `client.on(seq)` on the card's Live; `InFlight` card targets on every give-back and left-attachment path; the memo fed from live events (echoes, `SteeringDelivered`, `Dequeued`, `Idle`, `Attached`, the notice, the 10 s timer); `Live::history_settled()`; decoding `queued_message` in the snapshot, `CompactionApplied.queued_restored` and `DriverRevoked`; compaction tracking; Retry, and Resend with its check on the click; read-only without authority | **Mouse and form answers work**, and every delivery outcome shows on the card | The mouse halves of F1–F7 and F9; F16, F17 and F19–F22; the never-resolves-a-prompt test for clicks. Adapter fixtures, as sequences of real events: a card in pane A after B attaches; a tab switch with an answer out; a reconnect with both sessions open; a daemon-only restart, including a rollback below `seen`; a Cancel before the drain, with `Idle` ahead of the refreshed history; two answers at the terminal edge; held after an Error; two answers during a compaction and in the unheard-compaction gap; FIFO overflow and revocation | ~1.2k + ~1k tests | medium-high: routing and reconciliation |
| **D2b** | The keyboard layer: the composer chip; Ctrl+J across this window's panes, and its batches; digits, Enter, Tab, Space and Esc; focus back to the composer; the repeat, paint and IME exclusions. It calls D2a's send and adds no delivery path | **Keyboard-first answers** | F8, F13, and the keyboard halves of F1–F7 and F9; the §7 negative tests; the never-resolves-a-prompt test with focus moving between a card and the authorize card; Esc never denies a prompt | ~700 + ~500 tests | medium: key precedence against the composer and the authorize card |
| **D3** | Cross-session presence: the tab dot, rail NEEDS YOU and ASKED for attached sessions, Ctrl+Shift+J, Ctrl+P Waiting, and the OS notification for wait cards | **Never miss a waiting card** | F10–F12 and the presence counters | ~500 | low |
| **D4** | Change answer, Skip polish, and desk-local memory: preselect the last choice for the same question in the same cwd (highlight only) | Smart defaults | F15, and memory tests | ~400 | low |

**Order:** S1 → S2 (cards are usable from here) → S3 → D0 → D1 → D2a → D2b → D3 → D4, with S4 any time after S2. D3 builds on D2b's Ctrl+J order, and D2b uses D2a's send and nothing else of it.

**SynapsCLI:** no PRs. Close #439, #440, #444 and #446 unmerged.

**#56** (the A2UI preview behind `--agent-surface`) is untouched by this design. Removing it, if the preview is no longer wanted, is its own optional PR, not part of D0.

---

## 10. Decisions for Haseeb

1. **Repo visibility.** The desk's CI needs `cards-spec`, and a private git dependency needs a deploy key. **Decided: synaps-cards is public** (Haseeb, after S3 merged). It holds no secrets. The desk takes `cards-spec` as a git dependency pinned by a full `rev`, with no deploy key.
2. **Naming.** The plugin is `cards`, so the model sees `cards_ui`, short and plain. **Recommendation: yes.** Haseeb's install should clone into `~/.synaps-cli/plugins/cards`, and add `ext.cards:ui` to `tools.always_loaded` if disclosure is on.
3. **TUI widget.** v1 is plain text only. A live HUD needs `privacy.llm_content`, to clear it on answer. **Recommendation: plain text.** Revisit only if TUI users ask.
4. **"Always do this."** An auto-answer would send a user message without a user action. **Recommendation: not in v1.** D4 preselects only, and a real "Always" needs its own consent design.
5. **Would need core.** **Recommendation: none now.** If F11 hurts, #1 (the per-session extension status) is the one to propose, as a generic feature other extensions would use too. If unconfirmed answers turn out to be common, #6 (an idempotent, receipted `Submit`) is next; it would help typed input as much as cards.
