import json
import math
import os
import re
import shutil
import stat
import subprocess
import sys
from dataclasses import dataclass, field

SCHEMA_VERSION = 1
FATE_OPS = {"=": "verbatim", "~": "edited", ">": "moved"}
COMMENT_PREFIXES = {"rust": ("//",), "typescript": ("//",), "python": ("#",)}
ROW = re.compile(r"^\s*(?:(?P<label>[A-Za-z][A-Za-z0-9_]*)\s+(?P<op>[+=~>])\s+)?\|(?P<code>.*)$")
COMMIT_ID = re.compile(r"^c\d+$")
AUTHOR_NAME = "Marrow Fixture"
AUTHOR_EMAIL = "fixtures@marrow.invalid"
GIT_SETTINGS = (
    "core.autocrlf=false",
    "core.eol=lf",
    "core.safecrlf=false",
    "core.fsmonitor=false",
    "commit.gpgsign=false",
)

UNCHANGED = object()


class FixtureError(Exception):
    pass


@dataclass
class Commit:
    message: str
    files: dict
    dead: str = ""
    renames: dict = field(default_factory=dict)


@dataclass
class Fixture:
    mutation_class: str
    language: str
    summary: str
    commits: list

    @property
    def name(self):
        return f"{self.mutation_class}/{self.language}"


@dataclass
class Row:
    label: str | None
    op: str | None
    text: str


def squash(text):
    return re.sub(r"\s+", "", text)


def commit_day(index):
    return f"2026-01-{index + 1:02d}T00:00:00"


def parse_block(where, block):
    raw_rows = block.split("\n")
    while raw_rows and not raw_rows[0].strip():
        raw_rows.pop(0)
    while raw_rows and not raw_rows[-1].strip():
        raw_rows.pop()
    rows = []
    for raw in raw_rows:
        match = ROW.match(raw)
        if match is None:
            raise FixtureError(f"{where}: malformed row {raw!r}")
        code = match["code"]
        if code.startswith(" "):
            code = code[1:]
        if not code.strip():
            if match["label"] is not None:
                raise FixtureError(f"{where}: blank row must not carry a label: {raw!r}")
            rows.append(Row(None, None, ""))
            continue
        if match["label"] is None:
            raise FixtureError(f"{where}: code row needs a label and an op: {raw!r}")
        if COMMIT_ID.match(match["label"]):
            raise FixtureError(f"{where}: label {match['label']} looks like a commit id")
        if code != code.rstrip():
            raise FixtureError(f"{where}: trailing whitespace in {raw!r}")
        rows.append(Row(match["label"], match["op"], code))
    return rows


def resolve_files(fixture, index, commit, previous):
    files = {}
    for path in sorted(commit.files):
        block = commit.files[path]
        where = f"{fixture.name} c{index} {path}"
        if block is UNCHANGED:
            if path not in previous:
                raise FixtureError(f"{where}: UNCHANGED file does not exist in the previous commit")
            files[path] = [Row(row.label, "=" if row.label else None, row.text) for row in previous[path]]
        else:
            files[path] = parse_block(where, block)
    return files


def check_renames(fixture, index, commit, previous, files):
    where = f"{fixture.name} c{index}"
    if commit.renames and index == 0:
        raise FixtureError(f"{where}: the base commit cannot rename files")
    targets = set()
    for old_path, new_path in commit.renames.items():
        if old_path not in previous or old_path in files:
            raise FixtureError(f"{where}: rename source {old_path} must exist before this commit and be gone after it")
        if new_path in previous or new_path not in files:
            raise FixtureError(f"{where}: rename target {new_path} must be new in this commit")
        if new_path in targets:
            raise FixtureError(f"{where}: two files are renamed to {new_path}")
        targets.add(new_path)


def check_fate(where, label, op, old_path, old_text, new_path, new_text, renames):
    same_tokens = squash(old_text) == squash(new_text)
    corresponding_path = renames.get(old_path, old_path)
    if op == "=":
        if not same_tokens:
            raise FixtureError(f"{where}: {label} is declared verbatim but changed beyond whitespace")
        if new_path != corresponding_path:
            raise FixtureError(f"{where}: {label} changed file without a declared rename; declare it moved (>)")
    elif op == "~":
        if same_tokens:
            raise FixtureError(f"{where}: {label} is declared edited but only whitespace changed; use =")
        if new_path != corresponding_path:
            raise FixtureError(f"{where}: {label} is edited across files (edited-during-move is out of scope)")
    elif op == ">" and not same_tokens:
        raise FixtureError(f"{where}: {label} is declared moved but its text changed (edited-during-move is out of scope)")


def check_order(where, path, rows, live, renames):
    anchors = [
        (position, live[row.label][1])
        for position, row in enumerate(rows)
        if row.label is not None and row.op in ("=", "~")
    ]
    for (_, earlier), (_, later) in zip(anchors, anchors[1:]):
        if later <= earlier:
            raise FixtureError(f"{where}: verbatim/edited lines are out of order; declare the relocated block moved (>)")
    for position, row in enumerate(rows):
        if row.op != ">" or renames.get(live[row.label][0], live[row.label][0]) != path:
            continue
        old_line = live[row.label][1]
        lower = max((line for anchor, line in anchors if anchor < position), default=0)
        upper = min((line for anchor, line in anchors if anchor > position), default=math.inf)
        if lower < old_line < upper:
            raise FixtureError(f"{where}: {row.label} is declared moved but keeps its relative order; use =")


def line_kind(language, text):
    return "comment" if text.lstrip().startswith(COMMENT_PREFIXES[language]) else "code"


def build_fixture(fixture, target_dir, fixtures_root):
    if len(fixture.commits) < 2:
        raise FixtureError(f"{fixture.name}: needs a base commit and at least one mutation commit")
    repo = os.path.join(target_dir, "repo")
    reset_repo(repo, fixtures_root)

    records = {}
    live = {}
    previous = {}
    commits = []
    for index, commit in enumerate(fixture.commits):
        commit_id = f"c{index}"
        files = resolve_files(fixture, index, commit, previous)
        check_renames(fixture, index, commit, previous, files)
        present = {}
        for path, rows in files.items():
            where = f"{fixture.name} {commit_id} {path}"
            for line_no, row in enumerate(rows, start=1):
                if row.label is None:
                    continue
                label = row.label
                if label in present:
                    raise FixtureError(f"{where}: label {label} appears twice")
                present[label] = (path, line_no, row.text)
                if row.op == "+":
                    if label in records:
                        raise FixtureError(f"{where}: label {label} is already used; born lines need a fresh label")
                    records[label] = {
                        "line_id": label,
                        "kind": line_kind(fixture.language, row.text),
                        "text": row.text,
                        "birth": {"commit": commit_id, "path": path, "line": line_no},
                        "fates": [],
                    }
                    continue
                if index == 0:
                    raise FixtureError(f"{where}: every line in the base commit must be born (+)")
                if label not in live:
                    raise FixtureError(f"{where}: {label} is not alive in c{index - 1}")
                old_path, _, old_text = live[label]
                check_fate(where, label, row.op, old_path, old_text, path, row.text, commit.renames)
                records[label]["fates"].append(
                    {"commit": commit_id, "state": FATE_OPS[row.op], "path": path, "line": line_no}
                )
            check_order(where, path, rows, live, commit.renames)

        dead = commit.dead.split()
        for label in dead:
            if label not in live:
                raise FixtureError(f"{fixture.name} {commit_id}: {label} is declared dead but was not alive")
            if label in present:
                raise FixtureError(f"{fixture.name} {commit_id}: {label} is declared dead but still present")
            records[label]["fates"].append({"commit": commit_id, "state": "dead"})
        unaccounted = sorted(set(live) - set(present) - set(dead))
        if unaccounted:
            raise FixtureError(
                f"{fixture.name} {commit_id}: lines neither carried forward nor declared dead: {' '.join(unaccounted)}"
            )

        sha = commit_files(repo, index, commit.message, files)
        commit_record = {"id": commit_id, "sha": sha, "timestamp": f"{commit_day(index)}Z", "message": commit.message}
        if commit.renames:
            commit_record["renames"] = [{"from": old, "to": new} for old, new in sorted(commit.renames.items())]
        commits.append(commit_record)
        live = present
        previous = files

    return render_manifest(fixture, commits, records), summarize(records)


def summarize(records):
    counts = {"tracked": len(records), "verbatim": 0, "edited": 0, "moved": 0, "dead": 0, "born_later": 0}
    for record in records.values():
        if record["birth"]["commit"] != "c0":
            counts["born_later"] += 1
        for fate in record["fates"]:
            counts[fate["state"]] += 1
    return counts


def render_manifest(fixture, commits, records):
    def dump(value):
        return json.dumps(value, ensure_ascii=False)

    lines = sorted(
        records.values(),
        key=lambda record: (int(record["birth"]["commit"][1:]), record["birth"]["path"], record["birth"]["line"]),
    )
    header = {
        "schema_version": SCHEMA_VERSION,
        "fixture": fixture.name,
        "mutation_class": fixture.mutation_class,
        "language": fixture.language,
        "summary": fixture.summary,
    }
    out = ["{"]
    out.extend(f"  {dump(key)}: {dump(value)}," for key, value in header.items())
    out.append('  "commits": [')
    out.append(",\n".join(f"    {dump(commit)}" for commit in commits))
    out.append("  ],")
    out.append('  "lines": [')
    out.append(",\n".join(f"    {dump(record)}" for record in lines))
    out.append("  ]")
    out.append("}")
    text = "\n".join(out) + "\n"
    json.loads(text)
    return text


def _force_remove(function, path, _error):
    os.chmod(path, stat.S_IWRITE)
    function(path)


def remove_tree(path):
    if sys.version_info >= (3, 12):
        shutil.rmtree(path, onexc=_force_remove)
    else:
        shutil.rmtree(path, onerror=_force_remove)


def reset_repo(repo, fixtures_root):
    root = os.path.realpath(fixtures_root)
    real = os.path.realpath(repo)
    if os.path.basename(real) != "repo" or os.path.commonpath([root, real]) != root:
        raise FixtureError(f"refusing to reset {repo}: not a generated fixture repo")
    if os.path.exists(real):
        remove_tree(real)
    os.makedirs(real)
    git(real, "init", "--quiet", "--template=", "--object-format=sha1", "--initial-branch=main", ".", bound=False)


def git(repo, *args, index=0, bound=True):
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")}
    stamp = f"{commit_day(index)}+00:00"
    env.update(
        {
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": AUTHOR_NAME,
            "GIT_AUTHOR_EMAIL": AUTHOR_EMAIL,
            "GIT_AUTHOR_DATE": stamp,
            "GIT_COMMITTER_NAME": AUTHOR_NAME,
            "GIT_COMMITTER_EMAIL": AUTHOR_EMAIL,
            "GIT_COMMITTER_DATE": stamp,
        }
    )
    if bound:
        env["GIT_DIR"] = os.path.join(repo, ".git")
        env["GIT_WORK_TREE"] = repo
    command = ["git"]
    for setting in GIT_SETTINGS:
        command += ["-c", setting]
    result = subprocess.run(command + list(args), cwd=repo, env=env, capture_output=True, text=True)
    if result.returncode != 0:
        raise FixtureError(f"git {' '.join(args)} failed in {repo}: {result.stderr.strip()}")
    return result.stdout.strip()


def commit_files(repo, index, message, files):
    for entry in os.listdir(repo):
        if entry == ".git":
            continue
        path = os.path.join(repo, entry)
        if os.path.isdir(path):
            remove_tree(path)
        else:
            os.remove(path)
    for rel_path, rows in files.items():
        full_path = os.path.join(repo, *rel_path.split("/"))
        os.makedirs(os.path.dirname(full_path), exist_ok=True)
        with open(full_path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write("".join(row.text + "\n" for row in rows))
    git(repo, "add", "--all", index=index)
    git(repo, "commit", "--quiet", "--message", message, index=index)
    return git(repo, "rev-parse", "HEAD", index=index)
