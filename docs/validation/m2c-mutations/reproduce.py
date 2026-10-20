"""Run checked mutation edits, retain full-suite evidence, always restore bytes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time


def digest(data):
    return hashlib.sha256(data).hexdigest()


def terminate_process_group(process):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            pass  # The process may have exited between poll() and killpg().
        try:
            process.wait(timeout=20)
            return
        except subprocess.TimeoutExpired:
            if sig == signal.SIGKILL:
                raise


def run_one(root, out, mutation, command):
    originals = {}
    changed = {}
    for edit in mutation["edits"]:
        path = root / edit["file"]
        if path not in originals:
            originals[path] = path.read_bytes()
            changed[path] = originals[path].decode()
        expected = edit.get("expected_count", 1)
        actual = changed[path].count(edit["old"])
        if actual != expected:
            raise ValueError(f"{mutation['id']}: {path}: expected {expected} matches, got {actual}")
        changed[path] = changed[path].replace(edit["old"], edit["new"])
    result = {
        "id": mutation["id"], "rows": mutation.get("rows", []),
        "command": command, "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "source_diff_sha256": digest(subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=root)),
        "original_sha256": {str(p.relative_to(root)): digest(b) for p, b in originals.items()},
        "edits_sha256": digest(json.dumps(mutation["edits"], sort_keys=True).encode()),
    }
    untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard"], cwd=root, text=True).splitlines()
    result["untracked_sha256"] = {p: digest((root / p).read_bytes()) for p in untracked if (root / p).is_file()}
    log_path = out / f"{mutation['id']}.log"
    started = time.monotonic()
    process = None
    try:
        for path, source in changed.items():
            path.write_text(source)
        with log_path.open("wb") as log:
            process = subprocess.Popen(command, cwd=root, stdout=log, stderr=subprocess.STDOUT,
                                       start_new_session=True)
            try:
                result["returncode"] = process.wait(timeout=1200)
            except subprocess.TimeoutExpired:
                result["returncode"] = "timeout"
    finally:
        try:
            if process is not None and process.poll() is None:
                terminate_process_group(process)
        finally:
            for path, data in originals.items():
                path.write_bytes(data)
            result["restored"] = all(p.read_bytes() == b for p, b in originals.items())
    result["wall_seconds"] = time.monotonic() - started
    output = log_path.read_text(errors="replace")
    suites = re.findall(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
    result["passed"] = sum(int(p) for p, f, i in suites)
    result["failed"] = sum(int(f) for p, f, i in suites)
    result["ignored"] = sum(int(i) for p, f, i in suites)
    result["suite_count"] = len(suites)
    result["red"] = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", output, re.M)))
    result["compiled"] = bool(suites) and not re.search(r"could not compile|^error\[E\d+\]", output, re.M)
    result["log_sha256"] = digest(log_path.read_bytes())
    with (out / "results.jsonl").open("a") as stream:
        stream.write(json.dumps(result) + "\n")
    print(json.dumps(result), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("mapping", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("ids", nargs="*")
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args()
    mutations = json.loads(args.mapping.read_text())
    selected = set(args.ids)
    unknown = selected - {mutation["id"] for mutation in mutations}
    if unknown:
        parser.error("Unknown mutation ID(s): " + ", ".join(sorted(unknown)))
    args.root = args.root.resolve()
    args.out = args.out.resolve()
    for label, path in (("runner", Path(__file__).resolve()),
                        ("mapping", args.mapping.resolve()), ("output", args.out)):
        if path.is_relative_to(args.root):
            parser.error(f"The {label} must be outside the checkout")
    untracked = subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard"], cwd=args.root, text=True
    ).splitlines()
    if untracked:
        parser.error("Checkout has untracked files: " + ", ".join(untracked))
    args.out.mkdir(parents=True, exist_ok=True)
    command = ["scripts/test-all.sh"] + (["--release"] if args.release else [])
    for mutation in mutations:
        if selected and mutation["id"] not in selected:
            continue
        if not mutation.get("ready", True):
            raise ValueError(f"mutation not ready: {mutation['id']}")
        wanted = mutation.get("source_commit")
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.root, text=True).strip()
        if wanted and actual != wanted:
            raise RuntimeError(f"{mutation['id']}: checkout {wanted} before replay (current {actual})")
        if subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=args.root):
            raise RuntimeError("Tracked source must be clean before replay")
        untracked = subprocess.check_output(
            ["git", "ls-files", "--others", "--exclude-standard"], cwd=args.root, text=True
        ).splitlines()
        if untracked:
            raise RuntimeError("Checkout has untracked files: " + ", ".join(untracked))
        for filename, expected in mutation.get("original_sha256", {}).items():
            if digest((args.root / filename).read_bytes()) != expected:
                raise RuntimeError(f"Original source differs: {filename}")
        result = run_one(args.root, args.out, mutation, command)
        if not mutation["edits"] and (result["returncode"] != 0 or result["failed"] or not result["compiled"]):
            raise RuntimeError("unmutated baseline failed; mutation batch stopped")


if __name__ == "__main__":
    main()
