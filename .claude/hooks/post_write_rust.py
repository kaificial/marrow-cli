import json
import os
import subprocess
import sys

data = json.load(sys.stdin)
file_path = data.get("tool_input", {}).get("file_path", "")
if not file_path.endswith(".rs"):
    sys.exit(0)

project_dir = os.environ.get("CLAUDE_PROJECT_DIR", ".")
manifest = os.path.join(project_dir, "engine", "Cargo.toml")

subprocess.run(["cargo", "fmt", "--manifest-path", manifest, "--all"], check=False)
subprocess.run(
    [
        "cargo",
        "clippy",
        "--manifest-path",
        manifest,
        "--workspace",
        "--all-targets",
        "--fix",
        "--allow-dirty",
        "--allow-staged",
    ],
    check=False,
)
sys.exit(0)
