# Safety Scan — validation

How we keep the Forensic 9 classifier honest (plan T10). Two layers: a
deterministic gate that runs in CI with no model, and a live eval a human runs
against a real Gemma before a release.

## Deterministic gate (CI)

`crates/traceloupe-core/src/safety_scan/eval.rs` (module tests) runs on every
push — no model, no network:

- **Fixtures parse and cover the taxonomy.** `cases.json` must have ≥10
  positives, ≥5 hard negatives, and at least one positive per Forensic 9
  category. A dropped category fails the build.
- **Kind/label consistency.** Positives expect ≥1 category; negatives expect
  none; severities are 1–3; categories are valid slugs.
- **Scorer correctness.** A perfect classifier (labels → themselves) scores
  precision/recall 1.0 with zero false alarms; a cry-wolf classifier that
  flags harassment everywhere is measurably penalized. This guards
  `score_against` so the live numbers mean something.
- **Prompt snapshot** (in `prompt.rs` tests): the system prompt names every
  category and keeps the hard-negative guidance (lyrics, quotes, jokes) — so a
  careless prompt edit that drops a category is caught here, not in the field.

These gate *prompt and code changes* deterministically. What they can't do is
measure whether the model is actually good — that needs the model.

## Live eval (manual / pre-release)

`eval_against_live_model` is `#[ignore]` so CI skips it. It spins up the
sandboxed llama-server over the fixtures and prints a per-category
precision/recall/F1 table plus a hard-negative clean rate:

```
TRACELOUPE_EVAL_MODEL=~/.../gemma-4-E4B-it-Q4_K_M.gguf \
TRACELOUPE_LLAMA_SERVER=~/.../llama-server \
cargo test -p traceloupe-core eval_against_live_model -- --ignored --nocapture
```

It reuses the **production** path — same system prompt, same JSON schema, same
`verdicts_to_findings` validation — so the numbers reflect what a real scan
produces, not a bespoke test harness.

### Release checklist

Before shipping a prompt or model change:

1. Deterministic gate green (automatic in CI).
2. Live eval run on **both** tiers (E4B and E2B); record the tables below with
   date + model build.
3. Per-category recall not materially below the last recorded baseline, and the
   hard-negative clean rate ≥ 0.9 (false positives erode a reviewer's trust
   fastest).
4. A manual pass over the real dev backup, eyeballing the top findings.

### Baselines

Recorded on **2026-08-10**, Apple M3 / 24 GB / macOS 26.5.2, llama.cpp `b10075`,
Q4_K_M weights. This is the first time either tier has been measured; every
earlier claim about classification quality was an assumption.

> **The tables below are from the 31-case fixture set.** They are kept because
> they are what the shipped decisions were made on, but they are superseded by
> the 69-case numbers further down: with 1–3 examples per category, a cell could
> only ever read 0.00, 0.50 or 1.00, and the hard negatives were too few to
> contain the cases the classifier actually fails.

**Gemma 4 E4B**

| category | precision | recall | f1 |
|---|---|---|---|
| threat-violence | 0.60 | 0.75 | 0.67 |
| harassment-bullying | 0.50 | 1.00 | 0.67 |
| sexual-content | 0.50 | 1.00 | 0.67 |
| grooming-exploitation | 1.00 | 0.50 | 0.67 |
| self-harm | 0.67 | 1.00 | 0.80 |
| hate-identity | 1.00 | 1.00 | 1.00 |
| coercive-control | 1.00 | 1.00 | 1.00 |
| scam-fraud | 1.00 | 1.00 | 1.00 |
| drugs-illegal | 1.00 | 1.00 | 1.00 |

Hard-negative clean rate **0.60** — 4 false alarms of 10: `neg-song-lyrics`,
`neg-quoted-abuse`, `neg-banter`, `neg-clinical`.

**Gemma 4 E2B**

| category | precision | recall | f1 |
|---|---|---|---|
| threat-violence | 0.60 | 0.75 | 0.67 |
| harassment-bullying | 0.00 | 0.00 | 0.00 |
| sexual-content | 1.00 | 1.00 | 1.00 |
| grooming-exploitation | 1.00 | 0.50 | 0.67 |
| self-harm | 1.00 | 1.00 | 1.00 |
| hate-identity | 1.00 | 1.00 | 1.00 |
| coercive-control | 0.50 | 0.50 | 0.50 |
| scam-fraud | 1.00 | 0.50 | 0.67 |
| drugs-illegal | 1.00 | 1.00 | 1.00 |

Hard-negative clean rate **0.90** — 1 false alarm of 10: `neg-banter`.

#### What these numbers say

**The release gate does not currently pass.** The checklist below requires a
hard-negative clean rate ≥ 0.9. E4B scores 0.60, and every one of its false
alarms is a case the system prompt explicitly warns about — lyrics, quoted
abuse, banter, clinical discussion. The prompt names them and the model flags
them anyway.

**The cascade may have its tiers the wrong way round.** The sweep tier's job is
recall, because anything it misses is never re-checked; the strong tier supplies
precision. Measured, the tiers have the opposite strengths:

- E2B (the sweep) has recall holes — **0.00 on harassment-bullying**, 0.50 on
  coercive-control, scam-fraud and grooming-exploitation. A harassment finding
  that E2B never flags is one E4B never sees.
- E4B (the re-check) has the precision problem, not E2B.

On a two-tier machine this predicts near-zero harassment findings, which no
amount of re-checking can recover.

### The harness was measuring the wrong product (2026-08-11)

A review of the harness before running public corpora found four defects, all of
which distorted every number recorded above.

1. **Severity was ignored.** `score_against` compared category sets, so a
   verdict of "concerning" satisfied a fixture demanding "serious or imminent".
2. **The shipped severity floor was not applied.** The app hides severity 1 by
   default; the eval counted those as detections. Recall described a product
   nobody runs — and, in the other direction, so did the clean rate.
3. **Structurally-clean negatives inflated the clean rate.** The emoji cases
   cannot be failed by any classifier, and were in the divisor.
4. **The input shape was wrong.** Fixtures are 2-4 message cases; production
   sends 25-message chunks.

The scorer now applies the floor, compares severity to `minSeverity`, excludes
structural negatives, and can run at production chunk shape
(`TRACELOUPE_EVAL_CHUNKED=1`).

#### Correcting the record, in both directions

| | old scorer | corrected |
|---|---|---|
| hard-negative clean rate | 0.44 | **0.87** |
| coercive-control precision | 0.42 | 0.62 |
| threat-violence precision | 0.44 | 0.67 |

**The 0.44 clean rate was never the shipped product.** Most false alarms are
severity 1, and the floor already removes them. Every "the classifier flags a
parent's curfew" claim was measured with the floor switched off.

The corrected run also surfaces what the old one could not:

- **2 real findings are found and then hidden** by the floor — `scam-delivery`
  and `grooming-gifts`. That is the floor's true cost, and it was invisible.
- **3 severity miscalibrations**: grooming a minor, a threat over sexuality, and
  supplying drugs to a minor were all rated 2 where the fixtures demand 3.

### At production chunk shape, recall collapses

The same fixtures, the same model, the same prompt — each case bedded into
ordinary conversation to the real `WINDOW` of 25 messages:

| category | short cases | 25-message chunks |
|---|---|---|
| threat-violence | 0.80 | **0.00** |
| sexual-content | 0.80 | **0.00** |
| self-harm | 1.00 | **0.00** |
| hate-identity | 1.00 | **0.00** |
| scam-fraud | 0.80 | **0.00** |
| drugs-illegal | 1.00 | **0.00** |
| grooming-exploitation | 0.80 | 0.60 |
| coercive-control | 1.00 | 0.60 |
| harassment-bullying | 0.80 | 0.20 |

**Overall recall falls from roughly 0.85 to roughly 0.16.** The clean rate rises
to 1.00 only because almost nothing is flagged at all.

Verified directly rather than inferred: *"i know where you live and im going to
make you regret this"* is detected as a 3-message case and returns an **empty
verdict list** at index 12 of a 25-message conversation. Grooming and coercive
control still register, so this is not an empty-output bug — it is dilution.

#### What this means

Every quality figure recorded before today was measured on an input shape the
app never sends. On the shape it does send, the scan misses roughly five findings
in six.

That makes `WINDOW` the most important tunable in the system — ahead of the
prompt, the model, and any confirmer or prefilter, all of which were tuned
against the wrong distribution. The obvious experiment is a window sweep:
recall against chunk size, traded off against the extra wall clock of more
chunks and the loss of the cross-message context the pattern categories need.

### Baseline on the widened fixture set

The set was rebuilt to **69 cases — 5 positives per category and 25 hard
negatives** — because four categories previously rested on a single example.
E4B, same machine and date:

| category | precision | recall |
|---|---|---|
| threat-violence | 0.44 | 0.80 |
| harassment-bullying | 0.44 | 0.80 |
| sexual-content | 0.80 | 0.80 |
| grooming-exploitation | 1.00 | 0.80 |
| self-harm | 0.71 | 1.00 |
| hate-identity | 1.00 | 1.00 |
| coercive-control | 0.42 | 1.00 |
| scam-fraud | 0.83 | 1.00 |
| drugs-illegal | 1.00 | 1.00 |

Hard-negative clean rate **0.40** — 15 false alarms of 25.

#### The wider set is worse, and that is the point

The clean rate did not fall because the model changed. It fell because the
fixture set finally contains the cases it fails. Every one of these near-miss
negatives was written from the category definitions, not from observed failures,
and every one was flagged:

| flagged | what it actually is |
|---|---|
| `neg-parent-curfew` | a parent asking a child home by eleven |
| `neg-couple-checkin` | someone texting that they landed safely |
| `neg-jealousy-resolved` | a partner naming their own jealousy and owning it |
| `neg-victim-account` | a survivor describing what was done to them |
| `neg-fiction-draft` | a novelist quoting their villain |
| `neg-sports-trash` | "we're going to destroy you on Saturday" |
| `neg-security-advice` | someone explaining how to avoid a scam |

The pattern is consistent: the classifier reads **the words of harm** rather than
**the situation of harm**. A parent's curfew and a controlling partner's curfew
use the same sentence; recounting abuse and committing it quote the same threat.
Recall is largely intact (0.80–1.00 across categories), so the model is not
blind — it is indiscriminate.

Precision is worst exactly where the categories are defined by relationship
rather than vocabulary: coercive-control 0.42, threat-violence 0.44,
harassment-bullying 0.44.

### Prompt work: measured, and it is not the fix

The classifier reads the words of harm rather than the situation of harm, so the
prompt was rewritten to attack exactly that: every category gained an explicit
`NOT:` clause naming the confusions measured above, and a standing rule to ask
who is speaking to whom before flagging.

| | before | after |
|---|---|---|
| hard-negative clean rate | 0.40 | **0.44** |
| threat-violence precision | 0.44 | 0.50 |
| harassment-bullying precision | 0.44 | 0.57 |
| sexual-content precision | 0.80 | 1.00 |
| grooming-exploitation recall | 0.80 | 1.00 |

Strictly better — no category lost recall — and nowhere near enough. The release
gate wants 0.9.

**The decisive part is what did not change.** The prompt now says, in the
coercive-control definition itself, that a parent setting a curfew is NOT
coercive control. The model still flags `neg-parent-curfew`. It still flags
`neg-victim-account`, `neg-fiction-draft` and `neg-quoted-abuse`, each of which
is named as an exclusion in its own category.

Telling this model what not to flag does not stop it flagging that thing. Prompt
engineering has now been tried at the strongest form available — per-category
exclusions plus a situational rule — and it moved the clean rate by one case out
of twenty-five. Further prompt iteration is not where the remaining precision
is.

### Throughput

`measure_scan_throughput` (same file, also `#[ignore]`) times FULL-SIZE chunks —
25 messages, the real `WINDOW` — through the production client, prompt and
grammar. The eval above uses 2–4 message cases, so its timings say nothing about
a scan. Three runs per tier, `parallel=1`, 8 chunks after one uncounted warm-up,
on an otherwise idle machine.

| | E4B | E2B |
|---|---|---|
| per chunk (3 runs) | **7.5–8.9s** | **10.3–12.0s** |
| 100k messages (~5000 chunks) | **~11 hours** | **~15 hours** |
| findings on generated domestic chat | **6** (every run) | **10** (every run) | <!-- not-a-backup-count -->
| findings on generated work chat | **0** | not measured |
| chunks failing to classify | 0 of 8 | **1 of 8** (every run) |

No backup is involved: the messages are generated in the test — ordinary
conversation about dinner, traffic and wine. A scan's cost is prefill over chunk
text of a given size, and synthetic text of that size measures it without
touching anyone's data.

#### Three things this contradicts

**A scan takes hours, not minutes.** Between ~7 and ~12 hours for 100k messages
depending on machine state and on how much the model flags — see below, per-chunk
cost tracks generation. Every speed proposal in the tracker should be argued
against that, not against a guess.

**"E2B is ~2× faster" is backwards.** The model catalog tells users E2B is
"Smaller and ~2× faster". Measured, it is consistently ~30% *slower* than E4B.
E2B is a nested sub-model of E4B rather than an independent smaller one, and
per-chunk time is dominated by how much the model generates, not by parameter
count — E2B flags more, so it writes more.

**The cascade's premise does not hold.** Sweeping with E2B and re-checking with
E4B is only worth its second model load and second pass if the sweep is
substantially cheaper. It is slower, it has the recall holes, and it fails to
produce valid output on one chunk in eight. A two-tier scan currently costs more
wall clock than E4B alone and finds less.

#### The false alarms are one category, one severity, one kind of conversation

Counted on two generated corpora of equal size, same model, same run:

| corpus | findings | severity | category |
|---|---|---|---|
| colleagues discussing a deploy | **0** | — | — |
| a couple coordinating an evening | **6** | all severity 1 | all coercive-control | <!-- not-a-backup-count -->

The first framing of this number — "the classifier flags ~3% of ordinary
conversation" — was wrong, and wrong in a way worth recording: the first corpus
written happened to be domestic, and reporting it alone turned a property of the
test text into a claimed property of the model.

What is true is narrower and more serious. The model does not over-flag in
general; it over-flags **coercive-control on ordinary relationship logistics** —
"text me if you are late", "do not start without me". Coercive control is the
category defined by ordinary-looking messages, so this is the classifier failing
at the hard case rather than failing everywhere.

**Every false alarm is severity 1, and no labelled positive in the fixture set
expects severity 1** (all 16 expectations are 2 or 3). A severity ≥ 2 floor would
therefore remove every false alarm measured here at no measured cost to recall —
but "no positive is severity 1" is a property of how these fixtures were
labelled, not proof that no real harm warrants severity 1. Widen the fixtures
before making that floor permanent.

#### Over-flagging is also why it is slow

Per-chunk time tracks how much the model generates, not how much it reads:

| corpus | findings | per chunk |
|---|---|---|
| work | 0 | **3.5s** |
| domestic | 6 | **5.9s** |

Same prompt size either way (~4090 chars). This explains E2B being slower than
E4B despite being smaller — it flags more, so it writes more — and it means the
noise problem and the speed problem are partly the same problem. Reducing false
alarms shortens scans.

### Languages

Until 2026-08-10 every fixture was English, so every number above described
English performance only. That was a gap in the validation, not a footnote — a
backup is written in whoever's language.

Six Swedish cases were added as **matched translations** of existing English
ones, so any difference is the language rather than the content. E4B, same run:

| | result |
|---|---|
| `sv-threat-explicit` | detected |
| `sv-coercive-monitoring` | detected |
| `sv-self-harm-intent` | detected |
| `sv-grooming-secrecy` | detected |
| `sv-neg-song-lyrics` | **false alarm** |
| `sv-neg-banter` | **false alarm** |

**Detection survives translation.** All four Swedish positives were caught,
including the two categories that depend on reading intent rather than
vocabulary (coercive control, grooming). Whatever is wrong with this classifier,
it is not that it only understands English.

**So does the over-flagging.** Both Swedish hard negatives were flagged, the same
two kinds that fail in English — quoted lyrics and affectionate insults. Two
cases is far too few to claim Swedish is *worse*, but there is no sign of it
being better, and the defect is clearly not language-specific.

Adding them moved the overall hard-negative clean rate from 0.60 to **0.50**,
which is the honest number now that the fixture set is not monolingual.

### Configuration ruled out

Two hypotheses about the classifier being misconfigured were tested and both are
dead:

- **The system prompt does reach the model.** A probe with a system message
  ("every reply must begin with ARRR") came back in character, so llama-server
  applies Gemma's template correctly even though that template has no native
  system role.
- **`--jinja` changes nothing.** Running the whole eval with the GGUF's own Jinja
  template rather than llama.cpp's built-in one produced **byte-identical**
  results — every cell, every false alarm. Not worth passing.

Temperature is already 0 (`client.rs`), which is why every run is reproducible.
The over-flagging is the model's behaviour under correct configuration, not a
setup mistake.

#### Read these with the caveats

- **Small fixture set.** 15 positives across 9 categories, so most categories
  rest on one or two cases; 0.00 can mean "missed the only case". Directional,
  not precise.
- **The clean rate is flattered.** Three of the ten hard negatives are the emoji
  cases, which are unflaggable by construction (`is_contentless` refuses them
  before any verdict). On the seven *semantic* negatives the real rates are
  E4B 3/7 ≈ 0.43 and E2B 6/7 ≈ 0.86.
- **Three runs per tier, and the pipeline is deterministic** — every eval cell,
  every false alarm and every mundane-text finding count was identical across
  runs. That rules out sampling variance. It does NOT add statistical power over
  the fixture distribution: repeating a deterministic pipeline three times tells
  you the same thing three times. Widening the fixture set is what would.
- **Throughput is 8 chunks on one M3.** The ~11-hour figure and the E4B/E2B
  ordering both held across three runs, but a different machine or a longer run
  could move them. An earlier set of timings was discarded entirely because a
  second benchmark was running concurrently — model load went 15.7s → 50.9s and
  per-chunk doubled. Run these alone.

## Public datasets

The in-repo fixtures are the primary gate because they match our distribution
(conversational, multi-message, pattern categories). Public single-comment
moderation sets are a useful *supplement* for the categories they cover, run
offline:

- **Jigsaw Toxic Comment** → hate-identity, harassment-bullying (map `toxic` /
  `identity_hate` / `insult` columns).
- **PAN12 sexual-predator** → grooming-exploitation (chat-log format, closest
  public match to our pattern detection).
- A **threat corpus** (e.g. the hate/threat forensics set) → threat-violence.

To run one, export it to the same shape as `cases.json` (each row a case with
`messages` + `expect`) and point `score_against` at it. They are not wired into
CI: licences vary and the files are large. Coercive-control, scam-fraud, and
contextual self-harm have no clean public analogue — the in-repo fixtures are
their only coverage, which is exactly why the fixture set exists.

## Triage pipeline — end-to-end validation runs

The proven oracle is `tools/validate-triage-pipeline.py`; setup and the run
command are in `triage-validation-setup.md`. Pass criteria: end-to-end recall
within ~0.05 of 0.94 at precision ≥ 0.90, at census threshold 0.52
(`docs/safety-scan-journey.md` §6.1/§8).

### 2026-08-12 — the lab result reproduces (journey §8, step 1)

Apple M3 / 24 GB / macOS 26.5.2, llama.cpp `b10075`. Models: Gemma 4 E4B
Q4_K_M (classifier), EmbeddingGemma-300M Q8_0 (census), Llama Guard 3 8B
Q4_K_M (confirmer). GBNF grammars freshly dumped from this checkout
(`29ff51b`, `dump_grammars`); threats and clean bed from the Jigsaw set per the
setup doc; 160 seeded chunks (80 with a real threat), prototypes from held-out
threats.

| threshold | census ceiling | focused (recall / precision) | +Guard = end to end | batch baseline |
|---|---|---|---|---|
| 0.64 | 0.88 | 0.90 / 0.99 | **0.82 / 1.00** | 0.30 / 0.89 |
| 0.58 | 0.91 | 0.99 / 0.91 | **0.85 / 0.97** | 0.30 / 0.89 |
| 0.52 | **0.96** | 1.20 / 0.74 | **0.94 / 0.95** | 0.30 / 0.89 |

**PASS — every headline number matches §6.1 digit for digit**: baseline
0.30/0.89, census ceiling 0.96, end-to-end 0.94/0.95 at 0.52. The dial is
monotonic exactly as documented — lowering the threshold raises the ceiling
(0.88 → 0.91 → 0.96) and end-to-end recall (0.82 → 0.85 → 0.94), trading
Guard-stage precision (1.00 → 0.97 → 0.95) and deep-scan work (census keeps
71 → 81 → 110 of 160 chunks).

Reading notes:

- **Focused-stage "recall 1.20" is not a bug**: findings are (chunk, message)
  pairs, so at loose thresholds a real chunk can yield more than one finding
  and the intermediate recall over-counts. Guard trims the duplicates and false
  alarms (129 → 79 findings at 0.52); the end-to-end row is the real number.
  Chunk-level (deduplicated), the focused stage at 0.52 reads: recall 0.9625,
  precision 0.740 — the reference the Rust parity test below asserts against.
- **Stage attribution behaves as designed**: at 0.64 the census ceiling is the
  binding loss (0.88, a miss there is permanent); at 0.52 the ceiling lifts to
  0.96 and Guard supplies the precision.
- **This validates threat-violence only** (Jigsaw `threat` labels). The three
  relationship categories still have no external ground truth — journey §10.12.
- Wall clock ~50 min for the sweep on an otherwise idle machine: batch baseline
  ~13 min (computed once; it is threshold-independent because the chunks are
  seeded), then census+focused+Guard per threshold ~10–18 min, slower at looser
  thresholds. The 0.64 run was resumed from the stage checkpoint file after an
  interruption — the checkpoint/resume path works.

### 2026-08-12 — the wired Rust pipeline matches the oracle (stage-level)

Same day, same machine and models. The merged `run_triage` — the production
census/rank/window/classify path with real sidecars, the embedder→classifier
healthy-swap, the production prompt and grammar — was driven over the oracle's
exact seeded corpus by the `#[ignore]` test
`triage_pipeline_matches_reference` (`eval.rs`; corpus dumped by the oracle's
`TRIAGE_DUMP_CHUNKS` mode). Thorough mode (threshold 0.52, no confirmation —
oracle stages 1+2), chunk-level scoring:

| | wired Rust | oracle reference |
|---|---|---|
| census keeps (messages) | **146** | 146 — identical set size |
| census ceiling | **0.963** | 0.9625 |
| focused recall | **0.963** | 0.9625 |
| focused precision | **0.726** | 0.740 |

**PASS.** The census reproduces the oracle message-for-message; recall is
identical; precision sits 0.014 inside the tolerance band — the residual is
the wired prompt's window rendering (`Conversation: c<n>` labels vs the
oracle's constant label), not a pipeline defect. ~10 min wall clock (~800
embeddings + 146 focused calls, one M3).

~~What this does not yet cover: the Guard confirmation stage (no confirmer tier
in the catalog yet).~~ Covered the same day — next section.

### 2026-08-12 — the wired confirmation stage (Guard tier, #474)

Same machine and corpus. With Llama Guard 3 8B in the catalog as the confirmer
(`guard.rs`: the hand-rolled `/completion` prompt with the **Forensic 9
replacing** Guard's default categories — strategy B, journey §10.5), the parity
test's second scan runs Precise mode (threshold 0.58, confirmation on — the
sweep point the oracle also measured) over the incremental census, swapping
classifier→confirmer at the phase boundary. Chunk-level, scored from the
confirm decision stream:

| | wired Rust (9-category block) | oracle reference (threat-only block) |
|---|---|---|
| provisional findings vetoed | 26 of 90 | 17 of 87 |
| confirmed recall | **0.762** | 0.8125 |
| confirmed precision | **0.968** | 0.9701 |

**PASS (bands: recall ≥ 0.74, precision ≥ 0.90), with an honest delta**: the
production nine-category block vetoes more than the oracle's single
threat-category block — ~0.05 recall traded at essentially identical
precision. That is the expected direction (nine categories give Guard more
ways to judge a threat as fitting none of them well) and it is the real
behaviour of the shipped Balanced/Precise modes on threat-style content, not a
regression to hide: the reference was measured with a category block the
product deliberately does not use (a threat-only block would be category
narrowing, measured harmful in journey §5.5).

Two measurement notes for future runs:

- **Score confirmation from the decision stream, not the findings table.**
  `begin_scan` reuses the scan row for an identical scope, so querying run 2's
  findings by scan id silently merged run 1's — the first attempt printed the
  Thorough numbers for the confirm stage (the giveaway: 64 findings cannot
  flag 77 chunks). <!-- not-a-backup-count: Jigsaw corpus figures -->
- The shipped mode grid has no (0.52 + confirm) point, so the oracle's
  headline 0.94/0.95 config is bracketed by, not equal to, a named mode:
  Thorough ≈ 0.96 recall / 0.73 precision (wired), Precise ≈ 0.76 / 0.97
  (wired). If a mode matching the headline is ever wanted, it is one enum row —
  but it should be measured as its own sweep point first.

### 2026-08-12 — the fixture-cache end-to-end run, and #409 measured

Same machine and models. Two `#[ignore]` tests close the remaining §8
re-measure sliver:

**`triage_from_a_fixture_cache`** drives the whole data path from a REAL
message store: a seeded fixture `CacheDb` → `census_threads` (the production
reader) → fixture-positive prototypes → `run_triage` with live sidecars. Three
generated conversations, one planted threat — the exact sentence the dilution
work proved a batch scan misses at production shape. Result: every message
censused, every candidate deep-scanned (no budget, `unscanned 0`), the planted
threat found with its **durable `message_fingerprint`, sender and service
carried straight from the cache row** — the contract dismissals and finding
identity depend on. What the corpus-parity test could not prove — that the
store and the pipeline agree — is now proven. With `TRIAGE_GUARD_MODEL` set,
the same test then runs a Precise-mode scan over the (incremental, zero
re-embed) census: of the Thorough run's 3 findings, **Guard vetoed 2 and kept
exactly the planted threat** (`confirm_failed 0`) — the precision stage doing
its job from the real store, phase-4 swap included. (The Tauri command glue
itself — gate, events, watcher — has its own open §8 item; it must not ride
implicitly on the UI milestone.) One incident worth keeping: the first run of
the extended test tore the classifier down before the confirm phase, and the
§10.6 all-failed guard turned that into a loud harness error instead of a
clean-looking zero — the guard catching exactly the class of mistake it was
built for, in its own test. One observation for the prototype
work: on a 15-message mundane corpus, 10 messages cleared the 0.52 census
cut — the fixture-positive prototypes are permissive on small mundane sets,
which costs deep-scan work, not correctness (precision comes from the focused
stage). The generated-corpus §8 item is where that ceiling gets tightened.

**`measure_focused_prompt_cache`** answers #409 with numbers instead of the
standing assumption ("focused mode re-sends the system prompt per message —
the highest-value performance work"). After review, the harness sends the
EXACT production request body (`LlmClient::chat_json_body`) and production now
sends `cache_prompt: true` explicitly — the saving is a property of the
request, not of a server default a future llama.cpp bump could flip. Re-run
through that body, the numbers reproduce:

| | prompt tokens evaluated | prompt eval time |
|---|---|---|
| first call (cold) | 1009 | ~4.1–4.4 s |
| later calls, `cache_prompt: true` (what production sends) | **139** | **~0.7–0.8 s** |
| later calls, `cache_prompt: false` (counterfactual) | 1009–1015 | ~4.3–5.1 s |

**The prompt cache amortizes ~86% of the focused prompt** (the shared
system-prompt prefix) across sequential calls on the one slot the triage
pipeline uses — roughly 4 s saved per focused call — and production requests
it explicitly, so the behaviour survives server-default changes. The remaining upside (persistent cross-run
caches, multi-slot prefix sharing) is small against that and not currently
worth its complexity; the journey's prompt-caching item is closed with this
measurement. (#409 itself stays open for its other half — flash attention —
which is unmeasured.)
