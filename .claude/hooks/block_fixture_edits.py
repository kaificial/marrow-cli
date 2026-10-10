"""Guards the mutation corpus's ground truth.

The generated fixture repos and manifests under eval/fixtures/ are never edited by hand: the
generator writes them. The definitions under eval/generator/definitions/ hold the hand-labelled
answers. An agent may add to them, such as a fixture for a new language, but never change or
remove an answer that has been committed, because changing an answer is how a failing engine
gets made to look right. Lines added since the last commit are work in progress and can still
be corrected.
"""

import json
import os
import subprocess
import sys

GENERATED = "eval/fixtures/"
DEFINITIONS = "eval/generator/definitions/"


def keeps_every_line(old, new):
    """Whether `new` holds all of `old`'s lines, in order: an addition, not a change."""
    remaining = iter(new.splitlines())
    return all(line in remaining for line in old.splitlines())


def committed(path):
    """The file as HEAD has it, or None if it isn't committed yet."""
    directory = os.path.dirname(os.path.abspath(path)) or "."
    top = subprocess.run(
        ["git", "-C", directory, "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=False,
    )
    if top.returncode != 0:
        return None
    root = top.stdout.strip()
    relative = os.path.relpath(os.path.abspath(path), root).replace("\\", "/")
    shown = subprocess.run(
        ["git", "-C", root, "show", f"HEAD:{relative}"],
        capture_output=True,
        check=False,
    )
    if shown.returncode != 0:
        return None
    return shown.stdout.decode("utf-8").replace("\r\n", "\n")


def after_edit(tool, tool_input):
    """The whole file as it would be after this edit, or None if that can't be worked out."""
    path = tool_input.get("file_path", "")
    if tool == "Write":
        return tool_input.get("content", "")
    try:
        with open(path, encoding="utf-8") as existing:
            current = existing.read()
    except OSError:
        return None
    old = tool_input.get("old_string", "")
    new = tool_input.get("new_string", "")
    if tool_input.get("replace_all"):
        return current.replace(old, new)
    return current.replace(old, new, 1)


def refusal(tool, tool_input, committed_version=committed):
    """Why this edit is refused, or None if it may go ahead."""
    path = tool_input.get("file_path", "").replace("\\", "/")
    if GENERATED in path:
        return (
            f"{GENERATED} is written by the fixture generator. Change the definitions and run "
            "just fixtures instead."
        )
    if DEFINITIONS not in path or tool not in ("Edit", "Write"):
        return None
    base = committed_version(tool_input.get("file_path", ""))
    if base is None:
        return None
    result = after_edit(tool, tool_input)
    if result is None or not keeps_every_line(base, result.replace("\r\n", "\n")):
        return (
            f"{DEFINITIONS} holds the corpus's ground truth. Fixtures may be added, but a "
            "committed line can't be changed or removed by an agent. Ask the project owner."
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
