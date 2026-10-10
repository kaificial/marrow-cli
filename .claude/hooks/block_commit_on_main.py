"""Blocks an agent's `git commit` while the repository it commits to is on main.

The repository that matters is the one the commit runs in, not the one this hook lives in.
So the hook follows the command the way the shell would: it starts from the directory Claude
Code says the command runs in, applies any `cd` before the commit, and honours `git -C <dir>`.
"""

import json
import os
import re
import shlex
import subprocess
import sys

PROTECTED = "main"

# git options that take a value as the next word, which mustn't be mistaken for the subcommand.
OPTIONS_WITH_VALUE = {"-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env"}


def native(path):
    """A Git Bash path such as /c/Users/me as Windows spells it, C:/Users/me."""
    if os.name == "nt":
        drive = re.match(r"^/([a-zA-Z])(?:/|$)(.*)", path)
        if drive:
            return f"{drive.group(1)}:/{drive.group(2)}"
    return path


def resolve(base, path):
    return os.path.normpath(os.path.join(base, native(os.path.expanduser(path))))


def segments(command):
    """The simple commands in a command line, each as its words."""
    lexer = shlex.shlex(command.replace("\n", " ; "), posix=True, punctuation_chars=";&|")
    lexer.whitespace_split = True
    current = []
    for word in lexer:
        if word and set(word) <= set(";&|"):
            if current:
                yield current
            current = []
        else:
            current.append(word)
    if current:
        yield current


def commit_directories(command, cwd):
    """Every directory a `git commit` in this command line would run in."""
    directory = cwd
    for words in segments(command):
        words = [words[0].lstrip("(")] + words[1:]
        # Skip leading variable assignments, like GIT_AUTHOR_DATE=... git commit.
        while words and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", words[0]):
            words = words[1:]
        if not words:
            continue
        if words[0] in ("cd", "pushd"):
            directory = resolve(directory, words[1] if len(words) > 1 else "~")
            continue
        if words[0] != "git":
            continue
        target = directory
        index = 1
        while index < len(words):
            word = words[index]
            if word == "-C" and index + 1 < len(words):
                target = resolve(target, words[index + 1])
                index += 2
            elif word in OPTIONS_WITH_VALUE:
                index += 2
            elif word.startswith("-"):
                index += 1
            else:
                if word == "commit":
                    yield target
                break


def branch(directory):
    """The branch checked out in the repository at `directory`, or None if it isn't one.

    symbolic-ref rather than rev-parse, because it also names the branch of a repository with
    no commits yet, whose first commit would otherwise slip through onto main.
    """
    result = subprocess.run(
        ["git", "-C", directory, "symbolic-ref", "--short", "-q", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def blocked(command, cwd):
    """The repository a commit in `command` would land on main in, or None."""
    if "commit" not in command:
        return None
    try:
        directories = list(commit_directories(command, cwd))
    except ValueError:
        # Quotes that don't close: the shell will refuse it too, but err on the side of
        # checking where the agent is rather than letting it through.
        directories = [cwd] if re.search(r"\bgit\b.*\bcommit\b", command) else []
    for directory in directories:
        if branch(directory) == PROTECTED:
            return directory
    return None


def main():
    data = json.load(sys.stdin)
    command = data.get("tool_input", {}).get("command", "")
    cwd = data.get("cwd") or os.getcwd()
    repository = blocked(command, cwd)
    if repository is not None:
        print(
            f"Blocked: refusing to run `git commit` while {repository} is on {PROTECTED}. "
            "Create a branch first.",
            file=sys.stderr,
        )
        sys.exit(2)
    sys.exit(0)


if __name__ == "__main__":
    main()
