"""The catalog, members and loans as one JSON file."""

import datetime
import json

from .catalog import Catalog
from .loans import Loan, Loans
from .models import Book, Member


def dump(loans):
    return {
        "books": [vars(b) for b in loans.catalog.books.values()],
        "members": [vars(m) for m in loans.members.values()],
        "loans": [{**vars(l), "lent": l.lent.isoformat(), "due": l.due.isoformat(),
                   "returned": l.returned.isoformat() if l.returned else None} for l in loans.loans],
    }


def load_data(data):
    catalog = Catalog()
    for b in data.get("books", []):
        catalog.add(Book(b["isbn"], b["title"], list(b.get("authors", [])), b.get("year")))
    members = [Member(m["id"], m["name"], m.get("limit", 3)) for m in data.get("members", [])]
    loans = Loans(catalog, members)
    for l in data.get("loans", []):
        returned = datetime.date.fromisoformat(l["returned"]) if l.get("returned") else None
        loans.loans.append(Loan(l["isbn"], l["member"], datetime.date.fromisoformat(l["lent"]),
                                datetime.date.fromisoformat(l["due"]), returned))
    return loans


def save(path, loans):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(dump(loans), f, indent=2, ensure_ascii=False)


def load(path):
    with open(path, encoding="utf-8") as f:
        return load_data(json.load(f))
