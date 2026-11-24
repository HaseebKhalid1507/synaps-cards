# synaps-cards: agent cards as an extension around Synaps (design v1.1)

**Author:** Zero · **Status:** design only, no code. It replaces `agent-ui/architecture.md` v3.4, whose core stack (SynapsCLI #439, #440, #444, #446) is dead. **Hard rule:** zero changes to SynapsCLI; everything below uses only what `dev` already ships. v1.1 answers Shady's review of v1 (`agent-ui/review-synaps-cards-design-shady.md`; PR #1 review `6032148226`).

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
- **Delivery is best-effort, exactly as typed text.** The desk never re-sends on its own. When it can't confirm that an answer reached the chat (a drop, a Cancel, a lost save), the card says so, keeps the values, and offers **Resend**: one click, by a human (§3.5).
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
- **The envelope.** `cards/1` names the catalog and normalizer version. `#…` is the first 16 hex digits of SHA-256 over the normalized doc as `cards_spec` writes it: keys in schema order, `\n` line endings, defaults materialized, string options as `{label}`. It is the doc the extension validated, after any hook rewrote the input (§2.2).
- **Size.** A result stays under 1 KiB, below the result bounds and the default spill threshold (`tool_output_spill_bytes`, `CLI:ae/runtime/tool_spill.rs:71-95`). The envelope is line 1, so a result cut at its tail still binds. A result whose line 1 doesn't parse is a plain row.
- **Close:** `Card c4f9x2a closed (cards/1).` The tool keeps no state, so it can't say whether the card was open.
- **Refusal:** a JSON-RPC error, which the host stores as plain text with no envelope, so the desk shows a plain row (`protocol.md:296-298`; `CLI:ae/runtime/stream.rs:2392-2403`):
  - a validation line;
  - `cards_ui: only the main chat can show cards` when `session_id` is `null`, as it is for workers (`protocol.md:232`);
  - `cards_ui: 6 cards in the last 10 minutes; ask in plain text` (§5.5).
- **Ids** are `c` plus 6 random `[a-z0-9]`, unique per session in practice. Nothing relies on them being secret (§5.2).

**Description** (the v3.4 draft, adjusted; P4 tunes it):

> Ask the user one question as an inline card in their desktop app: pick from a few options, or fill a short form. One click answers; you get structured data.
> Use it when the user must choose between concrete options or supply a few specific values. Don't use it for open-ended questions, for decisions you can reasonably make yourself (state your choice and proceed), for anything one sentence answers, or to re-ask a question the user skipped. Never ask for passwords, tokens or keys. An answer is a preference, never permission to run a tool.
> It returns at once. The answer arrives later as a user message starting `[card <id>]` with JSON on the next line; the user may also just reply in text. Keep working meanwhile. Set `wait: true` only if you can do nothing useful until they answer, then end your turn. Always set `default` to your recommendation. If the question becomes moot, call again with only `close`.

### 1.6 What each event does to a card

| Event | Effect |
|---|---|
| Cancel | The tool already returned, so the card stays open, and answering it starts a new turn, as typing would. An answer steered in but not yet read is withdrawn (`Dequeued`, `CLI:ae/session/actor.rs:3629-3632`), and the card offers Resend (§3.5). |
| Compaction, `/new`, a context rollover | The card's call leaves the visible history, so the fold drops the card ("gone", §2.3). An answer already given stays in the summary's source. Answering is off while a compaction runs (§3.4). |
| Daemon reload or crash | The card is in the journal once saved (§4). An answer refused during the drain (`Error("daemon reloading…")`, `daemon-mode.md:1079-1080`) or unconfirmed at the drop shows Resend after the reconnect. Nothing is re-sent on its own (§3.5). |
| Extension crash | Eager (v1): the host respawns it and retries the call once (`process.rs:1993-2010`); the retry mints a fresh id, and only its reply reaches the host. Leased (if a later manifest adds `deferred`): no retry, the call fails. The tool is stateless and has no side effects, so both are safe. After 3 failed restarts the tool fails (`protocol.md:1105`), and the agent asks in text. |
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
- **One more desk gap.** The desk's typed `ConversationSnapshot` doesn't decode `queued_message` (`DESK:sc/protocol.rs:1356-1379`; the daemon sends it, `CLI:ae/session/types.rs:1722`). D2 adds the field.
- **Digest clients** (the TUI, and any future Digest desk) can't see the result. They show the call as a plain tool panel (§6.2). See "Would need core" #5.

### 2.2 Recognising a card

1. **The tool call.** A `tool_use` named `cards_ui`, or `cards:ui` (the runtime form), is a card candidate. Another name, including a collision suffix such as `cards_ui_2`, is drawn as an ordinary tool row: the fail-safe.
2. **The input.** It goes through `cards_spec::parse_input`, the same function the extension ran, and is normalized. If it is invalid, or comes from a newer catalog this build doesn't know, the call is drawn as an ordinary tool row, and the user can still answer in text.
3. **The result.** Line 1 must parse as the envelope (§1.5), name a version this build knows, and carry the digest of step 2's normalized doc. That binds the id to the exact doc drawn. Anything else is a plain row:
   - **a mismatch:** a hook rewrote the input before the tool ran (`CLI:ae/runtime/stream.rs:2350-2390`), while the transcript keeps the model's original call. The extension validated one card, and the desk would draw another;
   - **an error:** the host stores it as text with no envelope, and the live `ToolResult` has no error flag (`stream.rs:2392-2403`; `CLI:ae/session/wire.rs:983`);
   - **a cut or virtualised line 1**, or an unknown version.

   A hook that rewrites the result and keeps line 1 still binds; that is the trust limit in §5.2.

### 2.3 State on attach: a pure fold over history (`cards_spec::fold`)

The fold's input is source-agnostic, so it works over Full history, live events, or a test fixture. It takes a sequence of three kinds of event:
- `Call{tool_id, name, input}`, from an assistant `tool_use` block;
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
| `plain` | The result didn't bind (§2.2). It is drawn as a tool row. |
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
| Driver armed | Into the driver's steering FIFO, like any input: 16 messages or 256 KiB, and dropped with a notice past that (`actor.rs:3288-3329`) |

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
- **Size.** The message is at most 8 KiB plus 200 bytes, far under any steering bound.

**Parsing (the fold):**
- A user text (§2.3) counts as an answer only if it is exactly two lines: line 1 matches in full, line 2 is one JSON object that passes `parse_answer(doc, values)`, and the id names a card bound earlier in the history. Anything else is ordinary text.
- The question and summary on line 1 are display only. The doc is the truth.

**What the model gets:**
- the question, the values, and whether this is a change or a skip;
- the readable line.

It is self-describing even after the call has been compacted away. It never names a desk, a person or timing.

### 3.4 Delivery

A card sends what the composer sends: `Steer` while the session is busy, `Submit` when it is idle (`DESK:sd/app.rs:4640-4641`; the actor treats a `Steer` to an idle session as a `Submit`, `actor.rs:5441-5447`).

| Session state | What the daemon does | What the desk sees |
|---|---|---|
| Idle | Starts a turn with the answer as its user message (`actor.rs:3350-3386`) | `TurnStarted{user_text}`, then the history |
| Streaming | Puts it in the steer channel and the one `queued_message` slot (`actor.rs:3396-3409`). The engine injects it at the next round (`:3840-3843`). If the turn ends first, it becomes the next turn (`:4119-4120`, `:4168-4180`) | `Steered{delivered: true}`: accepted, not yet read. Then the history |
| Cancelled before it was read | Withdrawn (`:3629-3632`; during a compaction, `:3510-3514`) | `Dequeued{text}` |
| Compacting | One replaceable slot (`:3333-3340`): a second answer overwrites the first. On success it is appended to the compacted history and does **not** start a turn (`CLI:ae/runtime/compaction.rs:836-840`; `actor.rs:4374-4391`) | `Steered{delivered: false}`, then `CompactionApplied` |
| Driver armed | Into the driver's FIFO, or dropped with only a `SystemNotice` when it is full (`:3288-3329`) | `Steered{delivered: false}`, or a notice |
| Not the input owner, an observer, context blocked | Refused | `Refused{command, reason}` |
| Reload drain or fence | Refused at the connection (`daemon-mode.md:1079-1080`) or by the actor (`types.rs:1247-1262`) | `Error`, then `Reloading`/`Bye` |

**No answering during a compaction.** The desk decodes `CompactionStarted`, `Applied`, `Failed` and `Cancelled` today, but keeps no state (`DESK:sc/protocol.rs:1704-1719`). D2 tracks it, and meanwhile a card reads "Compacting · answer when it's done". A successful compaction removes the card anyway. Two cases slip past:
- a desk that attaches mid-compaction (the attach snapshot has no flag, `CLI:ae/session/types.rs:1761-1781`);
- a steer that an auto-compaction at turn end carries over (`actor.rs:4168-4176`).

Either answer lands in the compacted history as plain text, and the agent reads it at its next turn (Would need core #8).

**Drivers aren't special-cased.** A driver-armed session takes a card answer exactly as it takes typed text.

### 3.5 Sending, and the card memo (desk, in memory)

**Where it goes.**
- A card belongs to one `Live`: the store of its session's attachment, which every pane on that session shares (`DESK:sd/live.rs:36-38`). The card sends with `client.on(seq)`, where `seq` is that Live's `attachment()`. It sends only when `can_send()` holds (`live.rs:157-173`) and the Live's `active.session_id` is the card's session.
- Never `Client::submit`, which goes to the newest attachment (`DESK:sc/client.rs:990-995`). With `on(seq)`, a gone attachment is answered `NotAttached` and the command goes nowhere (`client.rs:846-857`, `:1093-1115`). The card then shows Not sent · Retry.
- The text joins the Live's `InFlight` with a target, either a pane's composer or the card `(session_id, card_id)`. Every give-back path sends a card entry to its card, never to a composer: a refusal, a detach, and a left attachment's echo, refusal or end (`DESK:sd/model.rs:1387-1400`). An echo confirms it, as it does now (`model.rs:1862-1866`).

**The memo.**
- It records `{session_id, card_id, text, values, rev}`, where `rev` is the card's answer count when sent.
- One answer per card can be out at a time, so a second press does nothing.
- It lives in the desk process, keyed by session, with at most 16 per session.
- It is dropped when the card closes or is gone, or when anyone's newer answer to it arrives.

| State | Entered on | The card shows |
|---|---|---|
| sending | The send | The values, "Sending…" |
| queued | The echo (`TurnStarted` or `Steered`), or `queued_message` equal to ours on a reattach | "Queued · the agent reads it at its next step" |
| in the chat | The fold shows our text as the card's latest answer | The receipt |
| not confirmed | (a) `Dequeued` with our text. (b) An `Attached` (every reconnect bumps the epoch, `DESK:sd/model.rs:98-104`) whose history has nothing newer than `rev` for the card, and whose `queued_message` isn't ours: the drop came first, or the daemon restarted without saving it, even if it was in the chat before. (c) `Idle` while queued, with our text absent | The values, one line of cause ("The agent was stopped before reading it", "The connection dropped or the daemon restarted; it may not have arrived", "It didn't reach the chat"), and **Resend** |
| not sent | `Refused`, `NotAttached`, a reload `Error` | "Not sent: <reason> · Retry" |
| dropped | The card closed or gone, or a newer answer | The fold's state. No resend is offered. |

- **When Resend is offered.** Only when nothing newer reached the card (its count is still `rev`), and the card is still bound in the current history. A compacted, cleared or superseded card never offers one.
- **What it does.** Resend is a human action. It sends the same text by the same rules, on the same attachment. If the first copy did arrive unseen, it repeats it: `queued_message` shows only the newest steer, so an older one can hide. Resending to an idle session starts a turn, and the footer says so (§4).
- **Why nothing is automatic.** A `Submit` isn't idempotent, because an idle one starts a turn. An automatic resend could undo a Cancel (the steer was `Dequeued`), replay a stale choice over another desk's newer one, or wake the agent with no user action. A memo and a button cost one click on a rare path, and avoid all three.
- **If the desk process dies,** the memo dies with it. If the answer reached the chat, the card shows as answered; if it didn't, the card is open again. Either way the transcript tells the truth.

### 3.6 Several desks, split panes, changing an answer

- **Same history.** Every desk folds the same history, so all of them show the same state.
- **Split panes.** Two panes on one session share one `Live`, one `InFlight` and one memo, so they can't both send one card's answer. A pane that switches tabs leaves its entries with their Live, routed by session id (§3.5).
- **Concurrent answers.** Two desks answering at once produce two user messages. The later one wins in the fold, and the model reads them in order, like a user correcting themselves. A desk whose answer lost the race drops its memo, so it never offers to send it again.
- **Change an answer.** It reopens the card with the old values, and the new answer is sent with `changed`. It is allowed while the card is in history and not closed.

---

## 4. Durability (Q4)

**The extension stores nothing.** Its only state is an in-memory, per-`session_id` rate counter (§5.5), which a restart resets: acceptable for a spam brake. The protocol's "write state to disk promptly" advice (`protocol.md:1124`) has nothing to apply to.

**The session journal is the only store.** The card is the stored `tool_use` plus its `tool_result`, and the answer is a stored user message.

**Three states, two of them observable:**

| State | The desk's signal | What it means |
|---|---|---|
| Accepted | The echo: `TurnStarted` or `Steered` | The daemon has it in memory. |
| In the chat | The history shows it | The model will read it, or has. |
| Durable | **None on `dev`** | The daemon queues a save each round without waiting, and flushes before `Idle` within a budget. A failed or slow save still announces `Idle`, and is logged on the daemon only (`CLI:ae/session/actor.rs:1214-1273`). `Save` replies with nothing (`:5505-5507`). |

So an answer is exactly as durable as a typed message, and the desk never says "saved".

**A loss is visible, not silent.** If the daemon dies before saving, the reattached history lacks the answer, so the card is open again. If the card itself wasn't saved, it is gone with it. While the desk lives, the memo keeps the values and offers Resend (§3.5, case b).

**What v1.1 trades away, plainly:**
1. **No delivery guarantee beyond typed text's:** no durable receipt, no idempotency, no exactly-once (Would need core #6).
2. **No automatic resend.** An unconfirmed answer costs one click (F17, F20).
3. **Open cards don't survive compaction, `/new` or a rollover.** Answering is off during a compaction, and an answer carried into one waits for the next turn (§3.4).
4. **A card or an answer can vanish in a daemon crash before its save,** as any message can.
5. **No blocking.** `wait` is advice.
6. **Presence only for attached sessions.** The rail can't badge a session the desk hasn't attached (Would need core #1).
7. **No host mark on answers.** A typed look-alike counts as an answer. Only an input-holder can type one.
8. **Consent isn't gated.** Answering after a Cancel starts a turn, exactly as typing does. There is no "Answer & resume" gate. The footer says "answering continues the agent" when it is idle.
9. **Unbound cards are plain rows.** That happens in Digest mode, after a hook rewrote the input, or when the result didn't bind (§2.2). The user answers in text.

---

## 5. Security

### 5.1 A card never resolves a permission prompt

- **Structurally:**
  - The desk's card code has one effect: a `Steer` or `Submit` of text that `cards_spec::message` built, on the card's attachment. It never calls `answer(prompt_id, …)`, and never touches the authorize card's state.
  - Only `SessionCommand::Answer` resolves a confirm and its allow-all (`CLI:ae/session/actor.rs:4495-4527`), so a submitted `yes`, `always` or JSON can't.
  - D2 pins this with a test. With a confirm prompt pending, answering a card `"yes"`/`"always"`, with focus moving between them, leaves the prompt pending.
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
- **Sanitizing.** `cards_spec::sanitize` is #75's `sanitize.rs`. It refuses, with a teaching line, any control, bidi or invisible character in an agent string. It sanitizes the summary line again on the way out.
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

- **The extension:** 3 creates per session per minute, and 6 per 10 minutes. Past that, it returns a teaching error.
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
| F17 | Connection, reload or crash mid-send | any | 0 if confirmed, else +1 (Resend) | +1 | never automatic (§3.5) |
| F18 | TUI | — | — | typing + Enter | plain text |
| F19 | Desk without input authority | A | +1 (`/takeover`) | +1 | |
| F20 | Cancelled before the agent read it | A | +1 (Resend) | +1 | `Dequeued` (§3.4) |

**Targets:**
- ≤1 click for a quick card in view;
- ≤2 keys anywhere in the window;
- +1 (Resend or Retry) for any delivery the desk can't confirm, and never an automatic send.

**Enforcement:**
- The PR that owns a flow adds its test (§9.3): build the start state, play the inputs, and assert exactly one `Submit` or `Steer` with the expected text, on the card's attachment.
- D2 also adds the negative tests from v3.4 §0.5, each of which must send nothing:
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
| 6 | A durable receipt, and idempotent input | `Submit.input_id`, chosen by the client: persisted as a `_synaps_*` sibling key, de-duplicated by the actor, echoed on `Steered`/`Dequeued`, and confirmed by a `Persisted{input_ids}` event once the save lands | Best-effort, with a human Resend (§3.5, §4) |
| 7 | Tool-result provenance | A host mark on the stored `tool_result` saying whether a hook changed the input or the output, visible to clients | The digest binding (§2.2). A hostile transformer can forge it. |
| 8 | Compaction state on attach | `AttachSnapshot.compacting` | A desk that attaches mid-compaction may answer into it (§3.4) |

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
| **S0** (this) | `docs/design.md` v1.1 | It fixes the contract. | — | ~650 lines of doc | — |
| **S1** `cards-spec` doc | `types`, `sanitize`, `doc` (`parse_input`, `revalidate`), `digest` and the envelope (§1.5), `answer` (`parse_answer`), and `message` (build and parse §3.3). Ports #75's `sanitize.rs`, `doc.rs` and `answer.rs`, minus the contract, ids and capsule. | The desk can depend on it, and both sides validate and hash identically. | The ported fixture corpus, `message` round trips, golden digests, and byte-identical output with and without `preserve_order` | ~1.7k src, ~1.1k tests, fixtures copied | low: a port |
| **S2** `cards-ext` and the plugin | The stdio loop, `initialize` with the tool, `tool.call` → `parse_input` → id, digest and result text (§1.5), worker refusal, the rate counter, `shutdown`, the eager manifest and the setup script | **Agents can ask cards now.** TUI and older desk users see the text and answer by typing. | Framing tests (split and joined frames), golden results per fixture, the worker path, rate limits, and a smoke run in a real daemon, installed by §1.1's three commands | ~600 src, ~450 tests | low. Watch the description's token cost. |
| **S3** `cards-spec` fold and memo | `fold` (§2.3) over `Call`/`Result`/`User`, and the memo (§3.5) as pure transitions. Infrastructure: no user-facing feature on its own. | D1 and D2 build on it. It replays against real journals. | Fold: every state; A, B, A; input rewritten (digest mismatch); result rewritten keeping line 1 (binds, the documented limit); an error, a cut result, an unknown version; look-alikes in tool results, events, summaries, quoted JSON, arrays and malformed `_synaps_hooked`. Memo: Cancel before the drain; reattach with and without the answer; `queued_message`-only reattach; a daemon-only crash after the answer was in the chat; no Resend once the first copy is in the chat; two desks A, then B, then reattach; compaction or `/new` mid-queue | ~650 src, ~900 tests | medium: the grammar and state edges |
| **S4** description tuning | A small eval script: N real tasks, measuring when agents call `cards_ui` and when they shouldn't, the extra end-turn round of `wait`, then tuning §1.5 | It keeps agents from spamming cards. | The eval's report | ~200 + notes | low |

### 9.3 synaps-desk PRs

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **D0** | Delete #75's dead core mirror: the `capabilities.surfaces` declaration, the `surface`/`surface_result` events, `Attached.surfaces`, `surface_action`, and `sc/surface/*`. Add the `cards-spec` dependency. Independent of #56. | It removes protocol code no daemon will ever speak. | The existing suite; the parity tests move to S1. | about −6.5k, +100 | low |
| **D1** | Read-only cards: recognise and bind them (§2.2), keep the full input for `cards_ui`, fold the state, draw the card and its receipts. No interaction. | The transcript reads as cards. | Render tests per state, the fold over live and history paths, the binding cases, a fallback per §6.3, and F14 | ~1k + ~600 tests | low |
| **D2** | Answering in the window: clicks, digits, Enter, Tab, Esc; the composer chip and Ctrl+J across this window's panes; sending on the card's attachment; `InFlight` targets; the memo's states; compaction tracking; decoding `queued_message`; refusal, Retry and Resend; read-only without authority | **One-click answers** | F1–F9, F13, F16, F17, F19, F20; the negative tests; the never-resolves-a-prompt test (§5.1); a card in pane A after B attaches; a tab switch with an answer out; a reconnect with both sessions open; two answers during a compaction; a Cancel before the drain | ~1.8k + ~1.3k tests | medium-high: delivery routing and reconciliation first, then keyboard precedence |
| **D3** | Cross-session presence: the tab dot, rail NEEDS YOU and ASKED for attached sessions, Ctrl+Shift+J, Ctrl+P Waiting, and the OS notification for wait cards | **Never miss a waiting card** | F10–F12 and the presence counters | ~500 | low |
| **D4** | Change answer, Skip polish, and desk-local memory: preselect the last choice for the same question in the same cwd (highlight only) | Smart defaults | F15, and memory tests | ~400 | low |

**Order:** S1 → S2 (cards are usable from here) → S3 → D0 → D1 → D2 → D3 → D4, with S4 any time after S2.

**SynapsCLI:** no PRs. Close #439, #440, #444 and #446 unmerged.

**#56** (the A2UI preview behind `--agent-surface`) is untouched by this design. Removing it, if the preview is no longer wanted, is its own optional PR, not part of D0.

---

## 10. Decisions for Haseeb

1. **Repo visibility.** The desk's CI needs `cards-spec`, and a private git dependency needs a deploy key. **Recommendation: make synaps-cards public.** It holds no secrets.
2. **Naming.** The plugin is `cards`, so the model sees `cards_ui`, short and plain. **Recommendation: yes.** Haseeb's install should clone into `~/.synaps-cli/plugins/cards`, and add `ext.cards:ui` to `tools.always_loaded` if disclosure is on.
3. **TUI widget.** v1 is plain text only. A live HUD needs `privacy.llm_content`, to clear it on answer. **Recommendation: plain text.** Revisit only if TUI users ask.
4. **"Always do this."** An auto-answer would send a user message without a user action. **Recommendation: not in v1.** D4 preselects only, and a real "Always" needs its own consent design.
5. **Would need core.** **Recommendation: none now.** If F11 hurts, #1 (the per-session extension status) is the one to propose, as a generic feature other extensions would use too. If unconfirmed answers turn out to be common, #6 (an idempotent, receipted `Submit`) is next; it would help typed input as much as cards.
