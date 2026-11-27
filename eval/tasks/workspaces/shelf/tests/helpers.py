import datetime

from shelf.catalog import Catalog
from shelf.loans import Loans
from shelf.models import Book, Member


def d(s):
    return datetime.date.fromisoformat(s)


BOOKS = [
    Book("9780140449136", "The Quiet Garden", ["A. Reed"], 1998),
    Book("9780306406157", "Gardens of Stone", ["M. Ortiz"], 2004),
    Book("9781861972712", "Émile and the Quiet Sea", ["J. Doré"], 2011),
    Book("9780262033848", "Introduction to Algorithms", ["T. Cormen", "C. Leiserson"], 2009),
]
MEMBERS = [Member("m1", "Ada"), Member("m2", "Grace", limit=1)]


def library():
    c = Catalog()
    for b in BOOKS:
        c.add(Book(b.isbn, b.title, list(b.authors), b.year))
    return Loans(c, [Member(m.id, m.name, m.limit) for m in MEMBERS])
