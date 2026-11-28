import datetime
import io
import json
import os
import tempfile
import unittest

from shelf import store
from shelf.loans import LOAN_DAYS, Loan, Loans, fine
from shelf.models import Book, Member
from tests.helpers import d, library

B1, B2, B3, B4 = "9780140449136", "9780306406157", "9781861972712", "9780262033848"


def run_cli(lib, *args, files=None):
    """`shelf.cli.main(args)` on `lib` saved as FILE in a temporary
    directory (with `files`, name: text, beside it; each of their names in
    `args` replaced by its path): the printed lines, and the library file
    loaded again after the run."""
    from shelf import cli

    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "library.json")
        store.save(path, lib)
        for name, text in (files or {}).items():
            with open(os.path.join(tmp, name), "w", encoding="utf-8") as f:
                f.write(text)
        out = io.StringIO()
        cli.main([path if a == "FILE" else os.path.join(tmp, a) if a in (files or {}) else a for a in args], out=out)
        return out.getvalue().strip().splitlines(), store.load(path)


class Check(unittest.TestCase):
    def lib(self):
        lib = library()
        lib.members["m2"].expires = d("2026-03-01")
        lib.members["m3"] = Member("m3", "Linus", 2, d("2026-06-30"))
        return lib

    def test_a_member_expires_or_never(self):
        self.assertIsNone(Member("x", "X").expires)
        self.assertEqual(Member("x", "X", 2, d("2026-01-01")).expires, d("2026-01-01"))

    def test_an_expired_member_cant_borrow(self):
        lib = self.lib()
        lib.lend(B1, "m2", d("2026-03-01"))
        lib.give_back(B1, d("2026-03-01"))
        with self.assertRaises(ValueError):
            lib.lend(B1, "m2", d("2026-03-02"))
        lib.lend(B2, "m1", d("2030-01-01"))
        self.assertEqual([m.id for m in lib.expired(d("2026-07-01"))], ["m2", "m3"])
        self.assertEqual([m.id for m in lib.expired(d("2026-03-01"))], [])

    def test_the_store_keeps_it_and_old_files_load(self):
        data = json.loads(json.dumps(store.dump(self.lib())))
        back = store.load_data(data)
        self.assertEqual({m.id: m.expires for m in back.members.values()}, {"m1": None, "m2": d("2026-03-01"), "m3": d("2026-06-30")})
        for m in data["members"]:
            m.pop("expires")
        self.assertEqual({m.expires for m in store.load_data(data).members.values()}, {None})

    def test_the_cli_lists_members(self):
        lines, _ = run_cli(self.lib(), "members", "FILE", "--today", "2026-04-01")
        self.assertEqual(lines, ["m1  Ada  never", "m2  Grace  expires 2026-03-01  (expired)", "m3  Linus  expires 2026-06-30"])


if __name__ == "__main__":
    unittest.main()
