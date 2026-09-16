import json
import subprocess
import sys

data = json.load(sys.stdin)
command = data.get("tool_input", {}).get("command", "")
if "git commit" not in command:
    sys.exit(0)

branch = subprocess.run(
    ["git", "rev-parse", "--abbrev-ref", "HEAD"],
    capture_output=True,
    text=True,
    check=False,
).stdout.strip()

if branch == "main":
    print(
        "Blocked: refusing to run `git commit` while HEAD is on main. "
        "Create a branch first.",
        file=sys.stderr,
    )
    sys.exit(2)

sys.exit(0)
