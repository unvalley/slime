#!/usr/bin/env python3
"""Compare complete Swift LIVE replay results without hiding wrong-to-wrong changes."""

import argparse
import json
from pathlib import Path


def load_rows(path):
    rows = json.loads(Path(path).read_text())
    if not isinstance(rows, list):
        raise ValueError(f"{path}: expected a JSON array")
    indexed = {}
    for row in rows:
        key = str(row["index"])
        if key in indexed:
            raise ValueError(f"{path}: duplicate index {key}")
        if not isinstance(row["live"], str) or not isinstance(row["explicit"], str):
            raise ValueError(f"{path}: invalid output at {key}")
        expected = row["expected"]
        if not isinstance(expected, list) or not expected or not all(
            isinstance(value, str) for value in expected
        ):
            raise ValueError(f"{path}: invalid expected outputs at {key}")
        indexed[key] = row
    if not indexed:
        raise ValueError(f"{path}: empty evaluation")
    return indexed


def compare(before, after):
    if before.keys() != after.keys():
        raise ValueError("baseline and candidate have different item IDs")
    counts = {"gains": 0, "losses": 0, "unresolved_changes": 0}
    changes = []
    explicit_changes = []
    for key, old in before.items():
        new = after[key]
        if set(old["expected"]) != set(new["expected"]):
            raise ValueError(f"expected outputs changed at {key}")
        old_correct = old["live"] in old["expected"]
        new_correct = new["live"] in old["expected"]
        if old["live"] != new["live"]:
            outcome = (
                "gains" if new_correct and not old_correct else
                "losses" if old_correct and not new_correct else
                "unresolved_changes" if not new_correct else "accepted_variant"
            )
            if outcome in counts:
                counts[outcome] += 1
            changes.append({"index": key, "before": old["live"],
                            "after": new["live"], "expected": old["expected"],
                            "outcome": outcome})
        if old["explicit"] != new["explicit"]:
            explicit_changes.append({"index": key, "before": old["explicit"],
                                     "after": new["explicit"]})
    return {"items": len(before), **counts,
            "before_correct": sum(r["live"] in r["expected"] for r in before.values()),
            "after_correct": sum(r["live"] in r["expected"] for r in after.values()),
            "changes": changes, "explicit_changes": explicit_changes}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline")
    parser.add_argument("candidate")
    args = parser.parse_args()
    try:
        result = compare(load_rows(args.baseline), load_rows(args.candidate))
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    print(json.dumps(result, ensure_ascii=False, indent=2))
    # A green comparison is not proof of overall IME quality. Changed incorrect
    # outputs require inspection even when the exact-match count is unchanged.
    if result["losses"]:
        return 1
    if result["unresolved_changes"] or result["explicit_changes"]:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
