#!/usr/bin/env python3
"""Generate labelled conversations to train the safety classifier.

    tools/corpus/generate.py --pilot 20 --out pilot.jsonl
    tools/corpus/generate.py --count 2500 --out corpus.jsonl

Needs a llama.cpp server already running (see --server). Two different
generator models should be used across runs and the outputs concatenated —
one model's habits become a signature the classifier can learn instead of
learning the behaviour.

WHY EACH RULE IS HERE, since they all cost something:

* Modes come from `taxonomy.json`, not from the prompt writer's imagination.
  #492 measured that covering missing modes is what moves detection, and that
  the scam corpus was missing two whole modes because the list was invented.
* Persona, intensity, register, locale and the other party's response are drawn
  at random per example. Repeating one prompt collapses a corpus into a single
  voice; varied seeding measurably raises diversity.
* Roughly a third of output is NEGATIVE, built from situations that share
  surface features with the harmful modes — a parent's curfew, a worried
  friend, quoted abuse. Every false alarm this project has measured lives
  there, and a positives-only corpus teaches a model to flag ordinary care.
* Nothing is generated FROM the sealed eval fixtures, and every generated line
  is checked against them before it is kept (#509). A model that has seen the
  test cannot be tested.
"""
from __future__ import annotations

import argparse
import json
import random
import re
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TAXONOMY = json.loads((Path(__file__).parent / "taxonomy.json").read_text())
FIXTURES = json.loads(
    (ROOT / "crates/traceloupe-core/fixtures/safety-scan/cases.json").read_text()
)

SYSTEM = (
    "You write realistic message exchanges as training data for a forensic tool "
    "that detects abuse and harm in phone backups, used by investigators and by "
    "people trying to understand what happened to them. Write only the dialogue, "
    "nothing else — no preamble, no explanation, no content warnings."
)


def sealed_phrases() -> list[list[str]]:
    """Word sequences from the sealed eval set, for the contamination check."""
    out = []
    for case in FIXTURES["cases"]:
        for m in case["messages"]:
            w = words(m["text"])
            if w:
                out.append(w)
    return out


def words(s: str) -> list[str]:
    return re.sub(r"[^a-z0-9]+", " ", s.lower()).split()


def contaminated(text: str, sealed: list[list[str]]) -> bool:
    """Same rule as `eval::overlaps_sealed_fixtures`: equality at any length, or
    four-plus consecutive shared words. Kept in step by hand; the Rust side is
    the one that gates CI."""
    w = words(text)
    if not w:
        return False
    for s in sealed:
        if w == s:
            return True
        short, long = (w, s) if len(w) <= len(s) else (s, w)
        if len(short) >= 4:
            n = len(short)
            if any(long[i : i + n] == short for i in range(len(long) - n + 1)):
                return True
    return False


def ask(server: str, prompt: str, max_tokens: int = 700) -> str:
    body = json.dumps(
        {
            # Gemma 4 is a reasoning model and will spend the whole budget
            # thinking, returning empty content — measured, not guessed.
            "chat_template_kwargs": {"enable_thinking": False},
            "messages": [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": prompt},
            ],
            "max_tokens": max_tokens,
            "temperature": 1.0,
            "top_p": 0.95,
        }
    ).encode()
    req = urllib.request.Request(
        f"{server}/v1/chat/completions", data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=300) as r:
        d = json.loads(r.read().decode(), strict=False)
    return d["choices"][0]["message"].get("content") or ""


def pick(key: str) -> str:
    return random.choice(TAXONOMY["variation"][key])


def build_prompt(kind: str, category: str | None, mode: str) -> str:
    n = random.choice([3, 4, 4, 5, 6, 8])
    common = (
        f"{pick('locale')}. {pick('register')}. "
        f"The two people are {pick('relationship')}. "
        f"Exactly {n} messages, alternating or not as feels natural. "
        "Format every line as 'A: text' or 'B: text' and write nothing else."
    )
    if kind == "positive":
        return (
            f"Write a text exchange in which one person {mode}. "
            f"Make it {pick('intensity')}, and {pick('response')}. {common}"
        )
    return (
        f"Write a text exchange showing {mode}. This is ORDINARY and not abusive — "
        f"it should look superficially similar to something concerning but be "
        f"entirely innocent in context. {common}"
    )


def parse(raw: str) -> list[dict] | None:
    msgs = []
    for line in raw.strip().splitlines():
        line = line.strip()
        m = re.match(r"^\**([AB])\**\s*[:\-]\s*(.+)$", line, re.IGNORECASE)
        if not m:
            continue
        text = m.group(2).strip().strip("*").strip()
        if text:
            msgs.append({"sender": "them" if m.group(1).upper() == "A" else "me", "text": text})
    return msgs if 2 <= len(msgs) <= 12 else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8096")
    ap.add_argument("--count", type=int, default=0)
    ap.add_argument("--pilot", type=int, default=0)
    ap.add_argument("--out", required=True)
    ap.add_argument("--model-tag", default="e4b", help="recorded on each row, so a corpus mixed from two generators stays attributable")
    ap.add_argument("--seed", type=int, default=None)
    args = ap.parse_args()
    total = args.pilot or args.count
    if not total:
        print("pass --count or --pilot", file=sys.stderr)
        return 2
    if args.seed is not None:
        random.seed(args.seed)

    sealed = sealed_phrases()
    cats = list(TAXONOMY["categories"].items())
    weights = [c["weight"] for _, c in cats]
    negatives = TAXONOMY["negatives"]["situations"]

    kept = dropped_parse = dropped_contam = 0
    mode_counts: dict[tuple[str, str], int] = {}
    t0 = time.time()
    with open(args.out, "w") as fh:
        while kept < total:
            # ~1 in 3 negative: enough that "looks concerning but is not" is a
            # first-class part of the task rather than an afterthought.
            if random.random() < 0.33:
                kind, category, mode = "negative", None, random.choice(negatives)
            else:
                kind = "positive"
                category = random.choices([c for c, _ in cats], weights=weights)[0]
                # Least-covered mode first, not a uniform draw. Measured on the
                # first 972-conversation run: random sampling left some modes
                # with one or two examples while others had a dozen, and a mode
                # the corpus barely covers is one the classifier will not learn
                # — which is the whole failure #492 diagnosed.
                ms = TAXONOMY["categories"][category]["modes"]
                fewest = min(mode_counts.get((category, m), 0) for m in ms)
                mode = random.choice([m for m in ms if mode_counts.get((category, m), 0) == fewest])
                mode_counts[(category, mode)] = fewest + 1

            try:
                raw = ask(args.server, build_prompt(kind, category, mode))
            except Exception as e:
                print(f"  request failed: {str(e)[:80]}", file=sys.stderr)
                continue

            msgs = parse(raw)
            if not msgs:
                dropped_parse += 1
                continue
            if any(contaminated(m["text"], sealed) for m in msgs):
                dropped_contam += 1
                continue

            fh.write(
                json.dumps(
                    {
                        "kind": kind,
                        "categories": [category] if category else [],
                        "mode": mode,
                        "messages": msgs,
                        "generator": args.model_tag,
                    }
                )
                + "\n"
            )
            fh.flush()
            kept += 1
            if kept % 10 == 0:
                rate = kept / max(time.time() - t0, 1e-9)
                left = (total - kept) / max(rate, 1e-9) / 60
                print(f"  {kept}/{total}  {rate*60:.1f}/min  ~{left:.0f} min left", flush=True)

    print(
        f"\nwrote {kept} to {args.out} "
        f"(dropped: {dropped_parse} unparseable, {dropped_contam} touching the sealed set)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
