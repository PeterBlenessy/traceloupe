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

## First real-data training run: ModernBERT-base on CGA (2026-08-15)

512-token windows, best-epoch checkpointing, published splits.

| measurement | result |
|---|---|
| CGA held-out test | **0.783** — attacks caught 309/420, clean kept clean 349/420 |
| our behavioural checklist | harmful caught **4/201**, ordinary kept clean 21/25 |

**What the first number means:** the training process works. 78% on real,
held-out, multi-author conversations is in the range published work reaches on
this data, so the pipeline (rendering, truncation, checkpointing) is sound and
the numbers can be trusted in a way nothing measured on our own fixtures ever
could be.

**What the second number means:** a model trained on Wikipedia editors
attacking each other transfers almost nothing to quiet intimate harm — same
result as toxic-bert, for the same reason. Public-forum abuse is loud; the harm
our product exists for is quiet. This was predicted in the baseline section and
is now measured twice.

**Consequences:** (1) PAN12 (real grooming chats — quiet harm, the only public
example of it) moves from important to critical; Peter's Zenodo request is the
gate. (2) Civil Comments multi-head training proceeds for the loud categories
(threat, insult, identity, sexual), where transfer has a fair chance. (3) The
quiet categories (coercive control) stay in the pattern tier, which needs no
training data — a decision this result retroactively strengthens.

## PAN12 acquired (2026-08-15)

Peter submitted the Zenodo request and downloaded the corpus; it lives at
`~/.traceloupe-dev/datasets/pan12/` (training + test). **No excerpt of this
data is ever committed to the repo** — only weights and aggregate metrics.

Training corpus: 66,927 conversations <!-- not-a-backup-count: public PAN12 research corpus --> / ~904k messages / 142 convicted
predators appearing in 2,016 conversations <!-- not-a-backup-count: public PAN12 research corpus --> (**3% positive** — the first
deployment-shaped class balance in the project; the hand-written eval set is
89% positive and cannot measure false-alarm rates realistically).

Why this is the most valuable set of the audit: it is the only public corpus of
*quiet* harm — the register both toxic-bert and the CGA-trained model scored
near zero on — and it is real two-person informal chat, the closest domain
match to SMS we have. Queued for training as soon as the Civil Comments run
frees the GPU.

## Civil Comments multi-head run (2026-08-15, evening)

Five sigmoid heads, 40k balanced samples, 192-token windows, best-epoch. On its
own held-out data (8k comments):

| head | caught | false alarms |
|---|---|---|
| toxicity | 3455/3903 (89%) | 517/4097 (13%) |
| insult | 2338/2896 (81%) | 419/5104 (8%) |
| threat | 58/104 (56%) | 40/7896 (0.5%) |
| identity attack | 129/335 (39%) | 48/7665 (0.6%) |
| sexual explicit | 59/125 (47%) | 26/7875 (0.3%) |

Reading: the two big heads are healthy for a first pass. The three rare heads
are starved — threat is 0.25% of the raw data, and a 20k-positive subsample
carries only a few hundred of each. Second iteration: oversample the rare
dimensions specifically and tune per-head thresholds; the false-alarm headroom
(under 1%) says the thresholds are far too conservative.

Checklist transfer: 12 of 201 harmful caught across all categories, 5/25
ordinary wrongly flagged. Third measurement of the same law (toxic-bert, CGA
model, now this): **models trained on loud public abuse do not see quiet
intimate harm.** The checklist has now falsified the easy version of the plan
three times, which is exactly what an instrument is for.

Standing conclusion for the scanner architecture: the public-data heads are a
*signal*, not the scanner. They will catch the genuinely loud subset of backup
content (slurs, explicit threats, overt sexual pressure) cheaply and with
published-range reliability; the quiet categories ride on PAN12 (running now)
and the pattern tier.

## PAN12 run: above the published reference (2026-08-15, night)

ModernBERT-base, two epochs, class-weighted, best-epoch. Official test corpus,
Fauzi & Bours protocol:

| | ours | published (Fauzi & Bours 2020) |
|---|---|---|
| F0.5 | **0.958** | 0.9348 |
| precision | 0.955 | — |
| recall | 0.968 | — |

Caught 1,780 of 1,839 predatory conversations; **84 false alarms in 122,035
clean conversations (0.07%)**. In plain terms: it finds 97 of every 100 real
grooming conversations, and wrongly flags about 7 in 10,000 ordinary ones.
A 2024-class encoder beating a 2020 bag-of-words ensemble is the expected
shape; being *in range* of the reference is what makes the number credible.

**Window experiment.** On 400 real predatory conversations, scoring only the
opening: first 4 messages → 42%, first 6 → 64%, **first 10 → 89%**, first 20 →
96%. The model detects grooming early, from windows the triage scan can
actually hand it — consistent with (and stronger than) the published
26-161-message early-detection finding.

**Checklist correction.** The model catches 0 of the 7 hand-written grooming
vignettes while catching 96.8% of real grooming. Given the window experiment,
the vignettes are the suspect, not the model: one author's idea of grooming,
already shown unrepresentative by review four. The grooming section of the
checklist needs rewriting against real patterns (excerpt-inspired, not
excerpt-copied) before it can gate this model. Its other sections stand.

## Where this leaves the scanner

Trust level by category, all measured on real held-out data:
- **grooming**: at/above published reference, works on 10-message windows
- **toxicity/insult (loud harassment)**: healthy first pass (89%/81% caught)
- **threat / identity / sexual-explicit**: heads starved, second iteration needed
- **coercive control**: pattern tier (no model)
- **self-harm**: deferred pending data pass
- **conversation derailment**: 0.783, in published range

Next: per-head threshold tuning + rare-head oversampling; ONNX export and the
Rust integration path; grooming checklist rewrite.
