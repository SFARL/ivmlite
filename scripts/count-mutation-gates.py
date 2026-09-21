#!/usr/bin/env python3
"""数出 docs/mutation-gates.md 的行数与验证状态，并核对文末的统计段。

门禁表的全部价值在于"声称"可以被机械核对。统计数字如果是手写的，
它自己就是一处未被核对的声称——第一版正是这样写错的（12/26）。

用法：scripts/count-mutation-gates.py        # 核对，不一致则非零退出
      scripts/count-mutation-gates.py --fix  # 按实际数字改写统计段
"""
import re
import sys
from pathlib import Path

DOC = Path(__file__).resolve().parent.parent / "docs" / "mutation-gates.md"
SENTENCE = "表内共 **{total}** 行：已验证 **{verified}** 条、未验证 **{unverified}** 条、不适用 **{na}** 条"


def count(text):
    total = verified = unverified = na = 0
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != 4 or cells[0] in ("spec 要求",) or set(cells[0]) <= set("-: "):
            continue
        total += 1
        status = cells[3]
        if "已验证" in status:
            verified += 1
        elif "未验证" in status:
            unverified += 1
        else:
            na += 1
    return dict(total=total, verified=verified, unverified=unverified, na=na)


def main():
    text = DOC.read_text()
    actual = count(text)
    want = SENTENCE.format(**actual)
    found = re.search(r"表内共 \*\*\d+\*\* 行：已验证 \*\*\d+\*\* 条、未验证 \*\*\d+\*\* 条、"r"不适用 \*\*\d+\*\* 条", text)
    if not found:
        print("统计段落不见了——它是本表可被核对的唯一入口", file=sys.stderr)
        return 1
    if found.group(0) == want:
        print(f"一致：{want}")
        return 0
    if "--fix" in sys.argv:
        DOC.write_text(text[: found.start()] + want + text[found.end() :])
        print(f"已改写为：{want}")
        return 0
    print(f"不一致\n  文中：{found.group(0)}\n  实际：{want}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
