# Public data audit for the Safety Scan classifier

Tracks #518. The decision this serves, agreed with Peter 2026-08-15: **focus on
public, multi-author data; get the scanner to a level we trust; only then decide
how to improve it without removing what works.** Model choice criterion, in his
words: he doesn't care which, "as long as it has been considered and the chosen
one performed well."

Licences below are verified against the hosting page on the date given, per the
standing decision to verify and act rather than ask.

## Verdicts so far (2026-08-15)

| Dataset | Covers | Size | Licence | Verdict |
|---|---|---|---|---|
| Civil Comments (Jigsaw) | toxicity, threat, insult, identity attack, sexual-explicit | ~2M comments, 7 scored dimensions | **CC0** (public domain) | **USE** — primary training set; label dimensions map to 4 of our categories |
| ConvoKit CGA (Wikipedia) | conversations that derail into personal attacks | 4,188 conversations / 30k comments | Wikipedia text = **CC BY-SA**; toolkit MIT | **USE** — the only established *conversation-level* set; attribution + share-alike terms noted |
| ConvoKit CGA-CMV (Reddit) | same, larger | 19,578 conversations | Reddit-sourced; check redistribution terms before shipping anything derived | USE for research/eval; revisit before any shipped artefact embeds it |
| HateXplain | hate speech vs offensive vs normal, with target group | 20,148 posts (Twitter+Gab) | **CC BY 4.0** (repo MIT) | **USE** — hate-identity category, attribution required |
| PAN12 predator chats | grooming | ~convictions-derived chat logs; the academic standard | Zenodo **restricted**: short use-statement form | **REQUEST** — draft the statement; may need Peter's Zenodo account to submit |
| CLPsych / Reddit self-harm sets | self-harm risk | varies | typically research agreements | **DEFER** — needs its own pass; do not touch anything requiring an IRB-style agreement without Peter |
| SMS corpora (NUS SMS, UCI SMS Spam, smishing sets) | domain adaptation + scam | varies | varies (UCI public, others CC BY) | **NEXT** — audit for the domain-shift question and the scam rule tier |

## Category mapping under the pivot

| Our category | Source | Mechanism |
|---|---|---|
| harassment-bullying | Civil Comments (insult/toxicity) + CGA | learned classifier |
| threat-violence | Civil Comments (threat) | learned classifier |
| hate-identity | Civil Comments (identity attack) + HateXplain | learned classifier |
| sexual-content | Civil Comments (sexual-explicit) | learned classifier |
| grooming-exploitation | PAN12 (pending access) | learned classifier |
| self-harm | deferred pending data pass | keep today's generative stage |
| coercive-control | — | **pattern tier**: census statistics (volume, one-sidedness, resumption after silence); relationship requirement dropped per Peter |
| scam-fraud | smishing sets | **rule tier** first; learned later if data supports |
| drugs-illegal | no established set | out of the learned scanner for now |

## Model selection protocol

Candidates: ModernBERT-base (incumbent choice), DeBERTa-v3 (stronger on short
text, 512-token cap), one 2025-era encoder if it publishes results on our
actual datasets (NeoBERT / Ettin), and **off-the-shelf toxicity classifiers**
(e.g. toxic-bert) as the zero-training baseline. All compared on the same
held-out real data; published results on Civil Comments/CGA are the external
reference that tells us whether our training is broken. The 522 hand-written
records serve as the behavioural checklist, never as training data.

## Blockers needing Peter

- PAN12 Zenodo request form (drafted use-statement will be provided).
- Nothing else so far.

## Zero-training baseline (2026-08-15)

`unitary/toxic-bert` — a free classifier already trained on the Jigsaw data —
scored against the 226-case behavioural checklist, conversation score = max
over its messages:

| threshold | harmful caught | ordinary wrongly flagged |
|---|---|---|
| 0.5 | 11 / 201 | 3 / 25 |
| 0.1 | 48 / 201 | 6 / 25 |
| 0.02 | 98 / 201 | 11 / 25 |

The harmful cases it misses have a **median top score of 0.015** — this is not
a threshold problem; the model sees nothing in them at all.

**What this actually measures.** Toxic-bert detects *overt* toxicity — slurs,
profanity, direct insults — because that is what news-site comment moderation
labels. Our checklist is dominated by *quiet* harm: "lovely woman. lives on her
own now doesnt she", "the app says youre still in town". No amount of
thresholding recovers a signal that is absent.

**Consequences for the plan:**
1. The zero-training shortcut is closed. Off-the-shelf models are a floor
   (11/201), not an option.
2. **Civil Comments alone will not be enough either** — a model trained on it
   inherits the same overt-toxicity blind spot. The conversation-level CGA data
   (which labels *derailment*, not swearing) and PAN12 (which labels grooming,
   the quietest category of all) carry the burden for veiled harm. This raises
   PAN12 access from nice-to-have to important.
3. The checklist has proven itself as an instrument: it separates "detects
   rudeness" from "detects harm", which is exactly the distinction the product
   lives on. It stays as the acceptance gate for every candidate model.

## PAN12 access request (draft for Peter to submit on Zenodo)

Record: https://zenodo.org/records/3713280 — files are restricted; the form
asks for a short statement of intended use. Draft:

> I am the developer of TraceLoupe, an open-source iPhone-backup forensics
> tool (github.com/PeterBlenessy/traceloupe). I am building an on-device
> safety feature that flags potentially harmful conversations — including
> grooming — in a user's own message history, for personal-safety and
> parental-safeguarding review. The corpus would be used to train and evaluate
> a small local classifier. The data never leaves the training machine, is not
> redistributed, and no excerpt of it is committed to the repository; only
> model weights and aggregate metrics are derived from it.

Submitting needs a Zenodo login, so this is Peter's single required action.
