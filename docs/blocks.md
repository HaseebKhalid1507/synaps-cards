# synaps-cards v2: the editable transcript (blocks), design v1.3

**Author:** Zero · **Status:** design only, no code. v1.3 answers Shady's third review (synaps-cards #7, round 3 on `725bb52`: REVISE, H 0 · M 3 · L 1, `review-blocks-design-shady.md`). v1.2 answered the second (H 0 · M 5 · L 2), and v1.1 the first (H 2 · M 5 · L 2) and took in Haseeb's five decisions (§9). **Builds on** `docs/design.md` v1.2, the cards design, which stays in force. **Hard rule, unchanged:** zero changes to SynapsCLI. Everything below is the cards extension plus the desk.

**Why this exists.** Haseeb: "the cards feel underwhelming… treat the entire transcript as something that can be edited / taken control of by the agent. They should also be able to mold the right rail too." He picked two mocks, `interact/widgets/` and `interact/wild2/` (`interact/PICK.md`). This document turns their union into a contract that the shipped stack can actually carry. Where the mocks assumed things the stack can't do, §0.2 says so.

**Read at** (re-checked for v1.3; all three are still the tips. The desk repository is `synaps-desk`, whose `main` is `cbae9df`. Round 2's `9fa77fb` is the `main` of the older `synaps-desktop` repository, from 2026-11-14):
- SynapsCLI `origin/dev` `0a71ea0d`, cited as `CLI:`. `ae/` is `crates/agent-engine/src/`.
- synaps-desktop `origin/main` `cbae9df`, cited as `DESK:`. `sc/` is `crates/synaps-client/src/` and `sd/` is `crates/synaps-desktop/src/`.
- synaps-cards `origin/main` `335e456`, cited as `SPEC:`. `cs/` is `crates/cards-spec/src/` and `cx/` is `crates/cards-ext/src/`. `design §n` is `docs/design.md`.

---

## TL;DR

- **A sibling tool.** The `cards` plugin gains a second tool, `block`, which the model sees as `cards_block`. Its input is `{"ops":[…]}`: up to 16 ops per call, 8 KiB in all. It validates the batch, returns `ok 3 ops (blocks/1 #<digest>)` at once, and keeps no state. `ok` means "valid", not "applied", and the agent is told never to retry for lack of a sign. It needs `tools.register` only, as `cards_ui` does.
- **History is the board.** The desk folds every bound `cards_block` call, every card, and every user edit and notice into one board per conversation (`cards_spec::board`).
  - A bound call's ops take effect at the call's place in history.
  - The live turn is a throwaway overlay, re-run from the confirmed board whenever it changes, and never undone op by op (§1.7).
  - Reattach, restart and a second desk rebuild the same board. Compaction empties it.
- **Identity.** A block id is the agent's choice. Each life of an id is an **incarnation**, with a 128-bit tag derived from the history (§1.2).
  - `replace`, and remove-then-put, start a new incarnation.
  - Item ids are never reused within an incarnation, and the user's items get `u-` ids that the agent can't mint.
  - A hand-back names its incarnation and base revision, so a stale edit can't land on a different block (§1.2, §1.4). So does every click and key: an action on a stale incarnation does nothing, and says so (§6.2).
- **Bounded, by owner.** The board's working set is ≤ 1 MiB per conversation, with every part capped. Revision records point into history rather than copying ops. The overlay, the memo, drafts, the archive, `blocks.json` and the user's unsent wishes each have their own cap. Nothing recoverable is evicted silently, and an unsent wish is never evicted at all (§1.8). Groups are one level deep, so they can't cycle (§1.3).
- **Blocks:**
  - **Kinds:** plan, status, diff, note, metric and file.
  - **Targets:** `transcript` (the default) or `rail`. In the rail, `slot` is `upper` (the default, between Prompt cache and Files) or `end` (after Files).
  - **Questions** stay cards, named `card:<id>`.
- **Ops:** `put` (create only), `patch`, `replace`, `collapse`, `merge`, `move` and `remove`.
- **Cost.** A typical 8-round turn spends about 455 output tokens on ops, plus at most one closing round. That is an estimate. B5 measures it, and the default turns on only if B5 meets its gate (§2.8).
- **The user's side:**
  - **Content edits go to the agent.** They arrive as `[block plan edited r4] …` plus a patch carrying the incarnation tag. The user wins on text and order, never on execution truth: a step that started or finished keeps its state (§3.4).
  - **Dismissals go to the agent too,** as a versioned line such as `[block tests v3 dismissed]` riding the user's next message, never as a message of their own. The higher version always wins, every send carries only the desk's latest wish, and a dismissal outlives the block's removal. Once in history, every desk shares them (§3.5).
  - **Presentation stays local:** hold, collapse, pin, rail slot, unpack, and viewing a revision.
- **Motion.** Heights come from structure and bounded line counts. A pointer lease keeps what's under a moving or pressed pointer where it is. A target an agent op just moved takes no click for 600 ms, and keys and the user's own changes are never held back (§6.2). A change glows once (480 ms), and then blocks ask for no frames.

## Changes from v1.2

Shady's round 3 on `725bb52` closed M4, the tag's strength and L2, and accepted the spoofing boundary. v1.3 amends the contract where round 3 found gaps. There is no new architecture.

| # | Shady found | v1.2 | v1.3 |
|---|---|---|---|
| M1a | A per-id ordinal outlives the tombstone that could remember it | The id's ordinal, 1, 2, 3… per generation | **A creation number** (§1.2): one board-wide count of incarnations per generation, in commit order. It's one integer with nothing per id, so tombstone eviction can't reset it |
| M1b | The desk-wide file cap could evict pending wishes | Desires lived in `blocks.json`, under its LRU caps | **A reserved store** (§1.8): `block-desires.json`, ≤ 64 per conversation, ≤ 512 per desk, 64 KiB. No cap evicts a desire: past one, a new dismiss or show is refused before it draws, and accepted means written. The ~3.6 MiB are labelled payload sums, and B2 counts owned bytes |
| M2a | An exact-text Resend could carry a stale wish past a newer local one | "The fold finds it stale wherever the resend lands" | **One rule** (§3.5, §3.7): notice lines are never part of a message's identity. Every send builds its trailer from the latest desires, a Resend included. An old notice already sent applies if it lands first, until the newer wish rides the next send. The description now teaches the version rule (§2.6) |
| M2b | Retirement compared the floor where admission compared the record | Two predicates | **One helper,** `effective_version(id)`: the record's version, else the floor. Admission, attachment, settlement, restart and feedback all use it |
| M3 | The 600 ms rule swallowed the user's own input | Any identity or rect change blocked clicks and keys | **Just moved** (§6.2): it blocks only a pointer action, and only on a target an agent op moved or replaced in the last 600 ms. Keys and the user's own changes are never blocked. A press holds the lease, so a lease ending can't void a click |
| L1 | The no-endpoint branch calibrated against an endpoint | "Calibrated against that endpoint" | **Calibrated against the provider's reported usage** (§2.8), by a fit over fixed probe pairs. A provider that can't be probed gets a labelled estimate that can't flip the default |

## Changes from v1.1

Shady's round 2 on `32bd281`. The highs stayed closed, and Haseeb's five decisions stand. "Unseen patches until the user next types" is accepted for v1.

| # | Shady found | v1.1 | v1.2 |
|---|---|---|---|
| M1 | The retention budget left out owners, and a 256 B record can't own a multi-KiB op | One table of "every piece", ~0.97 MiB | **Owners** (§1.8). The board's working set is ≤ 1 MiB, now ~0.86 MiB. Its revision records are **references** into history (position, author, op kind, ≤ 4 paths), read again on demand. The overlay, the re-fold scratch, the memo, drafts and orphans, the archive, renderer caches and `blocks.json` each have a count, a byte cap and a consequence. **Nothing recoverable is evicted silently:** with 32 unconfirmed edits or 8 drafts, the desk refuses the next one and says why. One archive per session, replaced at the next compaction, never a chain |
| M2 | Dismiss and show replayed an assignment, so an old dismissal could undo a later Show, and a remove before delivery lost the dismissal | "Attaching it twice is harmless"; a notice naming no live block was ignored | **Versioned notices** (§3.5), such as `[block tests v7 dismissed]`. Each conversation generation counts visibility changes. The fold applies a notice only if its version is above the id's record, so of two equal versions the first wins, and an older one is stale. The desk keeps one **desire** per id and attaches only the latest, so a superseded notice is never re-armed. A notice for an absent id is recorded. An id named twice in one message applies neither. Exact-text Resend stays safe, as its stale lines fold to nothing |
| M3 | A click targeted a reused name, and the 1.5 s lease weakened "no change under a resting pointer" | A click acted by id | **Painted targets** (§6.2, §3.8). Every click and key carries the id, tag, item and visibility version it was painted with, checked when it acts. A stale target is a no-op with a note, never a redirect. Pointer-down and pointer-up must hit the same target. **The weaker promise, stated:** the lease covers a moving pointer and 1.5 s after it. A target whose identity or rect changed in the last 600 ms takes no click and no key. Expiry is one timer wake, armed only while a change waits |
| M4 | A fifth user expansion had no legal victim, and groups had no slot rule | The last agent-expanded section folded | **The user's oldest expansion folds** (§4): the newest click wins, ordered by a counter on the desk. The desk's own sections never count. The clicked header keeps its place under the lease. A group sits in one slot with its members (§1.3) |
| M5 | The gate read per-tool tokens that providers don't report | "Provider-reported output tokens of `cards_block` calls" | **Two labelled kinds of number** (§2.8). *Reported* is the provider's per-response usage (`CLI:ae/runtime/api.rs:480-534`), summed per turn, on vs off. *Attributed* is the block calls' JSON arguments, counted with the provider's count endpoint or a proxy calibrated against it, plus a calibrated framing allowance, plus the classified closing rounds' input. The definition is measured on a fixed prefix. The turn, the classifier and a frozen, mixed task set are fixed before any data, and on/off pairs run three times |
| L1 | 8 hex digits are a collision check, not identity | 32 bits over the id, tool id and op index | **128 bits** (§1.2), over the generation (a hash of the history's first message, since an in-place compaction keeps the session id), the id, the id's incarnation ordinal, the tool id and the op index. The inputs differ within a generation by construction, and the text now promises a negligible collision chance, not "always" |
| L2 | `u-k3x9qa` broke its own alphabet | | `u-k3x7qa`. Every normative example in this doc becomes a B1 round-trip fixture (§8) |
| — | Coverage should be per intent; wrappers must stay whole; D6 is riskier than stated | | **Coverage keys** (§3.7) are a `set` path, an appended item or an ordered list. Hunk-line and series appends are never covered. Notices are never read inside an `<event>`, the summary message or a saved block (§3.5). D6 is now medium–high, and B5a and B5b medium (§8). B6's opt-out note leads the release note and the README (§5.5) |
| — | "The desk's tip is `9fa77fb`" | | That's the older `synaps-desktop` repository. `synaps-desk`'s `main` is still `cbae9df` (header) |
## Changes from v1

| # | Shady found | v1 | v1.1 |
|---|---|---|---|
| H1a | "User wins" can rewrite execution truth | The user wins on any path both touched | **The user wins on text and order, never on execution state.** Each user path has a guard, checked against the state where the edit lands: skip only a `todo` step, stop only a step not yet started, keep or drop only an open proposal. A path that fails its guard is a conflict: it isn't applied, and the edit's row says why (§3.4) |
| H1b | A removed and re-put id, or a same-kind rewrite, takes old patches | Ids only, with revisions continuing across a removal | **Incarnations** (§1.2): a tag from the creating call. `replace` and remove-then-put start a new one. Revisions count per incarnation, and item ids are never reused in one (§1.4). A hand-back carries `"of":<tag>`, and another incarnation applies none of it (§3.3) |
| H1c | Two desks can both add `u1` | `u1…` reserved, with no allocation rule | **`u-` plus 6 random base32 characters**, minted by the desk. The agent can't mint `u-`. A collision is a deterministic skip of that item and of every path naming it (§1.4) |
| H1d | Restore had no limits | `{"restore":N}` | **No restore op.** The desk builds an ordinary patch from rN's user paths, in the current incarnation only, under the same guards. Execution state and removed items never come back (§3.6) |
| H2a | A cap of 32 live blocks doesn't bound memory | The live cap, 16 bodies per block, and `blocks.json` capped by conversations only | **A table of retained state** (§1.8): tombstones, revision records, materialized bodies, item ids, dismissals, run titles, the op log, edit outcomes, and `blocks.json` by entries and by bytes. Each has an eviction rule, and the whole is ≤ 1 MiB per conversation. Anything evicted is re-folded on demand, within a budget |
| H2b | `merge` had no graph rules | — | **One level only:** a group can't be a member, `into` can't be a member, and a block is in at most one group. Members count toward the 32. Rules for removal, move, replace and dissolving (§1.3) |
| M1 | No canonical order, and no rollback algorithm | "Array order", and "reverted if it doesn't bind" | A bound call's ops take effect **at the call's position**, committed once its result message is read. The live overlay is rebuilt from confirmed state, never undone. The **bound prefix**; patch phases and byte-ordered paths; overlapping paths refused; semantic no-ops and `rev` rules; `at.after` resolved once (§1.7, §1.4) |
| M2 | A block memo can't reuse whole-answer supersession | The binding was (id, base); any later edit superseded | **Two layers.** Delivery is the memo: by exact text, bound to id, tag and base, and never moved by agent ticks. Application is per path, from the board. Only an edit covering every path supersedes, and a disjoint one never does. Discard is an explicit `forget` (§3.7) |
| M3 | Holes in compaction: holds, in-flight edits, r0 | r0 was restorable, and "the substance reaches the summary" | A held draft becomes an **orphan** with Copy to composer or Discard. A delivered edit whose block is gone says so. Summaries are best-effort. **r0 is gone**, and a read-only archive replaces it (§5.2) |
| M4 | Freezing a block's height doesn't stop it moving | The hovered block's height was frozen | **A pointer lease** (§6.2). The hovered row keeps its screen position and its identity, disruptive changes wait, and the rail defers cap changes. Rail overflow is set per kind (§4). It lands in D5 and D6, before B3 |
| M5 | `from:"edits"` isn't the file's diff | "the edits themselves" | **An edit preview**, labelled as requested and not read from disk, with the ledger's caveats and caps (§1.5) |
| L1 | The estimate was too crisp, and its table didn't add up | ~455 as a result, with a ≤ 400 budget | ~455 stays an estimate, broken down by round and reconciled (§2.7). B5 measures provider-reported tokens. The default flip has a gate to meet, and ≤ 400 is a tuning target (§2.8) |
| L2 | "Dropped" vs "hidden"; "never felt"; "0 frames" | | Ops on a dismissed block fold, unseen (§3.5). The extension's lock serializes whole exchanges (§2.2). Blocks add no frame demand, which is all a block can promise (§6.3) |
| — | `ok` invites blind retries | "A retried put is harmless" | `put` is **create-only**: a no-op on a live id. The description says `ok` means valid, so never retry (§1.3, §2.6) |
| — | Every advertised target needs a renderer before B3 | B3 after D5 | B3 comes after D5 and D6. `at:"top"` leaves v1, and D9 brings it with `blocks/2` (§8) |
| — | NOW followed the last `ToolUse` | | A fixed priority: a prompt, then a wait card, then the earliest running call, with counts (§4) |
| §9 | Haseeb's five decisions | Open | The rail's two slots; dismissals told to the agent; questions stay cards; at most one closing call; the gate off until D5 and D6, on after D8 and B5's gate (§9) |

---

## 0. What carries over, and what the mocks assumed

### 0.1 Reused unchanged from cards v1.2

| Piece | Where | Used for |
|---|---|---|
| The extension never blocks and stores nothing | design §1.2, §4 | `cards_block` returns at once. Holds, dismissals and conflicts are never its business |
| The result binds the call by a digest of the normalized input | `SPEC:cs/digest.rs:93-145`; design §2.2 | A hook-rewritten batch draws as a plain row, never as ops |
| The fold is pure, over Call, Result and User events | `SPEC:cs/fold.rs:43-53`, `:168-176` | The board wraps the card fold and reads the same stream in one pass |
| The return path is a user message in a two-line grammar | design §3.3; `SPEC:cs/message.rs:78`, `:127` | A block edit is `[block …]` plus JSON, like `[card …]` plus JSON |
| Delivery and the memo | design §3.4–§3.5; `DESK:sd/deliver.rs:1-40`, `:337` | A hand-back takes typed text's paths. B4 generalizes the memo, and adds a second layer for what applied (§3.7) |
| `sanitize`, the input gate, and keys that never authorize | `SPEC:cs/sanitize.rs`; `DESK:sd/live.rs:179`; design §5.1 | Every agent string is sanitized. Only an input-holder edits. No block key is an authorize key |
| Workers are refused, and the rate limiter | `SPEC:cx/tool.rs:31-37`, `:104-118` | The same `chat_of` rule, with blocks' own limits |
| Desk-local persistence | `DESK:sd/recall.rs:44-63`, `:97-113` | `blocks.json` and `block-desires.json` use the same atomic, private, swept writer |

### 0.2 What the mocks assumed that the stack can't do, and what replaces it

| Mock | Assumed | Why not | Here |
|---|---|---|---|
| point: `held: plan is being edited… Queued` result; wild2: "agent writes against a user-owned block fail" | The extension knows about holds | The extension can't reach the desk, and the desk can't reach it. Its only inbound methods are `memory.*` and `config.*` (design §1.2) | Holds are desk-local drafts. Agent ops still apply in the fold, and the holding desk keeps drawing its draft. The hand-back applies where its guards allow (§3.4) |
| point: "the lock… published as a `hold` line in history" | A history line that every desk sees | A user message `Submit`s, which wakes an idle agent, or `Steer`s, which the model reads and pays for. A steer can also take the one `queued_message` slot from the user's own steer (design §3.4 d3) | No cross-desk lock. The history orders the hand-backs (§5.4) |
| wild2: `if_rev` and `owner` on agent ops | The agent knows revisions | A stateless extension can't tell it the current revision, and the agent learns of user changes only from edit messages | The **hand-back** carries identity: the incarnation tag and the base revision (§3.3). Agent ops address the current incarnation by id |
| widgets: "parked ops are applied in order on hand-back" | Ops wait off the record | Ops are in history when they're made, and every desk folds them | They apply at their history position. Only the holding desk's *drawing* of them waits (§3.4) |
| widgets #4: a range card with samples | A new card kind | That would be a cards catalog change, cards/2 | Later. v1's question kind is today's card (§1.5) |
| wild2: the pinned mission strip | A top strip | Fine to build, but nothing renders it before D9 | Out of v1's catalog. D9 adds `at:"top"` with `blocks/2` (§6.4) |

**Borrowed from the unpicked mocks, as the pick allows:**
- from point: item ids (§1.4), the "N blocks changed above ↑" pill, block walk keys, the rail cap of 4 sections × 12 lines, the `rN` chip with `[` `]`, and the op log ("WIRE tape");
- from workspace: restore as a new revision (as a desk-built patch, §3.6), and a tombstone receipt for a dismissed block.

---

## 1. The model

### 1.1 The tool and its gate

- **Registration.** `initialize` declares two tools: today's `ui` and the new `block`, in `capabilities.tools` (`SPEC:cx/tool.rs:60-66`; `CLI:docs/extensions/protocol.md` §initialize). The runtime name is `cards:block` and the API name is `cards_block`.
- **The catalog** is `blocks/1`: the six kinds, the seven ops, the targets `transcript` and `rail`, and the rail slots `upper` and `end`. `at:"top"` isn't in it, and D9 adds it as `blocks/2`. A desk that knows only `blocks/1` reads a `blocks/2` result as an unknown catalog: a plain row (§1.6).
- **Gate.** A manifest config entry `blocks`, default `false`. The host resolves it into `initialize`'s `params.config`: env `SYNAPS_EXTENSION_CARDS_BLOCKS`, or user config `extension.cards.blocks` (`CLI:docs/extensions/protocol.md:149-197`). With it off, `block` isn't declared at all, so its description costs nothing.
  - It stays off until the desk renders both targets (D5 and D6).
  - It turns on by default once editing (D8) and the eval (B5b) have landed, and only if B5b meets its gate (decision 5, §2.8).
  - A TUI-only user sets it to `false`: the TUI shows ops as JSON panels (§5.5).
- **Permissions.** `tools.register` only. There are no hooks, no conversation access and no interception (`SPEC:.synaps-plugin/plugin.json`).
- **Workers.** `chat_of` refuses a worker's call (`SPEC:cx/tool.rs:104-118`) with `cards_block: only the main chat has blocks`. A worker's transcript is not the user's document.

**Why a sibling tool, not `ops` inside `cards_ui`:**
1. **Compatibility.** `cards_ui`'s schema is closed (`additionalProperties: false`, design §1.4). Adding ops would make it cards/2, and every shipped desk would draw ops calls as plain rows anyway, so nothing would be gained.
2. **Rate limits.** Cards allow 3 per minute (`SPEC:cx/tool.rs:34-35`). Blocks need about 10 times that.
3. **Two decisions, two descriptions.** "Ask the user" and "show your work" are different choices. B5 and S4 tune their descriptions separately, and the gate can turn blocks off while cards stay on.

The fold is still one: the board wraps `cards_spec::fold::Fold` (§1.7).

### 1.2 Blocks, incarnations, targets, places

- **The block id** is the agent's choice, matching `^[a-z][a-z0-9_-]{0,23}$`. At most one live block has it in a conversation's board. `card:<card id>` names a card (§1.5): the `:` can't appear in a block id, so the two namespaces can't collide.
- **An incarnation** is one life of an id:
  - **It starts** when an op creates content under the id: a `put` on an id with no live block, a `merge` into a fresh id, or a `replace` (which ends the live one and starts the next in the same place).
  - **It ends** at a `replace` or a `remove`, or at compaction, which empties the board.
  - **Its tag** is 128 bits: the first 32 hex digits of SHA-256 over `blocks/1`, NUL, the **generation**, NUL, the id, NUL, the **creation number** (decimal), NUL, the creating call's tool id, NUL, and the op's index in its call (decimal).
    - **The generation** is the SHA-256 of the history's first message: its `role` and `content` as normalized JSON, with `_synaps_*` keys left out. A compaction starts a new one, as its first message is the summary (`CLI:crates/agent-core/src/core/compaction.rs:213-229`). The session id can't serve, because an in-place compaction keeps it (`CLI:ae/runtime/compaction.rs:737-746`). A copy of the history keeps its tags.
    - **The creation number** counts every incarnation of every id in this generation, in the fold's commit order (§1.7): 1 for the generation's first, and one more at each incarnation that starts. A creation the fold skips takes no number. The board keeps one integer for it and nothing per id, so it never depends on a tombstone the caps evicted (§1.8). Streaming and replay agree: the overlay is a clone of the bound board, counter included, and the commit order is the history's.
    - **A rollback** gives the number of a creation it took back to the next creation, which is another call, under another tool id. A draft over the rolled-back incarnation goes stale at the re-fold that drops it (§3.2), before any reuse.
    - **The guarantee.** Within one generation, two incarnations differ in creation number, so their tags hash different inputs. Two tags then collide with a chance of about 2⁻¹²⁸, and B1 adds no collision rule. Across generations, the first messages differ unless two are byte-identical, and the creating tool ids, which providers mint unique, differ too. v1.1's 32 bits were a check, not identity.
    - Every desk derives the same tag from the same history. The tag guards against stale edits, not against a user who types a look-alike: any input-holder can type anything (§7).
- **What an incarnation owns:** kind, title, body, its revisions (r1 is its creation), its item ids (§1.4), and each field's last writer.
- **What the id's slot owns:** target, slot, place, group membership, collapse and dismissal. So a `replace` keeps the block where it is and in its group, and starts its content fresh.
- **`target`** is `"transcript"` (the default) or `"rail"`. It is placement, not identity: `move` changes it.
- **`slot`** places a rail block: `"upper"` (the default) or `"end"` (§4). `slot` implies `target:"rail"`. A `slot` together with `target:"transcript"` is refused by the extension.
- **Its place in the transcript is an anchor**, the history position of the op that last placed it (a `put` that created it, or a `move`):
  - The block draws where that call's tool row would be, after the text that precedes the call in its message.
  - Like a card, it is never inside a tool run. A block anchored mid-run draws right after the run, so the run stays one card (`DESK:sd/turns.rs:328-332`, the `Ask` rule).
  - Several blocks at one anchor draw in a list, in the order they were placed there.
  - A later `patch` or `replace` changes the block in place, wherever it is, and its own call row draws nothing.
- **`at`**, on `put` and `move`:
  - **Absent:** here, at this call's anchor, last in its list; in the rail, last in its slot.
  - **`{"after":"<id>"}`:** right after that block in its list: the same anchor's list in the transcript, or the same slot in the rail. It must name a live block that isn't a group member, in the target and slot the op places into.
    - It **resolves once**, when the op applies, to a place in a list. It is never a standing link: the placed block doesn't follow when the other one moves later, so links can't form a cycle.
    - `after` naming the op's own id is refused by the extension. Anything else that doesn't resolve sends a `put` to "here", marked, and makes a `move` skip, marked.
  - **`move` with neither `target`, `slot` nor `at`** means "here": re-anchored at this call. This is the mocks' "the plan moves down beside the summary".
  - **`move` with `at` alone** (no `target`, no `slot`) joins the `after` block's list, wherever that is: the same anchor's list in the transcript, or the same rail slot (decision D5, settled at B2). It is the one op that changes a block's target without naming one, because `after` names a concrete list. A card still never goes to the rail (§1.5): a `move` of `card:<id>` whose `after` is in the rail is skipped, marked.

### 1.3 Ops

```jsonc
{"op":"put",     "id":"plan", "kind":"plan", "title":"Close the refresh race", "target":"rail", "slot":"upper", "at":{"after":"cause"}, "body":{…}}
{"op":"patch",   "id":"plan", "set":{"steps.fix.note":"token_store.rs:118"}, "append":{"steps":[{…}]},
                              "remove":["steps.bandaid"], "order":{"steps":["fix","test","deploy"]}, "state":{"fix":"done","test":"doing"}}
{"op":"replace", "id":"hyp",  "kind":"note", "title":"Root cause", "body":{…}, "why":"first guess was wrong"}
{"op":"collapse","id":"diff"}  ·  {"op":"collapse","ids":["t1","t2"],"open":true}  ·  {"op":"collapse","run":true,"summary":"Explored relay"}
{"op":"merge",   "into":"research", "title":"Why the cutoff belongs to the session family", "ids":["schema","trace","note"]}
{"op":"move",    "id":"plan", "target":"rail"}  ·  {"op":"move","id":"metric","slot":"end"}  ·  {"op":"move","id":"plan"}   // no target/slot/at: here
{"op":"remove",  "id":"tests", "why":"passed"}  ·  {"op":"remove","ids":["t1","t2"]}
```

| Op | Does | On an id that's absent or removed |
|---|---|---|
| `put` | Creates a block: a new incarnation at r1, placed by `target`, `slot` and `at`, or here. **Create only.** On a live id it does nothing and is logged `exists`, so a retried put can never undo a user's edit. To change a block, `patch` or `replace` it | Creates it. A removed id gets a new incarnation. A dismissed id stays dismissed (§3.5) |
| `patch` | Changes paths in the current incarnation (§1.4). `state` is sugar for setting `state` on items of the kind's main list: `{"fix":"done"}` is `set {"steps.fix.state":"done"}`. A hunk's state is the user's (§3.4), so the agent's sugar ticks only a plan's steps | Skipped, and marked |
| `replace` | Starts a new incarnation in the same place and group: a new kind and body, both given, even to keep the kind (the extension checks a body against its kind, and keeps no state to look the old one up), and a new title if given. The header reads `r1 · replaced`, with `why` on hover. The old incarnation's revisions stay viewable (`[`) but can't be restored (§3.6). A user's draft of the old one goes stale (§3.2) | Skipped, and marked. A resolved card is the exception: `card:<id>` gets its first block incarnation (§1.5) |
| `collapse` | Folds a block (`open: true` unfolds it), one or several (`ids`, ≤ 8). `run: true` folds **the tool run this call sits in** under `summary`. That's the desk's own run grouping (`DESK:sd/turns.rs:1-5`), titled by the agent: tool rows merge with no tool ids named | Skipped, and marked |
| `merge` | Groups blocks into a `group` block `into` (rules below) | Members that don't qualify are skipped one by one |
| `move` | Re-places a block (§1.2): `target`, `slot`, `at`, or here. A grouped block leaves its group. A dismissed block can't be moved (§3.5) | Skipped, and marked |
| `remove` | Ends the incarnation, one or several (`ids`, ≤ 8). The block leaves the view, and a tombstone `rN · removed · why` stays in the op log (§1.8). A group's members are released, not removed | No-op |

"Marked" means: a line in the op log, with its reason (§3.8), and a dim `skipped` chip on the block, if the block is live.

`why` (on `replace` and `remove`) and a run's `summary` are one line of ≤ 80 characters, as a title is.

**Groups (`merge`), one level deep:**
- A group is a block of kind `group`, whose body is its members' ids. **A group can't be a member**, so groups never nest and can never cycle.
- **`into`** names one of three things:
  - a fresh id (absent or removed): a new group incarnation, at its first member's place, target and slot;
  - a live group: the members join at its end;
  - any other live block: the merge is skipped.
  `into` listed in `ids` is refused by the extension.
- **Each member** must be a live block that isn't a group, a card, dismissed, or already in a group, and must be in the group's target and, in the rail, its slot. Others are skipped one by one. A new group left with fewer than 2 members is skipped whole.
  - So `ids` may name `card:<id>`: the extension takes it like any reference, and the fold skips it as a member that doesn't qualify. The call is refused only for a malformed id, an id listed twice, or `into` among the `ids`. `into` names a block, never a card.
- **2 to 8 members.** Members count toward the 32 live blocks, and so does the group. The group's header counts its members: `3 blocks`.
- **Membership is by id.** It survives the member's `patch` and `replace`, and ends at its `remove` or `move`. A group left with one member dissolves: that member takes the group's place, and the group's incarnation ends with a tombstone.
- **On the group itself:**
  - `patch` may set only `title`, and `replace` is skipped;
  - `collapse` folds it, and groups are created folded;
  - `move` moves it with its members, all to one target and slot;
  - `remove` ends it, and releases its members at its place, in member order.
- **The user** may unpack a group locally (§3.5), and edit an unpacked member like any block.

**Batching:**
- A call carries 1 to 16 ops.
- **The extension** validates the whole batch or refuses it with one teaching line: syntax, ids, paths, overlaps, kinds, bodies and bounds.
- **The fold** applies the ops in array order, each one on its own. An op that doesn't apply (an unknown id, an item that isn't there, a broken cap) is skipped and marked, and the rest still land.
- **All ops of a call land in one frame.** That is wild2's "one agent burst changing transcript and rail together".

### 1.4 Paths, items, patches (items borrowed from point)

- **Items have ids.** A plan's steps and a diff's hunks are lists of items with ids matching `^[a-z0-9][a-z0-9_-]{0,23}$`, unique in their list. The prefix `u-` is the user's (below), and an agent item id that starts with it is refused.
- **An item id names one item for the whole incarnation.** Once removed, it can't be appended again in the same incarnation: the append is skipped, with `steps.fix was removed in r3`. An incarnation can name at most 64 item ids over its life, live and removed together, and past that appends are skipped with `replace the block`. So a stale patch to `steps.fix` finds the same step, or nothing.
- **The user's items.** The desk mints a step the user adds as `u-` plus 6 base32 characters (`a-z2-7`) from the OS's random source, 30 bits: `u-k3x7qa`.
  - The agent can't mint `u-` ids, but it may patch the user's items.
  - **A collision** means an id that exists or existed in this incarnation. It skips that item and every other path of the same patch that names it, and the edit's row says `1 added step collided · add it again`. That's deterministic: every desk folds the same skip. Two desks adding their first steps get different ids.
- **The path grammar** is `field` | `list.item` | `list.item.field`, as `^[a-z_]{1,16}(\.[a-z0-9][a-z0-9_-]{0,23}(\.[a-z_]{1,16})?)?$`. There are no indices.
- **Why ids, not indices.** A user who reorders or strikes steps can't make a later agent patch land on the wrong step, and a removed step can't come back as another.
- **The parts of a patch:**
  - `set` replaces scalars;
  - `append` adds items at the end of a list, or lines to a hunk (`"hunks.h1.lines"`), or numbers to a metric's `series`;
  - `remove` deletes items (the agent's only);
  - `order` lists item ids in order. Ids it doesn't name keep their relative order after the named ones, so an item the agent added that the user never saw goes last, not lost. Ids it names that aren't live are ignored.
- **Canonical application.** First, the overlap rule: the extension refuses an agent patch, and the fold skips a user patch, that does any of these:
  - names a path twice, once `state` sugar is expanded;
  - sets a field of an item that it also removes or appends;
  - orders one list twice.

  Then a patch applies in four phases:
  1. `remove`;
  2. `append`: lists in byte order of their paths, items in array order;
  3. `set`: in byte order of the paths;
  4. `order`.

  Each path is all or nothing: an append that would pass its list's cap is skipped whole, for that list. A path that would take the body past its kind's byte cap, or break a rule across its fields (§1.5), is skipped the same way. Paths are checked one at a time, in the phases' order, so of two paths that pass the byte cap only together, the later is skipped (decision D3, settled at B2). A skipped path is marked, and the rest still apply. The order never depends on how a JSON map happens to iterate.
- **Revisions.**
  - **An op adds one revision** to the incarnation when it changes the normalized kind, title or body. An op that changes nothing (a `set` to the current value, a `put` on a live id) adds none, and is logged `no change`.
  - **Not revisions:** placement (`move`, `at`, `target`, `slot`), `collapse`, and dismissal.
  - **A user edit** that applies at least one path adds one revision, authored by the user.
  - **Cap:** at most 9,999 revisions per incarnation. Ops past that are skipped, with `replace the block or use a new id`.
- **Semantic equality** is equality of the normalized form, as `cards_spec::block` normalizes it: defaults filled, strings sanitized, keys in schema order, map keys in byte order, and each number in one spelling: the shortest that reads back, a whole one below 10¹⁶ as an integer (`1e+16` from there). The digest hashes the same form (§1.6).

### 1.5 Kinds v1

A block's `title` (≤ 80) is top-level, and the `body` is its kind's. All strings go through `cards_spec::sanitize` (design §5.2). Each kind's draw height follows its structure (§6.2).

| Kind | Body (sketch) | Limits | Main list | The user's paths (guards in §3.4) |
|---|---|---|---|---|
| **plan** | `{"steps":[{"id":"fix","label":"Serialize rotate() per session","state":"todo","note":"token_store.rs:118","stop":false}]}` | 1–20 steps in a `put` or a `replace`; a stored plan may have none, once a patch removes its last step (decision D7, settled at B2: a stored state, so removing the last task, or removing then appending in one patch, works); label ≤ 100, note ≤ 80; state `todo·doing·done·failed·skipped`; ≤ 4 KiB. `stop` is the user's: an agent op naming it is refused | `steps` | order; a label; a skip and an unskip; `stop`; adding `u-` steps |
| **status** | `{"text":"Bisecting the cutoff","detail":"3 of 7 commits left","progress":{"n":3,"of":7},"state":"working"}` | text ≤ 80, detail ≤ 120; n ≤ of ≤ 10⁶; state `working·waiting·blocked·done`; ≤ 512 B | — | none (it's the agent's voice) |
| **diff** | `{"file":"crates/relay/src/token_store.rs","state":"editing","from":"edits"}`, or a proposal: `{"file":…,"state":"proposed","hunks":[{"id":"h1","at":"rotate()","lines":["- let t = …","+ let _g = slot.lock().await;"],"state":"open"}]}` | file ≤ 200; state `editing·proposed·written·reverted`; 1–6 hunks, ≤ 120 lines in all, a line ≤ 160; hunk state `open·kept·dropped`, where the agent writes `open` and `kept` and `dropped` are the user's (§3.4); ≤ 6 KiB | `hunks` | a proposal's hunks: kept or dropped |
| **note** | `{"text":"Two refreshes read the same token before either rotates it…"}` | ≤ 1500 chars, ≤ 30 lines, markdown-lite as a card's detail (design §5.2); ≤ 2 KiB | — | `text`, `title` |
| **metric** | `{"label":"Upstream rotations per burst","value":"1.0","was":"2.0","unit":"","series":[2,2,1.5,1],"state":"ok"}` | label ≤ 60; value, was and unit ≤ 16 (the agent formats them); series ≤ 32 numbers, any finite ones (no ceiling: the count and the 1 KiB bound them), and an append rolls off the oldest; state `ok·warn·bad·neutral`; ≤ 1 KiB | — | none |
| **file** | `{"path":"crates/relay/src/token_store.rs","line":84,"note":"TokenStore::rotate","state":"editing"}` | path ≤ 200, note ≤ 80; `line` a whole number ≥ 1, any that fits 64 bits; state `reading·editing·written·done`; ≤ 512 B | — | none |
| **question** | **A card.** It is made only by `cards_ui` (design §1.4), and named `card:c4f9x2a` in ops | the card's own: 8 KiB | — | answered as a card (design §3) |
| *group* | from `merge` only: `{"members":["schema","trace"]}` | 2–8 members, one level (§1.3) | — | unpack (local) |

**An edit preview (`"from":"edits"`), not the file's diff.** With `from:"edits"` the agent sends no lines. The desk draws this session's successful `edit`, `write` and `multi_edit` calls on `file` from its files ledger, and labels them for what they are:
- **The header says `EDIT PREVIEW`**, and the footer says "as requested · not read from disk". The ledger is "what the model asked for, not what landed on disk (this client never reads the files)" (`DESK:sd/files.rs:9-14`).
- **The ledger's caveats show as such** (`files.rs:10-14`):
  - a `write` is "N lines written", with no old side;
  - a `multi_edit` regex or line-prefix anchor shows as its pattern;
  - `{{placeholders}}` stay unexpanded;
  - an input that a hook rewrote shows as the model sent it.
- **A file a `bash` call changed** has no lines: "changed by a shell command · no preview" (`files.rs:16-18`).
- **Aggregation.** One hunk group per successful call, in call order, never merged across calls. It keeps the newest 12 groups and ≤ 120 lines, and older groups fold into "+N earlier edits". A line keeps ≤ 160 characters. (The ledger itself keeps 48 lines of 240 characters per preview, `files.rs:27-30`.)
- **Agent-written hunks are proposals,** labelled `PROPOSED · not applied`. The user may keep or drop them while the diff's state is `proposed` (§3.4).
- **The desk never claims to have checked the disk.**

**Questions stay cards** (decision 3). The asking machinery is already built and reviewed: the memo and delivery reconciliation (D2a), Ctrl+J entry and batches (D2b), presence and notifications (D3), recall (D4). A second asking path would fork all of it.

So "a question that becomes its answer" works like this:
1. `cards_ui` asks.
2. The answer comes back as today.
3. `{"op":"replace","id":"card:c4f9x2a","kind":"note","body":{"text":"Lock wait set to 900 ms"}}` turns the question into its outcome. That is the card's first block incarnation. The card fold's receipt stays, drawn as the block's first line, and one `[` away.
4. The user edits that block like any other, named `card:c4f9x2a` (§3.3). The question itself is never edited: an open or unconverted card has no block incarnation, so no edit can name one, and it is answered as a card (design §3).

Ops on a card apply only once it is resolved (answered, skipped, closed or replaced in the card fold). On an open card they are skipped: the agent closes it first (design §1.4 `close`). Cards don't go to the rail, or into groups, in v1.

**A resolved card has no block until a `replace` converts it** (decision D1, settled at B2). Only `replace` acts on an unconverted card. A `patch`, `collapse`, `move` or `remove` naming it is skipped and marked `no block yet`, and the card stays as the card fold draws it: its row, its receipt, its own fold. Once converted, `card:<id>` is a block like any other, and every op acts on it. A `remove` then ends the block; the card stays resolved, and a later `replace` converts it again as a new incarnation.

### 1.6 The result, the binding, refusals

- **Success** is one line of ≤ 64 bytes: `ok 3 ops (blocks/1 #3f9a1c2e4b5d6f70)`.
  - `blocks/1` is the catalog version (§1.1).
  - `#…` is the first 16 hex digits of SHA-256 over the normalized batch (§1.4), the same construction as cards' digest (`SPEC:cs/digest.rs:65`).
  - There is no echo of the ops. The agent wrote them, and every echoed token would be re-read on every later round.
- **Binding.** A call binds only when line 1 parses and its digest matches the call's own input, normalized by the same `cards_spec` (design §2.2 step 3).
  - **Unbound** means a hook rewrote the input, the call was refused, the catalog is unknown, or the line was cut. The call draws as a plain tool row, and **none** of its ops apply: the fail-safe.
- **Refusal** is a JSON-RPC error with one teaching line, stored as plain text, so it draws as a plain row:
  - `cards_block: ops[0].body.steps[2].state: "finished" is not a plan state (todo, doing, done, failed, skipped)`
  - `cards_block: ops[1]: "steps.fix.state" is named twice (state and set)`
  - `cards_block: 31 calls in the last minute; batch your ops or patch less`
- **What the agent is told: nothing about application.** The extension is stateless, so `ok` means "valid", not "applied". An op that doesn't apply shows on the desk (§3.8), not to the agent. Making it otherwise would take extension state that compaction, restarts and user edits would make wrong. An honest `ok` beats a confident wrong one.
- **So the agent never retries for lack of a sign.** Some ops are safe to repeat and some aren't:
  - `put` is create-only;
  - appending an item is idempotent by its id (§1.4);
  - appending hunk lines or series numbers is not.

  The description says it plainly: never retry a call because nothing came back (§2.6).

### 1.7 The board fold: order, commit, speculation (`cards_spec::board`)

**Input.** The card fold's events (`SPEC:cs/fold.rs:43-53`), with three changes:
- `events()` keeps a `cards_block` call's input too. Today only card calls keep theirs (`:326-328`).
- It keeps that call's result.
- A genuine user message is also read as a block edit (§3.3) and for notices (§3.5).

**The canonical order:**
1. **Positions** are the history's: messages in order; within an assistant message, its tool calls in model order; within a user message, its tool results, then its text.
2. **A block call's ops take effect at the call's position, if its result binds.**
   - A call's result is in the message right after the call's. That is the provider's tool_use/tool_result pairing, which the engine keeps even through a failure or a cancel (`CLI:ae/session/actor.rs:3495-3504`, `:3620-3624`).
   - So the fold **commits** an assistant message's block calls once it has read the next message's results: in call order, before that message's text and before any later event. A block call with no result by then is unbound for good.
   - Nothing that can touch the board lies between a call and its commit, only other calls and results of the same batch. Steering is a separate user message after the results (`CLI:ae/runtime/helpers.rs:136-157`).
   - `card:` ops resolve against the card fold at the commit. A card asked in the same batch is still open then, so ops on it are skipped, as on any open card.
3. **A user message** takes effect at its own position: its edit (§3.3), then its notices (§3.5).
4. **Within a call,** ops apply in array order. **Within a patch,** the four phases of §1.4 apply.

**The live turn: a throwaway overlay.** The board the desk draws is the history's board, cloned, with the live turn read on top. It is recomputed from that clone whenever a live event changes it. That's what `DESK:sc/cards.rs:203-230` already does for cards: `history.clone()` (`:228`), then the live events sorted by stream position (`:227`). Nothing is ever undone by an inverse op.
- **Call order, not result order.** Live block calls are read in call order (their `ToolUse` position, which follows the model's order), because results report as they finish (`CLI:ae/runtime/stream.rs:2881-2887`).
- **Pending.** A call whose input is whole but whose result hasn't come is **pending**: its ops draw provisionally, marked. A call whose result doesn't bind is left out, and the clone is re-run without it.
- **The bound prefix** is the live block calls, in call order, up to the first pending one. Its revisions are final, numbered as the history will number them. Ops past it draw as pending, and show no revision numbers.
- **Live user messages** count at the point where the history will hold them:
  - **a prompt** at its `TurnStarted` row, as for cards (`DESK:sc/cards.rs:223`; `DESK:sc/transcript.rs:1708-1713`);
  - **a steer** at its `SteeringDelivered`. The engine sends that event as it drains the steer into the history, after the round's results and before the next request (`CLI:ae/runtime/helpers.rs:147-155`; `stream.rs:3165-3170`), and the actor forwards it to every client (`CLI:ae/session/actor.rs:3820-3822`). So the live place is the history's place.
  - **Never at the `Steered` echo,** which only says the steer was queued. The transcript already swallows a card answer's echo (`transcript.rs:1757`), and D8 does the same for the block grammar.
  - A steer not yet delivered isn't on the board. Its memo shows where it is (§3.7), and the sending desk draws its own over the board, marked `sending`.
- **Caps** apply to pending ops too, in the same replay, so nothing draws that the confirmed fold would refuse.
- **Editing never starts from pending state.** Take uses the bound prefix (§3.2). A pending call's changes, once bound, count as agent changes since the base (§3.4).

**Equivalence.** Three readings must agree exactly: the live overlay at the turn's end; the history fold once it lands; and a reattach's fold of any split of the same history. B2 and D5 test all three.

**Caps the fold enforces** (the extension can't, as it has no state):
- ≤ 32 live blocks per conversation, groups and members included. A `put` or `merge` beyond that is skipped and marked;
- every bound in §1.8.

### 1.8 Retained state: owners, caps, consequences

The history is the record. Everything else has an owner, a count and a byte cap, and a stated consequence past that cap. **Nothing recoverable is evicted silently.** Where dropping something would lose the user's unsent work, the desk refuses the new thing instead, and says so.

**The board's working set** (`cards_spec::board`, one per conversation generation): the canonical fold.

| State | Cap | Past the cap |
|---|---|---|
| Live blocks (groups and members included): kind, title, place, body | 32, ≤ 18 KiB owned each, its item ids, last writers and list entry included (a maximal plan is the largest). A body is its kind's cap, ≤ 6 KiB of JSON (§1.5) | A creating op is skipped and marked. The extension refuses a body over its cap. The fold skips the **path** of a patch that would exceed a body's or a list's cap, and the patch's other paths still apply (decision D3, settled at B2: §1.4 makes each path all or nothing) |
| Tombstones: id, tag, last rev, a reference for `why` | 64, ≤ 96 B owned each | The oldest ending is dropped. A stale edit naming a dropped tag finds no live incarnation, so nothing applies |
| The creation number (§1.2) | One integer per generation | None: it only counts. Nothing is kept per id, so no tombstone eviction can reset it |
| Revision records: a **reference** (message, call, op), the author, the op kind, the count of changed paths and the first ≤ 4 of them | 64 per live incarnation and 1,024 across the board, ≤ 256 B owned each | The oldest are dropped, and the counter keeps counting. **A record never holds its op:** the op, its `why` and its other paths are read from the history at the reference |
| Materialized past bodies | 8 per incarnation, 192 KiB across the board | Least recently viewed first, then re-folded on demand |
| Item ids per incarnation, live and removed | 64, ≤ 56 B owned each, inside the block's 18 KiB | Appends are skipped (§1.4) |
| Field last-writers: the user's path and hand-back | ≤ 81 per block (a plan's 20 steps × label, skip, `stop` and an added step, and its order), ≤ 84 B owned each, inside the block's 18 KiB. The agent's writes aren't kept | Dropped with the incarnation, and with an item the agent removes |
| Visibility records (§3.5): id, state, version; plus the floor and the highest version seen | 64, ≤ 104 B owned each | Shown records go first, then dismissals of ids with no live block, oldest version first. A live block's dismissal (≤ 32 of them) never goes. An evicted version raises the floor (§3.5) |
| Run titles: the titling call's position → summary ≤ 80 characters | 64, ≤ 352 B owned each. Keyed by position, never a copy of the tool id, which has no bound | Oldest first |
| Edit outcomes: a reference to the hand-back, one code per path | The latest 32 hand-backs, ≤ 2 KiB owned each | Oldest first. An evicted outcome is re-folded when its row is drawn |
| Op log (§3.8): references and outcome codes | 256 entries, ≤ 256 B owned each | Oldest first |
| The open batch (§1.7): the calls waiting for the next message's results | A range of the history: two positions and two flags. Nothing per call: the board reads the calls again from the history at the commit and for the overlay | None: it has no cap to pass |
| **The working set** | **≤ 1 MiB.** The ceilings above, at their counts, sum to ~0.97 MiB. The card fold is reported apart: it grows with the history's cards, as it did before blocks | An invariant, which B2's property tests check under adversarial streams |

**A reference** is a position in the history the board was folded from. The board is a pure function of that history. When the history changes under it (a settled history without the live turn, a rollback, a compaction), the fold runs again from where the two diverge. So a reference never outlives its message.

**The other owners** are per attached session (one `Live`, shared by its split panes, `DESK:sd/live.rs`), unless the row says otherwise:

| Owner | Holds | Cap | Past the cap |
|---|---|---|---|
| The live overlay (§1.7) | The board, cloned, with the live turn on top | One, under the board's caps: ≤ 1 MiB. It exists only while a turn streams | The same caps skip the same ops, so nothing draws that the history's fold would refuse |
| Re-fold scratch | A board folded up to a past position, for a revision view or an evicted outcome | One at a time, ≤ 1 MiB, dropped when its view closes | The next request waits |
| The delivery memo for block edits (§3.7) | One entry per hand-back: its text, binding and state | 32 entries, ≤ 9 KiB each | Settled entries go first: retired, then `in the chat`, oldest first. An evicted delivered entry can't be offered again if a later history loses its message. **Recoverable entries never go** (sending, queued, held, checking, unconfirmed, not sent). With 32 of them, Hand back is off: `32 edits not confirmed · resend or discard one` |
| Drafts: holds and orphans (§3.2, §5.2) | Id, tag, base, the intents by path, field text | **8 per session**, holds and orphans together, across compactions. ≤ 8 KiB each | A 9th take is refused: `8 drafts open · hand back or discard one`. A compaction turns holds into orphans in place, so it never needs a 9th, and no draft is ever dropped for space |
| The archive (§5.2) | The predecessor's final blocks and op log, read-only | **One per session**, ≤ 256 KiB. The next compaction replaces it, so there's never a chain | Lost at a desk restart |
| Renderer caches, per pane (D5) | Shaped text and layout per drawn block | ≤ 32 blocks, plus the one revision in view | Dropped with what isn't drawn, and re-shaped on demand |
| `blocks.json`, per desk (§3.5) | Presentation: pins, slot moves, soft dismissals | ≤ 64 entries and 16 KiB per conversation, ≤ 256 conversations, ≤ 1 MiB | Oldest first, then the least recently touched conversation. It's presentation, so no wish is lost |
| `block-desires.json`, per desk (§3.5) | The user's unsent wishes: id, state, version, ≤ 64 B each, by conversation | ≤ 64 per conversation and ≤ 512 per desk, in a reserved 64 KiB that no other owner uses | **No cap evicts a desire.** A new dismiss or show past a cap is refused before anything draws: `64 changes waiting · send a message first`, or `512 changes waiting · send in another conversation, or discard`, where `discard` lists them by conversation. A desire leaves only when it settles, when the user cancels or discards it, or at a compaction (§5.2) |

So one session's blocks hold at most ~3.6 MiB in memory: 1 MiB each for the board, the overlay and the scratch, ~290 KiB of memo, 64 KiB of drafts and 256 KiB of archive. Renderer caches are per pane on top. The two files are ≤ 1 MiB and 64 KiB per desk. These are payload sums, not allocator measurements: B2 and D6 count owned bytes, container overhead included, and the caps apply to those counts. The board's figures are already owned bytes (decision D8, settled at B2), for 64-bit targets: a value's inline size and what it owns on the heap, strings and vectors by capacity (the board gives back what they hold past their length after every change), a map entry at twice its key and value plus a node's share. B2's test derives each ceiling from the inline sizes and the caps, and checks every owner's largest entry against it at every event of its property tests.

**The honest boundary is the history.** The board never copies it whole. But the transcript keeps a block call's input whole (≤ 8 KiB), where another tool's keeps a 4 KiB preview (`DESK:sc/transcript.rs:63`, `:336-342`), just as card inputs are kept today. So the transcript grows with the history by at most 4 KiB more per block call than today, at a pace the rate limit bounds (§7). Nothing here bounds the history itself: its length is up to the daemon.

**Re-folding** is pure. It runs off the frame, cooperatively, yielding every 50,000 events or 30 ms, whichever comes first. Events don't all cost the same, so the slice is a yield point, not a time per event. Past 200,000 events, the view says `too far back to show`.

**Tests (B2, B4, D8):**
- many incarnations, each with a delivered and then an unconfirmed edit;
- maximal multi-KiB patches kept as revision records: each record stays ≤ 256 B, and re-reading it returns the whole op;
- successive compactions while holding drafts: never more than 8, and none dropped;
- the memo full of recoverable entries: Hand back is off, and nothing is evicted;
- the board, the overlay, the scratch and the archive at their caps together;
- 200 ids created and removed, past the tombstone cap, then the first id put again: a fresh creation number, and the same tag streaming and replayed;
- the desire file full across many conversations, then one more dismiss: refused before it draws; then a restart: every accepted desire is still there.

### 1.9 Compaction, `/new`, a resume

- The new conversation's history is the summary, plus pending events and the queued message (`CLI:crates/agent-core/src/core/session.rs:292`; `CLI:ae/runtime/compaction.rs:830-840`). So the board is empty, as every card is gone (design §2.3).
- §5.2 says what the desk keeps meanwhile, and what happens to holds and hand-backs in flight.

---

## 2. Cost

### 2.1 What one call costs

| Part | Tokens | Paid |
|---|---|---|
| The call's wrapper `{"ops":[…]}`, tool name and id | ~8–10 + the ops | output, once |
| A two-step tick: `{"op":"patch","id":"plan","state":{"fix":"done","test":"doing"}}` | ~19 (proxy) | output, once |
| The same tick as a whole call | ~22–23 (proxy) | output, once |
| A five-step `put plan` call | ~107–109 (proxy) | output, once |
| The result `ok 1 ops (blocks/1 #…)` | ~25 (proxy) | input, re-read every later round (cache read) |
| The tool definition | ~885: description ~465 (proxy), schema ~420 (estimate) | input, cached prefix, every request |
| **A call alone in the final message** | the whole context | **one more model round**, read from cache |

"Proxy" means Shady's spot check with the cl100k and o200k tokenizers, not Anthropic's tokenizer and not a measured workload (review L1). B5 replaces every number here with counts made as §2.8 says: reported where the provider reports them, and attributed by a stated method where it doesn't.

The last row is the expensive one. A call to tick, on its own, costs a round trip that re-reads the context: at 60 k tokens of context, five such ticks in a turn are 300 k cache-read tokens. Everything below exists to avoid it.

### 2.2 Ride the work: ops share a message with the real calls

- **No extra round.** A model message may carry several tool calls. The engine runs them in one batch and returns all their results in one round (`CLI:ae/runtime/stream.rs:2549-2552`). An op that shares a message with a `read` or a `bash` costs only its own tokens.
- **The lane rule.** Extension tools inherit the default effect `NonIdempotent` (`CLI:ae/tools/mod.rs:530-531`), so `cards_block` runs in the serial lane (`stream.rs:2636-2650`). Once a serial call is queued, **every later call joins the serial lane behind it** (`stream.rs:2670-2680`). So `[cards_block, read, read]` serializes both reads.
- **The fix is in the description: put it after your reads and edits.** Reads are `ReadOnly` and concurrent, and edits are keyed by path (`CLI:ae/tools/grep.rs:22-23`, `CLI:ae/tools/edit.rs:9-10`). Calls before the first serial call keep their lanes. Put last, `cards_block` slows none of them. But it waits behind any serial call before it: after a `bash`, it runs when the `bash` ends.
- **What the user sees doesn't depend on lanes.** The desk draws ops at `ToolUse` time, as pending (§1.7), and each call's result is reported as it finishes (`stream.rs:2881-2887`).
- **The extension process** serializes whole exchanges under one lock (`CLI:ae/extensions/runtime/process.rs:710`, `:1836`), and a `tool.call` may take up to 120 s (`:1928`, `:1938`). A block call validates in microseconds, but it waits for any other exchange with the cards extension in flight. Today that can only be a `cards_ui` call, which also returns at once. No latency is promised, and B5 measures it.

### 2.3 Batch

- One call per message at most. Ops on the transcript and the rail go together.
- `state` sugar covers the common tick.
- `put` with `from: "edits"` replaces every `append` of diff lines.
- `collapse run` replaces listing tool ids.

### 2.4 What the desk derives, at zero agent cost

| Derived | From | Where it shows |
|---|---|---|
| **NOW**, what's happening | The live `ToolStarted` and `ToolQueued` (`CLI:ae/runtime/stream.rs:2697-2712`), the stream state, and what waits on the user in Ctrl+J's order (`DESK:sd/waiting.rs:1-5`) | The rail's first line (§4). It replaces a status op per round |
| **Files changed**, and an edit preview per file | The files ledger (`DESK:sd/files.rs:1-18`), and the right panel's Files section (`DESK:sd/chrome.rs:2504-2629`) | Files, and `diff` blocks with `from:"edits"` (§1.5) |
| **Tool-row merging** | The run fold: a finished run of ≥ 4 calls folds to one summary row (`DESK:sd/turns.rs:31`) | The agent only names it (`collapse run`) |
| **Tokens, context, cache, keep-warm** | The desk's own sections (`chrome.rs:2330-2503`) | Unchanged. The description says not to restate them |
| **A step still "doing" when the turn ends** | `Idle` with the board unchanged | Drawn "doing · turn ended": static and honest, never "done" |

### 2.5 The closing round (decision 4)

The last step's outcome often comes in the last tool result, after which the agent answers in text. There are three choices:
1. tick in the final message, which costs one round;
2. leave it, so the desk draws "doing · turn ended";
3. tick optimistically before the result is in, which is a lie when it fails.

**Rule: at most one closing call per turn, and only when the end state matters**, e.g. the plan done or the diff folded. Otherwise leave it, and tick in the next turn's first batch. No optimistic ticks.
- The rule is **guidance in the description, not enforcement.** A stateless extension can't tell which call closes a turn, so it can't count them.
- B5 measures how often closing rounds happen, and the gate bounds it (§2.8).

### 2.6 The description (B3 ships it; B5 tunes it)

> Keep a few living blocks in the user's desktop app instead of narrating progress: a plan that ticks, a status, a diff, a note, a metric, a file. They sit in the transcript, or in the right rail ("target":"rail"; "slot":"end" for its bottom). One call takes {"ops":[…]} (up to 16) and returns at once. "ok" means valid, not seen: never retry for lack of a sign.
> Ops: put creates (id, kind, title, body, at {"after":id}); on an existing id it does nothing. patch: set, append, remove, order by path like "steps.fix.label"; items by id, never index; "state":{"fix":"done"} ticks; name each path once. replace swaps kind and body, both given (give why), and drops the user's unsent edit, so prefer patch. collapse (id, ids, or run:true with a summary folds this run's tool calls), merge (ids into a group), move (no target: here), remove. A finished card can be replaced as "card:<id>".
> Put ops in a message that already calls tools, after its reads and edits. A call in your final message costs a round: make at most one closing call per turn, only when the end state matters. Patch on real transitions, not chatter. At most 6 blocks, 4 in the rail. Don't restate what the app shows: the running tool, changed files (use a diff with "from":"edits"), tokens. Never start a step marked stop: end your turn and say why.
> A user message "[block <id> edited r<N>] …" carries their edit as a patch on its next line. It applies where it still can: started or finished steps keep their state. A last line "[block <id> v<N> dismissed]" means they hid it: stop patching it, and don't put, move or recreate it, even under another id; you may remove it. "[block <id> v<N> shown]" means it's back: bring it up to date. Per id, only a v<N> above all earlier ones counts. After a compaction your blocks are gone: put again only what's useful. To ask the user, use cards_ui.

About 479 tokens by the proxy tokenizers (cl100k 479, o200k 473): 29 more than v1.1's, for the versions, "stop patching it" and v1.3's version rule. v1's text measured 397–401 there, not the 330 it claimed. The schema stays shallow, about 420 tokens: `ops` is an array of objects with `op` and `kind` enums, plus `id`, `target`, `slot` and `at`. The kind bodies are described rather than spelled out per kind. B5 measures the whole definition, and the gate caps it (§2.8).

### 2.7 An estimate for a typical turn

The turn: a bug fix in 8 rounds (read → plan → three edits → test → fix → test → answer). Every number is an estimate.

| Round | Rides | Ops | Output tokens |
|---|---|---|---|
| 2 | the reads | `put plan` (5 steps) | ~110 |
| 3 | the first edit | a tick, and `put diff from:"edits"` | ~60 |
| 4 | an edit | a tick | ~25 |
| 5 | the test run | a tick, and `put metric` in the rail | ~65 |
| 6 | the fix | a tick, and `patch metric` | ~45 |
| 7 | the test run | a tick, and `patch metric` | ~45 |
| close | **1 extra round** | the last tick, `collapse diff`, `collapse run` | ~50 |
| framing | 7 calls × ~8 (name, id, wrapper) | | ~55 |
| **Total** | **7 calls, 1 extra round** | | **~455** |

- **A spot check:** written out as JSON with these exact ops, the seven calls count 380 tokens in cl100k, wrappers included, before the provider's own tool-call framing. That's in line with the table, and still a proxy.
- **Into history:** the ~455 output tokens themselves, plus 7 results × ~25 ≈ 175. Every later round re-reads both as cached input: cheaper than fresh input, not free.
- **Plus:** the ~885-token definition in each request's cached prefix, and the closing round's cache read of the whole context.
- **Minus:** most per-round narration ("Now I'll run the tests…", ~30–40 tokens a round), which the description asks blocks to replace. B5 measures the net (§2.8).
- **Latency:** at 60–80 tok/s, ~455 output tokens are ~6–7 s spread over the turn, about 0.8 s a round, plus the closing round, which isn't bounded here. B5 reports it.
- **The mocks' naive pattern,** for comparison: a status `replace` every round, ticks with full paths, diff lines appended by hand, and a call per tick. That is ~1.4 k output tokens and 5 extra rounds.

### 2.8 B5: what it measures, how, and the gate for the default

**Two kinds of number, never mixed.** Providers report usage per response, not per tool call: the engine reads one usage per message (`CLI:ae/runtime/api.rs:480-534`, sent as one `SessionEvent::Usage` per response at `:623`). So B5 keeps two kinds of column, and labels them everywhere:
- **Reported:** the provider's own counts per response (input, output, cache read, cache write), summed per turn and compared on vs off.
- **Attributed:** an estimate of what blocks cost inside those responses, by the counting method below. It is never called billing.

**Fixed before any data.** B5a's PR states all of this, and B5b reuses it unchanged:
- **The task set, frozen and mixed:**
  - ≥ 20 coding tasks of ≥ 6 rounds;
  - ≥ 10 short tasks of 1–2 rounds;
  - ≥ 5 turns with no tool call;
  - ≥ 5 tasks in which the user answers a card or hands back an edit mid-task.

  The set and its prompts are committed before the first run. Tuning the description never changes them.
- **A turn** is one user-initiated run, from its `TurnStarted` to the session's `Idle`, steers included. Event-started turns are reported apart, outside the denominator.
- **A round** is one provider response.
- **The closing-round classifier.** Take the responses after the turn's last result from a tool other than `cards_block`, or after the user's message if there is none: R1 … Rk. With blocks off, k is 1.
  - **The turn has k − 1 closing rounds,** whichever response carried the final text.
  - **Their input** is the reported input of R2 … Rk: each re-read the context once more.
  - **Their output** isn't split by response, because the final text may ride with the block call or follow it. It is counted in the block-call attribution and in the net-output row.
- **Runs.** Each task runs with blocks on and with blocks off, on the same model and prompt, three times each, interleaved. A gate row passes only if it passes on the pooled runs **and** in at least 2 of the 3 repeats, so one lucky run isn't a licence.

**How the attributed numbers are counted:**
- **Block-call output:** the compact JSON of each `cards_block` call's input, as sent. It is counted with the provider's token-count endpoint where there is one. Otherwise a proxy tokenizer is used, **calibrated against the provider's own reported usage, never against itself:** a fixed calibration set of ≥ 200 recorded calls of varied length, each sent as a probe pair that differs only by that call. Fitting each pair's difference in reported input tokens to `framing + ratio × proxy count` gives the ratio, and the framing allowance below as the intercept. The PR reports the fit and its spread. A provider that can't be probed this way gets an uncalibrated estimate, labelled so, and its runs can't flip the default (B6).
- **Framing:** one fixed allowance per call, for the name, id and wrapper. It is calibrated once. With an endpoint, it's a probe message with one block call, minus the same message without it, minus the arguments' own count. Without one, it's the fit's intercept above.
- **Result input:** each result's tokens, counted the same way, times the rounds left in the turn that re-read it.
- **The definition:** a fixed-prefix control, never the on − off workload delta, since the histories diverge. The same system prompt, tools and first message are counted with `block` declared and without it.

**Also reported, with no gate:**
- closing rounds' input, and the result input;
- ops on dismissed blocks, by count and attributed tokens: the unseen patches (§3.5);
- refusals, repairs (a refusal followed by a corrected call), and `put` on a live id;
- skipped ops by reason, and notices delivered;
- latency per round.

**The gate.** B6, the default flip, ships only if every row holds:

| Measure | Kind | Gate | Tuning target |
|---|---|---|---|
| Block-call output per turn | attributed | median ≤ 500, p90 ≤ 900 | ≤ 400 |
| Block-call output per round | attributed | median ≤ 65 | ≤ 50 |
| Closing rounds | classified | ≤ 1 in every turn, and in ≤ 40% of turns | ≤ 25% |
| The definition | fixed-prefix control | ≤ 1,000 input tokens | ≤ 800 |
| Output per turn, on − off: per task, the difference of the repeats' means | reported | median ≤ +300 | ≤ +150 |
| Repairs and re-puts | counted | ≤ 1 per 10 turns | — |
| Skipped ops | counted | ≤ 5% of ops | ≤ 2% |
| Task outcome | judged per task | no task that passes with blocks off fails with them on | — |

- **If a gate fails,** B5 tunes the description and measures again, on the same frozen set.
- **If it still fails,** the default stays off, and Haseeb gets the report.
- **B5a** runs the same harness right after B3, on the read-only slice, so any chatter shows up before D7 and D8 are built.

---

## 3. The user's side

### 3.1 The principle: content and dismissals are shared, presentation is local

| Action | What it is | Reaches the agent | Other desks | Persisted |
|---|---|---|---|---|
| **Hand back** an edit | content | yes: a user message (§3.3) | yes, by history | in the journal |
| **Restore** a revision | content: a desk-built edit (§3.6) | yes | yes | in the journal |
| Answer a question | content | yes: today's card answer | yes | in the journal |
| **Dismiss**, and **show** again | a notice (decision 2) | yes: a line riding the user's next message (§3.5) | yes, once delivered | in the journal; `block-desires.json` until then |
| **Take** (hold) and edit a draft | a local lease | no | no | no (in memory) |
| **Pin** to the rail and back; move between **rail slots** | presentation | no | no | `blocks.json` |
| Collapse or expand, unpack a group | presentation | no | no | no |
| View revision N | presentation | no | no | no |

**Why.** Every message to the agent costs tokens, wakes an idle agent (a `Submit` starts a turn, `CLI:ae/session/actor.rs:5440-5447`), or competes for the one steer slot (design §3.4 d3).
- **The desk never sends on its own.** Only a deliberate hand-back, restore or answer sends, which is design §3.5's rule kept. Notices only ride a message the user sends anyway (§3.5).
- **Who may send** is today's `input_gate()` (`DESK:sd/live.rs:179`). Without it, the block draws read-only, with "edit from <owner> · /takeover" (design §3.2).

### 3.2 Take, edit, hand back

- **Taking a block:**
  - Press `e` in block walk (§3.8), or click an editable part: a step's label, its strike or stop gutter, a note's text, or a proposed hunk's keep/drop.
  - The block is then **held** on this desk: a local draft over the **bound** board (§1.7), never over pending ops. The draft records the id, the incarnation's tag, the base revision `B`, and the user's intents by item id and path.
  - The header reads `EDITING · r4`, in the theme accent, with the same focus ring an entered card has (`DESK:sd/ask_keys.rs:17-22`).
  - From the first agent change after `B`, it adds `· 2 agent changes`. Pending ones count once bound.
- **Editing, by kind:**
  - **plan:** move a step (drag its grip, or `Shift+J`/`Shift+K`); strike or unstrike it (`x`); toggle a stop before it (`b`, or a gutter click); rename it (`r`, an inline field); add a step after it (`a`, which gets a `u-` id).
    - A step that is doing, done or failed at `B` can be renamed and moved, since text and order are the user's. It can't be struck or given a stop: it says "already ran" (widgets).
  - **note:** its text, and its title.
  - **diff** (proposed): keep or drop each hunk (`y`/`n`, widgets and point).
  - **status, metric, file** aren't editable. Pin, fold and dismiss still work.
- **Hand back:**
  - Press `Enter` on the block, or click **Hand back**. In a text field, `Tab` leaves the field and `Enter` then hands back; in a one-line field `Enter` hands back, as a card form's does (`ask_keys.rs:44-47`).
  - **Ctrl/Cmd+Enter is not used**: it is the authorize card's Allow (design §5.1).
- **The pre-check at hand-back.** The desk runs the patch's guards (§3.4) against the current bound board.
  - Paths that would fail now are listed in the footer, e.g. "1 change no longer applies: deploy already ran". They are left out of the patch and of line 1.
  - If nothing is left, Hand back is off: "nothing left to hand back".
  - The fold checks the same guards again where the message lands. More agent changes can arrive between the send and the message's place in history.
- **A stale draft.** If the incarnation ended while held (a `replace`, a `remove`, a compaction, or a rollback that took its creation back), the header turns to `the agent rewrote this block (r1)` or `this block is gone`, and Hand back is off. **A stale patch is never sent.** The choices are:
  - **Copy to composer:** line 1's words, without the `[block …]` prefix, go into the composer, to edit and send as typed text if the user wants;
  - **Discard.**
- **Cancel:**
  - With no changes, `Esc` leaves.
  - With changes, the first `Esc` shows "Esc again discards · Enter hands back" in the footer, and the second `Esc` discards the draft. There's no dialog.
- **A hold doesn't pause the agent.** It is local, and the agent isn't told (§0.2). To stop the agent, Cancel or HALT as today. The hold header says "the agent keeps working".
- **A stop is an instruction, not a breakpoint.** A `stop` reaches the agent as text in its next round. The engine doesn't enforce it, and a step may already be under way by then (§3.4's guard).

### 3.3 The edit message (built and parsed by `cards_spec::block::message`)

```
[block plan edited r4] Close the refresh race → order fix, test, deploy, bandaid · skip bandaid · stop before deploy
{"of":"bab3b9916631463b1935420c710c9287","order":{"steps":["fix","test","deploy","bandaid"]},"set":{"steps.bandaid.state":"skipped","steps.deploy.stop":true}}

[block plan edited r6] Close the refresh race → add “Check staging logs” after test
{"of":"bab3b9916631463b1935420c710c9287","append":{"steps":[{"id":"u-k3x7qa","label":"Check staging logs","state":"todo"}]},"order":{"steps":["fix","test","u-k3x7qa","deploy"]}}

[block cause edited r5] Root cause → back to r1: text “First guess: the retry loop”
{"of":"0adc13ce1747120320cd140b074555a4","set":{"text":"First guess: the retry loop"}}

[block auth-diff edited r3] Fix: serialize rotation → keep h1, h2 · drop h3
{"of":"3ebc21f2849656a46ee66d493e83019c","set":{"hunks.h1.state":"kept","hunks.h2.state":"kept","hunks.h3.state":"dropped"}}
[block tests v3 dismissed]
```

- **Line 1, for people.**
  - It matches `^\[block ([a-z][a-z0-9_-]{0,23}|card:c[a-z0-9]{6}) edited r([1-9][0-9]{0,3})\] `, then the block's title (or its id), then ` → `.
  - **A card's block** is named `card:<id>`: the block incarnation that `replace card:<id>` made (§1.5, step 3). Its `of` is that incarnation's tag. An open or unconverted card has no incarnation, so no tag names it, and an edit naming it applies nothing, like one naming a removed block (admission, below). Answering a card stays the card's own message (`[card <id>] …`, design §3).
  - After that comes a summary of pieces joined by ` · `. Each piece is cut at 40 characters, and the line is at most 200 characters, sanitized (design §3.3's rules).
  - There is one verb. A restore is an edit whose summary says `back to rN` (§3.6).
  - `rN` is the **base**: the revision of incarnation `of` that the user edited from. It tells the agent what the user saw.
- **Line 2, for machines.** It is a patch in the agent's own vocabulary (§1.4), with `of` first: the incarnation's tag (§1.2). Then come the user's parts, `append`, `order` and `set`, in byte order of their keys, as all four examples write them, with no `op` or `id`, since line 1 names the block. That is only how line 2 is written: the parts still apply in §1.4's phases (`append`, `set`, `order`). It is ≤ 8 KiB.
  - **Read for what it says, not how it is spelled.** The desk always writes line 2 one way: compact, `of` first, the parts in byte order. Reading it, any JSON spelling of the same patch counts alike: keys in another order, spaces between tokens, another escape (`caf\u00e9` is `café`). Line 2 is still one line with no control character, ≤ 8 KiB as written, a JSON object with `of` and only the user's parts, no key twice. The memo matches a message by its body's own bytes (§3.7), so a respelled edit is matched as it was sent. The 8 KiB is on line 2 as received: read with what the user left out filled in (a step's `state`), the same patch can be longer, and it is still the edit. The desk writes an outgoing line 2 within 8 KiB and never rebuilds a received one. `of` is 32 hex digits (§1.2), about 8 tokens more per hand-back than v1.1's 8 digits, input only.
- **Then 0 to 2 notice lines** (§3.5).
- **Admission** (the fold, at the message's place in history). The message is read as an edit only if all of these hold:
  - all of the genuine user message is the grammar: lines 1 and 2, then optional notices (design §2.3);
  - `of` is the tag of the id's current incarnation, and `rN` is one of its revisions at that place, not one beyond its latest;
  - every path is a user path for the block's kind (§1.5, last column), and the patch passes the overlap rule (§1.4).

  Then each path applies under its guard (§3.4).
  - **Another incarnation, or a base beyond the latest:** the message applies nothing, and its row says `not applied: the block was rewritten`.
  - **Anything else the grammar doesn't admit** is ordinary text. Its notice lines are still read (§3.5).
- **Self-describing, best-effort.** The readable line states the result, not the diff, so a compaction summary that sees user text has the substance to keep. A summary can still drop it (§5.2).
- **A typed look-alike from an input-holder counts.** That is design §4's limit 7, unchanged; §7 lists the spoofing tests.
- **What the agent never sees:** drafts, keystrokes, the hold, how long it took, which desk, folds, pins, slots, or which revision was viewed (design §5.6).

### 3.4 Conflict rules: text and order are the user's, execution truth is the agent's

**The user's paths and their guards.** A guard is checked against the incarnation's state at the place where the edit lands. A path that fails its guard is a **conflict**: it isn't applied, and the edit's outcome says why (§3.7).

| Kind | Path | Who writes it | A user write applies only if |
|---|---|---|---|
| plan | `steps.X.label` | agent, user | X is live |
| plan | `order.steps` | agent, user | always: a permutation of the live ids it names (§1.4) |
| plan | `steps.X.state` = `skipped` | agent, user | X is `todo` |
| plan | `steps.X.state` = `todo` (an unskip) | agent, user | X is `skipped` |
| plan | `steps.X.state` = `doing`, `done`, `failed` | agent only | — |
| plan | `steps.X.stop` | user only | X is `todo` or `skipped` |
| plan | `steps.X.note` | agent only | — |
| plan | `append steps` | agent; the user with `u-` ids, state `todo`, no `stop` | the list stays ≤ 20, and the id is unused in the incarnation |
| note | `text`, `title` | agent, user | the note is live |
| diff | `hunks.H.state` = `kept` or `dropped` | user | the diff's `state` is `proposed`, and H is live |

**A hunk's state is the user's.** The agent writes a hunk `open`, in a `put`, a `replace` or an `append`, and never sets one. The extension refuses an agent's `kept` or `dropped` by `set`, by `state` sugar or in a hunk it writes, and an agent's `set` back to `open` too: a keep or a drop is the user's decision, not the agent's report. To withdraw a hunk, the agent removes it.

**Execution truth.** A step's `doing`, `done` and `failed` are the agent's report of what ran, and nothing the user hands back changes them. A skip or a stop that lands after the step started is a conflict, and the row says `deploy already ran`. The description tells the agent the same rule (§2.6), so "skip deploy" in line 1 over a finished step reads as what the user wanted, not as what happened.

**Agent ops that arrive while a block is held:**
1. **The fold applies them.** They are in history, and every desk agrees. **The holding desk draws the draft** over `B`, and counts the agent's changes since `B` in the header. Other desks draw the fold.
2. **On hand-back** the draft becomes a patch of the user's changes since `B`, by item id and path, never a whole body. After the pre-check (§3.2), it applies at its place in history, after the agent's ops, path by path, under the guards.
   - The agent's changes to paths the user didn't touch survive. A user reorder keeps the agent's `fix → done`.
   - On a text or order path both touched, the user's wins, because it's later. This is wild2's "rebase", as a pure fold.
3. **An agent `replace` or `remove` while held** ends the incarnation, and the draft goes stale (§3.2).
4. **An agent `move` or `collapse` while held** waits, in drawing only, until the hold ends: a held block never moves or folds under the user.
5. **On cancel,** the view shows the fold: every held change at once, with one glow.
6. **Afterwards.** Once the agent overwrites a path whose last writer was the user, that block's header carries `changed your edit` (with the path on hover) until the user next focuses it. That is honest, and silent otherwise.

**The traces B2 tests:**
- `deploy` is `todo` at r4. The user takes the block and strikes it, while the agent ticks it `doing` (r5), then `done` (r6).
  - Handed back after r5 is drawn, the pre-check drops the skip.
  - Sent before r5 landed, the fold skips it as a conflict.
  - Either way `deploy` stays `done`, and the agent reads the user's wish in line 1.
- The user reorders while `test` goes `doing`: the order applies, and `test` stays `doing`.
- The agent replaces `plan`, keeping item id `fix` for a new task: there's a new tag, the old draft goes stale, and nothing lands on the new plan.
- The agent removes and re-puts `plan`: the same.
- The agent removes `steps.fix`, then appends a new `fix`: the append is skipped (§1.4).
- Two desks each add a step: the ids differ, and both land. A forced collision skips one, visibly, on every desk.
- A restore across each of these: §3.6's rules.

### 3.5 Dismiss, show, pin, slot, collapse

**Dismiss** (decision 2: the agent is told):
- **The act:** `x` in block walk, or `×` in the header's hover actions. The desk draws it dismissed at once, on this desk:
  - **in the transcript,** the block shrinks to a fixed 24 px tombstone, `◇ tests · dismissed · u show` (workspace);
  - **in the rail,** the section goes, and the zone's footer counts `1 dismissed · show`.
- **Telling the agent, never waking it.** A dismissal becomes a **notice**, one line, `[block tests v3 dismissed]` (the version is below). It rides the next message the user sends on that session anyway:
  - **typed text:** a `Submit` when idle, or a `Steer` while a turn runs, which the engine delivers at its next round like any steer (`CLI:ae/runtime/stream.rs:3165-3170`);
  - **a hand-back.**

  The desk appends it as the message's last line when it sends (the composer's send, `DESK:sd/app.rs:4819`; D2a's, `DESK:sd/answer.rs:629`). The composer shows what will ride as a chip, `+ tells the agent: tests dismissed ×`, and `×` keeps the notice back for a later message.
- **Never on its own.** No message is ever sent just for a notice, and no notice rides a card answer:
  - **A notice-only `Submit`** would start a turn (`CLI:ae/session/actor.rs:5440-5447`).
  - **A notice-only `Steer`** would take the one queued slot from the user's own steer (`actor.rs:3409`). And a steer that misses the turn's last drain stays queued and starts the next turn (`actor.rs:3391-3395`, `:4119-4120`, `After::AutoSendQueued`). The desk can't close that race.
  - **A card answer** must be all grammar (design §3.3). A desk from before blocks would read an answer with a trailer as ordinary text, and leave the card open.
  - **A `before_message` hook** could add the line without a message (`CLI:docs/extensions/protocol.md:737-760`). But it needs `privacy.llm_content` (`:1069`) and extension state, and cards refuses both (design §1.2).

  So a dismissal reaches the agent with the user's next words. Until then the agent may keep patching the block, unseen. Those tokens buy nothing, and B5 counts them (§2.8).
- **The exact text.** `[block <id> v<N>[, <id> v<N>…] dismissed]`, and the same with `shown`. Each line holds at most 8 ids. A message carries at most one line of each kind, as its last lines, after a newline:
  `^\[block ([a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}(, [a-z][a-z0-9_-]{0,23} v[1-9][0-9]{0,8}){0,7}) (dismissed|shown)\]$`.
  When more than 8 are waiting, the rest ride the next message. A line with one id is ~7 tokens, input only.
- **The version.** Each conversation generation (§1.2) counts its visibility changes. A desk mints a notice's version as one more than the highest it has seen: in any notice of this generation's history (the fold keeps that number), and in its own desires (below). Two desks may mint the same number, for different ids or for the same one.
- **The fold** keeps a **visibility record** per id: dismissed or shown, and the version that set it (§1.8). It reads notices from the last lines of a genuine user message, after that message's edit (§1.7). For each id a notice names:
  - **It applies** only if its version is above `effective_version(id)`: the version of the id's record if it has one, else the **floor**. It sets the record whether or not a block is live under the id. This one helper serves the fold's admission and every desk rule below (attaching, settling, reconciling at a restart, the feedback), so they can't disagree.
  - **Otherwise it's stale,** and changes nothing. The op log says `stale v3: v5 is later`. Of two equal versions, the first in history wins.
  - **An absent id is recorded** like a live one. So a block the agent removed before the dismissal landed stays dismissed when it's put again, and what the agent read agrees with the board.
  - **An id named twice** in one message's notices, in one line or across both, applies neither, and is marked. The desk never builds such a message.
  - **The floor** is the highest version of any evicted record (§1.8 keeps 64). It keeps an evicted record's old notices stale. The cost: a notice for an id with no record, minted before 64 other ids' changes landed, is stale too, and its desk says so (below). The agent isn't told about the floor, so it may read such a notice as current until the user's next dismiss or show of that id.
  - **Wrappers stay whole.** Only the user's own text is read, by the host's rule (`SPEC:cs/fold.rs:302-343`, `:362-428`). That rules out three things:
    - an `<event …>` payload, which ends with `</event>` (`DESK:sc/events.rs:261-263`);
    - the summary message, whose last line is the host's (`CLI:crates/agent-core/src/core/compaction.rs:213-229`);
    - an extension's saved block, which that rule removes first.

    A typed look-alike counts, as for edits.
- **The desk keeps a desire per id, never a queue of notices.** For each id this desk changed, it keeps the latest desired state and its version (`block-desires.json`, §1.8), and in memory the carrier the notice went on, if any. At every send, it attaches a notice for each id whose desire is above `effective_version(id)` and isn't already in flight:
  - **Only the latest desire is ever attached.** Dismiss and then Show attach at most the Show, so a superseded notice is never re-armed.
  - **Accepted means written.** The caps are checked in memory, and the desire is written before the dismiss or show draws. A failed write refuses it with the error, so a restart never loses a desire the user saw accepted.
  - **In flight** means sent on a message that no settled history has shown yet, or lost. For a hand-back, its memo entry says which (§3.7). A settled history without the carrier (a dropped steer, a refused send, a rollback) clears the carrier, and the desire rides the next message at the same version.
  - **Settled.** Once `effective_version(id)` is at or above the desire's version, the desire is done and dropped. A retained record below it keeps the desire waiting, however high the floor: `tests` at v2 with the floor at 100 still takes a delayed Show at v3. If the shared state differs from the desire, the block or its tombstone says why and draws the shared state: `changed by a later message` when another desk's notice or a typed one landed first, or `not applied · dismiss again` when the id has no record and the floor made it stale.
  - **A desk restart** keeps the desires and loses the carriers. It reconciles them against the history it attaches to: a desire at or below `effective_version(id)` is done, and the rest ride again. A copy still queued in the daemon may land as well, and the second of two equal versions is stale.
  - **One rule for old notices.** Notice lines are never part of a message's identity. Every send builds its trailer from this desk's desires at that moment, a Resend included (§3.7), so no send carries a wish older than this desk's latest for that id. But a message already sent can't change, and an unsent wish can't touch the shared fold. So if an older notice lands first (a carrier still queued in the daemon, or another desk's), it applies: other desks and the agent see it until the newer wish lands, while this desk draws its own desire. The newer wish rides this desk's next send, and the higher version wins whichever lands later.
- **Shared once delivered.** Before delivery, a dismissal is this desk's: it draws at once here, and other desks draw the record. Once in history, it's every desk's, and another desk shows the block dismissed when that history lands (§5.4).
- **Show again:** `u` on the tombstone, or `show` in the rail's footer.
  - **A dismissal not yet sent** is simply cancelled: the desire goes, and nothing is sent.
  - **One in flight or in history:** the desk draws the block at once on this desk, and the desire becomes `shown` at a new version, which rides the next message. Whichever order the two land in, the later version wins. A shown block draws its current state.
- **What the fold does with a dismissed id:**
  - `patch`, `replace`, `collapse` and `remove` apply, unseen: no glow, no change mark, and the tombstone stays static.
  - `move` is skipped, so a dismissed block can't be moved back into view.
  - A `put` after a remove makes a new incarnation that stays dismissed.
  - A dismissed block can't join a group.
  - A dismissal belongs to the id, for the generation, until a later version shows it. Records are capped at 64 (§1.8).
- **What the agent may do after a dismissal** (§2.6): stop patching it, never put or move that id again, and never recreate it under another id. It may remove it. `[block <id> v<N> shown]` means the block is back, to be brought up to date. The desk can't enforce "not under another id", so B5 watches for it, and counts the patches that still come.
- **Across a compaction,** the successor's history holds no records, because it's a new generation. The desk carries the predecessor's dismissed ids across the compaction (`CompactionApplied`, `DESK:sc/transcript.rs:1798-1803`; a linked successor names its predecessor, `DESK:sd/favourites.rs:600`), but only as **soft** dismissals. A block put again with one of those ids draws as its tombstone, `dismissed before compaction · u show`, never fully hidden, because it may be a different block. Soft dismissals are local, and `x` on one sends a real notice, versioned in the new generation.
- **Tests (B2, D6):**
  - dismiss and Show while a carrier is queued;
  - a lost carrier, then Show, then the hand-back's Resend: it carries `shown` at the new version, never the old `dismissed`;
  - an old carrier landing before the newer wish is sent: it applies on every desk until the next send carries the newer one;
  - two desks' opposite notices at one version;
  - the agent removing the block between the click and delivery, then putting it again;
  - one id in both trailers;
  - a restart and a compaction with a carrier in flight;
  - 65 ids' changes, and the floor; a retained record at v2, the floor at 100, and a delayed Show at v3: admitted, attached and settled alike;
  - `<event>`, summary and saved-block wrappers ending in a line that looks like a notice;
  - never a notice sent on its own.

**Pin** (`p`) moves a transcript block to the rail's upper slot, and `p` again brings it back.
- It is local, and persisted in `blocks.json`.
- The transcript keeps a 24 px stub: `◇ plan · in the rail · p`.
- A user pin outranks the agent's `move`. The stub then adds "the agent moved it", and unpinning takes the agent's placement.

**Slot** (`m` in block walk, or `⇅` in a rail section's hover actions) moves a rail section between `upper` and `end` (§4).
- It is local, and persisted in `blocks.json` by id.
- It outranks the agent's `slot` and `move` until the user presses `m` again or picks "reset placement", and the section then takes the agent's placement. The hover note says `moved by you`.
- A group moves with its members. On a member, `m` is off: `in a group · move the group`.

**Collapse** (`z`/`c`, or the chevron) is local, and **the user's choice wins until the agent's next `collapse` op on that block** (widgets). It isn't persisted: it's cheap to redo, and stale folds after a restart would hide news.

**Unpack** shows a group's members in place, locally (wild2).

**Undo.** `Ctrl/Cmd+Z` in block walk undoes the last local action, one level per block: a collapse, an unpack, a pin, a slot move, or a dismissal not yet sent, which cancels its desire. A delivered dismissal is undone with Show. Sent content can't be undone: hand back another edit, or restore.

**The desk's two stores,** keyed by conversation (the transcript's journal id), with the caps of §1.8:
- **`blocks.json` is presentation:** pins, slot moves and soft dismissals. Past a cap, the oldest goes, then the least recently touched conversation.
- **`block-desires.json` is the user's unsent wishes,** in their own reserved budget. Nothing evicts one, and a cap refuses the new one. Dropping a conversation from `blocks.json` leaves its desires alone.
- Both use recall's writer: atomic, mode 0600, swept (`DESK:sd/recall.rs:44-63`).

### 3.6 Revisions

- **The header chip** `rN` reads `r5`, or `r1 · replaced` (with `why` on hover).
- **Walking.** `[` and `]` step through the current incarnation's revisions, then, behind a `— replaced —` divider, the revisions of the id's earlier incarnations that the board still holds (§1.8). The header reads `r3 of 5 · agent · 07:41`: who, and when, from the message's `_synaps_at` (`DESK:sc/transcript.rs:69-77`). A live row uses its event time.
- **No yanking.** An agent op landing while an old revision is shown doesn't move the view. A dot `r6 new` appears, and `]` or `Esc` returns to the latest.
- **Restore** (`R`, or a button on a past revision) works on the **current incarnation only**:
  - The desk builds an ordinary edit (§3.3): the user paths of rN that differ from the current state, as `set` and `order`. The base is the current revision, and the summary says `back to rN: …`.
  - It gets the same pre-check, the same guards and the same input gate. So execution state and notes don't come back, items removed since rN don't come back (their ids are spent, §1.4), and hunks come back only while the diff is a proposal.
  - **An earlier incarnation is view-only.** Its text can be selected and copied, never restored: it was another block.
- **No r0.** v1's "before compaction" revision is gone (§5.2).

### 3.7 Delivery and who may edit

- **Sending.** A hand-back takes the card path exactly: `Steer` when busy, `Submit` when idle, on the block's Live attachment (`client.on(seq)`), through D2a's send (`DESK:sd/answer.rs:619-636`).
- **Two layers, never mixed:**
  - **Delivery** (`cards_spec::memo`, generalized by B4 to a subject) answers "did my message reach the chat?".
    - There is one entry per hand-back, matched by the exact text of its edit: every line but the notice lines, which belong to the desires and are rebuilt at every send (§3.5).
    - Its binding is (block id, tag, base) as sent. **Agent ticks never move it**: the base is what was sent, not the block's current revision.
    - Its states are the cards': sending, queued, held, checking, in the chat, unconfirmed and not sent. Resend is only by a click, and only of the exact edit text (design §3.5), with a trailer built from the desires at that moment.
  - **Application** (`cards_spec::board`) answers "what did it do?". For each hand-back in history, it records each path as applied, as a conflict (and why), or as later overwritten by the agent. The hand-back's row draws it: `applied 3 of 4 · deploy already ran`.
  - "In the chat" never means "applied".
- **Supersession is by coverage.**
  - The cards rule, that a later answer supersedes (`SPEC:cs/memo.rs:186-244`, rule 4), assumes whole answers. A block edit is partial.
  - **Coverage keys** are an edit's intents, item by item:
    - each `set` path;
    - each appended item: appending `u-k3x7qa` to `steps` is the key `steps.u-k3x7qa`;
    - each ordered list, such as `order steps`.

    Appended hunk lines and series numbers have no key. Each such append is its own intent, and nothing covers it. Notice lines aren't keys either: they belong to the desire (§3.5).
  - **A later edit** to the same incarnation retires an entry only if its keys include every key of the entry's patch. So appending one step never retires the append of another. That holds whoever sent it: another desk's covering edit counts, and the row says so.
  - **A disjoint or partial later edit** leaves the entry, with its Resend.
  - **Discard** retires an entry deliberately (`Memo::forget`, `memo.rs:464`), as D2a's reconciliation does for cards (`DESK:sd/deliver.rs:585-600`).
- **When the incarnation ends** (a replace, a remove, a compaction), the delivery entry carries on until it settles, since the message is still on its way or in the chat. The application layer says `the block was rewritten` or `the block is gone`, and Resend is off, because a resend would apply nothing.
- **Bounds** (§1.8): 32 block-edit entries per session. Settled ones go first, and recoverable ones never go: with 32 of those, Hand back is off until one is resent or discarded. The card memo keeps its own 32, under its own rule (`SPEC:cs/memo.rs:44`, `:325-340`).
- **Identical texts.** The memo's seen-prefix rule (`memo.rs:179-206`) matches ours past what this desk had seen when it sent, as for card answers.
- **One edit in flight per block, per desk.** While one is, the block shows "Sending…" and takes no new hold. The sending desk draws the edit over the board, marked `sending`, until the board holds it (§1.7). That's a drawing only, and never part of the fold.
- **B4's matrix:**
  - the queued slot displaced (`Dequeued`);
  - a history rollback (a settled history without ours);
  - `DriverRevoked`;
  - an agent tick while sending;
  - another desk's disjoint edit, and its covering edit;
  - Discard;
  - compaction while queued;
  - a reattach mid-send;
  - two identical texts.
- **Two split panes on one session** share the Live, its memo, its holds and its desires (`DESK:sd/live.rs`; design §3.6). A block held in one pane shows `EDITING` in the other.

### 3.8 Keys and mouse

- **Block walk:**
  - **Entering.** `Ctrl/Cmd+G` enters block walk in the focused pane. It is unbound on `main`, as is `Ctrl/Cmd+O`. Ctrl+J stays "what's waiting for you" (design §6.1).
  - **Walking.** It starts on the newest block in view. `j`/`k` (or `↓`/`↑`) walk this pane's blocks in reading order, then the rail's agent sections, upper slot first.
  - **Acting** on the block under the walk:
    - `e` takes it;
    - `z` or `c` folds it;
    - `x` dismisses it;
    - `p` pins it;
    - `m` moves a rail section to the other slot;
    - `[` and `]` walk its revisions;
    - `R` restores a revision;
    - `u` shows a dismissed block again.
  - **Leaving.** `Esc` returns to the composer, never to the window, so a second `Esc` can't deny a prompt (design §5.1).
  - **Activation** is the entered-card rule: keys act from the first frame that paints the walk ring, and only fresh bare presses count (`DESK:sd/ask_keys.rs:17-40`).
  - **Every key acts on the painted target** (§6.2): the walked block's id and tag, and the item under the step cursor, as the ring last painted them. If the board's incarnation or item differs now, the key does nothing and the block says why. A key names its target by identity, never by position, so the just-moved rule (§6.2) never blocks one.
- **A status line** shows the current mode's keys, as point's does.
- **Mouse:** hovering a header shows `rev · edit · rail · ⇅ · fold · ×`. These **overlay the header's right end and never reflow it** (point). A chevron folds, and a drag grip moves a plan step.
- **`Ctrl/Cmd+O` opens the op log,** the mutation stream (wild2, point's WIRE).
  - It is an overlay in `DESK:sd/overlay.rs`'s frame, listing the board's latest 256 ops: time, who, op, id, and the outcome (applied, skipped and why, no change, or an unbound call). Clicking one jumps to its block.
  - It is derived and read-only. It is also where an agent's skipped op, which the agent never learns of, becomes visible.
  - Its "before compaction" tab is the archive (§5.2).

---

## 4. The rail (decision 1: two slots)

- **Two slots.** The right panel today is Context, Tokens, Prompt cache, then Files (`DESK:sd/chrome.rs:2330-2659`; assembled at `:2630-2659`). Agent sections go in one of two places:
  - **`upper`,** after Prompt cache and before Files. It is the default, for what's live now: a plan, a status.
  - **`end`,** after Files, last in the panel: reference that can sit below a long Files list, such as a metric's history or a findings note.
  - The agent picks a slot per section (§1.2). Files is the one open-ended section, up to 40 files plus the shell list (`chrome.rs:34`, `:2541-2629`), and the panel already scrolls (`chrome.rs:2635`).
- **The look:**
  - The upper slot opens with a `section(...)` header in the desk's own style: `AGENT`, the theme accent dot, and `N sections · M dismissed`. Then comes **NOW** (below), one fixed 22 px mono line.
  - The end slot opens with a thin `AGENT` divider, with no NOW.
  - Each section has a 28 px header with the kind glyph, the title, an `AGENT` chip, `rN`, the chevron and `×`, then a body of ≤ 12 lines in its kind's layout.
- **Order within a slot:** the user's first (pins and slot moves, in the order the user made them), then the agent's in placement order. Each placing op puts its section last in its slot, unless `at.after` names a section in the same slot.
- **The cap: 4 expanded, across both slots together.** The desk walks the sections in drawing order, upper then end:
  - a section the user expanded is expanded, and takes a place first;
  - one the user collapsed draws as its header plus one summary line (plan `3/5 done`, metric `1.0 · was 2.0`, status its text), and takes no place;
  - the rest are expanded in order while places remain.

  Each slot draws its own leftovers as one `+2 more · plan, tests` row, and clicking it lists their headers. Nothing is ever dropped.
- **A user expansion past the cap** (M4, decided here):
  - **If an agent-expanded section holds a place,** the last one in drawing order folds into its slot's `+N` row, as before.
  - **If all four places are the user's,** the user's **oldest** expansion folds, and the newest click wins. "Oldest" is by a counter on this desk that each expansion takes, so there are no ties. The folded section goes back to the agent's default, and expands again when a place frees.
  - **Why fold rather than refuse.** An expansion is a deliberate act on one section. Refusing it would send the user off to fold another one first. The cap exists to bound the panel, not to overrule a click, and folding the one opened longest ago is the accordion rule people already know. Expansions are local presentation, so no other desk has to agree.
  - **The desk's own sections** (Context, Tokens, Prompt cache, Files) never count toward the cap, and never fold for an agent section.
  - **Under the lease,** the user's own expansion applies at once, since it's the click. The clicked header keeps its screen y, and a section folding above it scrolls the panel instead, as far as the panel can scroll. The victim's fold is the one change the lease doesn't defer.
  - **Tests (D6):** four user expansions spread across upper and end, then a fifth in either slot; a collapse, and a "reset placement", each freeing a place; a fifth expansion while another section is leased.
- **A group in the rail** sits in one slot, with all its members (§1.3).
- **Overflow per kind,** in 20 px lines, ≤ 12 per section:
  - **plan:** ≤ 11 step rows, a window around the first step not done, plus one line `+8 more · 3 done`. The section scrolls inside on the wheel;
  - **diff:** 10 lines, then `+N lines`;
  - **note:** 12 lines, then `more`, which is the user's own resize;
  - **status** 2 lines, **metric** 3, **file** 1.
- **Under the pointer lease** (§6.2), cap changes, additions, removals and reorders in the rail wait, except the fold that a user's own expansion causes (above).
- **Fit.** Sections fit the panel's floor of 296 px (`DESK:sd/app.rs:679`), so 248 px of content inside the 24 px padding. Values ellipsize, and a metric's sparkline is ≤ 32 quads.
- **Collapse and dismiss** each section (§3.5).
- **A folded panel, or a window too narrow for it** (`Side::Strip`, `DESK:sd/app.rs:744-748`; the centre keeps 480 px, `:299-300`):
  - The strip (`chrome.rs:1457-1459`) gains a `◆3` count under its context gauge.
  - Clicking it opens the panel.
  - Rail sections are also reachable from block walk, and `p` brings one into the transcript.
- **Split panes.** The panel describes the focused pane's session (`chrome.rs:2331`), and so does its zone.
- **The desk's own sections are out of reach.** They aren't in the board, no id names them, and an agent section always carries the `AGENT` chip and the agent frame. A block titled "PROMPT CACHE" still reads as the agent's.
- **NOW**, derived by the desk, takes the first that applies:
  1. a permission prompt waiting: `waiting for you · allow bash`;
  2. an open card the agent waits on: `waiting for your answer`;
  3. running calls: the verb and target of the **earliest-started** call still running, plus `+2 running · 1 queued` (`ToolStarted`, `ToolQueued`). The label stays on that call while it runs, so it doesn't flicker as parallel reads come and go;
  4. writing; thinking; idle.

  This is Ctrl+J's order (`DESK:sd/waiting.rs:1-5`). NOW beats on the window's 8 Hz heartbeat only while the turn streams.

---

## 5. History, replay, compaction, several desks, the TUI

### 5.1 Replay and restart

- **One fold builds the board, from the Full history.** The desk runs Full (design §2.1). It keeps block calls' input whole, as it keeps card inputs past the 4 KiB preview cut (`DESK:sc/transcript.rs:63`, `:336-342`).
- **A mid-turn attach** gets the calls from the replay ring, as any row does. **A late desk** draws exactly what the others draw, dismissals included once delivered, minus their local presentation.
- **A daemon restart** reloads the journal. An op or an edit that wasn't saved is lost as any message is (design §4). A hand-back the desk can't confirm shows Resend while the memo holds it (§3.7).
- **A desk restart** keeps what the two stores hold: pins, slots and soft dismissals, and every desire. It loses drafts, folds, the memo and the archive, as cards lose the memo (design §4).

### 5.2 Compaction

- **What survives: the summary, and nothing on the board.** The successor's history is the summary, plus pending events and the queued message (§1.9). So every block and card is gone, and the rail's agent zone empties.
- **The same id is a new block.** The agent learns of compaction from the `<context-summary>`, and the description tells it to put again only what's still useful. Reusing ids is fine, but a re-put id is a new incarnation (§1.2), and the desk never treats it as the old block. Pins and slots carry over by id, as preferences; dismissals carry over only as soft tombstones (§3.5).
- **The summary is best-effort.** It is either a model's summary of everything, or the local fallback, which keeps 300-byte excerpts of the last 4 messages (`CLI:ae/runtime/compaction.rs:490-533`). A disclosure policy can exclude user text altogether (`compaction.rs:105-110`). Line 1 is written in words to give a hand-back a chance of surviving, not as a promise.
- **A held draft** whose block vanished becomes an **orphan**. Its header reads `compacted · this draft has no block`, with Copy to composer and Discard (§3.2). Orphans count with holds: ≤ 8 drafts per session, across compactions, never 8 more per generation (§1.8). They stay until discarded or until the desk restarts.
- **A hand-back in flight** (sending or queued) when compaction lands: the queued message moves to the successor (`compaction.rs:836-840`), where its id names no block. It is ordinary text to the board, and still words to the agent. The memo follows its delivery as usual, and the row says `delivered · its block was compacted away`.
- **Pending desires** for the predecessor's blocks are dropped, and its dismissed ones become soft dismissals (§3.5). A carrier still in flight lands in the successor, because the queued message moves (`compaction.rs:836-840`). There its notices count as the new generation's, so the fold and the agent read the same lines.
- **The archive** replaces v1's r0. The desk keeps **one per session**: the predecessor's final blocks and op log, read-only, ≤ 256 KiB, in the op log's "before compaction" tab (§3.8). The next compaction replaces it, so there's never a chain. It is never a revision of a new block, never restorable and never matched by id, and its text can be copied. A desk restart loses it.
- **Tests:** a compaction during a hold, during a send, while queued; successive compactions while holding drafts (never more than 8, none dropped); a desk restart and a reattach during compaction.

### 5.3 Daemon reload, crash, cancel

- The extension holds nothing in flight (design §1.6), and neither do block calls.
- **A Cancel** keeps paired results in the history (`CLI:ae/session/actor.rs:3495-3504`), so a bound call stays bound. A call the history lacks isn't on the board: the live overlay that drew it was only an overlay (§1.7).
- A held answer or a dequeued hand-back is the memo's (design §3.4 d4–d5).

### 5.4 Two desks, one session

- **Content agrees,** and so do delivered dismissals. Both desks fold the same history, so they draw the same blocks, revisions, edits and outcomes.
- **Presentation doesn't.** Pins, slots and folds are each desk's own, and so is a dismissal until it is delivered.
- **Holds are per desk, and nobody "holds" a block globally.** There is no core channel for one desk to tell another, and a history line would reach the agent (§0.2).
  - If another hand-back lands while desk A holds the same block, A's header shows `another edit arrived · r7`. It doesn't say "on another desk": the transcript doesn't say who sent a message.
  - A's draft stays a patch over its base. When A hands back, its paths apply after the other edit's, under the guards. On a text or order path both touched, A's (later) wins, and A's header said so before A sent.
  - The agent reads both messages in order. The latest wins, as for card answers (design §2.3, §3.6).
- **Would need core:** a client-to-client presence frame, so other desks could show "held on another desk". It is listed beside design §8, not designed around.

### 5.5 The TUI, older desks, Digest clients

| Client | Sees |
|---|---|
| **The TUI** | The generic tool panel (`CLI:crates/agent-tui/src/tui/render.rs:552-560`): `cards_block`, its ops JSON, `ok 3 ops (blocks/1 #…)`. Desk hand-backs show as two-line user messages that read well, and a desk user's notice lines as part of their message. Nothing to answer, nothing lost: the agent's prose still carries the turn |
| **A desk before D5** | Ordinary tool rows, which fold in runs of ≥ 4 (`DESK:sd/turns.rs:31`). Harmless and noisy, which is one more reason for the off-by-default gate (§1.1) |
| **A Digest client** | Plain panels, inputs only (design §2.1) |

**A TUI-only user** should set `blocks` to false: every op would cost them tokens they never see drawn. The tool can't know who is attached (design §8 #2). Until B6, nothing changes for them, as the gate is off. **When the default flips (B6),** the opt-out leads the release note and the README's configuration section, with the setting to change (`extension.cards.blocks` to `false`, §1.1). That only tells them how to stop the spend; it doesn't make the spend useful to them.

**Later, not v1:** a TUI HUD of the rail. The extension sees every rail op it validates, and `widget.upsert` frames reach daemon-mode TUIs per session (`CLI:ae/extensions/notify_router.rs:1-20`). It would be live only, never replayed, and blind to desk presentation.

---

## 6. Motion and look

### 6.1 Look

- **The cards-D language** (`DESK:sd/cards.rs:1-10`): square corners everywhere, read by fill (`card_d`) with no outline, Sans for words and mono for targets and values.
- **A living block** marks itself with a **2 px spine in the theme accent** on its left edge. That isn't an outline, and it's the one sign that the agent can still change it. A removed or tombstoned block loses the spine.
- **The header** is 28 px: the kind glyph (the life glyph family, `DESK:sd/glyph.rs`), the title, then `rN`, a state word and the chevron, right-aligned.
- **A held block** turns the spine and the ring to the focus colour, and reads `EDITING`.
- **A pending op** (§1.7) draws its change with a dim `pending` mark and no glow. The glow comes when it binds.
- **Never used:** the authorize card's warning colours and icon (design §5.2).
- **Subagent boxes** keep their own theme colour, and blocks use the accent, so the two never read as each other.

### 6.2 Heights, and the pointer lease

**Heights come from structure and bounded line counts:**

| Kind | Height |
|---|---|
| plan | header + steps × 28 px. A label ellipsizes, and its note sits in the same row, right, dim |
| status | header + 2 fixed lines (text, then detail and the progress meter) |
| diff | header + min(lines, 12) × 20 px. Past 12, the well scrolls inside and the box stops growing. Appends keep the well pinned to its newest line unless the user scrolled it |
| note | header + min(wrapped lines, 6) × 20 px. This one depends on content, bounded at 6. Past 6, `more` opens it, which is the user's own resize |
| metric | header + 44 px (the value, and the label with `was`), + 16 px with a series |
| file | header only (the path, `:line`, the note) |
| group, tombstone, stub, a collapsed block | 24–28 px, one line |

So an agent op can change a block's height only by changing its structure (a step added, a block unfolded) or a note's bounded line count.

**Above the viewport,** the list keeps its anchor (`DESK:sd/app.rs:1312-1330`, the hold, or the follow it keeps). A pill, `2 blocks changed above ↑`, sits at the list's top edge (a layer, as sticky headers are, `DESK:sd/sticky.rs:1-9`). Clicking it walks to them.

**The pointer lease.** It starts when the pointer moves over the transcript list or the rail's agent zone. It lasts while the pointer moves, and for 1.5 s after it last moved.
- **The promise is the weaker one, and it's stated.** Nothing moves under a *moving* or pressed pointer. A parked pointer releases the lease. Otherwise a mouse left over the transcript would freeze the agent's changes under it for as long as the user is away. After the lease, a click is protected by the painted target and the just-moved rule, below.
- **Position.** The row under the pointer keeps its screen y. If something above it grows or shrinks, the content above scrolls instead: the list's anchor becomes that row and its offset, the existing hold extended. Nothing under the pointer slides.
- **Identity.** A change that would put something else under the pointer isn't drawn yet:
  - a `move` or `remove` of that block, or a `merge` taking it;
  - **any** `replace` of it, of another kind or the same, because a new incarnation is another block;
  - a group dissolving;
  - an item removed or reordered under the pointer;
  - a rail cap change or reorder.

  The old geometry stays drawn, with a `changed` dot.
- **In place.** A change that keeps the structure (a tick, a label) draws in place. A row that would be added clips behind `+1`, and a removed one leaves its space.
- **The painted target.** Every hit region carries what it was painted as: the block id, its incarnation tag, the item id if there is one, and, on a tombstone or in the rail footer's list, the id's visibility version (§3.5). A click or a key acts only if the bound board still holds that target: the same tag, the item still live, the same version.
  - **Otherwise it does nothing.** It never redirects to whatever holds the name now. The block or its tombstone says `this block changed · look again` until the next input, so the note needs no timer.
  - **One rule for every action,** so Edit and Restore route as fold and pin do:
    - content: take, restore, keep or drop, strike, stop;
    - shared: dismiss, show;
    - presentation: fold, pin, slot, unpack.
- **Down and up.** The target is taken at pointer-down, and pointer-up acts only on that same target, if it's still current. **A press holds the lease** until pointer-up, so nothing deferred draws under a held button, and a long press can't expire into a no-op. The bound board can still change between the two (a settled history, a rollback): a target that changed there makes the click a no-op, with the same note.
- **Just moved.** A pointer-down on a target that **an agent op** moved or replaced in the last 600 ms does nothing, once, and says `this moved just now`. It's never queued to act later: the user clicks again.
  - **Moved** means its painted screen rect or its incarnation changed in a frame that drew an agent op. Anchoring that keeps it in place on screen isn't a move.
  - **An agent op** is an op applied to the board or the overlay, or a deferred change released (below). The user's own changes never start the window: a scroll, a window or pane resize, their own expand, fold, pin, slot or edit, a walk. A frame that draws both counts as the agent's: the safe side, and rare.
  - **A slide counts once:** the window runs 600 ms from the end of its motion, not from each frame of it.
  - **Keys are exempt,** and so are `Esc`, navigation, text fields and IME. A key acts on the walked target by identity (§3.8), so a displacement can't misdirect it.
  - 600 ms is time to see the change, with margin over a visual reaction (~250 ms). It's the idea behind a browser's delay before a security dialog's buttons take input.
  - It's a timestamp per hit region, compared when input arrives: no timer and no frame.
- **Release.** When the lease ends, the pointer leaves, or a key acts, everything that waited draws at once, with one glow, and the targets it moves or replaces count as just moved. A moved block slides (Full) or appears (Reduced, Off), as §6.3 says.
- **Keys** act on the walked block's painted target, never on a position. The key is checked first, and the lease is released after. A key on a stale target is a no-op with the note, and the release then shows why.
- **Frames.** The lease's end is **one timer wake**, 1.5 s after the last move, armed only while a change waits under the lease. It draws that change and its glow (≤ 480 ms). With nothing waiting, the lease arms nothing (§6.3).

D5 ships the lease and painted targets for the transcript, and D6 for the rail. Both land before B3 lets an agent change blocks.

**Tests:**
- block A, above a hovered block B, grows and shrinks;
- B is removed, moved or merged;
- B is removed and put again, or replaced by a block of the same kind, while its old view is painted: the click is a no-op with the note, and never lands on the new B;
- an item removed or replaced under the pointer;
- a resting pointer clicking 300 ms after an agent op moved its target (a no-op, once, never replayed), 700 ms after (it acts), and 300 ms after an op moved something else (it acts);
- the user's own change, then an action at once: a scroll then `e`, a walk that scrolls then a key, an expand then navigation, a fast resize then a click, `Esc` and IME input while rows move. All act;
- a press held 2 s while an agent op waits: it acts on what it pressed, and the op draws at release. A rollback that drops its target between down and up makes it a no-op;
- a key after a release, on a stale incarnation;
- the expiry wake, armed only with a change waiting;
- a 5th rail section promoted, and a fifth user expansion (§4);
- a full 20-step plan at rail width;
- split panes.

### 6.3 Motion (`DESK:sd/motion.rs`)

| Event | Full | Reduced | Off |
|---|---|---|---|
| A changed value or label | One afterglow: role → ink, decaying in three steps over 480 ms (`Motion::glow`, `motion.rs:2290-2298`, `GLOW` `:130`) | A 120 ms flash | — |
| A new block or row | `appear` (`motion.rs:324`) | Opacity only | At once |
| A tick todo → doing → done | The glyph changes. "doing" scans on the 8 Hz beat layer while the turn streams, repainting only that layer over the cached frame (`motion.rs:17-22`) | Still glyph | Still glyph |
| A move within view | One slide by the y delta (spatial) | At once | At once |
| A move from out of view | Gone at the source (anchored), `appear` at the destination | Same, opacity | At once |
| A rail section added or removed | A height change, deferred under the pointer lease (§6.2) | Same | Same |

**No frame demand at rest.** What blocks promise, and all they can promise: they ask for no frames once their last afterglow ends (≤ 480 ms). The one addition is the lease's expiry: a single wake, armed only while a change waits under the pointer, which ends in that change's own glow (§6.2). A "doing · turn ended" step is static. "doing" and NOW beat only while the turn streams, on the window's one frame source and its one 8 Hz beat (`motion.rs:9-22`), with no timer per block or per section.
- **The window idles** only when nothing else asks for frames. The focused caret, for one, blinks until 10 s after the last key (`DESK:sd/theme.rs:1551-1556`).
- **The beat layer saves the app's render and layout work, not the present.** GPUI presents whole frames (`review-pr-desk23-shady.md:151-153`). So D5's bench (`DESK:sd/bench.rs:42`) counts presents as well as app renders.

### 6.4 320 px

- **Width.** Every transcript block fits a 320 px pane. Narrow panes give way one order at a time, as the desk's do (#95): the plan's step notes go first, then `rN`, then the state word, keeping the glyph and the label.
  - A diff well scrolls sideways.
  - A metric stacks its value over its label.
  - A rail section's content width is ≥ 248 px (§4).
- **The top strip** (wild2's pinned mission) isn't in v1. D9 brings it with `blocks/2`'s `at:"top"`: one note or status, drawn as a fixed 2-line strip over the list in the sticky layer (`DESK:sd/sticky.rs`), which reflows nothing. The user can unpin it locally (`p`), and dismiss it like any block.

---

## 7. Security and bounds

- **A block's only effect is to draw.**
  - No link opens, no file opens, no command runs, and no key is bound.
  - Diff lines are inert mono text. Paths are text: the desk never reads them from disk (§1.5).
  - Notes are markdown-lite with links as plain text (design §5.4).
  - The only sends are the user's: hand-backs, restores, card answers, and notices riding the user's own messages. `cards_spec` builds all of their text.
- **It never resolves a permission prompt.** Hand-back and restore never call `answer(prompt_id, …)`, and only `SessionCommand::Answer` resolves one (design §5.1). D8 repeats the tests: with a confirm pending, a hand-back whose text is `yes` or `always` leaves it pending, and `Esc` in block walk never denies.
- **Provenance.**
  - Every block has the agent frame and the spine, and rail sections have the `AGENT` chip.
  - Agent strings never appear in chrome: the `EDITING` and `AGENT` labels, the counts, NOW and the tombstones are the desk's.
  - No block can take a user message's style, a desk section's chrome, or the authorize card's colours.
  - Every string goes through `cards_spec::sanitize`: control and bidi characters are refused, invisibles dropped, and ZWJ and ZWNJ kept (design §5.2).
- **Spoofing.** Only a genuine outer user message can be an edit or a notice. The host's unwrapping rule is reused (`SPEC:cs/fold.rs:302-343`, `:362-428`), so an assistant paragraph or a tool result can't be one.
  - Any input-holder can type the grammar, and in shared mode so can another authorized client. The transcript doesn't authenticate a click, a person or a desk, so the desk never presents an edit as verified provenance. That is the cards boundary, unchanged.
  - B2 tests assistant text, tool results, `<event>` and `<context-summary>` wrappers, saved extension blocks, and a genuine typed look-alike.
  - The trailing-line grammar is read only from the user's own text, with `<event>`, summary and saved-block wrappers whole (§3.5).
  - A typed look-alike can also spend the version counter. Past v999,999,999, dismissals stay on this desk until the next compaction, and the tombstone says so.
- **Bounds:**

| Bound | Limit |
|---|---|
| A call | 1–16 ops, ≤ 8 KiB compact, depth ≤ 6 per op, scalars counted (≤ 8 with `{"ops":[…]}`): §1.5's proposed diff has its lines 6 deep in its op |
| A block's body | the kind's cap (§1.5), ≤ 6 KiB |
| Blocks | ≤ 32 live per conversation, groups and members included |
| Groups | one level, 2–8 members |
| The rail | ≤ 4 expanded across both slots, ≤ 12 lines each |
| An edit message | line 1 ≤ 200 characters, line 2 ≤ 8 KiB as sent (read back normalized, it can be longer: §3.3), ≤ 2 notice lines of ≤ 8 ids |
| An incarnation tag | 128 bits, 32 hex digits (§1.2) |
| A notice | ≤ 8 ids a line, one line per kind, a version ≤ 999,999,999, an id once per message (§3.5) |
| Retained state | §1.8: the board's working set ≤ 1 MiB per conversation; the overlay, scratch, memo, drafts and archive capped per session; nothing recoverable evicted silently, and no desire evicted at all |
| `blocks.json` | ≤ 64 entries and 16 KiB per conversation, ≤ 256 conversations, ≤ 1 MiB |
| `block-desires.json` | ≤ 64 desires per conversation, ≤ 512 per desk, 64 KiB reserved; refused past a cap, never evicted |

- **Rate**, per chat (`chat_of`):
  - ≤ 30 calls a minute and ≤ 120 ops a minute;
  - ≤ 200 calls per 10 minutes.

  Past that, a teaching refusal. The limiter is in memory, so a restart resets it, which is acceptable for a brake (design §5.5). It slows churn, and §1.8 bounds what churn can leave behind.
- **What the agent sees of the user:**
  - **It sees** hand-back patches and restores (§3.3), dismiss and show notices (§3.5), and card answers.
  - **It never sees** drafts, holds, pins, slots, folds, timings or which desk (design §5.6).
  - Hand-backs and notices are user messages, so extensions with `privacy.llm_content` and memory extensions see them as they see typed text.

---

## 8. The PR plan

### 8.1 synaps-cards

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **B0** (this, v1.3) | `docs/blocks.md` | It fixes the contract before any type exists | — | ~1,114 lines of doc | — |
| **B1** `cards-spec::block` | The op and kind types; validation (`sanitize` reused); the path grammar, item ids and the `u-` reservation; the overlap rule; normalization with byte-ordered map keys; the 128-bit incarnation tag, the generation key and the creation number; the `blocks/1` envelope and digest; the edit grammar with `of`; the versioned notice grammar; `schema-blocks.json` | The extension and the desk share one validator | A valid and invalid fixture per op and kind; every cap at ±1; paths (indices, `u-` ids, depth); overlaps; golden digests and tags; byte-identical output with and without `preserve_order`; message and notice round trips, look-alikes, cut lines; every normative example in this doc as a round-trip fixture (L2); notice versions at their bounds | ~1.4k + ~1.2k | medium: a new grammar |
| **B2** `cards-spec::board` | The reference fold (§1.7–§1.8): commit at the result message; incarnations and item identity; the guards; groups; caps and eviction by owner (§1.8); revision records as references into history; visibility records, versions, the floor and `effective_version` (§3.5); edit outcomes; the op log; a re-fold API with a work budget | It replays real journals into boards. D5 builds on it | **Property tests:** retained size under the caps for adversarial streams (put/patch/remove loops, maximal bodies, item churn, merges); any split of a history equals the whole; live overlay = history fold = reattach fold; call, result and user interleavings; cancelled and unbound creates followed by patches; caps reached by pending calls; unknown catalogs; overlapping paths. **Traces:** §3.4's list; §3.5's notice list; §1.8's retention list, tombstone churn included; graph rules; `card:` ops on open cards skipped, receipts kept, the namespaces apart; §7's spoofing list | ~1.6k + ~2k | **high**: it is the protocol |
| **B3** `cards-ext` `block` | The second tool behind `extension.cards.blocks` (default off); batch validation; worker refusal; rate limits; the ≤ 64-byte result; the description (§2.6). It lands after D5 and D6 | **Agents can keep blocks**, opt-in. The TUI shows JSON panels | Framing; golden results; every refusal; the rate windows; the gate on and off in `initialize`; a smoke run in a real daemon (`scripts/smoke.py`) | ~350 + ~350 | low |
| **B5a** first measurement | B5's harness (§2.8) on the opt-in slice: read-only blocks and the rail. Its PR fixes the task set, the turn, the classifier and the counting method before any run | Chatter shows before the editors are built | The report, with the calibration of the proxy and the framing allowance | ~200 + notes | medium: little code, real measurement risk |
| **B4** memo subjects | `cards_spec::memo` over a subject: a card answer or a block edit. Delivery and application kept apart; supersession by coverage keys (§3.7); a binding agent ticks don't move; a block cap that never evicts a recoverable entry | D8's hand-back gets D2a's delivery truth | The S3 memo suite run again for block edits, plus §3.7's matrix; matching without notice lines, and a Resend's rebuilt trailer; two appends of different items; a memo full of recoverable entries | ~400 + ~700 | **high**: partial intent in a reviewed state machine |
| **B5b** the eval | The full harness and gate (§2.8) on B5a's frozen set; description tuning | The evidence for the default | The report against the gate: three repeats, reported and attributed columns apart | ~100 + notes | medium: measurement |
| **B6** default on | `blocks` defaults to `true`. The opt-out for TUI-only users leads the release note and the README (§5.5). It ships only if B5b met the gate | Blocks for everyone | `initialize` with the new default | ~10 | low code; the decision is the risk |
| **B7** (later) `blocks/2` | `at:"top"` | D9's strip | Catalog tests; a `blocks/1` desk reads it plain | ~150 + ~150 | low |

### 8.2 synaps-desk

| PR | Contents | Useful alone because | Tests | Size | Risk |
|---|---|---|---|---|---|
| **D5** blocks, read-only | Bind block calls (pending at `ToolUse`, confirmed at the result); the overlay re-run; the bound prefix; block inputs kept whole; the six kinds at their anchors; in-place updates with glow; collapse, merge and `collapse run` titles; `card:` replace; "doing · turn ended"; edit previews with their labels; the pointer lease in the list; painted targets, down/up with the held lease, and the just-moved rule; bounded caches and the re-fold budget | **The transcript becomes a living document** | A render per kind and state at 320 px and wide; binding and re-run; a split history; the three-way equivalence; list anchoring above the view, and the pill; §6.2's lease tests; Reduced and Off; presents and app renders counted after the last glow (`DESK:sd/bench.rs:42`) | ~1.8k + ~1.1k | medium–high: list geometry |
| **D6** the rail | Both slots; the cap across them, and the fifth-expansion rule; groups in one slot; NOW's priority; overflow per kind; the lease and painted targets in the rail; dismiss and show as versioned desires, with notices riding typed text and the composer chip; `blocks.json`, and `block-desires.json` with its reserve; soft dismissals across compaction; pins and slot moves; the strip count | **The agent molds the rail** | Slot order; the cap across slots; a notice rides a typed `Submit` and a `Steer`, never alone; a dropped steer re-arms the desire, and a Show supersedes it; §3.5's notice list and §4's cap tests; dismissed stays dismissed across reattach, restart and a second desk; pin and slot outrank `move`; both files' caps and crash sweep; a full desire file refuses before drawing, and a restart loses no desire; the strip | ~1.2k + ~1k | **medium–high**: a persistent, shared visibility state machine |
| **D7** block walk and views | `Ctrl/Cmd+G` walk, `j k z c x p m u`, `[ ]` revisions, the op log (`Ctrl/Cmd+O`) with its archive tab, the hover-action overlay, local undo | **Keyboard-first navigation** | Activation on paint; bare presses only; `Esc` to the composer, never denying a prompt; no reflow on hover; keys act on painted targets, checked before the lease releases, and never meet the just-moved rule | ~700 + ~500 | medium: key precedence |
| **D8** take, edit, hand back | Drafts over the bound board; the plan, note and proposed-diff editors; the pre-check; stale drafts and orphans with Copy to composer; edit and restore messages, with notices, through B4 and D2a's send; outcomes on the hand-back rows; `another edit arrived`; `changed your edit`; read-only without input authority | **The user edits the agent's document** | §3.4's traces end to end; §3.7's matrix; compaction during a hold, a send and a queue; a hand-back refused, dequeued or held; 8 drafts across successive compactions; the never-resolves-a-prompt tests (click and key); IME in label fields | ~1.6k + ~1.4k | **high**: partial intent, delivery, focus |
| **D9** the top strip | `at:"top"` in the sticky layer, with B7 | wild2's pinned mission | Layer tests; the cap of 1 | ~400 + ~200 | low |

**Order:** B0 → B1 → B2 → D5 → D6 → B3 (opt-in) → B5a → D7 → B4 → D8 → B5b → B6 (only if the gate holds); later, B7 with D9.
- B3 comes after D5 and D6, so everything its description offers has a renderer.
- B5a measures as soon as agents can keep blocks.
- D8 waits for B4. B6 waits for B5b's gate.
- #56 and the cards PRs are untouched.

**SynapsCLI:** no PRs.

---

## 9. Decisions (decided by Haseeb, 2026-11-26)

1. **Rail: two slots. Decided.** Sections go `upper` (between Prompt cache and Files, the default) or `end` (after Files), picked per section with `slot`. The cap is 4 expanded across both slots, with each slot's leftovers in its own `+N` row. Within a slot, the user's sections come first, then the agent's in placement order. The user moves a section between slots with `m` or `⇅`, locally (§4, §3.5). v1.2 settles a fifth expansion when all four are the user's: the oldest folds (§4).
2. **Dismissals: the agent is told. Decided.** A notice `[block <id> v<N> dismissed]` (and `[block <id> v<N> shown]`) rides the next message the user sends: typed text, as a `Submit` when idle or a steer while a turn runs, or a hand-back. It never goes as a message of its own, and never on a card answer. Once in history every desk shares it. The agent must not put, move or recreate the id. It should stop patching it, and may remove it (§3.5). v1.2 versions the notices so that a later change always wins (§3.5). The decision is unchanged.
   - **How "delivered the way steering is" is read here:** a notice rides a steer the user sends while a turn runs. It is never sent as a steer of its own, because such a steer can miss the turn's last drain and start the next turn (`CLI:ae/session/actor.rs:4119-4120`), and it would take the user's queued slot (`:3409`). If Haseeb meant a standalone steer, that breaks "never a standalone wake" without a core change.
3. **Questions stay `cards_ui` cards. Decided.** Blocks address them as `card:<id>` once resolved (§1.5).
4. **At most one closing call per turn. Decided.** It is guidance in the description, not enforcement, since a stateless extension can't tell which call closes a turn. Optimistic ticks are never allowed (§2.5). B5 measures, and the gate bounds it (§2.8).
5. **The setting. Decided.** `extension.cards.blocks` stays off until the desk renders blocks (D5 and D6). It turns on by default once editing (D8) and the eval (B5b) land, and only if B5b meets its gate (§2.8). TUI-only setups are told to keep it off (§5.5).
