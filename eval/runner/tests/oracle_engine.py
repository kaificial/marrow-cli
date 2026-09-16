import argparse
import json
import os
import sys


def main():
    parser = argparse.ArgumentParser(description="Test double that replays a fixture manifest as a perfect trace.")
    commands = parser.add_subparsers(dest="command", required=True)
    trace = commands.add_parser("trace")
    trace.add_argument("--repo", required=True)
    trace.add_argument("--json", action="store_true", required=True)
    args = parser.parse_args()

    with open(os.path.join(os.path.dirname(os.path.abspath(args.repo)), "manifest.json"), encoding="utf-8") as handle:
        manifest = json.load(handle)
    sha_of = {commit["id"]: commit["sha"] for commit in manifest["commits"]}
    lines = []
    for record in manifest["lines"]:
        fates = []
        for fate in record["fates"]:
            replayed = {"commit": sha_of[fate["commit"]], "state": fate["state"]}
            if fate["state"] != "dead":
                replayed.update(path=fate["path"], line=fate["line"])
            replayed.update(similarity_score=1.0, deciding_layer="oracle")
            fates.append(replayed)
        birth = record["birth"]
        lines.append({"birth": {**birth, "commit": sha_of[birth["commit"]]}, "fates": fates})
    json.dump(
        {
            "contract_version": 1,
            "engine": {"name": "oracle", "version": "test"},
            "commits": [commit["sha"] for commit in manifest["commits"]],
            "lines": lines,
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
