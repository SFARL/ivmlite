"""Replay full native suites serially; source bytes are restored by the audited runner."""
import argparse
import importlib.util
import json
from pathlib import Path
import signal
import subprocess
import sys

sys.dont_write_bytecode = True
import verify


def snapshot(root):
    diff = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=root)
    untracked = subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard"], cwd=root, text=True).splitlines()
    return {"source_diff_sha256": verify.sha(diff), "untracked_sha256": {
        p: verify.sha((root / p).read_bytes()) for p in untracked if (root / p).is_file()}}


def interrupted(signum, _frame):
    raise KeyboardInterrupt(f"signal {signum}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("out", type=Path, help="New empty directory outside checkout")
    parser.add_argument("ids", nargs="*", help="Omit to replay all mapped mutations")
    parser.add_argument("--evidence", type=Path, default=Path(__file__).resolve().parent)
    args = parser.parse_args()
    evidence = args.evidence.resolve()
    mapping = verify.read_mapping(evidence)
    unknown = set(args.ids) - {m["id"] for m in mapping}
    if unknown:
        parser.error("Unknown mutation ID(s): " + ", ".join(sorted(unknown)))
    root, out = args.root.resolve(), args.out.resolve()
    if out.is_relative_to(root) or out.is_relative_to(evidence):
        parser.error("Output must be outside checkout and evidence package")
    if out.exists() and (not out.is_dir() or any(out.iterdir())):
        parser.error("Output directory must be new or empty")
    verify.check(root, evidence)
    selected = [m for m in mapping if not args.ids or m["id"] in set(args.ids)]
    verify.preflight(root, selected)
    spec = importlib.util.spec_from_file_location("audited_runner", evidence / "audited_runner.py")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    frozen = snapshot(root)
    out.mkdir(parents=True, exist_ok=True)
    (out / "frozen-source.json").write_text(json.dumps(frozen, indent=2) + "\n")
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    baseline = None
    scenarios = [{"id": "baseline", "edits": []}] + selected + [{"id": "clean-restored", "edits": []}]
    for mutation in scenarios:
        verify.require(snapshot(root) == frozen, "Source drift before " + mutation["id"])
        result = runner.run_one(root, out, mutation, ["scripts/test-all.sh"])
        verify.require(result["restored"] and snapshot(root) == frozen, "Source restoration failed")
        binaries, red = verify.parse_log((out / (mutation["id"] + ".log")).read_text(errors="replace"))
        verify.require(result["compiled"], "Compilation invalid")
        result["binaries"] = binaries
        verify.require(red == result["red"], "Red test inventory differs")
        if not mutation["edits"]:
            verify.require(result["returncode"] == 0 and not result["failed"] and not red, "Clean suite failed")
            if baseline is None:
                baseline = result
            verify.require(binaries == baseline["binaries"], "Restored baseline differs")
        else:
            verify.require(result["returncode"] == 1 and result["failed"] > 0, "Mutation stayed green")
            verify.require(set(mutation["primary_tests"]) <= set(red), "Primary guard stayed green")
            verify.require(len(binaries) == len(baseline["binaries"]), "Partial binary inventory")
            for current, base in zip(binaries, baseline["binaries"]):
                verify.require(all(current[k] == base[k] for k in ("binary", "ignored", "measured", "filtered_out"))
                               and current["passed"] + current["failed"] == base["passed"],
                               "Partial binary inventory: " + current["binary"])
        with (out / "accepted.jsonl").open("a") as stream:
            stream.write(json.dumps(result) + "\n")
        print("ACCEPTED " + mutation["id"], flush=True)


if __name__ == "__main__":
    main()
