import argparse
import os
import sys

sys.dont_write_bytecode = True

RUNNER_DIR = os.path.dirname(os.path.abspath(__file__))
EVAL_DIR = os.path.dirname(RUNNER_DIR)
PROJECT_ROOT = os.path.dirname(EVAL_DIR)
sys.path.insert(0, RUNNER_DIR)

from harness import (
    STATES,
    ContractError,
    HarnessError,
    check_repo_current,
    class_metrics,
    discover_fixtures,
    load_thresholds,
    run_engine,
    score_fixture,
    tally,
    validate_trace,
)

EXIT_PASS, EXIT_THRESHOLD, EXIT_HARNESS = 0, 1, 2


def fmt_ratio(value):
    return "-" if value is None else f"{value:.2f}"


def fmt_position(fate):
    return f"{fate['path']}:{fate['line']}"


def describe_expected(fate):
    return "dead" if fate["state"] == "dead" else f"{fate['state']} @ {fmt_position(fate)}"


def describe_actual(decision):
    actual = decision.actual
    if actual is None:
        return f"nothing ({decision.missing_reason})"
    where = "" if actual["state"] == "dead" else f" @ {fmt_position(actual)}"
    wrong_place = " (wrong place)" if decision.outcome == "misplaced" else ""
    evidence = f" [score {actual['similarity_score']:.2f}, layer {actual['deciding_layer']}]"
    return f"{actual['state']}{where}{wrong_place}{evidence}"


def render_fixture(result, show, out):
    status = "PASS " if result.passed else "FAIL "
    print(
        f"{status} {result.name:<30} errors {result.errors:>3} / threshold {result.threshold}"
        f"   ({len(result.decisions)} fate decisions)",
        file=out,
    )
    if result.passed:
        return
    failures = [decision for decision in result.decisions if decision.outcome != "correct"]
    shown = failures if show == 0 else failures[:show]
    for decision in shown:
        birth = decision.birth
        print(
            f"      {decision.line_id} (born {birth['commit']} {birth['path']}:{birth['line']}) at {decision.commit}: "
            f"expected {describe_expected(decision.expected)}, got {describe_actual(decision)}",
            file=out,
        )
        print(f"         `{decision.text}`", file=out)
    for (commit, path, line), claimants in result.conflicts:
        print(f"      conflict at {path}:{line} in {commit[:7]}: claimed by {'; '.join(claimants)}", file=out)
    hidden = len(failures) - len(shown)
    if hidden:
        print(f"      ... {hidden} more; rerun with --only {result.name} --show 0 to list all", file=out)
    if result.unmatched_engine_lines:
        print(
            f"      note: {result.unmatched_engine_lines} engine lines were born where the manifest has no birth "
            f"(blank lines, or births the engine created instead of a continuation)",
            file=out,
        )


def render_tables(results, out):
    print("\n== Precision / recall per fixture (precision / recall; '-' = undefined, nothing predicted or expected) ==", file=out)
    header = f"{'fixture':<30}" + "".join(f"{state:>14}" for state in STATES) + f"{'errors':>9}"
    print(header, file=out)
    for result in results:
        metrics = class_metrics(tally(result.decisions))
        cells = "".join(
            f"{fmt_ratio(metrics[state].precision) + ' / ' + fmt_ratio(metrics[state].recall):>14}" for state in STATES
        )
        print(f"{result.name:<30}{cells}{result.errors:>9}", file=out)

    all_decisions = [decision for result in results for decision in result.decisions]
    matrix = tally(all_decisions)
    metrics = class_metrics(matrix)
    print(f"\n== Aggregate over {len(results)} fixtures ({len(all_decisions)} fate decisions) ==", file=out)
    print(f"{'class':<10}{'expected':>10}{'predicted':>11}{'correct':>9}{'precision':>11}{'recall':>8}", file=out)
    for state in STATES:
        m = metrics[state]
        print(
            f"{state:<10}{m.expected:>10}{m.predicted:>11}{m.correct:>9}{fmt_ratio(m.precision):>11}{fmt_ratio(m.recall):>8}",
            file=out,
        )

    columns = (*STATES, "misplaced", "missing")
    print("\nConfusion matrix (rows = expected fate, columns = engine's answer; misplaced = right state, wrong line)", file=out)
    print(f"{'':<10}" + "".join(f"{column:>11}" for column in columns), file=out)
    for state in STATES:
        print(f"{state:<10}" + "".join(f"{matrix[state][column]:>11}" for column in columns), file=out)

    by_layer = {}
    for decision in all_decisions:
        if decision.outcome != "correct":
            layer = decision.actual["deciding_layer"] if decision.actual else "(no decision)"
            by_layer[layer] = by_layer.get(layer, 0) + 1
    if by_layer:
        print("\nErrors by deciding layer", file=out)
        for layer, count in sorted(by_layer.items(), key=lambda item: (-item[1], item[0])):
            print(f"  {layer:<40}{count:>6}", file=out)


def main(argv=None, out=None):
    out = out or sys.stdout
    parser = argparse.ArgumentParser(
        prog="run.py",
        usage="run.py [--only TEXT] [--show N] [--timeout SECONDS] -- ENGINE_COMMAND...",
        description="Score a genealogy engine's `trace --repo <path> --json` output against the mutation corpus.",
    )
    parser.add_argument("--only", default="", help="only fixtures whose name contains TEXT")
    parser.add_argument("--show", type=int, default=10, help="failing lines to list per fixture (0 = all)")
    parser.add_argument("--timeout", type=float, default=300.0, help="seconds allowed per engine run")
    parser.add_argument("engine", nargs=argparse.REMAINDER, help="engine command, after --")
    args = parser.parse_args(argv)
    engine = args.engine[1:] if args.engine[:1] == ["--"] else args.engine
    if not engine:
        parser.error("an engine command is required after --")

    readme = os.path.join(EVAL_DIR, "README.md")
    try:
        fixtures = discover_fixtures(os.path.join(EVAL_DIR, "fixtures"))
        thresholds = load_thresholds(readme, [name for name, _, _ in fixtures])
    except HarnessError as error:
        print(f"HARNESS ERROR: {error}", file=out)
        return EXIT_HARNESS
    selected = [fixture for fixture in fixtures if args.only in fixture[0]]
    if not selected:
        print(f"HARNESS ERROR: no fixture name contains {args.only!r}", file=out)
        return EXIT_HARNESS

    print(f"Marrow eval: {len(selected)} fixtures, thresholds from eval/README.md", file=out)
    print(f"Engine command: {' '.join(engine)} trace --repo <fixture repo> --json", file=out)
    print(file=out)

    results = []
    harness_errors = []
    engine_names = set()
    for name, manifest, repo in selected:
        try:
            check_repo_current(repo, manifest)
            trace = run_engine(engine, repo, args.timeout, PROJECT_ROOT)
            validate_trace(trace, [commit["sha"] for commit in manifest["commits"]])
        except ContractError as error:
            harness_errors.append(name)
            print(f"ERROR {name:<30} engine output violates docs/specs/trace-cli.md: {error}", file=out)
            continue
        except HarnessError as error:
            harness_errors.append(name)
            print(f"ERROR {name:<30} {error}", file=out)
            continue
        result = score_fixture(name, manifest, trace, thresholds[name])
        engine_names.add(result.engine["name"])
        results.append(result)
        render_fixture(result, args.show, out)

    if results:
        render_tables(results, out)

    failed = [result.name for result in results if not result.passed]
    print("\n== Result ==", file=out)
    if engine_names - {"marrow"}:
        names = ", ".join(sorted(engine_names))
        print(f"WARNING: engine reports name {names!r}, not 'marrow'. These numbers do not grade the real engine.", file=out)
    if harness_errors:
        print(
            f"HARNESS ERROR: {len(harness_errors)} fixture(s) could not be scored: {', '.join(harness_errors)} (exit {EXIT_HARNESS})",
            file=out,
        )
    if failed:
        print(
            f"FAIL: {len(failed)} of {len(results)} scored fixtures exceed their threshold in eval/README.md: "
            f"{', '.join(failed)} (exit {EXIT_THRESHOLD if not harness_errors else EXIT_HARNESS})",
            file=out,
        )
    if harness_errors:
        return EXIT_HARNESS
    if failed:
        return EXIT_THRESHOLD
    print(f"PASS: all {len(results)} fixtures within their thresholds", file=out)
    return EXIT_PASS


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
