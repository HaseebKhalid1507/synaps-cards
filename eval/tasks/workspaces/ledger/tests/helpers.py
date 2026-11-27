import datetime

from ledger.accounts import default_chart
from ledger.journal import Entry, Journal, Line


def d(s):
    return datetime.date.fromisoformat(s)


def journal(*entries):
    j = Journal(default_chart())
    for date, memo, lines in entries:
        j.post(Entry(d(date), memo, [Line(a, c) for a, c in lines]))
    return j


SAMPLE = (
    ("2026-01-05", "Owner's capital", [("1000", 500000), ("3000", -500000)]),
    ("2026-01-06", "Rent for January", [("5000", 120000), ("1000", -120000)]),
    ("2026-01-20", "Invoice 17", [("1100", 80000), ("4000", -80000)]),
    ("2026-02-03", "Rent for February", [("5000", 120000), ("1000", -120000)]),
    ("2026-02-10", "Invoice 17 paid", [("1000", 80000), ("1100", -80000)]),
)
