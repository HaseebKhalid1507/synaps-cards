"""CSV import: one row per line of an entry, grouped by a shared entry id.

    entry,date,memo,account,amount
    1,2026-01-05,Rent for January,5000,1200.00
    1,2026-01-05,Rent for January,1000,-1200.00

Blank lines are skipped. A row's amount is in the money module's format.
"""

import csv
import datetime
import io

from . import money
from .journal import Entry, Line

COLUMNS = ("entry", "date", "memo", "account", "amount")


def read_csv(text):
    """The entries `text` holds, in the order their ids first appear."""
    rows = csv.DictReader(io.StringIO(text))
    missing = [c for c in COLUMNS if c not in (rows.fieldnames or [])]
    if missing:
        raise ValueError(f"missing columns: {', '.join(missing)}")
    entries = {}
    for row in rows:
        if not any((v or "").strip() for v in row.values()):
            continue
        key = row["entry"].strip()
        date = datetime.date.fromisoformat(row["date"].strip())
        if key not in entries:
            entries[key] = Entry(date, row["memo"].strip())
        entries[key].lines.append(Line(row["account"].strip(), money.parse(row["amount"])))
    return list(entries.values())


def import_into(journal, text):
    """Post every entry of `text` to `journal`: how many."""
    entries = read_csv(text)
    for e in entries:
        journal.post(e)
    return len(entries)
