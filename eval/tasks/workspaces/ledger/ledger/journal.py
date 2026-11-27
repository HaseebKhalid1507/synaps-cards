"""Journal entries: each a date, a memo and lines that balance exactly."""

import datetime
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Line:
    account: str
    cents: int  # debits positive, credits negative


@dataclass
class Entry:
    date: datetime.date
    memo: str
    lines: list = field(default_factory=list)

    def total(self):
        return sum(line.cents for line in self.lines)


class Journal:
    def __init__(self, chart):
        self.chart = chart
        self.entries = []

    def post(self, entry):
        """Add `entry`, if its accounts exist and its lines balance."""
        if len(entry.lines) < 2:
            raise ValueError("an entry has at least two lines")
        for line in entry.lines:
            self.chart.get(line.account)
        if entry.total() != 0:
            raise ValueError(f"entry {entry.memo!r} doesn't balance: off by {entry.total()} cents")
        self.entries.append(entry)
        return entry

    def lines(self, account, until=None):
        """`account`'s lines, in date order, with their entries."""
        out = []
        for e in sorted(self.entries, key=lambda e: e.date):
            if until is not None and e.date > until:
                continue
            for line in e.lines:
                if line.account == account:
                    out.append((e, line))
        return out

    def balance(self, account, until=None):
        """`account`'s balance in cents, signed by its kind: positive is its
        normal side."""
        from .accounts import DEBIT_NORMAL

        raw = sum(line.cents for _, line in self.lines(account, until))
        return raw if self.chart.get(account).kind in DEBIT_NORMAL else -raw
