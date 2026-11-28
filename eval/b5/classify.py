"""The closing-round classifier (docs/blocks.md §2.8), on one turn's timeline
(turns.py).

Take the responses after the turn's last result from a tool other than
`cards_block`, or after the user's message if that came later: R1 … Rk.
- **The turn has k − 1 closing rounds**, whichever response carried the
  final text. With blocks off, k is 1.
- **Their input** is the reported input of R2 … Rk: each re-read the
  context once more (input, cache reads and cache writes, each kept apart
  too).
- **Their output** isn't split by response: the final text may ride with
  the block call or follow it. It is in the block-call attribution and in
  the net-output row, not here.

**"The user's message"** is the turn's latest: its opening text, or a steer
the engine delivered in it (§2.8: a turn includes its steers). A response
after a steer answers the user, so it isn't closing; and with blocks off
the response after a steer is the turn's one, so k stays 1 as §2.8 says.
This is the one reading the classifier adds to the text.

**A tail metric, not a chatter detector.** It counts the block-only
responses at the turn's end, as §2.8 defines closing rounds; it doesn't see
one that another tool call follows (a block-only response, then `bash`,
then the final text: no closing round), and a later tool result or steer
moves the anchor past the earlier ones. Every block-only response,
wherever it falls, is `block_only_rounds` (turns.py's `Round.block_only`;
report.py, reported with no gate).

A `cards_ui` result counts as another tool's: a card is the turn's work.
A turn that ended with no response after its anchor (cancelled, or an
error) has k = 0: no closing rounds, and `complete` is false. So is a turn
that isn't `measured` (turns.py: no `idle`, an error, aborted, no
response at all, or a response with no usage, which could hide a closing
round).
"""

from dataclasses import dataclass

from .turns import BLOCK_TOOL


@dataclass
class Closing:
    k: int
    closing_rounds: int
    closing_input: int  # R2 … Rk's reported input, all of it
    closing_uncached: int
    closing_cache_read: int
    closing_cache_write: int
    rounds: list  # R1 … Rk
    complete: bool


def anchor(turn, block_tool=BLOCK_TOOL):
    """The timeline index the R's come after: the later of the last result
    from another tool and the last user's message; -1 if neither (an
    event-started turn with no text and no other tool)."""
    at = -1
    for i, item in enumerate(turn.timeline):
        if item[0] == "user" or (item[0] == "result" and item[1].name != block_tool):
            at = i
    return at


def closing(turn, block_tool=BLOCK_TOOL):
    at = anchor(turn, block_tool)
    rs = [item[1] for item in turn.timeline[at + 1:] if item[0] == "round"]
    later = rs[1:]
    u = lambda key: sum((r.usage or {}).get(key, 0) for r in later)  # noqa: E731
    return Closing(
        k=len(rs),
        closing_rounds=max(len(rs) - 1, 0),
        closing_input=sum(r.input_side for r in later),
        closing_uncached=u("input_tokens"),
        closing_cache_read=u("cache_read_input_tokens"),
        closing_cache_write=u("cache_creation_input_tokens"),
        rounds=rs,
        complete=bool(rs) and turn.measured,
    )
