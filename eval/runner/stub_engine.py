import argparse
import difflib
import json
import os
import subprocess
import sys

LAYER = "stub_exact_line_sequence"


def git(repo, *args):
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith("GIT_")}
    env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    result = subprocess.run(
        ["git", "--git-dir", os.path.join(repo, ".git"), *args],
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    )
    return result.stdout


def snapshot(repo, sha):
    files = {}
    for path in git(repo, "ls-tree", "-r", "--name-only", sha).splitlines():
        files[path] = git(repo, "show", f"{sha}:{path}").split("\n")[:-1]
    return files


def trace(repo):
    commits = git(repo, "rev-list", "--first-parent", "--reverse", "HEAD").split()
    lines = []
    live = {}
    previous = {}
    for sha in commits:
        files = snapshot(repo, sha)
        next_live = {}
        for path, old_lines in previous.items():
            mapping = {}
            if path in files:
                matcher = difflib.SequenceMatcher(a=old_lines, b=files[path], autojunk=False)
                for block in matcher.get_matching_blocks():
                    for offset in range(block.size):
                        mapping[block.a + offset + 1] = block.b + offset + 1
            for number, text in enumerate(old_lines, start=1):
                if not text.strip():
                    continue
                entry = live[(path, number)]
                if number in mapping:
                    new_line = mapping[number]
                    entry["fates"].append(
                        {"commit": sha, "state": "verbatim", "path": path, "line": new_line,
                         "similarity_score": 1.0, "deciding_layer": LAYER}
                    )
                    next_live[(path, new_line)] = entry
                else:
                    entry["fates"].append({"commit": sha, "state": "dead", "similarity_score": 0.0, "deciding_layer": LAYER})
        for path, new_lines in files.items():
            for number, text in enumerate(new_lines, start=1):
                if text.strip() and (path, number) not in next_live:
                    entry = {"birth": {"commit": sha, "path": path, "line": number}, "fates": []}
                    lines.append(entry)
                    next_live[(path, number)] = entry
        live = next_live
        previous = files
    return {
        "contract_version": 1,
        "engine": {"name": "stub", "version": "0"},
        "commits": commits,
        "lines": lines,
    }


def main():
    parser = argparse.ArgumentParser(description="Placeholder engine for the eval harness; not Marrow.")
    commands = parser.add_subparsers(dest="command", required=True)
    trace_command = commands.add_parser("trace")
    trace_command.add_argument("--repo", required=True)
    trace_command.add_argument("--json", action="store_true", required=True)
    args = parser.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    json.dump(trace(args.repo), sys.stdout, ensure_ascii=False)
    return 0


if __name__ == "__main__":
    sys.exit(main())
