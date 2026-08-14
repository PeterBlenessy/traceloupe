#!/usr/bin/env python3
"""Train a small encoder classifier on generated conversations, and score it
against the sealed eval set.

    tools/corpus/train_classifier.py --data corpus.jsonl --epochs 4

This is the challenger in the bake-off described in
`docs/plans/safety-classifier-plan.md`. The incumbent is the 4B generative
classifier, whose score on the same sealed cases is measured by
`focused_stage_on_pattern_categories`:

    coercive-control       13 / 14
    harassment-bullying     7 / 13
    threat-violence         4 / 5     (control)

This script prints the same table so the two are directly comparable. It does
NOT print precision/recall — the question is "how many of the conversations a
reviewer would want flagged does it catch", and that is the number.

WHY ModernBERT-base: 8,192-token context, so a whole conversation fits with
room to spare; 149M parameters against the incumbent's 7.5B, which is the
entire point — the incumbent takes ~6.5 s per conversation and this should take
milliseconds. If it wins but underperforms, ModernBERT-large is the next rung.

THE SEALED SET IS NEVER TRAINED ON. It is loaded here only to score against,
and the generator already refuses to emit anything that overlaps it (#509).
"""
from __future__ import annotations

import argparse
import json
import random
from pathlib import Path

import numpy as np
import torch
from torch.utils.data import DataLoader, Dataset
from transformers import AutoModelForSequenceClassification, AutoTokenizer

ROOT = Path(__file__).resolve().parents[2]
CATEGORIES = [
    "threat-violence",
    "harassment-bullying",
    "sexual-content",
    "grooming-exploitation",
    "self-harm",
    "hate-identity",
    "coercive-control",
    "scam-fraud",
    "drugs-illegal",
]
IDX = {c: i for i, c in enumerate(CATEGORIES)}


def render(messages: list[dict]) -> str:
    """One conversation as one string. Speaker markers matter: the harm in these
    categories is relational, so who said what is part of the signal."""
    return "\n".join(f"{m['sender']}: {m['text']}" for m in messages)


class Conversations(Dataset):
    def __init__(self, rows, tok, max_len=512):
        self.rows, self.tok, self.max_len = rows, tok, max_len

    def __len__(self):
        return len(self.rows)

    def __getitem__(self, i):
        text, labels = self.rows[i]
        enc = self.tok(
            text, truncation=True, max_length=self.max_len, padding="max_length", return_tensors="pt"
        )
        return {
            "input_ids": enc["input_ids"][0],
            "attention_mask": enc["attention_mask"][0],
            "labels": torch.tensor(labels, dtype=torch.float),
        }


def load_generated(path: Path) -> list[tuple[str, list[float]]]:
    """A file, or a directory of .jsonl files.

    Directories are the normal case: the corpus is split by category, and
    naming one file at a time is how a file silently stops being trained on —
    the same failure the Rust guards exist to catch on the fixture side.
    """
    files = sorted(path.glob("*.jsonl")) if path.is_dir() else [path]
    if not files:
        raise SystemExit(f"no .jsonl files under {path}")
    rows = []
    for f in files:
        n = 0
        for line in f.read_text().splitlines():
            if not line.strip():
                continue
            d = json.loads(line)
            y = [0.0] * len(CATEGORIES)
            for c in d.get("categories") or []:
                if c in IDX:
                    y[IDX[c]] = 1.0
            rows.append((render(d["messages"]), y))
            n += 1
        if path.is_dir():
            print(f"  {f.name}: {n}")
    return rows


def load_sealed() -> list[tuple[str, list[float], str]]:
    """The eval set. Positives only — the question is what it catches."""
    import glob
    cases = json.loads(
        (ROOT / "crates/traceloupe-core/fixtures/safety-scan/cases.json").read_text()
    )["cases"]
    # Plus the 226 hand-written cases (#515). Scoring against the old 14
    # coercive-control conversations could not resolve anything.
    for f in sorted(glob.glob(str(ROOT / "crates/traceloupe-core/fixtures/safety-scan/eval/*.jsonl"))):
        for line in open(f):
            if line.strip():
                cases.append(json.loads(line))
    out = []
    for case in cases:
        if case["kind"] != "positive":
            continue
        y = [0.0] * len(CATEGORIES)
        for e in case.get("expect", []):
            if e["category"] in IDX:
                y[IDX[e["category"]]] = 1.0
        out.append((render(case["messages"]), y, case["id"]))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True, help="a .jsonl file, or a directory of them")
    ap.add_argument("--model", default="answerdotai/ModernBERT-base")
    ap.add_argument("--epochs", type=int, default=4)
    ap.add_argument("--batch", type=int, default=8)
    ap.add_argument("--lr", type=float, default=3e-5)
    ap.add_argument("--threshold", type=float, default=0.5)
    ap.add_argument("--out", default=None)
    ap.add_argument("--seed", type=int, default=0, help="seeds the classifier head init AND the split")
    args = ap.parse_args()
    torch.manual_seed(args.seed)

    device = "mps" if torch.backends.mps.is_available() else "cpu"
    print(f"device: {device}")

    rows = load_generated(Path(args.data))
    random.seed(args.seed)
    random.shuffle(rows)
    cut = max(1, int(len(rows) * 0.9))
    train_rows, val_rows = rows[:cut], rows[cut:]
    pos = sum(1 for _, y in rows if max(y) > 0)
    print(f"generated: {len(rows)} conversations ({pos} harmful, {len(rows)-pos} ordinary)")
    print(f"split: {len(train_rows)} train / {len(val_rows)} val")

    tok = AutoTokenizer.from_pretrained(args.model)
    model = AutoModelForSequenceClassification.from_pretrained(
        args.model, num_labels=len(CATEGORIES), problem_type="multi_label_classification"
    ).to(device)

    dl = DataLoader(Conversations(train_rows, tok), batch_size=args.batch, shuffle=True)
    val_dl = DataLoader(Conversations(val_rows, tok), batch_size=args.batch)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr)

    # Nine labels, and any one conversation carries at most one or two of them,
    # so ~8 of 9 targets are zero for every example. Un-weighted BCE therefore
    # has a comfortable optimum at "predict nothing", which is exactly what the
    # first run did — every sealed case scored zero. Weight the positive class
    # by how rare it actually is in this corpus.
    counts = np.array([sum(y[i] for _, y in train_rows) for i in range(len(CATEGORIES))])
    pos_w = torch.tensor(
        [(len(train_rows) - c) / max(c, 1) for c in counts], dtype=torch.float, device=device
    ).clamp(max=50.0)
    print("positives per category:", dict(zip(CATEGORIES, counts.astype(int))))
    lossf = torch.nn.BCEWithLogitsLoss(pos_weight=pos_w)

    # Keep the BEST epoch, not the last one.
    #
    # Measured 2026-08-14 on 1,322 conversations: validation loss bottoms at
    # epoch 2 (0.75) and then climbs — 1.24 at epoch 3, 1.40 at epoch 4 — while
    # training loss falls to 0.08. Every result reported before this was taken
    # from a model trained 4-5 epochs, i.e. well past the point where it stopped
    # generalising and started memorising. That probably also inflated the
    # run-to-run variance, since behaviour past the optimum is unstable.
    import copy
    best_val = float("inf")
    best_state = None
    best_epoch = 0
    for ep in range(args.epochs):
        model.train()
        total = 0.0
        for i, b in enumerate(dl):
            b = {k: v.to(device) for k, v in b.items()}
            out = model(input_ids=b["input_ids"], attention_mask=b["attention_mask"])
            loss = lossf(out.logits, b["labels"])
            loss.backward()
            opt.step()
            opt.zero_grad()
            total += loss.item()
            if i % 20 == 0:
                print(f"  epoch {ep+1} step {i}/{len(dl)} loss {loss.item():.4f}", flush=True)
        model.eval()
        vl = 0.0
        with torch.no_grad():
            for b in val_dl:
                b = {k: v.to(device) for k, v in b.items()}
                o = model(input_ids=b["input_ids"], attention_mask=b["attention_mask"])
                vl += lossf(o.logits, b["labels"]).item()
        vloss = vl / max(len(val_dl), 1)
        print(f"epoch {ep+1}: train {total/max(len(dl),1):.4f}  val {vloss:.4f}")
        if vloss < best_val:
            best_val, best_epoch = vloss, ep + 1
            best_state = copy.deepcopy(model.state_dict())

    if best_state is not None and best_epoch != args.epochs:
        model.load_state_dict(best_state)
        print(f"\nrestored epoch {best_epoch} (val {best_val:.4f}) — later epochs overfit")
    model.eval()

    # --- score against the sealed set, in the incumbent's terms ---------------
    sealed = load_sealed()
    model.eval()
    hits = {c: [0, 0] for c in CATEGORIES}
    probs_seen: list[float] = []
    missed: dict[str, list[str]] = {c: [] for c in CATEGORIES}
    with torch.no_grad():
        for text, y, cid in sealed:
            enc = tok(text, truncation=True, max_length=512, padding="max_length", return_tensors="pt")
            enc = {k: v.to(device) for k, v in enc.items()}
            p = torch.sigmoid(model(**enc).logits)[0].cpu().numpy()
            probs_seen.append(float(p.max()))
            for c, i in IDX.items():
                if y[i] > 0:
                    hits[c][1] += 1
                    if p[i] >= args.threshold:
                        hits[c][0] += 1
                    else:
                        missed[c].append(cid)

    print("\n=== against the sealed eval set (never trained on) ===")
    print("category                 caught / total   missed")
    for c in CATEGORIES:
        got, tot = hits[c]
        if tot:
            print(f"  {c:<22} {got:>3} / {tot:<3}      {', '.join(missed[c][:4]) or '-'}")
    if probs_seen:
        # If these are all near zero the model collapsed to "predict nothing"
        # and the table above says nothing about the architecture.
        print(f"\nhighest score per case: min {min(probs_seen):.3f} "
              f"median {sorted(probs_seen)[len(probs_seen)//2]:.3f} max {max(probs_seen):.3f}")
    # The sealed set has 14 coercive-control cases and 13 harassment ones, so a
    # single run's score there moves by several cases on random initialisation
    # alone — measured: 12, 6 and 8 of 14 across three runs that differed only
    # in training size. The held-out slice of the generated corpus is ~10x
    # larger and is the development signal; the sealed set is the arbiter, and
    # needs several seeds before any difference in it means anything.
    vhit = vtot = 0
    with torch.no_grad():
        for text, y in val_rows:
            if max(y) == 0:
                continue
            enc = tok(text, truncation=True, max_length=512, padding="max_length", return_tensors="pt")
            p2 = torch.sigmoid(model(**{k: v.to(device) for k, v in enc.items()}).logits)[0].cpu().numpy()
            for i in range(len(CATEGORIES)):
                if y[i] > 0:
                    vtot += 1
                    if p2[i] >= args.threshold:
                        vhit += 1
    print(f"\nheld-out generated data (the steadier signal): {vhit}/{vtot} "
          f"= {100*vhit/max(vtot,1):.0f}%")

    # WHERE the caught cases came from, not just how many.
    #
    # The training conversations and most of the eval conversations were
    # written by the same author in the same session from the same mode list.
    # The word-overlap check guarantees no shared TEXT, but it cannot see
    # shared STYLE — and a model that has learned "this is how this author
    # writes a stalking conversation" scores well without having learned what
    # stalking is. The eval set happens to contain cases written months
    # earlier, in a different session and register, which are not in any
    # training data: those are the honest generalisation test, so report them
    # separately rather than letting them average away.
    origins = {"older (different session)": [], "this session": []}
    with torch.no_grad():
        for text, y, cid in sealed:
            key = "this session" if cid.startswith(("cc-e-", "rh-e-", "tv-e-", "sh-e-",
                                                    "sx-e-", "gr-e-", "hi-e-", "sc-e-",
                                                    "dr-e-", "neg-e-")) else "older (different session)"
            enc = tok(text, truncation=True, max_length=512, padding="max_length", return_tensors="pt")
            p3 = torch.sigmoid(model(**{k: v.to(device) for k, v in enc.items()}).logits)[0].cpu().numpy()
            for i in range(len(CATEGORIES)):
                if y[i] > 0:
                    origins[key].append(1 if p3[i] >= args.threshold else 0)
    print("\ncaught, split by who wrote the test case:")
    for k, v in origins.items():
        if v:
            print(f"  {k:<28} {sum(v)}/{len(v)} = {100*sum(v)/len(v):.0f}%")

    print("\nincumbent (4B, focused stage, full context, no time limit):")
    print("  coercive-control        78 / 89")
    print("  harassment-bullying     17 / 78")
    print("  threat-violence         16 / 20   (control)")

    if args.out:
        model.save_pretrained(args.out)
        tok.save_pretrained(args.out)
        print(f"\nsaved to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
