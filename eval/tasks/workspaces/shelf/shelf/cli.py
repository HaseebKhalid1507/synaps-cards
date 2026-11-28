"""`python3 -m shelf.cli search FILE QUERY` and `loans FILE`."""

import argparse
import sys

from . import store


def main(argv=None, out=sys.stdout):
    ap = argparse.ArgumentParser(prog="shelf")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("search", help="search titles")
    s.add_argument("file")
    s.add_argument("query")
    l = sub.add_parser("loans", help="books on loan")
    l.add_argument("file")
    a = ap.parse_args(argv)
    loans = store.load(a.file)
    if a.cmd == "search":
        for b in loans.catalog.search(a.query):
            print(f"{b.isbn}  {b.title}", file=out)
    else:
        for loan in loans.loans:
            if loan.returned is None:
                print(f"{loan.isbn}  {loan.member}  due {loan.due.isoformat()}", file=out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
