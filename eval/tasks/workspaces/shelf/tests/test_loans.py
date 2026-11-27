import unittest

from tests.helpers import d, library


class Lending(unittest.TestCase):
    def test_a_loan_is_due_in_21_days(self):
        loan = library().lend("9780140449136", "m1", d("2026-03-01"))
        self.assertEqual(loan.due, d("2026-03-22"))

    def test_a_book_on_loan_cant_be_lent(self):
        lib = library()
        lib.lend("9780140449136", "m1", d("2026-03-01"))
        with self.assertRaises(ValueError):
            lib.lend("9780140449136", "m2", d("2026-03-02"))

    def test_the_member_limit(self):
        lib = library()
        lib.lend("9780140449136", "m2", d("2026-03-01"))
        with self.assertRaises(ValueError):
            lib.lend("9780306406157", "m2", d("2026-03-01"))

    def test_a_late_return_is_fined_per_day(self):
        lib = library()
        lib.lend("9780140449136", "m1", d("2026-03-01"))
        self.assertEqual(lib.give_back("9780140449136", d("2026-03-25")), 75)

    def test_overdue(self):
        lib = library()
        lib.lend("9780140449136", "m1", d("2026-03-01"))
        self.assertEqual(len(lib.overdue(d("2026-03-23"))), 1)
        self.assertEqual(lib.overdue(d("2026-03-22")), [])
