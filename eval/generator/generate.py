import argparse
import os
import sys

sys.dont_write_bytecode = True

GENERATOR_DIR = os.path.dirname(os.path.abspath(__file__))
FIXTURES_ROOT = os.path.join(os.path.dirname(GENERATOR_DIR), "fixtures")
sys.path.insert(0, GENERATOR_DIR)

from definitions import ALL_FIXTURES
from fixture_lib import FixtureError, build_fixture


def main():
    parser = argparse.ArgumentParser(description="Generate the Marrow mutation-corpus fixture repos and manifests.")
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if a committed manifest differs from a fresh generation instead of overwriting it",
    )
    args = parser.parse_args()

    names = [fixture.name for fixture in ALL_FIXTURES]
    if len(names) != len(set(names)):
        print("error: duplicate fixture names", file=sys.stderr)
        return 1

    drifted = []
    for fixture in ALL_FIXTURES:
        target = os.path.join(FIXTURES_ROOT, fixture.mutation_class, fixture.language)
        try:
            manifest, counts = build_fixture(fixture, target, FIXTURES_ROOT)
        except FixtureError as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        manifest_path = os.path.join(target, "manifest.json")
        if args.check:
            try:
                with open(manifest_path, encoding="utf-8") as handle:
                    committed = handle.read()
            except FileNotFoundError:
                committed = None
            if committed != manifest:
                drifted.append(fixture.name)
        else:
            with open(manifest_path, "w", encoding="utf-8", newline="\n") as handle:
                handle.write(manifest)
            print(f"{fixture.name:<30} " + "  ".join(f"{key}={value}" for key, value in counts.items()))

    if drifted:
        print(f"error: manifests differ from a fresh generation: {', '.join(drifted)}", file=sys.stderr)
        return 1
    if args.check:
        print(f"fixtures-check: {len(ALL_FIXTURES)} fixture repos regenerated; every manifest matches")
    return 0


if __name__ == "__main__":
    sys.exit(main())
