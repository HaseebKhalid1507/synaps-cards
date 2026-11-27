import os
import tempfile
import unittest

from shelf import store
from tests.helpers import d, library


class Store(unittest.TestCase):
    def test_a_round_trip_keeps_loans(self):
        lib = library()
        lib.lend("9780140449136", "m1", d("2026-03-01"))
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "library.json")
            store.save(path, lib)
            back = store.load(path)
        self.assertEqual(len(back.catalog.books), 4)
        self.assertEqual(back.loans[0].due, d("2026-03-22"))
