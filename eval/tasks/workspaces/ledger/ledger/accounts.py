"""The chart of accounts."""

from dataclasses import dataclass

KINDS = ("asset", "liability", "equity", "income", "expense")
# The kinds whose balance grows with debits.
DEBIT_NORMAL = ("asset", "expense")


@dataclass(frozen=True)
class Account:
    code: str
    name: str
    kind: str

    def __post_init__(self):
        if self.kind not in KINDS:
            raise ValueError(f"unknown kind {self.kind!r}: one of {', '.join(KINDS)}")
        if not self.code.isdigit():
            raise ValueError(f"an account code is digits: {self.code!r}")


class Chart:
    def __init__(self, accounts=()):
        self._accounts = {}
        for a in accounts:
            self.add(a)

    def add(self, account):
        if account.code in self._accounts:
            raise ValueError(f"account {account.code} exists")
        self._accounts[account.code] = account

    def get(self, code):
        try:
            return self._accounts[code]
        except KeyError:
            raise KeyError(f"no account {code}") from None

    def __iter__(self):
        return iter(sorted(self._accounts.values(), key=lambda a: a.code))

    def __len__(self):
        return len(self._accounts)


def default_chart():
    return Chart([
        Account("1000", "Cash", "asset"),
        Account("1100", "Receivables", "asset"),
        Account("2000", "Payables", "liability"),
        Account("3000", "Equity", "equity"),
        Account("4000", "Sales", "income"),
        Account("5000", "Rent", "expense"),
        Account("5100", "Supplies", "expense"),
    ])
