"""Guards the mutation corpus's ground truth.

The generated fixture repos and manifests under eval/fixtures/ are never edited by hand: the
generator writes them. The definitions under eval/generator/definitions/ hold the hand-labelled
answers. An agent may add to them, such as a fixture for a new language, but never change or
remove what is already there, because changing an answer is how a failing engine gets made
to look right.
"""

import json
import os
import sys

GENERATED = "eval/fixtures/"
DEFINITIONS = "eval/generator/definitions/"


def keeps_every_line(old, new):
    """Whether `new` holds all of `old`'s lines, in order: an addition, not a change."""
    remaining = iter(new.splitlines())
    return all(line in remaining for line in old.splitlines())


def refusal(tool, tool_input):
    """Why this edit is refused, or None if it may go ahead."""
    path = tool_input.get("file_path", "").replace("\\", "/")
    if GENERATED in path:
        return (
            f"{GENERATED} is written by the fixture generator. Change the definitions and run "
            "just fixtures instead."
        )
    if DEFINITIONS not in path:
        return None
    if tool == "Edit":
        old = tool_input.get("old_string", "")
        new = tool_input.get("new_string", "")
        if tool_input.get("replace_all") or not keeps_every_line(old, new):
            return (
                f"{DEFINITIONS} holds the corpus's ground truth. Fixtures may be added, but "
                "existing lines can't be changed or removed by an agent. Ask the project owner."
            )
        return None
    if tool == "Write" and os.path.exists(tool_input.get("file_path", "")):
        with open(tool_input["file_path"], encoding="utf-8") as existing:
            old = existing.read()
        if not keeps_every_line(old, tool_input.get("content", "")):
            return (
                f"{DEFINITIONS} holds the corpus's ground truth. Rewriting this file would change "
                "or remove existing lines, which an agent can't do. Ask the project owner."
            )
    return None


def main():
    data = json.load(sys.stdin)
    reason = refusal(data.get("tool_name", ""), data.get("tool_input", {}))
    if reason:
        print(f"Blocked: {reason}", file=sys.stderr)
        sys.exit(2)
    sys.exit(0)


if __name__ == "__main__":
    main()
