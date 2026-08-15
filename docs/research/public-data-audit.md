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
