#!/usr/bin/env python3
"""Reject empty selections, ignored tests, early permission returns, or test errors."""
import argparse
import json
import pathlib
import re
import sys

def verify(text, name, exit_code):
    if exit_code != 0:
        raise ValueError(f"test process exited {exit_code}")
    clean = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    if not re.search(r"(?m)^test " + re.escape(name) + r" \.\.\.", clean):
        raise ValueError("the exact named test did not run")
    summaries = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;", clean)
    if summaries != [("ok", "1", "0", "0")]:
        raise ValueError("expected exactly one passed test and no failures or ignored tests")
    if re.search(r"\bskip(?:ped|ping)?\b|跳过|没有辅助功能权限|没有屏幕.*权限", clean, re.I):
        raise ValueError("the test reported a skip or missing permission")
    return {"test": name, "passed": 1, "failed": 0, "ignored": 0}

def main():
    p = argparse.ArgumentParser()
    p.add_argument("log", type=pathlib.Path)
    p.add_argument("name")
    p.add_argument("exit_code", type=int)
    a = p.parse_args()
    try:
        print(json.dumps(verify(a.log.read_text(errors="replace"), a.name, a.exit_code)))
    except (OSError, ValueError) as error:
        print(f"verification rejected: {error}", file=sys.stderr)
        return 1
    return 0

if __name__ == "__main__":
    sys.exit(main())
