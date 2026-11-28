# ledger

A small double-entry ledger: a chart of accounts, a journal of balanced
entries, amounts in integer cents, reports and a CSV importer.

    python3 -m ledger.cli balance journal.csv
    python3 -m ledger.cli statement 1000 journal.csv

Run the tests with `python3 -m unittest`.
