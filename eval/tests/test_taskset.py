"""The frozen set's own tests: its shape is §2.8's, it is the set its digest
names, and every check is a real one: it fails on the task's start and
passes with the reference solution."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from b5 import taskset

TASKS = taskset.load()


class Shape(unittest.TestCase):
    def test_the_set_is_2_8s(self):
        self.assertEqual(taskset.problems(TASKS), [])

    def test_hand_backs_wait_for_d8_and_nothing_else_waits(self):
        for t in TASKS:
            hands = any("hand_back" in s for s in t["turns"])
            self.assertEqual(t["needs"], ["D8"] if hands else [], t["id"])
        runnable = [t for t in TASKS if not t["needs"]]
        self.assertGreaterEqual(sum(t["category"] == "interactive" for t in runnable), 5, "B5a runs ≥ 5 card tasks")

    def test_every_card_answer_names_its_label_and_the_prompt_asks_for_it(self):
        # The answer means one thing (cards-eval's policy): the label the
        # check judges, which the prompt asks the agent to offer exactly.
        cards = [t for t in TASKS if any("answer" in s for s in t["turns"])]
        self.assertEqual(len(cards), 5)
        for t in cards:
            with self.subTest(task=t["id"]):
                (pick,) = [s["answer"] for s in t["turns"] if "answer" in s]
                self.assertEqual(set(pick), {"pick"})
                for label in pick["pick"]:
                    self.assertIn(f"`{label}`", t["turns"][0]["say"])
                self.assertIn("exactly", t["turns"][0]["say"])
        bad = [dict(cards[0], turns=[cards[0]["turns"][0], {"answer": {"prefer": ["hide"]}}])]
        self.assertIn(f"{cards[0]['id']}: an answer names the labels it picks", taskset.problems(bad + TASKS[1:]))

    def test_a_hand_back_is_frozen_in_both_arms(self):
        # §2.8's intent, the user's call: the same task with the user's edit
        # as a plain message. The edit is the task's, not the proposal's, and
        # the message carries it; the prompts differ only by where the
        # proposal goes.
        import re
        hands = [t for t in TASKS if any("hand_back" in s for s in t["turns"])]
        self.assertEqual(len(hands), 3)
        for t in hands:
            with self.subTest(task=t["id"]):
                (hb,) = [s["hand_back"] for s in t["turns"] if "hand_back" in s]
                self.assertTrue(taskset.HAND_BACK <= set(hb))
                (edit,) = hb["edit"].items()
                self.assertIn(edit[0], taskset.EDITS)
                self.assertNotIn("skip", hb["edit"], "an edit that depends on the agent's plan")
                for code in re.findall(r"`([^`]+)`", str(edit[1])):
                    self.assertIn(code, hb["message"])
                say, off = t["turns"][0]["say"], t["turns"][0]["say_off"]
                self.assertIn(f"{hb['block']['kind']} block", say)
                self.assertNotIn("block", off)
                self.assertIn("check", t)

    def test_the_long_tasks_are_real_work_with_spares(self):
        # Review #14, P1 4: no one-line repairs among the long tasks, and 2
        # spares above §2.8's 20 for the ones that run under 6 rounds.
        coding = [t for t in TASKS if t["category"] == "coding"]
        self.assertGreaterEqual(len(coding), taskset.CATEGORIES["coding"] + 2)
        self.assertEqual(taskset.SPARES, {"coding": 2})
        one_line = dict(coding[0], solution="solutions/one-line.json")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for sub in ("solutions", "checks", "workspaces"):
                (root / sub).mkdir()
            (root / "workspaces" / one_line["workspace"]).mkdir()
            (root / one_line["check"]["tests"]).write_text("")
            (root / "solutions/one-line.json").write_text('[{"path": "a.py", "old": "x - 1", "new": "x"}]')
            self.assertTrue(any("1 line(s)" in p for p in taskset.problems([one_line], root)))
        multi = sum(len({e["path"] for e in json.loads((taskset.TASKS / t["solution"]).read_text())}) >= 3 for t in coding)
        self.assertGreaterEqual(multi, 10, "the new long tasks touch 3 files or more")

    def test_the_readme_freezes_the_set_at_b5a(self):
        readme = (taskset.TASKS.parent / "README.md").read_text()
        self.assertIn("## The set is frozen at B5a", readme)
        self.assertNotIn("before B5b's first run", readme)

    def test_the_set_is_frozen(self):
        self.assertEqual(taskset.digest(), taskset.FROZEN.read_text().split()[0],
                         "the task set changed: it is frozen (§2.8); a deliberate change updates tasks/set.sha256")


class Checks(unittest.TestCase):
    def test_each_workspace_passes_its_own_tests(self):
        for ws in sorted({t["workspace"] for t in TASKS}):
            with tempfile.TemporaryDirectory() as tmp:
                d = taskset.prepare({"workspace": ws, "seed": []}, Path(tmp) / ws)
                r = subprocess.run([sys.executable, "-m", "unittest", "-q"], cwd=d, capture_output=True, text=True)
                self.assertEqual(r.returncode, 0, f"{ws}: {r.stderr[-800:]}")

    def test_every_check_fails_at_the_start_and_passes_solved(self):
        for t in TASKS:
            if "tests" not in t["check"]:
                continue
            with self.subTest(task=t["id"]), tempfile.TemporaryDirectory() as tmp:
                d = taskset.prepare(t, Path(tmp) / "ws")
                before = taskset.judge(t, d)
                self.assertFalse(before["pass"], f"{t['id']}: the check passes before any work")
                taskset.solve(t, d)
                after = taskset.judge(t, d)
                self.assertTrue(after["pass"], f"{t['id']}: the reference solution fails: {after['detail']}")

    def test_every_expect_passes_its_reference_and_refuses_its_wrong_answers(self):
        # Shady's: `nt-names` and `nt-haiku` took "I refuse.", `nt-half-even`
        # a wrong explanation, `ledger-q-debit` every account kind.
        refusals = ["", "I refuse.", "I can't answer that without running the code.", "Sorry, I don't know."]
        expects = [t for t in TASKS if "expect" in t["check"]]
        self.assertEqual(len(expects), 15)
        for t in expects:
            with self.subTest(task=t["id"]):
                c = t["check"]
                self.assertTrue(taskset.judge(t, text=c["reference"])["pass"], c["reference"])
                self.assertGreaterEqual(len(c["wrong"]), 3)
                for w in c["wrong"] + refusals:
                    self.assertFalse(taskset.judge(t, text=w)["pass"], f"{w!r} passed")

    def test_a_right_answer_in_its_usual_dress_passes(self):
        by = {t["id"]: t for t in TASKS}
        for tid, text in (("ledger-q-split", "`[34, 33, 33]`"), ("ledger-q-kinds", "- asset\n- liability\n- equity\n- income\n- expense"),
                          ("ledger-q-kinds", "Assets, liabilities, equity, income and expenses."), ("shelf-q-book", "`shelf/models.py`"),
                          ("ledger-q-tests", "12."), ("nt-half-even", "0, 2, 2, 4"), ("nt-sort", "apple, apricot, banana, pear."),
                          ("nt-double-entry", "debit: Office supplies (expense), credit: Cash"), ("ledger-q-debit", "1. Asset\n2. Expense")):
            with self.subTest(task=tid, text=text):
                self.assertTrue(taskset.judge(by[tid], text=text)["pass"], taskset.judge(by[tid], text=text))

    def test_numbers_and_items_are_read_as_the_docs_say(self):
        self.assertEqual(taskset.numbers("978-0-306-40615-7"), [978, 0, 306, 40615, 7])
        self.assertEqual(taskset.numbers("-2, 0.5 and 12."), [-2, 0.5, 12])
        self.assertEqual(taskset.items("1. `asset`\n2) expense."), ["asset", "expense"])
        self.assertEqual(taskset.items("standard and expense; 'x'"), ["standard", "expense", "x"])

    def test_the_counted_answers_are_the_workspaces(self):
        # The short tasks that ask for a count: the count their workspace's suite gives.
        for tid, ws in (("ledger-q-tests", "ledger"), ("shelf-q-tests", "shelf")):
            t = next(t for t in TASKS if t["id"] == tid)
            with tempfile.TemporaryDirectory() as tmp:
                d = taskset.prepare(t, Path(tmp) / ws)
                r = subprocess.run([sys.executable, "-m", "unittest"], cwd=d, capture_output=True, text=True)
            self.assertIn(f"Ran {t['check']['reference']} tests", r.stderr, tid)


if __name__ == "__main__":
    unittest.main()
