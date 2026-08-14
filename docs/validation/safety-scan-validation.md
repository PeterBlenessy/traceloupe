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

### 2026-08-12 — the first run against a real device, and what it cost

The public iOS 17 research image (Joshua Hickman / Digital Corpora, the corpus
this repo validates parsers against) imported through the production native
path in **4 s** — 29 threads, 576 messages in scope — and was then censused and
deep-scanned with live sidecars (`triage_on_a_public_research_image`, Thorough,
deep-scan budget 40). Two things came out of it, and the second matters more
than the first.

**Two shipped defects that no corpus here could have caught (#485).** The census
died on HTTP 500 within seconds. `llama-server` cannot split a pooled embedding
across physical batches and refuses above one — default 512 tokens — so the
first long message in a real conversation aborted the whole scan. Raising the
sidecar's batches to its context fixed that and exposed the second layer: the
binding limit is really the 2048-token **context**, and characters are a poor
proxy for tokens. Measured on the pinned build:

| input | 4,000 chars |
|---|---|
| ordinary prose | embeds |
| dense ASCII (URLs, hashes, base64, code) | **HTTP 500 between 2,000 and 3,000 chars** |

Dense text tokenises roughly 3× denser than prose — but review pushed on
whether chars bound tokens at all, and measuring said no. The tokenizer falls
back to one token per BYTE outside its vocab:

| 1,500 chars of… | bytes | result |
|---|---|---|
| ASCII | 1,530 | embeds |
| Japanese | 4,530 | embeds (vocab covers it) |
| emoji + ZWJ | 5,446 | embeds (vocab covers it) |
| Linear B / hieroglyphs | 6,030 | **HTTP 500** — and fails from 700 chars up |

So a CHAR cap is safe for the scripts a person is likely to type and unsafe for
the ones they are not, which is the wrong way round when the cost is a dead
scan. The cap is therefore **1,500 bytes** on a char boundary, which holds at
the fallback's worst case. Well-covered scripts pay for it (~500 Japanese chars,
~375 emoji), accepted deliberately: the census scores a RANKING signal from the
opening of a message and the focused stage still reads all of it.

Two guard corrections came out of the same pass. The first version probed
*prose* at the cap — which passes at 4,000 chars even when the cap is wrong, so
it proved nothing; it now probes byte-fallback text at the cap, and both raising
the cap and removing the batch flags fail it. And the census no longer dies when
the embedder refuses a message at all: it skips it, counts it as `unscorable`,
and reports it, because tokenisation has now surprised this pipeline twice and
the blast radius of the third time should be one message rather than a scan.

**The census selects 55% of a real backup — which inverts the architecture's
premise.**

| | this image | Jigsaw corpus (the tuning set) |
|---|---|---|
| messages in scope | 576 | 800 |
| candidates at threshold 0.52 | **318 (55.2%)** | 146 (18.3%) |
| census throughput | 64 msg/s (100k ≈ 26 min) | — |
| focused classification | ~6.5 s/call | ~7 s/call |

Triage is supposed to census cheaply and spend depth on a small ranked subset.
At 55% selectivity it does not: extrapolating this rate to a 100k-message
backup gives ~55,000 focused calls ≈ **100 hours**, against the shipped batch
scan's measured ~11 hours. As currently tuned, triage on a real backup would be
roughly **nine times slower than the thing it replaces** — while the census
itself is cheap and fast exactly as designed (26 min for 100k).

The likely mechanism, and it is testable: the 0.52 threshold was calibrated
against a corpus scored with **one** prototype (threat-violence), while
production scores against **nine** and takes the max. More prototypes means
more chances to clear the bar, so the same threshold is systematically looser in
production than where it was measured. This is the §7 lesson again — a number
measured on one distribution and applied to another — and it is the same shape
as the WINDOW=25 error: the pipeline was right, the calibration was for a
different input.

Caveats: one device, 576 messages, no ground truth in this image, and the <!-- not-a-backup-count: public DFIR research image, not anyone's own backup -->
deep-scan was budgeted at 40 of 318 candidates, so nothing here says anything
about recall or precision on real data. What it does establish is the SHAPE —
volume, selectivity, throughput — which is what the cost argument rests on.
Follow-up: #486.

### 2026-08-12 — why the census keeps 55%: two causes, not one (#486)

Embeddings do not depend on the prototype set, so every message of the public
iOS 17 image was embedded once and then scored against each prototype subset
offline (`census_selectivity_by_prototype_count`). Keep rate, 576 messages: <!-- not-a-backup-count: public DFIR research image -->

| prototypes | 0.52 | 0.55 | 0.58 | 0.64 |
|---|---|---|---|---|
| 1 category | 32.5% | 17.7% | 11.1% | 1.2% |
| 3 categories | 47.6% | 31.4% | 17.7% | 3.1% |
| 5 categories | 51.7% | 35.1% | 18.8% | 3.1% |
| **9 categories (production)** | **55.2%** | 38.5% | 20.5% | 3.1% |

**The suspected cause is real but only half the story.** Scoring against nine
prototypes instead of one moves the keep rate 32.5% → 55.2% at the shipped
threshold. But the same single prototype that kept 18.3% of the Jigsaw corpus
keeps 32.5% here, so the gap decomposes roughly evenly:

- **+14 points** because real conversation is not the tuning distribution;
- **+23 points** because production takes the max over nine prototypes.

Fixing one without the other would not have been enough, and either alone would
have looked like the whole answer.

**The threshold dial still works — it is just set in the wrong place for this
input.** Cost of a 100k-message backup at ~6.5 s per focused call, against the
batch scan's measured ~11 h:

| threshold | candidates | focused calls | wall clock |
|---|---|---|---|
| 0.52 (Thorough) | 55.2% | 55,200 | ~100 h |
| 0.55 (Balanced) | 38.5% | 38,500 | ~70 h |
| 0.58 (Precise) | 20.5% | 20,500 | ~37 h |
| **0.64** | **3.1%** | **3,100** | **~5.6 h** |

**Every shipped mode is currently more expensive than the batch scan it
replaces**; only around 0.64 does triage become the cheaper option it was
designed to be. What that costs in recall on real data is NOT known — this
image has no ground truth, and on Jigsaw 0.64 dropped the census ceiling from
0.96 to 0.88. Recalibration therefore needs a labelled real-shaped corpus,
which is the same missing asset as the three unlabelled categories.

**The prototypes are wildly uneven**, which is a cheaper lever than the
threshold. Keep rate at 0.52 for each category alone:

| category | keeps | | category | keeps |
|---|---|---|---|---|
| sexual-content | **43.2%** | | coercive-control | 26.2% |
| threat-violence | 32.5% | | drugs-illegal | 25.5% |
| grooming-exploitation | 31.1% | | self-harm | 14.4% |
| harassment-bullying | 29.9% | | hate-identity | 11.5% |
| | | | scam-fraud | 11.3% |

One prototype — sexual-content — flags 43% of an ordinary phone's messages by
itself, nearly four times what scam-fraud or hate-identity flag. That is a
prototype-quality problem, not a threshold problem: a centroid built from a few
fixture positives sits in a region of the space that ordinary chatter also
occupies. Improving or splitting the loosest prototypes would buy selectivity
without trading recall the way raising the threshold does.

### 2026-08-12 — the census thresholds, re-derived on a real distribution (#489)

**The first attempt at this was train-on-test and its numbers are void.** The
planted positives were the fixture positives, and the prototypes are built from
those same fixtures, so every planted message was scored against a centroid
containing its own text. It reported recall 1.000 at 0.52 where the held-out
oracle measured a 0.96 ceiling — the tell was in the table. Journey §7 names
this trap twice; this was the third time.

Re-measured with LEAVE-ONE-OUT prototypes (each case scored against centroids
built from every other case of its category, which is what production faces —
a real harmful message is always unseen relative to the fixtures). Bed: the
public iOS 17 image's own conversation. Positives: all 128 messages of the <!-- not-a-backup-count: fixture positives planted into a public research image -->
committed fixtures.

| threshold | census recall (held-out) | (train-on-test) | ~e2e | +confirm | cost /100k |
|---|---|---|---|---|---|
| 0.52 | 0.921 | 1.000 | 0.86 | 0.75 | 100 h |
| 0.55 | 0.849 | 0.968 | 0.79 | 0.70 | 70 h |
| 0.58 | 0.722 | 0.929 | 0.67 | 0.59 | 37 h |
| **0.61** | **0.563** | 0.802 | 0.52 | 0.46 | 20 h |
| **0.64** | **0.397** | 0.659 | 0.37 | 0.32 | 5.6 h |
| **0.67** | **0.333** | 0.508 | 0.31 | 0.27 | 1.3 h |

**The bias grew with the threshold** — up to 0.26 at 0.64 — which is exactly
where thresholds get chosen, so the first attempt was wrong in the most
expensive possible place. Had it shipped, Balanced would have promised 0.66
census recall and delivered 0.40.

**Shipped: 0.61 / 0.64 / 0.67** (Thorough / Balanced / Precise). Against the
full read's 0.30 recall and ~11 h per 100k:

Read the column each mode ACTUALLY runs — Thorough has no confirmation stage,
Balanced and Precise always confirm, and confirmation trims recall to buy
precision:

| mode | cut | recall it delivers | vs the full read |
|---|---|---|---|
| Thorough | 0.61 | 0.52 (no confirm) | **1.7× recall, 1.9× time** |
| Balanced | 0.64 | 0.32 (+confirm) | **1.1× recall, 0.5× time** |
| Precise | 0.67 | 0.27 (+confirm) | **0.9× recall, 0.1× time** |

- **Thorough** is the only posture that finds materially more than a full read,
  and it costs roughly twice as long — the slowest option in the app.
- **Balanced** is a wash on recall at half the time, with fewer false alarms.
- **Precise** finds slightly LESS than a full read, very fast and much cleaner.
  Its name is honest: it is a precision play, not a recall one.

An earlier version of this section quoted Balanced at 1.3× and Precise at
parity by reading the no-confirmation column for modes that always confirm.
They do not run that column.

**Two conclusions that matter more than the constants.** Triage's quality
advantage is far smaller than the lab's 0.94 implied — that figure came from
5-message Jigsaw chunks scored against non-held-out prototypes, and neither
condition holds in the product. And **no threshold is clearly better than the
full read on both axes**: the curve is dominated by prototype quality, so
moving this number cannot buy what better centroids would. Retune after the
prototypes change, not before.

Caveats: one device; fixture phrasing may be blunter than real harm; the
0.30 / 11 h baseline is a Jigsaw measurement, so every multiplier here is
indicative rather than like-for-like (nothing in this run re-measured the full
read on this distribution); and the held-out column is PESSIMISTIC relative to
production, because each centroid here is built from one fewer case than the
one production scores against.

### 2026-08-13 — the census gets its own prototype corpus (#491)

Every centroid was built from the five hand-written positives per category in
`cases.json` — which is also the eval ground truth. Two problems in one: the
centroids were thin and uneven (`sexual-content` alone kept 43% of an ordinary
phone), and using the eval set to build them made every measurement
train-on-test unless held out by hand, a trap this project fell into three
times.

So the census now has `fixtures/safety-scan/prototypes.json`: 83 examples,
weighted deliberately — most and longest for the relationship-defined
categories (coercive-control 14, grooming 12, harassment 10), fewest and
tightest for `sexual-content` (8), whose centroid was the one over-keeping
ordinary chatter. A test asserts the two corpora share no text, so the coupling
cannot come back. **With disjoint corpora the leave-one-out apparatus is gone**
— there is nothing to hold out, because the ground truth was never in the
centroids.

Measured through the production path (same bed, same ground truth):

| threshold | recall | selectivity | cost /100k |
|---|---|---|---|
| 0.58 | 0.913 | 42.9% | 77.4 h |
| 0.61 | 0.778 | 25.9% | 46.7 h |
| **0.64** | **0.603** | 11.5% | 20.7 h — Thorough |
| **0.67** | **0.452** | 2.6% | 4.7 h — Balanced |
| **0.70** | **0.286** | 0.9% | 1.6 h — Precise |

(Re-measured 2026-08-13 after the review of #491, on 85 examples. Two
corrections moved these from the numbers first recorded: the ground-truth loop
had embedded once per *case × category*, so multi-category cases were counted
twice in the denominator — it is 126 planted messages, not 128 — and the
`scam-fraud` corpus was extended, below. The shipped cuts did not move: the
selectivity column is unchanged to the tenth of a percent, so the cost
argument that chose 0.64/0.67/0.70 is untouched.)

The richer centroids score everything higher, so the same postures sit **one
notch up the scale**. At matched cost the corpus is a modest win, not a step
change:

| operating point | old prototypes | new corpus |
|---|---|---|
| ~20 h | 0.563 | **0.603** (+0.040) |
| ~5 h | 0.397 @ 5.6 h | **0.452 @ 4.7 h** (+0.055, and cheaper) |
| ~1.5 h | 0.333 | 0.286 (−0.047) |

**Shipped: 0.64 / 0.67 / 0.70.** Thorough now delivers ~1.8× the full read's
recall at 1.9× its time; Balanced ~1.2× at 0.4×; Precise ~0.8× at 0.15×.

Two honest notes. The old column carried a leave-one-out penalty (centroids
built from one fewer case than production used) while the new one is exact, so
part of that gap is measurement, not improvement — the matched-cost rows are
the defensible comparison, and they say "modestly better", not "fixed". And
**per-category the spread did not narrow the way it should have**: at the new
Balanced cut, `scam-fraud` recalls 0.25 and `drugs-illegal` 0.42 against
`self-harm` 0.80 and `hate-identity` 0.73. My scam examples read like phishing
templates and the fixtures' scams read like conversations; that is a corpus
bug, and the next iteration should fix it rather than move a threshold.

#### The scam diagnosis was wrong, and measuring it said so

That last paragraph contained a confident, untested causal claim: the scam
examples read like phishing templates while the fixtures' scams read like
conversations. Acting on it, all eight `scam-fraud` examples were rewritten
into longer conversational registers ("my darling the airline is holding my
luggage and every card i have is blocked out here").

**It made things worse.** Scam recall fell 0.25 → 0.17 at the Balanced-adjacent
cut and to 0.00 at both tighter ones. The diagnosis had the direction
backwards: the fixtures are the terse, imperative ones — `URGENT: your bank
account is locked. verify now at secure-bank-verify.link`, `this is the tax
office, there is a warrant out in your name` — and the rewrite walked the
centroid away from them.

The third variant kept the original terse register and instead added the two
fixture *modes* the corpus never covered at all: an authority-threat
("proceedings have been raised against your name … or officers attend this
afternoon") and a verify-at-a-link. That is what worked:

| scam-fraud corpus | @0.64 | @0.67 | @0.70 | overall @0.64 |
|---|---|---|---|---|
| original 8, terse | 0.25 | 0.08 | 0.08 | 0.595 |
| rewritten 10, conversational | 0.17 | 0.00 | 0.00 | 0.587 |
| **terse 8 + 2 uncovered modes** | **0.33** | **0.17** | 0.00 | **0.603** |

Selectivity was identical across all three (11.5% / 2.6% / 0.9%), so the win is
free — it buys recall without keeping one extra message of an ordinary phone.

Two things this earns. **Coverage of modes beats register**: what the centroid
was missing was a kind of scam, not a way of speaking, and adding examples in a
register borrowed from the fixtures is also the shape most likely to leak
train-on-test. And **the honest scale of the result** — `scam-fraud` has about
twelve ground-truth messages, so 0.25 → 0.33 is one additional message
recovered. It is the right direction on a sample far too small to call it more
than that; the category needs a real corpus before its number means anything.

The larger conclusion stands and is now better supported: examples help, but
they are not the step change. The ceiling is the embedding space's ability to
separate harm from ordinary conversation, which is what a fine-tune would
address.

### 2026-08-13 — a cost ceiling per posture, enforced (#486)

Selectivity is the entire economic premise of triage, and it has now slipped
twice without anything failing: the Jigsaw-fitted cuts kept 55% of a real phone
(~100 h per 100k against the full read's ~11), and the first re-derivation was
itself wrong because production scores against nine prototypes rather than one.
Both were caught by a person reading a table. `safety_scan::cost_model` makes
that a test instead.

The cost model is one formula, shared by the CI guard and the measurement
harness so they cannot drift: `hours_per_100k(selectivity) = 100_000 ×
selectivity × 6.5 s`. It deliberately ignores census time (~26 min per 100k),
which makes every number here a LOWER bound — a guard that under-estimates cost
fails late, never early.

Each posture gets a ceiling derived from what its name promises the user, not
from what it happens to measure:

| posture | ceiling | why | measured |
|---|---|---|---|
| Thorough | 33.0 h (3× full read) | openly trades time for recall, but a scan that runs for days is not a product | **20.7 h** |
| Balanced | 11.0 h (1×) | the default; a default costing more than the scan it replaced is a regression however good its recall | **4.7 h** |
| Precise | 3.7 h (⅓×) | chosen for speed | **1.6 h** |

Two tests enforce it. In CI, the recorded selectivity of each posture is costed
and checked against its ceiling, and the record's thresholds must equal the
shipped `census_threshold()` — so moving a cut without re-measuring fails with
an error naming the harness to re-run. In the live harness, the same ceilings
are checked against a FRESH measurement, and the freshly measured selectivity
is compared against the checked-in constants (tolerance 0.1 pp, under one
message on this bed) so the CI guard can never be quietly checking a stale
number.

Verified against the public iOS 17 image: 11.5% / 2.6% / 0.9%, matching the
record exactly. Mutation-proven both ways — restoring #486's 55.2% fails with
"99.7 h per 100k against a ceiling of 11.0 h", and moving Balanced's cut to
0.61 without re-measuring fails the coupling test.

**What this does not cover.** The ceiling is a cost bound, not a quality one:
nothing here fails when recall drops, and the per-category spread remains wide
(`scam-fraud` 0.33 against `hate-identity` 0.82 at the same cut). A single
max-over-prototypes score with one global threshold is what produces that
spread — the remaining open question on #486.

### 2026-08-13 — per-category thresholds do not buy recall (#486)

The census scores a message against nine centroids, keeps the MAX, and compares
it to one number. So every category is read at whatever cut suits the loosest
centroid — the shape of a spread that has survived every corpus revision
(`hate-identity` 0.82 against `scam-fraud` 0.33 at the Thorough cut). #486's
remaining question was whether that is a corpus problem or a **scoring** one.

`per_category_cuts_vs_one_global_cut` answers it. Each category gets its own
threshold, set at the same quantile of that category's score distribution over
the bed — so every category is allowed to keep an equal share of ordinary
chatter — and a message is a candidate if ANY category clears its own bar. That
is the cheapest honest version of "normalise per category": it needs no
per-category ground truth, only the bed, so it could actually ship.

Compared at genuinely matched cost (the global curve on a 0.0025 grid, so
"same cost" means the same selectivity rather than the nearest coarse point):

| per-category q | recall | selectivity | global at the same cost | Δ |
|---|---|---|---|---|
| 0.800 | 0.921 | 49.3% | 0.5675 → 0.929 | −0.008 |
| 0.900 | 0.865 | 31.6% | 0.6025 → 0.833 | +0.032 |
| 0.950 | 0.770 | 20.7% | 0.6175 → 0.738 | +0.032 |
| 0.975 | 0.683 | 13.9% | 0.6300 → 0.698 | −0.016 |
| 0.990 | 0.556 | 7.3% | 0.6500 → 0.571 | −0.016 |
| 0.995 | 0.476 | 4.0% | 0.6675 → 0.476 | +0.000 |

**A wash.** Six operating points, deltas from −0.016 to +0.032, straddling zero
with no consistent direction; on 126 positives ±0.03 is about four messages.
Per-category thresholds do not buy recall, and **the scoring scheme is
therefore not what is costing it** — which is more support for the standing
conclusion that the embedding space's ability to separate harm from ordinary
conversation is the ceiling.

What they *do* is redistribute. At matched cost (13.9% vs 13.5%):

| category | per-category | global | its own cut |
|---|---|---|---|
| scam-fraud | **0.83** | 0.42 | 0.606 |
| drugs-illegal | **0.67** | 0.58 | 0.617 |
| harassment-bullying | 0.65 | 0.65 | 0.637 |
| self-harm | 0.80 | 0.80 | 0.640 |
| sexual-content | 0.69 | 0.75 | 0.651 |
| threat-violence | 0.69 | **0.85** | 0.637 |
| hate-identity | 0.82 | **0.91** | 0.599 |
| coercive-control | 0.59 | **0.71** | 0.648 |
| grooming-exploitation | 0.47 | **0.71** | 0.654 |

The spread narrows at the top (0.91 → 0.83) and the worst category more than
doubles (0.42 → 0.83), paid for by the categories that were already well
served. That makes it a **values question, not an efficiency one**: uniform
coverage across harm types, or the most total detections for the same money?
Nothing in the measurement decides that.

**Not shipping it on this evidence.** Each category has ~12 ground-truth
messages, so every per-category move above is three to five messages, and the
total is a wash. The honest position is that this needs the generated corpus
(the open §8 item) before it can be decided — and if it is ever taken, it
should be taken as a deliberate equal-coverage policy, not sold as an
improvement.

#### The harness bug that nearly became a finding

The first run of this experiment reported per-category cuts beating the global
one by +0.087 at the ~50 h point. It was wrong. `build_prototypes` applies the
task prefix and the byte cap **itself**, and the new harness passed it the
eval's `embed_one`, which applies them too — so every centroid was built from
double-prefixed text. The centroids scored about 0.025 high on everything,
which shifted the whole curve by one grid step: this harness's "global" column
read 25.2% selectivity at 0.64 where `census_recall_vs_cost` read 11.5%.

Every number was internally consistent and entirely plausible. What exposed it
was **two harnesses computing the same column and disagreeing** — the bed
hashed identically (576 messages, same sha) and the scoring cross-checked to <!-- not-a-backup-count: public DFIR research image -->
0.000000, which left the embeddings as the only suspect, and the mean bed score
differed (0.5827 against 0.5602). A cross-check inside one harness could not
have caught it, because the mistake was made identically in both of that
harness's constructions.

Two rules. **A shared helper that silently normalises its input is a trap**;
`build_prototypes` now documents in its own signature that the closure must be
a raw embed, and all other call sites (including production) were audited and
are correct. And **keep two independent harnesses computing at least one column
in common** — that redundancy is what turns a plausible wrong number into a
visible contradiction.

### 2026-08-13 — ground truth first, then the prototypes earn their keep (#499)

The negative result that produced this: nine coercive-control mode prototypes
(tracking device, blackmail, intimate material as leverage, isolation from
children, employment, lock-out, home surveillance, unauthorized recording,
confinement) moved coercive-control recall by **exactly zero** and made Balanced
38% more expensive. The modes did not exist in the ground truth, so nothing
written against them could register as recall.

So the ground truth was written first: **nine positive cases, one per mode,
27 messages** — conversations rather than lines, because coercive control is a
pattern. Coercive-control ground truth went **17 → 45 messages**; the whole
positive set **126 → 154**. Adding ground truth does not touch selectivity (it
is a property of the bed), which the guard confirmed.

Then the same nine prototypes were re-tested against it. Both columns are on
the identical 154-message ground truth:

| cut | 14 prototypes | 23 prototypes |
|---|---|---|
| 0.64 Thorough | 0.591 @ 11.5% — 20.7 h | **0.617 @ 11.3% — 20.4 h** |
| 0.67 Balanced | 0.448 @ 2.6% — 4.7 h | 0.455 @ **3.6%** — 6.6 h |
| 0.70 Precise | 0.253 @ 0.9% — 1.6 h | **0.273 @ 0.9% — 1.6 h** |

and on coercive-control itself: **0.56 / 0.38 / 0.11 → 0.62 / 0.40 / 0.16**.

**The same nine prototypes that were worthless are now worth shipping.** Nothing
about them changed — only whether the eval set exercised what they describe.
Thorough is strictly better (more recall, slightly cheaper) and so is Precise
(more recall, identical cost).

**Balanced is the honest blemish.** It pays a 40% cost rise (4.7 → 6.6 h per
100k) for +0.007 overall recall. Accepted, not hidden: it stays well inside its
11.0 h ceiling and the same change strictly improves the other two postures. The
right fix is not to revert but to **re-derive Balanced's cut** — on this corpus
its old 2.6% keep-rate now sits nearer 0.68 than 0.67. Left as the next
threshold pass rather than guessed at here.

The rule this earns, and it inverts the intuition: **a prototype corpus cannot
be evaluated ahead of the ground truth it is scored against.** Widening
prototypes against a narrow eval set is measurable only as cost, and looks like
evidence that the examples do not help. #492 found that covering missing modes
moves recall; the precondition is that the mode exists on *both* sides.

#### Re-deriving Balanced's cut: 0.67 → 0.675

The corpus change above left Balanced costing 6.6 h per 100k. Its posture claims
*"about as much as a full read in roughly half the time"* — the full read is
~11 h, so "roughly half" is ~5.5 h, and 6.6 h is 0.6× rather than 0.5×. The cut
had stopped matching the name.

A fine sweep (0.005 steps, now a permanent part of the harness — "somewhere
between 0.67 and 0.70" is not a threshold):

| cut | recall | selectivity | h/100k |
|---|---|---|---|
| 0.665 | 0.494 | 4.2% | 7.5 |
| 0.670 | 0.455 | 3.6% | 6.6 |
| **0.675** | **0.422** | **2.8%** | **5.0** ← 0.45× the full read |
| 0.680 | 0.390 | 1.7% | 3.1 |

**Shipped 0.675.** The arbiter is the promise, not the measurement: a posture
whose cut drifts until its name stops being true has been retuned into
something else.

**The honest cost.** At approximately matched cost in this region the widened
corpus is slightly *worse* — 0.448 at 2.6% before it, 0.422 at 2.8% after —
about −0.03, on the same 154-message ground truth. The same corpus makes
Thorough better on **both** axes (0.591 @ 11.5% → 0.617 @ 11.3%) and Precise
better at identical cost (0.253 → 0.273 @ 0.9%). So the nine coercive-control
prototypes help at loose and tight cuts and hurt in the middle, which is worth
knowing and is not what anyone predicted.

Three postures, three different answers from one corpus change, is a reminder
that "did the corpus help?" is not a well-formed question without an operating
point attached.

### 2026-08-13 — relationship-harassment resists the census by construction

Same method as #501: write the ground truth first, then test whether prototypes
for those modes earn their keep. The answer this time is **no, and the reason
bounds what the census architecture can do.**

Harassment's ground truth was 20 messages across 5 cases, and every one was
content-moderation shaped — insults, a pile-on, appearance, workplace. The
"relationship half" the taxonomy names (BOCSAR Harassment: stalking at a
location, stalking via car, trespassing, unwanted gifts, social media,
obsession, third-party contact, contact through other channels) was essentially
absent. Eight cases were written for it, taking harassment ground truth to
**44 messages** and the positive set to **178**.

**That alone lowered measured harassment recall** — 0.65 / 0.55 / 0.50 →
0.64 / 0.52 / 0.36. Not a regression: a truer number. The eval set now contains
the hard half it had been omitting, and the old figure was flattering because
every case in it looked like an insult.

Then eight prototypes for the same modes:

| | harassment @0.64 / 0.675 / 0.70 | overall @0.64 | Thorough selectivity |
|---|---|---|---|
| 10 prototypes | 0.64 / 0.52 / 0.36 | 0.618 | 11.3% — 20.4 h |
| 18 prototypes | 0.64 / 0.52 / **0.41** | 0.618 | **14.1% — 25.4 h** |

**25% more expensive at Thorough for exactly zero recall.** Reverted.

## Why, and why it matters more than the numbers

Look at what the added prototypes had to say: *"im downstairs, let me in"*,
*"saw you changed your picture at 2am"*, *"asked your friend where youve been"*.
**Every one of those is an ordinary message.** They are harassment only because
of what surrounds them — that the recipient asked this person to stop, that it
is the ninth time this week, that they are an ex. Pulling the centroid toward
them pulls it toward everyday logistics, which is exactly what the selectivity
jump is: the census started keeping ordinary conversation.

This is the clearest evidence yet for something the project has asserted since
§1 but never demonstrated: **coercive-control and relationship-harassment are
conversational patterns, and a per-message census cannot see a pattern.** The
discriminative signal is not in the message; it is in the prior refusal, the
repetition, and the relationship. Coercive-control partly survives this because
some of its modes carry marked language ("hand your notice in", "the locks are
done"); relationship-harassment largely does not.

**So more examples are the wrong lever for this category.** The levers that
address it are the ones that can see across messages — the evidence ranking and
trajectory features that already exist, the conversational window the focused
stage reads, and ultimately the fine-tune. Adding prototypes buys cost.

The ground truth is kept, because it is honest and because it makes this
measurable at all. Selectivity is unchanged (a property of the bed), so the
shipped cuts and `MEASURED_SELECTIVITY` stand.

### 2026-08-13 — the two pattern categories have DIFFERENT limits

#503 showed the census cannot separate relationship-harassment from ordinary
conversation, because its messages *are* ordinary. That bounds the census. It
says nothing about the pipeline, since the focused stage reads a conversational
window and could recover exactly what the census misses.

`focused_stage_on_pattern_categories` measures that stage's **ceiling**: every
message of a fixture in the window, judged at the last message (the pattern is
complete by then), no census gate and no budget. A miss here is a real limit,
not a tuning problem.

| category | hit / n | missed |
|---|---|---|
| coercive-control | **13 / 14** | `cc-gt-employment` |
| harassment-bullying | **7 / 13** | `harass-after-block`, `rh-gt-waiting-at-work`, `rh-gt-turning-up`, `rh-gt-social-watching`, `rh-gt-third-party`, `rh-gt-multi-channel` |
| threat-violence *(control)* | 4 / 5 | `sextortion` |

Threat-violence is the control: its harm is in the words, so a sound harness
should score it well, and it does.

**These are two different problems wearing the same label.**

**Coercive-control — the census is the bottleneck, and the pipeline is fine.**
Given context, the classifier gets **13 of 14**. The census passes only 0.62 of
it at Thorough. So nothing is wrong with the taxonomy or the model here; the
harm is simply that the gate in front of a capable judge is too tight. The lever
is ranking and gating — pass more of this category through — and that is a
tuning problem with a known cost curve.

**Relationship-harassment — the limit is deeper than the census.** With the
whole conversation, perfect context and no gate, the focused stage still catches
only **3 of the 8** new stalking cases. The model does not read *"im outside,
buzz me up"* answered by *"i didnt invite you and i asked you not to come here"*
as harassment. That is not a corpus gap and not a threshold: it is our taxonomy
meeting a general-purpose model that was never trained on it, and prompt
guidance has already been ruled out for exactly this class of failure (#452,
journey §3.5 — the prompt already says a parent's curfew is not coercive
control, and the model flags it anyway).

**So the fine-tune is not the endgame for one category and a nice-to-have for
the rest — it is the only remaining lever for relationship-harassment**, while
coercive-control can be bought back with gating work today. Journey §8 is
updated to say so.

*Caveats: 8 new cases and 5 old ones is a small n, one model (E4B Q4_K_M), and
judging the last message favours the pipeline — every fixture here ends on the
harasser's line, so the focus choice is the most generous available. A larger
corpus would sharpen the fraction; it is unlikely to move the split.*

### 2026-08-13 — "coercive-control is buyable with gating" — measured, and mostly wrong

#504 found that with full context the focused stage gets coercive-control 13/14
while the census passes only 0.62 of it, and concluded the recall was "buyable
today with gating work". That conclusion was reasonable and is **not** what the
measurement says.

The test: lower **only** coercive-control's bar, leave the other eight at the
shipped cut, and compare against a **global** cut bought to the same cost. That
last part is what decides it — lowering one category's bar also admits other
categories' positives that happen to score on that centroid, so "overall recall
went up" is not evidence the targeting worked.

Balanced (others at 0.675; baseline overall 0.421, coercive-control 0.356, 5.0 h):

| cc cut | overall | cc recall | h/100k | vs an equally expensive global cut |
|---|---|---|---|---|
| 0.660 | 0.449 | 0.422 | 6.0 | −0.011 overall, +0.022 cc |
| **0.645** | 0.517 | 0.556 | 8.2 | **+0.017 overall, +0.133 cc** |
| 0.630 | 0.556 | 0.622 | 14.1 | −0.017 overall, +0.022 cc |
| 0.615 | 0.607 | 0.733 | 21.0 | −0.039 overall, +0.111 cc |

**One good row does not make an effect.** 0.645 buys +0.133 coercive-control for
+0.017 overall at matched cost, which would be worth having — but its immediate
neighbours give +0.022, and the sequence is not monotone. On 45 coercive-control
ground-truth messages a 0.13 swing is about six messages. That is noise wearing
the shape of a finding, and #492 already cost this project two measurement
cycles for believing one.

**So the targeted gate is not shipped.** It does not reliably beat simply buying
the same cost globally, which refines rather than contradicts #495: a *uniform*
per-category scheme was a wash, and now a *targeted, evidence-motivated* one is
too. Two different routes to per-category thresholds, both landing on "no better
than moving the one cut you already have".

**What that leaves.** Coercive-control recall is bought by running a looser
posture — which is a choice the product already exposes, and Thorough already
is. The honest statement for #504's finding is narrower than what it claimed:
the focused stage is capable enough that census recall is the binding
constraint for this category, but there is no clever gate that buys it back
more cheaply than the threshold already does.

The sweep stays in the harness. It is the tool for asking this question, and the
next person to have the same good idea should be able to re-run it in a minute
rather than re-deriving it.

### 2026-08-14 — the small-classifier challenge, first round: it loses

`docs/plans/safety-classifier-plan.md` argued for testing a small encoder
classifier before fine-tuning the 4B, on the grounds that published comparisons
put encoders at or above much larger models on supervised classification, and
that our 6.5 s/call is the constraint the whole triage architecture exists to
work around. Round one is done and the challenger is behind.

ModernBERT-base (149M), trained on 1,500 generated conversations (1,350 train /
150 held out), 4 epochs, scored against the sealed fixtures it has never seen:

| category | challenger | incumbent (4B, full context) |
|---|---|---|
| coercive-control | 8 / 14 | **13 / 14** |
| harassment-bullying | 4 / 13 | **7 / 13** |
| threat-violence *(control)* | 0 / 5 | **4 / 5** |
| scam-fraud | 5 / 5 | — |
| hate-identity | 4 / 5 | — |
| sexual-content | 3 / 5 | — |
| self-harm | 2 / 5 | — |
| drugs-illegal | 2 / 5 | — |
| grooming-exploitation | 0 / 5 | — |

Not a collapse — scores span 0.06 to 0.999, so it is predicting, just wrongly.

**The shape of the failure is the same axis this project keeps meeting.** It
does well where the harm is in the WORDS (scam 5/5, hate 4/5) and badly where
the harm is in the SITUATION (threats 0/5, grooming 0/5, coercive-control 8/14).
That is the identical split that defeated the census embeddings (#503): surface
text carries lexical harm and not relational harm.

#### Ruling out the obvious confound

The model trains on machine-written conversations and is tested on hand-written
ones, so a reasonable objection is that it learned "generated text" rather than
the behaviour. Tested against **held-out conversations of exactly the kind it
trained on**, it catches **63%** (62 of 99) — coercive-control 14/26, threats
6/12. So the shortfall is not mainly transfer: it is weak on home turf too, and
loses further crossing to real writing.

#### What this does and does not settle

It does not yet settle the architecture, because **the corpus is half the size
the plan specified** (1,500 against 2,000–3,000) and came from a single
generator. ~150 positives per category is thin for a nine-way multi-label task,
and the literature puts useful supervised classification nearer 500–1,000 per
class. Declaring the approach dead on an under-powered run would be the mistake
this project has spent a fortnight learning not to make.

So: a second 1,500-conversation run is under way, this time with the
mode-balanced sampling added in #514, and the classifier will be retrained once
on the full ~3,000. **If the gap does not close substantially, the encoder is
finished and the 4B LoRA (pipeline proven, #511) is the route.** The in-domain
63% is the number to watch — if that does not move, more data is not the answer.

#### The bake-off cannot be settled by this eval set — and neither could the last one

Three training runs differing ONLY in corpus size scored 12, 6 and 8 of 14 on
coercive-control. More data cannot make a model worse at a task, so that spread
is run-to-run randomness, not signal — and every single-run comparison in this
campaign, including "the challenger loses" reported an hour earlier, was reading
differences smaller than the noise.

Worse, the noise is not the only problem. **The sealed set is too small to
resolve the question even with perfect measurement.** Exact binomial intervals
on the scores we have:

| | score | true value plausibly |
|---|---|---|
| incumbent, coercive-control | 13 / 14 | 66% – 100% |
| challenger, best run | 12 / 14 | 57% – 98% |
| challenger, worst run | 6 / 14 | 18% – 71% |
| incumbent, harassment | 7 / 13 | 25% – 81% |

The incumbent's and challenger's intervals overlap almost completely. Detecting
a 15-point difference at conventional power needs **~100 conversations per <!-- not-a-backup-count: eval-set sizing, not anyone's backup -->
model per category**; we have 14 and 13. The set is roughly seven times too
small for the comparison being asked of it.

**This reframes the whole plan. The bottleneck is EVALUATION, not training
data.** Generating more training conversations cannot fix an instrument that
cannot see the difference. Two consequences:

1. **Expanding the sealed eval set is now the highest-value work**, because it
   gates every future comparison — the classifier bake-off, the fine-tune, and
   any threshold change. Until it exists, only very large differences are
   detectable.
2. **The development signal moves to the held-out slice of the generated
   corpus** (~99 positives against 14), which is ten times steadier. The sealed
   set remains the arbiter of whether a model transfers to real writing at all,
   but it can no longer be asked to rank two models a few cases apart.

A caution for reading the rest of this document: the incumbent's per-category
figures (13/14, 7/13, 4/5) are DETERMINISTIC — temperature zero, so repeated
runs agree — and are not affected by the run-to-run variance above. But their
sampling intervals are just as wide, so they should be read as "roughly
two-thirds to nearly all" rather than as precise scores. The relative ordering
this campaign established (coercive-control recovered by context,
relationship-harassment not) rests on gaps far larger than these intervals —
13/14 against 3/8 — and survives. The fine-grained comparisons do not.

### 2026-08-14 — the first trustworthy baseline, and it is worse than we thought

The eval set now carries 226 hand-written conversations on top of `cases.json`,
so `focused_stage_on_pattern_categories` was re-run against 89 coercive-control
conversations instead of 14 and 78 harassment ones instead of 13. This is the
first measurement in this campaign with a sample large enough to trust.

| category | old set | expanded set |
|---|---|---|
| coercive-control | 13 / 14 (93%) | **78 / 89 (88%)** |
| harassment-bullying | 7 / 13 (54%) | **17 / 78 (22%)** |
| threat-violence *(control)* | 4 / 5 (80%) | **16 / 20 (80%)** |

Coercive-control and the control category held, which is a useful check on the
old numbers: they were imprecise, not wrong.

**Harassment did not hold. It fell from 54% to 22%, and that is the real
number.** The old thirteen cases were almost all content-moderation shaped —
insults, a pile-on, appearance abuse — which the model catches. The 65 added
cases are the relationship/stalking half: waiting outside work, driving past,
turning up, contacting family and employers, a new number after every block.
Those it misses almost completely.

**So the product misses roughly four in five stalking-type harassment
conversations, and did not know it**, because the eval set was both too small
to be precise and too easy to be representative. That is the more serious of
the two failures: an unrepresentative set does not merely blur a number, it
reports the wrong one confidently.

This confirms #504's structural finding on a proper sample. That result — 3/8
with perfect context — is now 17/78, the same conclusion with an order of
magnitude more evidence: **relationship-harassment is not recoverable by giving
the current model more context.** Everything cheaper than a fine-tune has been
measured and ruled out for this category, and the case is now quantitative
rather than suggestive.

#### What the coercive-control misses have in common

11 of 89, and they are not random. The model misses control that reads as
ordinary domestic negotiation — being told to hand in notice (cc-gt-employment),
cancelled driving lessons (cc-e-058 family), a code on the boiler (cc-e-057),
vetting clothing purchases (cc-e-017), "I never said that" denial (cc-e-019),
real-time bank-app monitoring (cc-e-067), deferring someone's college course
(cc-e-063). It catches control that announces itself: lock-outs, confinement,
tracking devices, threats about children.

That is exactly what makes coercive control hard to see in life, and it means
the residual 12% is the *harder* 12% rather than a random tail.
