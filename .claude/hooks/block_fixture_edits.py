import json
import sys

PROTECTED = ("eval/fixtures/", "eval/generator/definitions/")

data = json.load(sys.stdin)
file_path = data.get("tool_input", {}).get("file_path", "")
normalized = file_path.replace("\\", "/")
for prefix in PROTECTED:
    if prefix in normalized:
        print(
            f"Blocked: {prefix} holds the mutation corpus's ground truth and must not be "
            "edited by an agent. Ask the project owner to change it.",
            file=sys.stderr,
        )
        sys.exit(2)

sys.exit(0)
