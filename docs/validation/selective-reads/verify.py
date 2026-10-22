"""Verify selective-read evidence and source bindings using Python only."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import re
import subprocess


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def inside(root, name):
    path = (root / name).resolve()
    require(path.is_relative_to(root) and path != root, f"Unsafe path: {name}")
    return path


def lines(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def source_paths(root):
    """Full Rust workspace closure, including example test registration/build helpers."""
    paths = {"Cargo.toml", "Cargo.lock", "scripts/test-all.sh", "scripts/build-extension.sh"}
    inventory = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=root).decode().split("\0")
    paths.update(name for name in inventory if name and (
        name.endswith(".rs") or Path(name).name in {"Cargo.toml", "Cargo.lock"}
        or name.startswith(".cargo/")))
    return paths


def read_mapping(evidence):
    mapping = json.loads((evidence / "mapping.json").read_text())
    ids = [m["id"] for m in mapping]
    require(ids and len(ids) == len(set(ids)), "Empty mapping or duplicate mutation IDs")
    for m in mapping:
        require(re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", m["id"]), "Unsafe mutation ID")
        require(m["id"] not in {"baseline", "clean-restored"}, "Reserved mutation ID")
        require(m["edits"] and m["primary_tests"], f"Missing edits/primary guards: {m['id']}")
        require(len(m["primary_tests"]) == len(set(m["primary_tests"])), "Duplicate primary guard")
        for edit in m["edits"]:
            require(edit["old"] and edit["old"] != edit["new"], "Empty or ineffective edit")
            require(type(edit.get("expected_count", 1)) is int and edit.get("expected_count", 1) > 0,
                    "Invalid exact-match count")
    return mapping


def preflight(root, mapping):
    """Check every selected edit before any runner is allowed to write source."""
    for m in mapping:
        changed = {}
        for edit in m["edits"]:
            path = inside(root, edit["file"])
            text = changed.get(path, path.read_bytes().decode())
            require(text.count(edit["old"]) == edit.get("expected_count", 1),
                    f"Exact edit mismatch: {m['id']}: {edit['file']}")
            changed[path] = text.replace(edit["old"], edit["new"])


def parse_log(text):
    binary, records, red = None, [], []
    binary_red = []
    for line in text.splitlines():
        header = re.match(r"^\s*Running (.*?) \((.*)\)\s*$", line)
        if header:
            require(binary is None, f"Missing binary summary: {binary}")
            stem = re.sub(r"-[0-9a-f]+$", "", Path(header[2]).name)
            binary, binary_red = f"{header[1]} [{stem}]", []
        elif re.match(r"^\s*Doc-tests ", line):
            require(binary is None, f"Missing binary summary: {binary}")
            binary, binary_red = line.strip(), []
        failure = re.match(r"^test (\S+) \.\.\. FAILED", line)
        if failure:
            require(binary is not None, "Red test without binary")
            red.append(failure[1])
            binary_red.append(failure[1])
        result = re.match(r"^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;", line)
        if result:
            require(binary is not None, "Summary without binary")
            record = dict(binary=binary, status=result[1], **dict(zip(
                ("passed", "failed", "ignored", "measured", "filtered_out"), map(int, result.groups()[1:]))))
            require(record["failed"] == len(binary_red), f"Red test count mismatch: {binary}")
            require(record["status"] == ("FAILED" if record["failed"] else "ok"), "Invalid test status")
            records.append(record)
            binary, binary_red = None, []
    require(records and binary is None and not binary_red, "Missing or partial binary summaries")
    require(not re.search(r"could not compile|^error\[E\d+\]", text, re.M), "Compilation invalid")
    require(not any(b["filtered_out"] or b["measured"] for b in records), "Filtered/measured tests")
    return records, sorted(set(red))


def check(root, evidence):
    root, evidence = root.resolve(), evidence.resolve()
    manifest = json.loads((evidence / "manifest.json").read_text())
    require(manifest["schema"] == 1, "Unknown evidence schema")
    required = {"mapping.json", "accepted.jsonl", "results.jsonl", "frozen-source.json",
                "verify.py", "replay.py", "audited_runner.py"}
    require(required <= manifest["files"].keys(), "Missing evidence file bindings")
    for name, expected in manifest["files"].items():
        require(sha(inside(evidence, name).read_bytes()) == expected, f"Evidence hash: {name}")
    require(source_paths(root) <= manifest["source_files"].keys(), "Incomplete source manifest")
    for name, expected in manifest["source_files"].items():
        require(sha(inside(root, name).read_bytes()) == expected, f"Source hash: {name}")
    mapping = read_mapping(evidence)
    require(len(mapping) == manifest["mutation_groups"], "Mutation count differs")
    preflight(root, mapping)
    accepted = lines(evidence / "accepted.jsonl")
    raw_results = lines(evidence / "results.jsonl")
    expected_ids = ["baseline"] + [m["id"] for m in mapping] + ["clean-restored"]
    require([r["id"] for r in accepted] == expected_ids, "Accepted inventory/order differs")
    require([r["id"] for r in raw_results] == expected_ids, "Raw result inventory/order differs")
    require(set(manifest["logs"]) == set(expected_ids), "Log inventory differs")
    frozen = json.loads((evidence / "frozen-source.json").read_text())
    mutations = {m["id"]: m for m in mapping}
    baseline = accepted[0]
    for record, raw in zip(accepted, raw_results):
        run_id = record["id"]
        require(all(record[k] == v for k, v in raw.items()), f"Raw result differs: {run_id}")
        require(record["command"] == ["scripts/test-all.sh"], f"Partial suite command: {run_id}")
        require(record["compiled"] and record["restored"], f"Compile/restoration failed: {run_id}")
        for key in ("source_diff_sha256", "untracked_sha256"):
            require(record[key] == frozen[key], f"Frozen source differs: {run_id}: {key}")
        require(record["head"] == baseline["head"], f"HEAD changed within campaign: {run_id}")
        log = manifest["logs"][run_id]
        compressed = inside(evidence, log["path"]).read_bytes()
        require(sha(compressed) == log["gzip_sha256"], f"Compressed log hash: {run_id}")
        data = gzip.decompress(compressed)
        require(sha(data) == log["normalized_sha256"], f"Normalized log hash: {run_id}")
        require(log["raw_sha256"] == record["log_sha256"], f"Raw log binding: {run_id}")
        binaries, red = parse_log(data.decode())
        require(binaries == record["binaries"] and red == record["red"], f"Parsed log differs: {run_id}")
        for field in ("passed", "failed", "ignored"):
            require(record[field] == sum(b[field] for b in binaries), f"Totals differ: {run_id}")
        require(record["suite_count"] == len(binaries), f"Summary count differs: {run_id}")
        mutation = mutations.get(run_id, {"edits": [], "primary_tests": []})
        require(record["edits_sha256"] == sha(json.dumps(mutation["edits"], sort_keys=True).encode()),
                f"Exact edit hash differs: {run_id}")
        changed_files = {edit["file"] for edit in mutation["edits"]}
        require(record["original_sha256"] == {p: manifest["source_files"][p] for p in changed_files},
                f"Original source binding: {run_id}")
        if mutation["edits"]:
            require(record["returncode"] == 1 and record["failed"] > 0, f"Mutation stayed green: {run_id}")
            require(set(mutation["primary_tests"]) <= set(red), f"Primary guard stayed green: {run_id}")
            require(len(binaries) == len(baseline["binaries"]), f"Binary count differs: {run_id}")
            for current, base in zip(binaries, baseline["binaries"]):
                require(all(current[k] == base[k] for k in ("binary", "ignored", "measured", "filtered_out"))
                        and current["passed"] + current["failed"] == base["passed"],
                        f"Partial binary inventory: {run_id}: {current['binary']}")
        else:
            require(record["returncode"] == 0 and not record["failed"] and not red,
                    f"Clean suite failed: {run_id}")
            require(binaries == baseline["binaries"], "Restored baseline differs")
    return manifest, mapping


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="Checkout whose source bytes must match")
    parser.add_argument("--evidence", type=Path, default=Path(__file__).resolve().parent)
    args = parser.parse_args()
    _, mapping = check(args.root, args.evidence)
    print(f"Verified {len(mapping)} mutations, baseline/restored suites, source and log hashes.")
