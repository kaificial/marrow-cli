import json
import sys

data = json.load(sys.stdin)
file_path = data.get("tool_input", {}).get("file_path", "")
normalized = file_path.replace("\\", "/")
if "eval/fixtures/" in normalized:
    print(
        "Blocked: eval/fixtures/ is the ground-truth mutation corpus and must "
        "not be edited by an agent. Ask the project owner to change it.",
        file=sys.stderr,
    )
    sys.exit(2)

sys.exit(0)
