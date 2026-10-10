"""PostToolUse hook: tell marrow about a file the agent just wrote.

Capture must never get in the way of the write itself, so a missing binary or an
unbuilt engine is silent, and nothing here can block a tool call. A record that
fails for some other reason is worth seeing, so its message is passed through.
"""

import os
import subprocess
import sys

payload = sys.stdin.read()
project_dir = os.environ.get("CLAUDE_PROJECT_DIR", ".")
name = "marrow.exe" if os.name == "nt" else "marrow"
candidates = [
    os.environ.get("MARROW_BIN"),
    os.path.join(project_dir, "engine", "target", "release", name),
    os.path.join(project_dir, "engine", "target", "debug", name),
]
binary = next((path for path in candidates if path and os.path.exists(path)), None)
if binary is None:
    sys.exit(0)

result = subprocess.run(
    [binary, "record", "--hook"],
    input=payload,
    text=True,
    capture_output=True,
    check=False,
)
if result.returncode != 0:
    sys.stderr.write(result.stderr)
    # A warning, not a block: the write already happened and marrow is only watching.
    sys.exit(1)
# Shown in transcript mode, so what marrow recorded can be seen as it happens.
sys.stdout.write(result.stdout)
sys.exit(0)
