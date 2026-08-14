#!/usr/bin/env python3
"""Assemble a per-category-balanced training set from several corpora.

    tools/corpus/balance.py --out train.jsonl hand.jsonl machine.jsonl

WHY THIS EXISTS, since `cat *.jsonl > train.jsonl` is the obvious alternative
and is what produced the measurement that justifies this file:

    trained on              harassment    coercive-control   threats
    108 hand-written        73/78 (94%)   9/89               0/20
    1500 machine-written    56/78 (72%)   69/89 (78%)        6/20
    both, concatenated      71/78 (91%)   39/89 (44%)        3/20

Concatenation HALVED coercive-control. The hand-written set is 60 harassment
conversations, 28 coercive-control and no threats, so adding it to an
already-spread corpus tips the mixture and the model reallocates. The failure
looks like "the hand-written data is bad" and is really "the proportions are
wrong".

Sources are given in PREFERENCE ORDER — earlier files are drawn from first.
Hand-written material is far more efficient per example (60 conversations
reaching 94% where 1,500 mixed ones reach 72%), so it should fill each
category's quota before machine-written material tops it up.
"""
from __future__ import annotations

import argparse
import json
import random
from collections import defaultdict
from pathlib import Path


def load(path: Path) -> list[dict]:
    return [json.loads(l) for l in path.read_text().splitlines() if l.strip()]


def key(row: dict) -> str:
    cats = row.get("categories") or []
    return cats[0] if cats else "negative"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("sources", nargs="+", help="JSONL corpora, in preference order")
    ap.add_argument("--out", required=True)
    ap.add_argument("--per-category", type=int, default=0,
                    help="cap per category; 0 = the largest category that every source can fill")
    ap.add_argument("--negative-share", type=float, default=0.33,
                    help="negatives as a fraction of the total")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()
    random.seed(args.seed)

    # Bucket by category, keeping source order so preference is preserved.
    buckets: dict[str, list[dict]] = defaultdict(list)
    for src in args.sources:
        for row in load(Path(src)):
            row["_source"] = Path(src).stem
            buckets[key(row)].append(row)

    positives = {k: v for k, v in buckets.items() if k != "negative"}
    negatives = buckets.get("negative", [])
    if not positives:
        print("no positive categories found", file=__import__("sys").stderr)
        return 2

    cap = args.per_category or min(len(v) for v in positives.values())
    out: list[dict] = []
    print(f"{'category':<24}{'available':>10}{'taken':>7}   sources")
    for cat, rows in sorted(positives.items()):
        take = rows[:cap]
        out.extend(take)
        srcs = ", ".join(sorted({r["_source"] for r in take}))
        print(f"  {cat:<22}{len(rows):>10}{len(take):>7}   {srcs}")

    # Negatives sized against the whole, not per category — "is this ordinary"
    # is one question, not nine.
    want_neg = int(len(out) * args.negative_share / (1 - args.negative_share))
    neg = negatives[:want_neg]
    out.extend(neg)
    print(f"  {'negative':<22}{len(negatives):>10}{len(neg):>7}")

    random.shuffle(out)
    with open(args.out, "w") as fh:
        for row in out:
            row.pop("_source", None)
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")
    print(f"\nwrote {len(out)} conversations to {args.out} "
          f"({cap} per category + {len(neg)} negatives)")
    if cap < 50:
        print(f"NOTE: capped at {cap} by the smallest category — every category is "
              f"limited by the thinnest one, so write more of that before more of anything else.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
