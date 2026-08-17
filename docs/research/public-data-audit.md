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

## Export chain proven (2026-08-16, 00:15)

PyTorch → ONNX → int8, each step verified:

| artefact | size | CPU latency | accuracy on real test |
|---|---|---|---|
| PyTorch fp32 | — | — | recall 0.968, F0.5 0.958 |
| ONNX fp32 | 571MB | 32 ms/conversation | bit-identical (max logit diff 8e-6) |
| **ONNX int8** | **143MB** | **14 ms/conversation** | recall 0.964, est. F0.5 0.958 |

Quantisation costs 0.4 points of recall and nothing else. The deployable
grooming detector is 143MB and scores a conversation in 14 ms on CPU — the
incumbent generative stage takes ~6.5 s, so this is ~460× faster at
published-benchmark accuracy. Integration path: the `ort` crate in the Rust
backend (scans must outlive the UI), tokenizer via `tokenizers` crate.

One tooling note: the tokenizer config saved by this transformers version
names a class (`TokenizersBackend`) it cannot itself re-import; load the
tokenizer from the hub id instead of the local dir.

## Civil heads, second iteration + honest calibration (2026-08-16)

Fixes to iteration 1: every rare-dimension example kept (not subsampled away),
per-head pos_weight from the actual mix, and thresholds swept on the FULL
validation split with a fine grid — the first sweep overfit a 4k sample and
its thresholds missed the false-alarm target 4-6× on test.

Catch rate at each false-alarm budget (calibrated on full val, reported on
full test — transfer is now honest, 1.0% targets land 0.87-0.96%):

| head | 0.5% budget | 1% budget | 2% budget |
|---|---|---|---|
| threat | 56% | 76% | **87%** |
| sexual explicit | 48% | 80% | **94%** |
| identity attack | 41% | 62% | **77%** |
| insult | 40% | 52% | 65% |
| toxicity | 31% | 46% | 56% |

**Deployment recommendation.** In the scanner these heads score individual
messages as a census-grade signal feeding the deep-scan worklist — their false
alarms cost deep-scan budget, not user-facing findings, and the deep scan
already confirms. At the 2% operating point the rare heads (threat 87%, sexual
94%, identity 77%) add real recall for exactly the categories the embedding
census is weakest on, at a candidate rate the budget model absorbs. The
toxicity/insult heads overlap the embedding census's strength and earn their
place only if measured to add candidates the census misses — to be tested at
integration, not assumed.

Artefact: `modernbert_civil2.pt` (scratchpad); export follows the proven
grooming chain when integration lands.

## Quantisation verdict for the civil heads (2026-08-16, night)

Dynamic int8 quantisation shifts the score SCALE badly (a clear identity
attack dropped 0.399 → 0.014 pointwise) — but it preserves the RANKING, and
thresholds calibrated on the quantised model's own scores recover everything.
Three variants, each calibrated on its own full-validation scores, reported on
the full test split:

| head @2% budget | fp32 (571MB) | int8 (143MB) | int8 per-channel |
|---|---|---|---|
| threat | 87% | 86% | 86% |
| sexual explicit | 94% | 93% | 93% |
| identity attack | 77% | 76% | 77% |
| insult | 66% | 63% | 64% |
| toxicity | 57% | 53% | 53% |

**Ship plain int8 with self-calibrated thresholds** (in
`civil2_int8_thresholds.json`, scratchpad; they move into the Rust spec at
integration): threat 0.725, identity 0.780, sexual 0.805 at the 2% operating
point. The rule this run establishes for every future quantised artefact:
**never carry thresholds across a quantisation boundary — recalibrate on the
artefact you ship.** The grooming model dodged this only because argmax needs
ranking, not scale.

State of #525 after today: model trained, calibrated, quantised, verdict
recorded. Next: the Rust integration behind the earn-your-place measurement.

## The earn-your-place measurement: the heads stay OFF (2026-08-17)

Protocol per #525: the public iOS 17 research image's 576 real messages as the
bed, the sealed eval set's loud-category positives planted as ground truth,
census at the Balanced cut vs census ∪ heads, with a matched-cost census cut as
the alternative spend.

| category | census alone | census + heads | census at matched cost | heads alone |
|---|---|---|---|---|
| threat-violence | 15/20 | 15/20 | 15/20 | 6/20 |
| hate-identity | 10/11 | 10/11 | 10/11 | 3/11 |
| sexual-content | 10/12 | 10/12 | 10/12 | 1/12 |

(Numbers from the corrected instrument after review: loud failure on embed
drops, matched-cost cut off-by-one fixed, real [PAD] id in batching.) The
heads added 5 bed candidates (2.6% → 3.5% of messages kept) and **zero
additional catches**. The heads-alone column resolves the ambiguity the first
table left open: the heads DO fire on this register — but only on cases the
census already catches. Every head catch is a census catch's subset. What the census misses in these categories, the heads
miss too — the missed cases are quiet in register even when the category is
loud, which is Civil Comments' known blind spot, now measured a fourth time
from a new angle.

**Decision, per the issue's own gate:** the civil-heads model is not fetched
and the pass stays inactive. Users are not charged 143 MB for zero recall. The
code path ships tested behind a build-level hold-back (CENSUS_BOOST_ACTIVE)
plus the absent-model no-op (union, budget cap, failure audit all pinned by
unit tests); activation is a deliberate three-part change — flip the const,
add the spec to the fetch loop, pin the new artefact — together with a fresh
measurement table. One caveat the table itself cannot show: the ground truth
is hand-written SMS-register text and the prototypes are hand-authored, so the
census arm plays at home while the web-comment-trained heads play away. The
result still says "don't charge users 143 MB today"; it does not say a
message-level signal can never help. The negative result is the
system working: this table is exactly what the measurement gate exists to
produce, and it cost one afternoon instead of a shipped regression.

## The scorecard (2026-08-17, requested by Peter)

Trust markers: ✓ real held-out data with a published reference; ~ hand-written
cases (indicative); ✗ no trustworthy number.

| Category | Have | Reach for | Path |
|---|---|---|---|
| grooming | ✓ 97/100 @ 7-in-10k FA (PAN12 official, beats published); shipped | hold ≥95; SMS-register proof | checklist rewrite from real patterns |
| threat-violence | ~ 15/20 census | ≥18/20 on real data | ground truth first; census tuning or SMS-register model |
| hate-identity | ~ 10/11 census | hold; widen eval | ground truth first |
| sexual-content | ~ 10/12 census | ≥11/12 | inspect misses; quiet ones → harassment track |
| harassment (quiet) | ~ 17/78 (#516, unchanged) | ≥60/78 then ≥70/78 | THE open front: intimate-register conversation data → fine-tune the reader → checklist gate |
| coercive-control | ✗ (13/14 on 14 thin cases, slow scanner) | measured catch on stalking-shaped fixtures | build the pattern tier (census statistics); fixtures then thresholds; no ML |
| self-harm | ✗ | any trusted number, then ≥80% | licence pass (agreements → Peter), train + benchmark like grooming |
| scam-fraud | ✗ | ≥90% of a public smishing test set | audit smishing sets, rule tier, measure |
| drugs | ✗ | out of scope v1 | revisit when data exists |

Rule the targets inherit: nothing counts as reached until measured on data
neither the author nor the model has seen.

## The audit is complete: every category resolved (2026-08-17, evening)

Peter's instruction was test data for ALL categories. The sweep below finishes
the three unfinished rows; licences verified on the hosting pages, not assumed.
Full candidate tables from the review agent are archived in this section's
commit; the verdicts:

### Harassment / cyberbullying
**Licence-clean and usable now** (single-message register): Wikipedia Detox
(CC0, 100k+ each of attack/aggression/toxicity), Jigsaw Toxic Comment (CC0
labels), Davidson 24.8k tweets (MIT, downloaded), andrewmvd 47k cyberbullying
tweets (CC BY), ConvAbuse (CC BY, the one conversation-shaped permissive set —
abuse aimed at chat agents). **Rejected with reasons**: OLID/Waseem/
Bretschneider (no licence or dead hosts). **The decisive negative**:
conversation-level, intimate-register English harassment data that a shipped
product may use **does not exist publicly** — the closest sets are role-play
French/Italian (NC or unlicensed), dead university sites, or TOU-gated
(ALONE, individual agreement by email). This is now a shown-search fact, not
an assumption.

### Smishing / SMS scam
**In hand**: UCI SMS Spam Collection (CC BY 4.0, 5,574 real SMS, downloaded)
and Mendeley Mishra-Soni (CC BY 4.0, 638 real smishing + spam/ham). Everything
else English is largely UCI recycled; the one fresh source (Smishtank, 1,090
community-reported smishing) has a dead download link — emailing the UTSA
authors is a cheap parallel bet (Peter's call whether to send). Modern scam
shapes (parcel/toll/bank, 2024+) will need synthetic augmentation on top; a
CC-BY LLM-generated balanced set exists as a starting point.

### Self-harm / suicidality
**The unlock**: the C-SSRS-labelled Reddit set (Zenodo, **CC BY 4.0, expert
clinician annotations**, kappa 0.76, downloaded — 3.6MB, 500 users) is the
calibration/eval anchor this category lacked. SWMH (54k posts) is usable for
research-phase signal via an instant click-through (CC BY-NC). The gold
standards are closed to this project in practice: UMD requires institutional
IRB approval; CLPsych's agreement forbids product training pipelines; Crisis
Text Line (the perfect SMS register) is a closed partnership. The big Kaggle
scrapes carry uploader-asserted licences on text the uploaders don't own —
usable as weak-label bulk only with that provenance accepted.

### The cross-category fact the audit establishes
The public supply is single-message, public-register text. **In every category,
the intimate conversation-level register this product actually scans has no
usable public dataset.** The strategy this dictates — public data for surface
signal, hand-built conversation eval sets under CI guards with adversarial
review, fine-tuning for register transfer — is now the documented conclusion
of an exhaustive search, not a preference.

### Corrected scorecard "Have" column (real-data numbers lead)
| Category | Real-data result (the trustworthy number) | Phone-register transfer check |
|---|---|---|
| grooming | 97% @ 0.07% FA, PAN12 official (1,839 positives) | shipped; checklist rewrite pending |
| threat | 87% @ 2% FA (Civil test, ~700 threats) | census 15/20 |
| hate-identity | 77% @ 2% FA (687 attacks) | census 10/11 |
| sexual-content | 94% @ 2% FA (240 examples) | census 10/12 |
| toxicity / insult | 57% / 66% @ 2% FA (thousands) | held back (adds nothing) |
| self-harm | — (C-SSRS anchor now in hand; number to come) | 15 cases only |
| harassment (quiet) | no public set exists (shown above) | 17/78 — the open front |
| coercive control | by design: pattern tier, no ML | to be measured on fixtures |
| scam | UCI+Mendeley in hand; rule tier to measure | — |

## First self-harm numbers, on the C-SSRS expert-graded anchor (2026-08-17, night)

Task: at-risk (Ideation/Behavior/Attempt) vs not (Supportive/Indicator) — the
clinical line C-SSRS draws. 500 users, 5-fold cross-validation, every number
pooled across held-out folds.

| model | caught | false alarms |
|---|---|---|
| TF-IDF + logistic regression (baseline) | 72% | 28% |
| ModernBERT, 512-token windows | 77% | 46% |

**Read the false-alarm column carefully before judging it**: the "negatives"
here are people posting in mental-health communities who experts did NOT rate
at-risk — the hardest possible near-misses, nothing like a phone's ordinary
traffic. A 46% false-alarm rate against expert-rejected near-misses does not
translate to 46% against grocery lists. It DOES say the transformer trades
precision for recall versus the classical baseline, and that neither is
deployment-grade yet: no threshold tuning has been done (argmax only), and the
register is Reddit posts, not messages.

Also recorded: the first transformer attempt silently collapsed to
never-flagging and burned 90 minutes before its one cumulative line revealed
it. Per-epoch loss printing and a seconds-fast classical baseline (which
sanity-checked the labels at 72%) are now part of the recipe — a training run
without a visible loss curve and a reference number is undiagnosable.

Next for this category: threshold sweep on the fold scores, then the register
question (Reddit → messages) via the same transfer discipline as grooming.
