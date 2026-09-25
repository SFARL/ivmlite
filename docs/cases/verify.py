"""Verify the archived corpus offline, without executing any upstream code."""

import hashlib
import html
import json
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parent


def read_json(path):
    return json.loads(path.read_bytes())


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_bytes(entry):
    data = (ROOT / entry["path"]).read_bytes()
    require(len(data) == entry["bytes"], f"Size changed: {entry['path']}")
    require(
        hashlib.sha256(data).hexdigest() == entry["sha256"],
        f"Hash changed: {entry['path']}",
    )
    return data


def extract(entry):
    text = (ROOT / entry["source"]).read_bytes().decode("utf-8")
    for operation, argument in entry["steps"]:
        if operation == "json_path":
            text = json.loads(text)
            for key in argument:
                text = text[key]
        elif operation == "fence":
            text = re.findall(r"```[^\n]*\n(.*?)```", text, re.S)[argument - 1]
        elif operation == "html_pre":
            text = html.unescape(
                re.findall(r"<pre\b[^>]*>(.*?)</pre>", text, re.S)[argument - 1]
            )
        elif operation == "inline_sql":
            text = re.findall(r"`(select[^`]+)`", text, re.I)[argument - 1]
        else:
            raise ValueError(f"Unknown extraction operation: {operation}")
    return text.encode("utf-8")


def main():
    sources = read_json(ROOT / "manifest.json")["sources"]
    snippets = read_json(ROOT / "extractions.json")["files"]
    paths = [entry["path"] for entry in sources + snippets]
    require(len(paths) == len(set(paths)), "Duplicate manifest paths")

    captured = 0
    for entry in sources:
        if entry["status"] == "unavailable":
            require(not (ROOT / entry["path"]).exists(), "Stale failed download")
            continue
        require(entry["status"] == "captured", "Unknown download status")
        verify_bytes(entry)
        require(
            'rel="next"' not in (entry.get("next_page") or ""),
            f"Uncaptured HTTP page: {entry['path']}",
        )
        captured += 1

    for entry in snippets:
        require(
            verify_bytes(entry) == extract(entry),
            f"Extraction differs from source: {entry['path']}",
        )

    issues = 0
    for path in ROOT.glob("*/issue-*.json"):
        if path.stem.endswith("-comments"):
            continue
        issue = read_json(path)
        comments = read_json(path.with_name(path.stem + "-comments.json"))
        require(len(comments) == issue["comments"], f"Missing comments: {path}")
        require(len({c["id"] for c in comments}) == len(comments), "Repeated comment")
        issues += 1

    topic = read_json(ROOT / "org-roam/forum-topic.json")["post_stream"]
    rest = read_json(ROOT / "org-roam/forum-remaining-posts.json")["post_stream"]
    posts = topic["posts"] + rest["posts"]
    require(
        len(posts) == len(topic["stream"])
        and {p["id"] for p in posts} == set(topic["stream"]),
        "Incomplete Discourse thread",
    )
    pr = read_json(ROOT / "org-roam/vulpea-pr-116.json")
    files = read_json(ROOT / "org-roam/vulpea-pr-116-files.json")
    require(len(files) == pr["changed_files"], "Incomplete PR file list")
    for path in ["org-roam/vulpea-tree.json", "taproot-assets/tree.json"]:
        require(not read_json(ROOT / path)["truncated"], f"Truncated tree: {path}")

    print(
        f"Verified {captured} source snapshots, {len(snippets)} exact extractions, "
        f"{issues} complete issue comment lists, {len(posts)} forum posts and "
        f"{len(files)} PR file records. "
        f"Recorded {len(sources) - captured} unavailable sources."
    )


if __name__ == "__main__":
    main()
