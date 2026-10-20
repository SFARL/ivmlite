"""Validate published mutation evidence with Python only; never build or run tests."""
import gzip
import hashlib
import importlib.util
import io
import json
from contextlib import redirect_stdout
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from unittest.mock import Mock, call, patch

ROOT = Path(__file__).resolve().parent

def sha(data):
    return hashlib.sha256(data).hexdigest()

def read_lines(name):
    return [json.loads(line) for line in (ROOT / name).read_text().splitlines()]

def check_replay_cleanup():
    spec = importlib.util.spec_from_file_location("mutation_reproduce", ROOT / "reproduce.py")
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    timeout = subprocess.TimeoutExpired("fixture", 20)
    cases = (
        ("never-exits", [timeout, timeout, timeout], None, subprocess.TimeoutExpired),
        ("signal-error", [timeout], OSError("fixture cleanup failure"), OSError),
        ("kill-escalation", [timeout, timeout, -9], None, None),
        ("term-exit", [timeout, -15], None, None),
    )
    for name, waits, signal_error, expected_error in cases:
        with tempfile.TemporaryDirectory(prefix=f"ivmlite-replay-{name}-") as directory:
            temporary = Path(directory)
            source = temporary / "fixture.rs"
            source.write_bytes(b"original")
            mutation = {"id": "fixture", "edits": [
                {"file": "fixture.rs", "old": "original", "new": "mutated"}
            ]}
            process = Mock(pid=12345)
            process.poll.return_value = None
            outcomes = iter(waits)
            def wait(*, timeout):
                assert source.read_bytes() == b"mutated", "Fixture must exercise an applied edit"
                outcome = next(outcomes)
                if isinstance(outcome, Exception):
                    raise outcome
                return outcome
            process.wait.side_effect = wait
            with patch.object(runner.subprocess, "check_output", side_effect=["head\n", b"", ""]), \
                 patch.object(runner.subprocess, "Popen", return_value=process), \
                 patch.object(runner.os, "killpg", side_effect=signal_error) as killpg, \
                 redirect_stdout(io.StringIO()):
                caught = None
                try:
                    result = runner.run_one(temporary, temporary, mutation, ["fixture"])
                except Exception as error:
                    caught = error
                assert source.read_bytes() == b"original", f"Source restoration: {name}"
                if expected_error is None:
                    assert caught is None, (name, caught)
                    assert result["restored"] and result["returncode"] == "timeout"
                else:
                    assert isinstance(caught, expected_error), (name, caught)
                signals = [runner.signal.SIGTERM]
                if name in ("never-exits", "kill-escalation"):
                    signals.append(runner.signal.SIGKILL)
                assert killpg.call_args_list == [call(12345, sig) for sig in signals], name
                assert process.wait.call_args_list == [call(timeout=1200)] + \
                    [call(timeout=20)] * (len(waits) - 1), name

def check():
    manifest = json.loads((ROOT / "manifest.json").read_text())
    for filename, expected in manifest["files"].items():
        assert sha((ROOT / filename).read_bytes()) == expected, filename
    ledger = ROOT.parents[1] / "mutation-gates.md"
    if ledger.exists():
        assert sha(ledger.read_bytes()) == manifest["ledger_sha256"], "ledger binding"
    check_replay_cleanup()
    mappings = {m["id"]: m for m in json.loads((ROOT / "mapping.json").read_text())}
    with tempfile.TemporaryDirectory(prefix="ivmlite-bad-mutation-id-") as directory:
        temporary = Path(directory)
        output = temporary / "must-not-be-created"
        rejected = subprocess.run(
            [sys.executable, "-B", str(ROOT / "reproduce.py"),
             str(temporary / "nonexistent-checkout"), str(ROOT / "mapping.json"),
             str(output), "__unknown_mutation__"], capture_output=True, text=True,
        )
        assert rejected.returncode == 2 and "Unknown mutation ID(s)" in rejected.stderr
        assert not output.exists(), "Unknown IDs must fail before touching the checkout or output"
    results = read_lines("results.jsonl")
    attempts = read_lines("attempts.jsonl")
    assert len(results) == len(mappings) == manifest["mutation_groups"]
    assert len({r["id"] for r in results}) == len(results)
    baselines = {r["id"]: r for r in attempts if r["status"] == "accepted_baseline"}
    for record in attempts:
        compressed = (ROOT / record["published_log"]).read_bytes()
        assert sha(compressed) == record["published_log_gzip_sha256"], record["attempt_id"]
        data = gzip.decompress(compressed)
        assert sha(data) == record["published_log_sha256"], record["attempt_id"]
        assert b"/Users/" not in data, record["attempt_id"]
        text = data.decode()
        suites = re.findall(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", text)
        assert sum(int(v[0]) for v in suites) == record["passed"]
        assert sum(int(v[1]) for v in suites) == record["failed"]
        assert len(suites) == record["suite_count"]
        assert sum(int(v[2]) for v in suites) == record["ignored"]
        assert len(record["binaries"]) == len(suites)
        assert [(int(v[0]), int(v[1]), int(v[2])) for v in suites] == [(b["passed"], b["failed"], b["ignored"]) for b in record["binaries"]]
        assert sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", text, re.M))) == record["red"]
        if record["status"] == "rejected_compile":
            assert record["id"] == "row390" and not record["compiled"]
            assert "could not compile" in text
            assert record["rejected_mapping"]["edits"]
        else:
            assert record["compiled"] and record["restored"]
            assert "could not compile" not in text
    for record in results:
        mapping = mappings[record["id"]]
        assert record["status"] == "accepted_mutation"
        assert record["failed"] > 0 and record["returncode"] == 1
        assert record["head"] == mapping["source_commit"]
        assert record["original_sha256"] == mapping["original_sha256"]
        assert sha(json.dumps(mapping["edits"], sort_keys=True).encode()) == record["edits_sha256"]
        assert mapping["edits_sha256"] == record["edits_sha256"]
        assert set(mapping["primary_tests"]).issubset(record["red"]), record["id"]
        baseline = baselines[mapping["baseline_id"]]
        assert record["passed"] + record["failed"] == baseline["passed"]
        assert record["suite_count"] == baseline["suite_count"]
        assert [b["binary"] for b in record["binaries"]] == [b["binary"] for b in baseline["binaries"]]
        assert record["source_diff_sha256"] == baseline["source_diff_sha256"]
        assert record["untracked_sha256"] == baseline["untracked_sha256"]
        assert not any(b["filtered_out"] for b in record["binaries"])
    print(f"Validated {len(results)} accepted mutations, {len(baselines)} baseline records and "
          f"{sum(r['status'] == 'rejected_compile' for r in attempts)} rejected attempt; all published log hashes match.")

if __name__ == "__main__":
    check()
