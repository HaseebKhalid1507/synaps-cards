"""The frozen set's own tests: its shape is §2.8's, it is the set its digest
names, and every check is a real one: it fails on the task's start and
passes with the reference solution."""

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

    def test_every_expect_matches_its_reference_and_not_nothing(self):
        for t in TASKS:
            if "expect" in t["check"]:
                with self.subTest(task=t["id"]):
                    self.assertTrue(taskset.judge(t, text=t["check"]["reference"])["pass"])
                    self.assertFalse(taskset.judge(t, text="")["pass"])

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
