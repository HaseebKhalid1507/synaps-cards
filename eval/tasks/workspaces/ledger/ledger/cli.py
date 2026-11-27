"""`python3 -m ledger.cli balance FILE` and `statement CODE FILE`."""

import argparse
import sys

from . import report
from .accounts import default_chart
from .importer import import_into
from .journal import Journal


def load(path):
    journal = Journal(default_chart())
    with open(path, encoding="utf-8") as f:
        import_into(journal, f.read())
    return journal


def main(argv=None, out=sys.stdout):
    ap = argparse.ArgumentParser(prog="ledger")
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("balance", help="the trial balance")
    b.add_argument("file")
    s = sub.add_parser("statement", help="one account's statement")
    s.add_argument("code")
    s.add_argument("file")
    a = ap.parse_args(argv)
    journal = load(a.file)
    if a.cmd == "balance":
        print(report.render(report.trial_balance(journal)), file=out)
    else:
        print(report.render(report.statement(journal, a.code)), file=out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
