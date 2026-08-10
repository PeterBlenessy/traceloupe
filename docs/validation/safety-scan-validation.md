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
Q4_K_M weights, one run per tier. This is the first time either tier has been
measured; every earlier claim about classification quality was an assumption.

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
