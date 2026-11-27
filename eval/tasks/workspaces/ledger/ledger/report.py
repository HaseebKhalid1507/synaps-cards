"""Reports: the trial balance and an account's statement."""

from . import money


def trial_balance(journal):
    """Every account with its balance, by account code: rows of
    (code, name, balance in cents)."""
    rows = []
    for account in sorted(journal.chart, key=lambda a: a.code):
        rows.append((account.code, account.name, journal.balance(account.code)))
    return rows


def statement(journal, code):
    """`code`'s lines with a running balance: rows of (date, memo, cents,
    running cents), signed by the account's kind."""
    from .accounts import DEBIT_NORMAL

    sign = 1 if journal.chart.get(code).kind in DEBIT_NORMAL else -1
    rows, running = [], 0
    for entry, line in journal.lines(code):
        running += sign * line.cents
        rows.append((entry.date, entry.memo, sign * line.cents, running))
    return rows


def render(rows):
    """Rows as aligned text, amounts formatted."""
    out = []
    for row in rows:
        cells = [money.format(c) if isinstance(c, int) else str(c) for c in row]
        out.append("  ".join(cells))
    return "\n".join(out)
