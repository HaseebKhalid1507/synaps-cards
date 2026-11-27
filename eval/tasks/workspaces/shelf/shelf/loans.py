"""Loans: who has which book, until when, and what it costs late."""

import datetime
from dataclasses import dataclass

LOAN_DAYS = 21
FINE_PER_DAY = 25  # cents


@dataclass
class Loan:
    isbn: str
    member: str
    lent: datetime.date
    due: datetime.date
    returned: datetime.date | None = None


class Loans:
    def __init__(self, catalog, members):
        self.catalog = catalog
        self.members = {m.id: m for m in members}
        self.loans = []

    def on_loan(self, isbn):
        return next((l for l in self.loans if l.isbn == isbn and l.returned is None), None)

    def held_by(self, member):
        return [l for l in self.loans if l.member == member and l.returned is None]

    def lend(self, isbn, member, today):
        """Lend `isbn` to `member`: the loan, due in LOAN_DAYS days."""
        self.catalog.get(isbn)
        m = self.members[member]
        if self.on_loan(isbn):
            raise ValueError(f"{isbn} is on loan")
        if len(self.held_by(member)) >= m.limit:
            raise ValueError(f"{m.name} has {m.limit} books already")
        loan = Loan(isbn, member, today, today + datetime.timedelta(days=LOAN_DAYS))
        self.loans.append(loan)
        return loan

    def give_back(self, isbn, today):
        """Return `isbn`: the fine in cents, 0 if on time."""
        loan = self.on_loan(isbn)
        if loan is None:
            raise ValueError(f"{isbn} isn't on loan")
        loan.returned = today
        return fine(loan, today)

    def overdue(self, today):
        return [l for l in self.loans if l.returned is None and l.due < today]


def fine(loan, today):
    late = (today - loan.due).days
    return max(late, 0) * FINE_PER_DAY
