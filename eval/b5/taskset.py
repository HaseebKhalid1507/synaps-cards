"""The frozen task set (docs/blocks.md §2.8): `eval/tasks/set.json`, the
workspaces it runs in, the checks that judge it, and the reference
solutions that prove each check can pass.

**Frozen.** The set's digest (`digest`) covers set.json and every file
under the workspaces, checks and solutions. It is committed beside them
(`tasks/set.sha256`), a test keeps the two equal, and every run records
it. B5b reuses the set unchanged: a run of a set with another digest isn't
B5's.

**A task** runs in a fresh copy of its workspace (`prepare`): the
workspace, then the task's seed edits (a bug put in on purpose). Its
`turns` are the user's side, in order:
- `say`: a message, sent when the session is idle: it starts a turn;
- `steer`: a message sent while the turn runs, once the turn has had
  `after_round` rounds (if the turn ends first, it is said instead, as the
  next turn's message);
- `dismiss`: the user dismisses a block (`first_block`: the first live block
  on the board, harness.py's "A dismissal") once the turn has had
  `after_round` rounds; the notice rides the user's next message, as the
  desk sends it (desk.py);
- `answer`: once the session is idle, the user answers the card the agent
  last showed, by the fixed policy (`cards-eval answer`): the option whose
  label is one of `pick`. The prompt asks the agent for exactly the labels
  it may pick, so the answer means what the check judges. A card with no
  such option, or two, is an interaction failure: nothing is sent, the step
  says why, and the run is judged as it stands;
- `hand_back`: the user edits a block and hands it back (D8), then the
  edit message starts a turn.

**Judged per task** (`judge`), the same in both arms:
- `tests`: the workspace's own tests and the task's hidden check
  (`checks/<id>.py`, copied in as `tests/test_b5_check.py` after the run)
  all pass: `python3 -m unittest` exits 0, compiled from source (no
  bytecode the run left is read);
- `expect`: the text of the task's last turn matches every pattern in
  `all` and one in `any` (case-folded).

**`needs`** names what a task waits for: `D8` (take, edit, hand back). B5a
marks such a task "needs D8, not run"; B5b runs it unchanged.
"""

import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TASKS = Path(__file__).resolve().parent.parent / "tasks"
SET = TASKS / "set.json"
FROZEN = TASKS / "set.sha256"
CATEGORIES = {"coding": 20, "short": 10, "no_tool": 5, "interactive": 5}  # §2.8's minimums
STEPS = ("say", "steer", "dismiss", "answer", "hand_back")
NEEDS = ("D8",)
CHECK_NAME = "test_b5_check.py"


def load(path=SET):
    return json.loads(Path(path).read_text())["tasks"]


def digest(root=TASKS):
    """SHA-256 over set.json and every file of the workspaces, checks and
    solutions, by path in byte order (caches left out)."""
    h = hashlib.sha256()
    files = [root / "set.json"] + sorted(
        p for d in ("workspaces", "checks", "solutions") for p in (root / d).rglob("*")
        if p.is_file() and "__pycache__" not in p.parts)
    for p in files:
        rel = p.relative_to(root).as_posix().encode()
        data = p.read_bytes()
        h.update(len(rel).to_bytes(4, "big") + rel + len(data).to_bytes(8, "big") + data)
    return h.hexdigest()


def problems(tasks, root=TASKS):
    """Everything wrong with the set's shape: empty when it is §2.8's."""
    out = []
    ids = [t["id"] for t in tasks]
    if len(ids) != len(set(ids)):
        out.append("task ids repeat")
    for cat, n in CATEGORIES.items():
        have = sum(t["category"] == cat for t in tasks)
        if have < n:
            out.append(f"{have} {cat} tasks: §2.8 asks at least {n}")
    for t in tasks:
        where = t["id"]
        if t["category"] not in CATEGORIES:
            out.append(f"{where}: no category {t['category']!r}")
        if not (root / "workspaces" / t["workspace"]).is_dir():
            out.append(f"{where}: no workspace {t['workspace']!r}")
        if any(n not in NEEDS for n in t["needs"]):
            out.append(f"{where}: needs {t['needs']}")
        turns = t["turns"]
        if not turns or "say" not in turns[0]:
            out.append(f"{where}: the first step says something")
        for s in turns:
            keys = [k for k in s if k in STEPS]
            if len(keys) != 1:
                out.append(f"{where}: a step is one of {STEPS}: {s}")
            elif keys[0] in ("steer", "dismiss") and not isinstance(s.get("after_round"), int):
                out.append(f"{where}: a {keys[0]} says after which round")
            elif keys[0] == "answer" and not (isinstance(s["answer"].get("pick"), list) and s["answer"]["pick"]
                                              and all(isinstance(p, str) and p.strip() for p in s["answer"]["pick"])):
                out.append(f"{where}: an answer names the labels it picks")
            elif keys[0] == "hand_back" and "D8" not in t["needs"]:
                out.append(f"{where}: a hand-back needs D8")
        c = t["check"]
        if "tests" in c and not (root / c["tests"]).is_file():
            out.append(f"{where}: no check {c['tests']}")
        if "expect" in c and not set(c["expect"]) <= {"all", "any"}:
            out.append(f"{where}: an expect has all and any")
        if t["category"] == "coding" and "tests" not in c:
            out.append(f"{where}: a coding task is judged by tests")
    return out


def apply(workspace, edits):
    """Apply `edits` to the files under `workspace`: each replaces one exact
    occurrence of `old` with `new` in `path`, or writes `content`."""
    for e in edits:
        p = Path(workspace) / e["path"]
        if "content" in e:
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(e["content"])
            continue
        text = p.read_text()
        n = text.count(e["old"])
        if n != 1:
            raise ValueError(f"{e['path']}: the edit's text is there {n} times, not once: {e['old'][:60]!r}")
        p.write_text(text.replace(e["old"], e["new"]))


def prepare(task, dest, root=TASKS):
    """A fresh copy of the task's workspace at `dest`, seeded."""
    dest = Path(dest)
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(root / "workspaces" / task["workspace"], dest, ignore=shutil.ignore_patterns("__pycache__"))
    apply(dest, task["seed"])
    return dest


def solve(task, workspace, root=TASKS):
    """Apply the task's reference solution (the dry run's model, and the
    set's own test)."""
    apply(workspace, json.loads((root / task["solution"]).read_text()))


def judge(task, workspace=None, text="", root=TASKS, timeout=120):
    """`{"pass": bool, "how": …, "detail": …}` for one run (module docs)."""
    c = task["check"]
    if "tests" in c:
        ws = Path(workspace)
        shutil.copyfile(root / c["tests"], ws / "tests" / CHECK_NAME)
        # Compiled from source, always: a .pyc in the workspace (an earlier
        # test run) is trusted by its source's whole-second mtime and size,
        # so an edit of the same length in the same second would be judged
        # as the code before it.
        with tempfile.TemporaryDirectory() as cache:
            try:
                r = subprocess.run([sys.executable, "-m", "unittest", "-q"], cwd=ws, capture_output=True, text=True, timeout=timeout,
                                   env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPYCACHEPREFIX": cache})
                ok, detail = r.returncode == 0, (r.stdout + r.stderr)[-1500:]
            except subprocess.TimeoutExpired:
                ok, detail = False, f"the tests ran past {timeout}s"
            finally:
                (ws / "tests" / CHECK_NAME).unlink(missing_ok=True)
        return {"pass": ok, "how": "tests", "detail": detail}
    e = c["expect"]
    found = lambda p: re.search(p, text, re.IGNORECASE | re.DOTALL) is not None  # noqa: E731
    ok = all(found(p) for p in e.get("all", [])) and (not e.get("any") or any(found(p) for p in e["any"]))
    return {"pass": ok, "how": "expect", "detail": text[-400:]}
