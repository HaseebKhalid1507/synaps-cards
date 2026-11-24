# synaps-cards: agent cards as an extension around Synaps (design v1)

**Author:** Zero · **Status:** design only, no code. It replaces `agent-ui/architecture.md` v3.4, whose core stack (SynapsCLI #439, #440, #444, #446) is dead. **Hard rule:** zero changes to SynapsCLI; everything below uses only what `dev` already ships.

**Read at:**
- SynapsCLI `origin/dev` `189a5a19`, cited as `CLI:`. `ae/` is `crates/agent-engine/src/`, and `docs/` is the CLI's docs.
- synaps-desktop `origin/main` `3f61cff` (#75 merged), cited as `DESK:`. `sc/` is `crates/synaps-client/src/`, and `sd/` is `crates/synaps-desktop/src/`. Every anchor was re-read at those commits.

---

## TL;DR

- **The extension.** A plugin named `cards` registers one tool, `ui`, which the model sees as `cards_ui`. It needs only the `tools.register` permission: no hooks, no conversation access, no tool interception.
- **The tool never blocks.** It validates the card, mints an id, and returns at once. Blocking a `tool.call` is not viable on `dev` (§1.2).
- **The desk draws the card from the transcript.** Its input is the `cards_ui` tool call, and the tool result says whether it went live.
- **The answer is an ordinary user message** that the desk sends with `Submit`, in a fixed two-line grammar: a readable line, then the values as JSON. The daemon already enforces who may send input. It already forwards over SSH, and already shows every attached client what was sent. So answering needs no new socket and no new protocol.
- **What happens on delivery:**
  - an idle agent wakes on the answer, because a `Submit` starts a turn;
  - a busy agent gets it at its next round, because a `Submit` while streaming becomes a steer.
  - "Wait" therefore means "end your turn now".
- **History is the only store.** Card state is a pure fold over the session history: the call, its result, and the answer messages. The extension stores nothing.
  - Delivery is at-least-once.
  - A rare duplicate is the same answer twice, which is harmless and folds to one.
- **Two crates, one validator.** `cards-spec` holds the doc schema, validation, the answer grammar, the fold and the outbox, ported from #75's pure logic. The extension and the desk both depend on it.
- **The TUI** gets a plain-text fallback: the tool panel shows the question with numbered options, and the user answers in words.

---

## 0. What carries over, what's dropped

| Carried over (from v3.4 and #75) | Dropped (it existed only because the core owned state) |
|---|---|
| The friction budget, now re-counted (§7) | The `surface`/`surface_result` wire, `Attached.surfaces` and capability gating |
| The card schema: one question per call, options, a small form (§1.4) | The sidecar store, the answer ledger, tombstones, and the commit pipeline |
| The keyboard flow: no auto-focus, Ctrl+J entry, digits, Enter on the default, Tab, Esc (§6.1) | The contract digest, CAS, epochs, and "Answer & resume" consent |
| "A card never approves anything" (§5.1) | Exactly-once delivery and the reload fence |
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
- **Building and installing.** The process is a Rust binary built by `extension.setup`. `extension.command` points at the plugin-relative `target/release/cards-ext` (`CLI:docs/extensions/plugin-setup-scripts.md:5-30`). Install is `git clone … ~/.synaps-cli/plugins/cards` (`CLI:docs/extensions/README.md:47-53`).

### 1.2 Can `tool.call` block until the user answers? No. These are the facts on `dev`:

| Fact | Where |
|---|---|
| The host caps `tool.call` at **120 s**, with no configuration (30 s for other methods). A leased call is capped the same way. | `CLI:ae/extensions/runtime/process.rs:1937-1953`, `:1928-1933` |
| One `call_lock` **serializes every request to an extension process** for the whole exchange. | `process.rs:708-710`, `:1957-1958` |
| Hooks go through the same lock, and their 5 s budget includes waiting for it. A blocked card would make every hook of this extension in **every session** fail open after 5 s. | `process.rs:2434-2443`; fail-open at `protocol.md:1094-1105` |
| In daemon mode, one extension process serves every session. One blocked card queues all the others. | `protocol.md:236-240` |
| **Cancel** drops the tool future (`select!` on the cancel token). The protocol has no cancel message, so the extension is never told, and its late reply is discarded. | `CLI:ae/runtime/stream.rs:120-137`; the methods at `protocol.md:68-343` |
| **Crash:** the host respawns the process and **retries the in-flight request** once. A blocking card would be raised twice. | `process.rs:1993-2010`; `protocol.md:1114-1124` |
| **Compaction:** a blocked call keeps the turn running, and `/compact` is refused while a turn runs. | `CLI:ae/session/actor.rs:4270-4275` |
| **Daemon reload** first waits for every session to go idle (≤ `--drain-secs`). It then cancels each running turn and stops the sidecars. A blocked card holds the reload, then dies. | `CLI:docs/daemon-mode.md:1079-1085`, `:1093` |
| The extension cannot push into a session, and has no request it can make that delivers an answer. Its inbound methods are `memory.*` and `config.*` only. | `process.rs:1254-1345`; "does not initiate calls", `protocol.md:70` |
| An answer sent as a user message while the tool blocks is only a steer, delivered after the tool round ends. Blocking on (b)'s path would deadlock. | `CLI:ae/runtime/helpers.rs:136-158`; `actor.rs:3282-3305` |

**What replaces blocking:**
- `ui` returns at once.
- The answer arrives later as a user message (§3).
- `wait: true` tells the agent to end its turn after the call, and the answer starts its next turn.

It costs the same number of provider requests as a blocked call would: one request after the answer either way. Nothing is ever in flight, so Cancel, compaction, reload and crash have nothing to lose (§1.6).

### 1.3 Modes

| Mode | When the agent uses it | What it does | What the user sees |
|---|---|---|---|
| **Live** (default) | It can keep working. | It keeps working. If the answer comes mid-turn, it is steered in at the next round. If it comes after the turn ended, it starts a turn. | An open card. |
| **Wait** (`wait: true`) | It can't proceed without the answer. | The result tells it to end its turn now. The answer starts the next one. | NEEDS YOU: the composer chip, a tab dot, the rail for attached sessions, and an OS notification (§6.1). |

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
- **Success** is plain text. Its **first line is the machine anchor** the desk reads, `Card <id> is showing: <question>`. Then come numbered options, the field names, and the instruction:

  ```
  Card c4f9x2a is showing: Which approach?
    1. Refactor the parser (suggested)
    2. Rewrite from scratch
  Its answer will come as a user message starting "[card c4f9x2a]", values as JSON on the next line.
  The user may also just reply in text. Keep working meanwhile.
  ```

  With `wait: true`, the last line instead reads: `You can't continue without it: end your turn now; the answer starts your next turn.`
- **Close:** `Card c4f9x2a closed.` The tool keeps no state, so it can't say whether the card was open.
- **Refusal:** a JSON-RPC error, which the host turns into a tool failure (`protocol.md:296-298`):
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
| Cancel | Nothing: the tool already returned. The card stays open, and answering it starts a new turn, just as typing would. |
| Compaction, `/new`, a context rollover | The card's call leaves the visible history, so the fold drops the card ("gone", §2.3). An answer already given stays in the summary's source. |
| Daemon reload or crash | Nothing to lose: the card is in the journal. A desk answer sent during the drain is refused, with `Error("daemon reloading…")` (`daemon-mode.md:1079-1080`), and is re-sent after the reconnect (§3.5). |
| Extension crash | The host respawns it and retries the call (`process.rs:1993-2010`). The tool is stateless, so the retry is harmless. After 3 failed restarts the tool fails (`protocol.md:1105`), and the agent asks in text. |
| Extension reload or uninstall | New calls fail. Existing cards still answer, because answers don't touch the extension. |

---

## 2. Rendering (Q2)

### 2.1 What a client receives (verified)

| Path | Tool input | Tool result | Answer messages |
|---|---|---|---|
| **Live, any mode** | `Stream(Llm(ToolUse{tool_name, tool_id, input: Value}))`: **full JSON**. `ToolUseStart`/`ToolUseDelta` come first and are partial. | `ToolResult{tool_id, result: String}` | `TurnStarted{user_text}` when idle, `Steered{text}` when streaming, and then the history |
| **Attach, Full** (`CLI:docs/daemon-mode.md:742-774`) | `Attached.conversation.api_messages`: the stored `tool_use` block, in full | the stored `tool_result` block | user messages in the history |
| **Attach, Digest** (`daemon-mode.md:775-790`) | `display_tail` `ToolUse{input}`: the full input, as a JSON string | **absent**: tool-result arrays are not projected | `User` items |

**Sources and notes:**
- **Wire types.** `CLI:ae/session/wire.rs:982-983` (variants), `:1098-1099` (conversion). `forwarded()` drops only `MessageHistory` for Digest (`CLI:ae/daemon/conn.rs:949-957`).
- **The display tail.** `CLI:ae/session/display.rs:17-23` defines the items. Tool-use input is kept in full (`:111-116`). Event payloads and non-string user content are skipped (`:83-94`). The tail is capped at `tail_items` with an `omitted` count.
- **The desk runs Full.** That is `Identity`'s default (`DESK:sc/conn.rs:59-60,73-74`), and no desktop code calls `with_history`. So it has the input and the result, live and on attach.
- **One desk gap.** `synaps-client` cuts a tool's `input` to a 4 KiB preview (`DESK:sc/transcript.rs:63,1223`; live at `:1516`), but a card input may be up to 8 KiB. D1 keeps the full input for `cards_ui` calls; the spec's 8 KiB bound limits it. This is a desk-only change.
- **Digest clients** (the TUI, and any future Digest desk) can't see the result. They show the call as a plain tool panel (§6.2). See "Would need core" #5.

### 2.2 Recognising a card

1. **The tool call.** A `tool_use` named `cards_ui`, or `cards:ui` (the runtime form), is a card candidate. Another name, including a collision suffix such as `cards_ui_2`, is drawn as an ordinary tool row: the fail-safe.
2. **The input.** It goes through `cards_spec::parse_input`, the same function the extension ran. If it is invalid, or comes from a newer catalog this build doesn't know, the call is drawn as an ordinary tool row, and the user can still answer in text.
3. **The result.** It must be a success result whose first line matches `^Card (c[a-z0-9]{6}) is showing: `. That binds the id. If the result is an error, or another extension's output `replace` rewrote it, the call is not answerable and is shown as a plain row.

### 2.3 State on attach: a pure fold over history (`cards_spec::fold`)

The fold's input is source-agnostic, so it works over Full history, live events, or a test fixture. It takes a sequence of three kinds of event:
- `Call{tool_id, input}`;
- `Result{tool_id, text, is_error}`;
- `User{text}`. This is the user's own text, with the trailing extension blocks counted by `_synaps_hooked` excluded (`protocol.md:838-860`).

| State | Rule |
|---|---|
| `pending` | A call without a result yet (milliseconds). It is drawn, but not answerable. |
| `open` | A successful create result, and no answer yet. |
| `answered{values, rev, at}` | The latest **valid** answer message to that id that comes after the card (§3.3). `rev` counts the answers. "Latest wins" also covers concurrent desks and changing an answer later. |
| `skipped` | The latest answer is a skip. |
| `closed` | A later `cards_ui {close: id}` call that succeeded. An answer already given keeps its receipt. |
| `failed` | The create's result is an error. |
| *(absent)* | The call is no longer in the visible history (compaction, `/new`, rollover): the card is gone. |

There is no clock-based expiry. A card is moot when the agent closes it, the user skips it, or the history drops it.

**Late-attach semantics:**
- A desk that attaches later computes exactly what every other desk shows, from the same history.
- A mid-turn attach gets the call and result from the attach replay ring (`Attached.replay`), as for any tool row.
- An answer still queued as a steer shows in `ConversationSnapshot.queued_message` (`CLI:ae/session/types.rs:1702-1722`). The fold overlays it as `sending`.

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
| Driver armed | Into the driver's steering FIFO, like any input: 16 messages or 256 KiB (`actor.rs:3282-3305`) |

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
- A user text counts as an answer only if line 1 matches, the id names a card earlier in the history, and line 2 passes `parse_answer(doc, values)`. Anything else is ordinary text.
- The question and summary on line 1 are display only. The doc is the truth.

**What the model gets:**
- the question, the values, and whether this is a change or a skip;
- the readable line.

It is self-describing even after the call has been compacted away. It never names a desk, a person or timing.

### 3.4 Delivery

| Session state | What `Submit` does | Confirmation the desk sees |
|---|---|---|
| Idle | Starts a turn, so the answer wakes the agent | `TurnStarted{user_text}` |
| Streaming | Steers: it becomes its own user message at the next round (`actor.rs:3282-3305`, `3396-3409`; `helpers.rs:136-158`) | `Steered{text, delivered}`, then the history |
| Compacting | Queued until the compaction lands | `Steered{delivered: false}` |
| Context blocked, not the input owner, or owned | Refused | `Refused{client, command: "submit", reason}` |
| Reload drain or fence | Refused at the connection (`daemon-mode.md:1079-1080`) or by the actor (`types.rs:1247-1262`) | `Error`, then `Reloading`/`Bye` |

### 3.5 The desk outbox (`cards_spec::outbox`, pure, in memory)

It reuses the desk's existing in-flight input queue:
- `InFlight` keeps input that hasn't been confirmed, oldest first (`DESK:sd/live.rs:301-330`).
- An echo confirms it (`DESK:sd/model.rs:1862-1866`).
- A refusal gives the oldest back (`:368-380`).

A card answer is one more entry in that queue. Its "give back" target is the card, not a composer.

1. **Send.** A click or key builds the message: `changed` if the fold says the card is answered, `skipped` for Skip. The desk then:
   - pushes it to `InFlight`;
   - calls `Client::submit(text)` (`DESK:sc/client.rs:992-995`);
   - shows the card as `sending`.

   One answer per card can be in flight. A second press does nothing.
2. **Accepted.** An echo with the identical text means the daemon took it, and the card shows the answer. The entry stays in the outbox **until the message appears in the history**. A steer is in memory until its round, and a daemon crash in that window loses it.
3. **Refused.** The values return to the card: "Not sent: <reason> · Retry". This costs 1 click (F16).
4. **Reconnect** (a reload, a crash, a network drop). After `Attached`, the desk checks each entry:
   - if the text is in the history, or is `queued_message`, the entry is done;
   - otherwise it is re-sent **once**, automatically;
   - a second miss shows Retry.
5. **The desk process dies.** The outbox is gone with it. Either the answer had reached the history, or the card is still open and the user answers again.

**At-least-once.** A duplicate needs every one of these:
- two steers were queued together, and a later one hid ours from `queued_message`;
- the connection dropped before the round;
- our steer was still delivered afterwards.

The result is the same text twice. The fold treats a repeat as one answer, and the model sees a repeated statement of the same choice.

### 3.6 Several desks, split panes, changing an answer

- **Same history.** Every desk folds the same history, so all of them show the same state. A pane is a view of its session's fold, so two panes on one session can't both send: there is one outbox per session.
- **Concurrent answers.** Two desks answering at once produce two user messages. The later one wins in the fold, and the model reads them in order, like a user correcting themselves.
- **Change an answer.** It reopens the card with the old values, and the new answer is sent with `changed`. It is allowed while the card is in history and not closed.

---

## 4. Durability (Q4)

**The extension stores nothing.**
- Its only state is an in-memory, per-`session_id` rate counter (§5.5).
- A restart resets it, which is acceptable for a spam brake.
- The protocol's "write state to disk promptly" advice (`protocol.md:1124`) has nothing to apply to.

**The session journal is the only store.**
- The card is the stored `tool_use` plus its `tool_result`.
- The answer is a stored user message.
- The daemon already makes both durable, reloads them and replays them.

**Never lost.**
- Until the answer appears in the history, the desk's outbox holds it and re-sends it after a reconnect (§3.5).
- The only way to lose an answer is for the desk process and the daemon to fail together before its round. Even then, the user sees the card still open.

**Never double-applied, in effect.**
- An answer is a declarative value, not a command, so a duplicate changes nothing.
- The fold deduplicates identical answers.
- The agent reads "the user chose X" twice.

**What this trades away, plainly:**
1. **Exactly-once.** A rare duplicate message is possible (§3.5).
2. **Open cards don't survive compaction, `/new` or a rollover.** The call leaves the visible history, so the card can't be answered by click. The user can still answer in text.
3. **No blocking.** `wait` means "end your turn".
4. **Presence only for attached sessions.** The rail can't badge a session the desk hasn't attached (Would need core #1).
5. **No host mark on answers.** A typed look-alike counts as an answer. Only an input-holder can type one.
6. **Consent isn't gated.** Answering after a Cancel starts a turn, exactly as typing does. There is no "Answer & resume" gate. The footer says "answering continues the agent" when it is idle.
7. **No answer to an unbound card.** In Digest mode, or if another extension rewrote the result, the card can't bind its id and can't be answered by click.

---

## 5. Security

### 5.1 A card never approves anything

- **Structurally:**
  - The desk's card code has one effect: `Client::submit(text)`, with text that `cards_spec::message` built. It never calls `answer(prompt_id, …)`, and never touches the authorize card's state.
  - D2 pins this with a test. With a confirm prompt pending, answering a card `"yes"`/`"always"` leaves the prompt pending.
- **The extension** has `tools.register` only. It can't block, modify or confirm a tool call, which needs `tools.intercept` (`protocol.md:1064-1072`). It sees no conversation.
- **Approvals stay separate.** An answer has a typed message's authority and no more. Tool confirmations still go through the host's own prompt (`protocol.md:591-606`), which no message can answer.
- **Keys.** Agent cards bind none of the authorize card's keys: Ctrl/Cmd+Enter is Allow, bare Esc is Deny, and arrows and PgUp/PgDn scroll (`DESK:sd/app.rs:1737-1749`).
  - Leaving card mode returns focus to the composer, never to the window, so a second Esc can't deny a prompt (v3.4 §4.1).
  - Ctrl+J visits authorize prompts first.

### 5.2 Provenance and sanitizing

- **Chrome.** The desk draws it: "◇ The agent asks", the session, and the card frame. Agent strings never appear in the chrome. Cards never use the authorize card's warning colours or icon.
- **Sanitizing.** `cards_spec::sanitize` is #75's `sanitize.rs`. It refuses, with a teaching line, any control, bidi or invisible character in an agent string. It sanitizes the summary line again on the way out.
- **Detail** is markdown-lite: bold, italic, inline code and lists. A link shows as plain text.
- **Ids are not credentials.** Authority comes from the daemon's input rules (§3.2). A guessed id lets you do only what typing already lets you do.
- **A spoofed `cards` plugin gains nothing.** A project-local plugin named `cards` overrides the user's (`docs/extensions/README.md:53`), but cards carry no power. A spoof gets what any message gets, while running code it could already run.

### 5.3 Bounds

- **Per card:** §1.4.
- **Per answer:** 8 KiB, nested at most 3 deep. A text field is ≤200 characters, a long field ≤2000, and a note 1–500.
- **Per session:** at most 3 open cards are drawn expanded, and older open ones collapse to "+2 more questions".
- **Everything is validated twice:** in the extension, and in the desk's fold.

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

A count runs from the moment the card appears until the agent is unblocked: the `Submit` was accepted.

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
| F17 | Connection, reload or crash mid-send | any | **0** | **0** | auto re-send (§3.5). It was +1 in v3.4. |
| F18 | TUI | — | — | typing + Enter | plain text |
| F19 | Desk without input authority | A | +1 (`/takeover`) | +1 | |

**Targets:**
- ≤1 click for a quick card in view;
- ≤2 keys anywhere in the window;
- 0 for every failure the outbox can absorb.

**Enforcement:**
- D2 adds one desk test per row: build the start state, play the inputs, and assert exactly one `Submit` with the expected text.
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
| 4 | Telling a desk answer from typed text | `Submit.meta`, persisted as a `_synaps_*` sibling key the way `_synaps_hooked` is (`protocol.md:838-860`), and visible to clients | The grammar plus input authority (§3.1) |
| 5 | Card state for Digest clients | A `DisplayItem::ToolResult{tool_id, preview}` in `display_tail` | The desk stays Full. Digest clients get the plain panel. |

None of these blocks v1. Revisit #1 first if F11 hurts in use.

---

## 9. The repo and the PR plan (Q6)

### 9.1 Layout

```
synaps-cards/                       (also the plugin root: clone as ~/.synaps-cli/plugins/cards)
  .synaps-plugin/plugin.json        name "cards", tools.register, setup → scripts/setup.sh
  scripts/setup.sh                  cargo build --release -p cards-ext
  Cargo.toml                        workspace
  crates/cards-spec/                lib, no I/O: serde, serde_json, unicode tables; no tokio, no gpui
    src/{types,sanitize,doc,answer,message,fold,outbox}.rs
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
| **S0** (this) | `docs/design.md` | It fixes the contract. | — | ~550 lines of doc | — |
| **S1** `cards-spec` doc | `types`, `sanitize`, `doc` (`parse_input`, `revalidate`), `answer` (`parse_answer`), and `message` (build and parse §3.3). Ports #75's `sanitize.rs`, `doc.rs` and `answer.rs`, minus the contract, ids and capsule. | The desk can depend on it, and both sides validate identically. | The ported fixture corpus, `message` round trips, and byte-identical output with and without `preserve_order` | ~1.6k src, ~1k tests, fixtures copied | low: a port |
| **S2** `cards-ext` and the plugin | The stdio loop, `initialize` with the tool, `tool.call` → `parse_input` → id + result text (§1.5), worker refusal, the rate counter, `shutdown`, the manifest and the setup script | **Agents can ask cards now.** TUI and older desk users see the text and answer by typing. | Framing tests (split and joined frames), golden results per fixture, the worker path, rate limits, and a smoke run in a real daemon | ~600 src, ~400 tests | low. Watch the description's token cost. |
| **S3** `cards-spec` fold and outbox | `fold` (§2.3) over `Call`/`Result`/`User` events. `outbox` (§3.5) transitions as pure functions. | The desk's D1 and D2 build on it. It is replayable against real journals. | Transcript fixtures: open, answered, changed, skipped, closed, failed, gone after compaction, concurrent desks, look-alikes, `_synaps_hooked` messages, and reconnect/resend | ~500 src, ~600 tests | medium: the grammar edges |
| **S4** description tuning | A small eval script: N real tasks, measuring when agents call `cards_ui` and when they shouldn't, then tuning §1.5 | It keeps agents from spamming cards. | The eval's report | ~200 + notes | low |

### 9.3 synaps-desk PRs

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **D0** | Delete #75's dead core mirror: the `capabilities.surfaces` declaration, the `surface`/`surface_result` events, `Attached.surfaces`, `surface_action`, and `sc/surface/*`. Add the `cards-spec` dependency. | It removes protocol code no daemon will ever speak. | The existing suite; the parity tests move to S1. | about −6.5k, +100 | low |
| **D1** | Read-only cards: recognise them (§2.2), keep the full input for `cards_ui`, fold the state, draw the card and its receipts. No interaction. | The transcript reads as cards. | Render tests per state, the fold over live and history paths, and a fallback per §6.3 | ~900 + tests | low |
| **D2** | Answering: clicks, the Ctrl+J flow, digits, Enter, Tab, Esc, the outbox through `InFlight`, refusal and Retry, read-only when there is no authority | **One-click answers** | F1–F10 and F13–F19 friction tests, the negative tests, and the never-approves test (§5.1) | ~1.3k + ~900 tests | medium: keyboard precedence |
| **D3** | Presence: the composer chip, tab dot, rail NEEDS YOU and ASKED for attached sessions, Ctrl+Shift+J, Ctrl+P Waiting, and the OS notification for wait cards | **Never miss a waiting card** | F8–F12 and the presence counters | ~600 | low |
| **D4** | Change answer, Skip polish, and desk-local memory: preselect the last choice for the same question in the same cwd (highlight only) | Smart defaults | F15, and memory tests | ~400 | low |

**Order:** S1 → S2 (cards are usable from here) → S3 → D0 → D1 → D2 → D3 → D4, with S4 any time after S2.

**SynapsCLI:** no PRs. Close #439, #440, #444 and #446 unmerged.

**#56** (the A2UI preview behind `--agent-surface`) is untouched by this design. D0 may delete it if the preview is no longer wanted.

---

## 10. Decisions for Haseeb

1. **Repo visibility.** The desk's CI needs `cards-spec`, and a private git dependency needs a deploy key. **Recommendation: make synaps-cards public.** It holds no secrets.
2. **Naming.** The plugin is `cards`, so the model sees `cards_ui`, short and plain. **Recommendation: yes.** Haseeb's install should clone into `~/.synaps-cli/plugins/cards`, and add `ext.cards:ui` to `tools.always_loaded` if disclosure is on.
3. **TUI widget.** v1 is plain text only. A live HUD needs `privacy.llm_content`, to clear it on answer. **Recommendation: plain text.** Revisit only if TUI users ask.
4. **"Always do this."** An auto-answer would send a user message without a user action. **Recommendation: not in v1.** D4 preselects only, and a real "Always" needs its own consent design.
5. **Would need core.** **Recommendation: none now.** If F11 hurts, #1 (the per-session extension status) is the one to propose, as a generic feature other extensions would use too.
