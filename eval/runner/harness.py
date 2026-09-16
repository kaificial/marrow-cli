import glob
import json
import math
import os
import re
import subprocess
from dataclasses import dataclass

CONTRACT_VERSION = 1
STATES = ("verbatim", "edited", "moved", "dead")
FIXTURE_ROW = re.compile(r"^\|\s*(?P<fixture>[a-z0-9-]+/[a-z0-9]+)\s*\|.*\|\s*(?P<errors>\d+)\s+errors?\s*\|$")


class HarnessError(Exception):
    pass


class ContractError(HarnessError):
    pass


def discover_fixtures(fixtures_root):
    fixtures = []
    for manifest_path in sorted(glob.glob(os.path.join(fixtures_root, "*", "*", "manifest.json"))):
        with open(manifest_path, encoding="utf-8") as handle:
            manifest = json.load(handle)
        directory = os.path.dirname(manifest_path)
        expected_name = "/".join(os.path.normpath(directory).split(os.sep)[-2:])
        if manifest.get("fixture") != expected_name:
            raise HarnessError(f"{manifest_path}: fixture name {manifest.get('fixture')!r} does not match its directory")
        fixtures.append((expected_name, manifest, os.path.join(directory, "repo")))
    if not fixtures:
        raise HarnessError(f"no fixtures found under {fixtures_root}; run `just fixtures`")
    return fixtures


def load_thresholds(readme_path, fixture_names):
    thresholds = {}
    with open(readme_path, encoding="utf-8") as handle:
        for raw in handle:
            match = FIXTURE_ROW.match(raw.strip())
            if match is None:
                continue
            name = match["fixture"]
            if name in thresholds:
                raise HarnessError(f"{readme_path}: more than one threshold row for {name}")
            thresholds[name] = int(match["errors"])
    missing = sorted(set(fixture_names) - set(thresholds))
    if missing:
        raise HarnessError(f"{readme_path}: no threshold recorded for {', '.join(missing)}")
    unknown = sorted(set(thresholds) - set(fixture_names))
    if unknown:
        raise HarnessError(f"{readme_path}: threshold rows for fixtures that do not exist: {', '.join(unknown)}")
    return thresholds


def git(repo, *args):
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")}
    env.update(
        {
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_DIR": os.path.join(repo, ".git"),
            "GIT_WORK_TREE": repo,
        }
    )
    result = subprocess.run(["git", *args], cwd=repo, env=env, capture_output=True, text=True)
    if result.returncode != 0:
        raise HarnessError(f"git {' '.join(args)} failed in {repo}: {result.stderr.strip()}")
    return result.stdout


def check_repo_current(repo, manifest):
    if not os.path.isdir(os.path.join(repo, ".git")):
        raise HarnessError(f"fixture repo {repo} does not exist; run `just fixtures`")
    history = git(repo, "rev-list", "--first-parent", "--reverse", "HEAD").split()
    if history != [commit["sha"] for commit in manifest["commits"]]:
        raise HarnessError(f"fixture repo {repo} does not match its manifest; run `just fixtures`")


def repo_state(repo):
    return git(repo, "rev-parse", "HEAD") + git(repo, "status", "--porcelain=v1", "--ignored", "--untracked-files=all")


def run_engine(engine_command, repo, timeout, cwd):
    before = repo_state(repo)
    command = [*engine_command, "trace", "--repo", os.path.abspath(repo), "--json"]
    try:
        result = subprocess.run(
            command, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout
        )
    except FileNotFoundError:
        raise HarnessError(f"engine command not found: {engine_command[0]}") from None
    except subprocess.TimeoutExpired:
        raise HarnessError(f"engine timed out after {timeout:g}s") from None
    if result.returncode != 0:
        stderr = result.stderr.strip()[-2000:] or "(no stderr)"
        raise HarnessError(f"engine exited with code {result.returncode}: {stderr}")
    if repo_state(repo) != before:
        raise HarnessError("engine modified the fixture repo; `marrow trace` must be read-only")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ContractError(f"stdout is not valid JSON ({error}); stdout began {result.stdout[:200]!r}") from None


def _check_position(where, value, require_commit, commit_order):
    if not isinstance(value, dict):
        raise ContractError(f"{where} must be an object")
    if require_commit and value.get("commit") not in commit_order:
        raise ContractError(f"{where}.commit {value.get('commit')!r} is not in the commits list")
    path = value.get("path")
    if not isinstance(path, str) or not path or "\\" in path or path.startswith(("/", "./")) or ":" in path:
        raise ContractError(f"{where}.path must be a repo-relative path with forward slashes, got {path!r}")
    line = value.get("line")
    if isinstance(line, bool) or not isinstance(line, int) or line < 1:
        raise ContractError(f"{where}.line must be a 1-indexed integer, got {line!r}")


def validate_trace(trace, expected_commits):
    if not isinstance(trace, dict):
        raise ContractError("output must be a JSON object")
    if trace.get("contract_version") != CONTRACT_VERSION:
        raise ContractError(f"contract_version must be {CONTRACT_VERSION}, got {trace.get('contract_version')!r}")
    engine = trace.get("engine")
    if not isinstance(engine, dict) or not all(isinstance(engine.get(key), str) for key in ("name", "version")):
        raise ContractError("engine must be an object with string name and version")
    commits = trace.get("commits")
    if commits != expected_commits:
        raise ContractError(
            f"commits must be the repo's first-parent history, oldest first: expected {expected_commits}, got {commits!r}"
        )
    order = {sha: index for index, sha in enumerate(commits)}
    lines = trace.get("lines")
    if not isinstance(lines, list):
        raise ContractError("lines must be an array")

    births = set()
    for index, entry in enumerate(lines):
        where = f"lines[{index}]"
        if not isinstance(entry, dict):
            raise ContractError(f"{where} must be an object")
        birth = entry.get("birth")
        _check_position(f"{where}.birth", birth, True, order)
        key = (birth["commit"], birth["path"], birth["line"])
        if key in births:
            raise ContractError(f"{where}: another line is already born at {birth['path']}:{birth['line']} in {birth['commit']}")
        births.add(key)
        fates = entry.get("fates")
        if not isinstance(fates, list):
            raise ContractError(f"{where}.fates must be an array")
        next_index = order[birth["commit"]] + 1
        for position, fate in enumerate(fates):
            fate_where = f"{where}.fates[{position}]"
            if not isinstance(fate, dict):
                raise ContractError(f"{fate_where} must be an object")
            if next_index >= len(commits):
                raise ContractError(f"{fate_where}: fate after the last commit")
            if fate.get("commit") != commits[next_index]:
                raise ContractError(f"{fate_where}: expected the fate for commit {commits[next_index]}, got {fate.get('commit')!r}")
            state = fate.get("state")
            if state not in STATES:
                raise ContractError(f"{fate_where}.state must be one of {', '.join(STATES)}, got {state!r}")
            score = fate.get("similarity_score")
            if isinstance(score, bool) or not isinstance(score, (int, float)) or math.isnan(score) or not 0 <= score <= 1:
                raise ContractError(
                    f"{fate_where}.similarity_score must be a number in [0, 1]; every fate decision carries its score, got {score!r}"
                )
            layer = fate.get("deciding_layer")
            if not isinstance(layer, str) or not layer:
                raise ContractError(f"{fate_where}.deciding_layer must be a non-empty string, got {layer!r}")
            if state == "dead":
                if "path" in fate or "line" in fate:
                    raise ContractError(f"{fate_where}: a dead fate must not have path or line")
                if position != len(fates) - 1:
                    raise ContractError(f"{fate_where}: nothing may follow a dead fate")
            else:
                _check_position(fate_where, fate, False, order)
            next_index += 1
        alive = not fates or fates[-1]["state"] != "dead"
        if alive and next_index != len(commits):
            raise ContractError(f"{where}: line is still alive but has no fate for commit {commits[next_index]}")


@dataclass
class Decision:
    line_id: str
    text: str
    birth: dict
    commit: str
    expected: dict
    actual: dict | None
    outcome: str
    missing_reason: str = ""


@dataclass
class Metrics:
    expected: int
    predicted: int
    correct: int

    @property
    def precision(self):
        return self.correct / self.predicted if self.predicted else None

    @property
    def recall(self):
        return self.correct / self.expected if self.expected else None


@dataclass
class FixtureResult:
    name: str
    threshold: int
    engine: dict
    decisions: list
    conflicts: list
    unmatched_engine_lines: int

    @property
    def errors(self):
        return sum(decision.outcome != "correct" for decision in self.decisions) + len(self.conflicts)

    @property
    def passed(self):
        return self.errors <= self.threshold


def judge(expected, actual):
    if actual is None:
        return "missing"
    if actual["state"] != expected["state"]:
        return "wrong_state"
    if expected["state"] != "dead" and (actual["path"], actual["line"]) != (expected["path"], expected["line"]):
        return "misplaced"
    return "correct"


def find_conflicts(trace):
    claims = {}
    for entry in trace["lines"]:
        birth = entry["birth"]
        origin = f"line born {birth['path']}:{birth['line']} in {birth['commit'][:7]}"
        claims.setdefault((birth["commit"], birth["path"], birth["line"]), []).append(f"birth of {origin}")
        for fate in entry["fates"]:
            if fate["state"] != "dead":
                claims.setdefault((fate["commit"], fate["path"], fate["line"]), []).append(f"{fate['state']} {origin}")
    return [(key, claimants) for key, claimants in claims.items() if len(claimants) > 1]


def score_fixture(name, manifest, trace, threshold):
    sha_of = {commit["id"]: commit["sha"] for commit in manifest["commits"]}
    engine_lines = {(entry["birth"]["commit"], entry["birth"]["path"], entry["birth"]["line"]): entry for entry in trace["lines"]}
    decisions = []
    matched = 0
    for record in manifest["lines"]:
        birth = record["birth"]
        entry = engine_lines.get((sha_of[birth["commit"]], birth["path"], birth["line"]))
        if entry is not None:
            matched += 1
        engine_fates = {fate["commit"]: fate for fate in entry["fates"]} if entry else {}
        for fate in record["fates"]:
            actual = engine_fates.get(sha_of[fate["commit"]])
            reason = ""
            if actual is None:
                reason = "engine reported no line born here" if entry is None else "engine's line had already died"
            decisions.append(
                Decision(
                    line_id=record["line_id"],
                    text=record["text"],
                    birth=birth,
                    commit=fate["commit"],
                    expected=fate,
                    actual=actual,
                    outcome=judge(fate, actual),
                    missing_reason=reason,
                )
            )
    return FixtureResult(
        name=name,
        threshold=threshold,
        engine=trace["engine"],
        decisions=decisions,
        conflicts=find_conflicts(trace),
        unmatched_engine_lines=len(trace["lines"]) - matched,
    )


def tally(decisions):
    matrix = {expected: {actual: 0 for actual in (*STATES, "misplaced", "missing")} for expected in STATES}
    for decision in decisions:
        row = matrix[decision.expected["state"]]
        if decision.outcome in ("missing", "misplaced"):
            row[decision.outcome] += 1
        else:
            row[decision.actual["state"]] += 1
    return matrix


def class_metrics(matrix):
    metrics = {}
    for state in STATES:
        metrics[state] = Metrics(
            expected=sum(matrix[state].values()),
            predicted=sum(matrix[row][state] for row in STATES) + matrix[state]["misplaced"],
            correct=matrix[state][state],
        )
    return metrics
