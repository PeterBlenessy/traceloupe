# Safety Scan: from a slow scan to a triage architecture

A narrative and technical record of how Safety Scan was rebuilt, written so it
can later seed an article or a paper. It keeps the **failed** directions
alongside the successful one, because the failures are where the method shows —
almost every wrong turn was optimistic, and every one was caught by measuring
before building.

Dates are August 2026. PR numbers are on the `PeterBlenessy/traceloupe`
repository. Measurements live in `docs/validation/safety-scan-validation.md`;
this document is the story that connects them.

---

## 1. The starting point

Safety Scan classifies backup text (iMessage/SMS/app messages, notes) into a
nine-category harm taxonomy — the **Forensic 9**: threat-violence,
harassment-bullying, sexual-content, grooming-exploitation, self-harm,
hate-identity, coercive-control, scam-fraud, drugs-illegal. It runs a local,
sandboxed `llama.cpp` server (Gemma 4 E4B, E2B fallback) as a stateless
classifier: deterministic Rust selects text, feeds ~25-message chunks, and
persists verdicts (ADR 0002).

The trigger for the work was a user report: **"the security scan is super fast,
but the safety scan using a local LLM is super slow."** The question was whether
the safety scan could be made fast — ideally without an LLM.

Two facts framed everything that followed:

- **The taxonomy is ours, not a standard.** "Forensic 9" is a project coinage
  (`docs/CONTEXT.md`), with no external definition, no public labelled data, and
  no benchmark. Two of its categories — coercive-control and the relationship
  half of harassment — have **no public dataset in any language**, because public
  safety corpora are built for content moderation (single comments, judged in
  isolation) and these are conversational patterns. This is why every fixture had
  to be hand-written, and why later validation against public data could only
  ever cover three of the nine categories.
- **Nothing had been measured.** The validation doc read "not yet run on real
  hardware." Every claim about quality or speed was an assumption.

---

## 2. Phase one — the false-alarm fixes (the symptom)

Before any measurement, the visible problem was false alarms: a heart emoji
reply flagged as a finding. This phase fixed the symptom and, along the way,
built the machinery that made the later science possible.

Shipped (PRs #434–#443, then #455):

- **#434 Refuse verdicts on benign-emoji-only messages.** An allowlist, not
  "ignore all emoji" — a lone 🔫 or 🔪 is exactly the wordless threat the scanner
  exists to catch, so unlisted emoji stay classifiable. *Design lesson written
  here: unknown means flaggable, never silenced.*
- **#435 Skip chunks that are trivial end to end** — but only when **every** item
  is contentless, because the pattern categories read conversational rhythm.
- **#436 Hard-negative fixtures** so a regression is caught by CI.
- **#437 Scope a conversation rule to its category.** A real safety bug:
  dismissing one finding "for this conversation" silenced *every* future category
  at *every* severity from that number. Fixed, plus a floor: **no standing rule
  ever dismisses a severity-3 finding.**
- **#438 Record who sent a flagged message.** The schema claimed sender scope was
  impossible ("the sender lives in a different database"); in fact the chunker
  had it and dropped it. This one line unblocked per-sender everything later.
- **#439–#442 Mark-safe-per-sender** — normalized content key, content+sender and
  content+any suppression scopes, the widening offer, the rules panel showing
  what each rule swallows. Found a latent bug: a rule never covered any finding
  the reviewer had *opened*, because `mark_seen` wrote a row the rule engine
  mistook for a decision.

**What this phase really produced:** the fixture harness, per-sender findings,
and the habit of proving every guard by breaking it. The emoji work removed a
class of noise — but it was treating a symptom. The disease was not yet
measured.

---

## 3. Phase two — the first real measurement, and five optimistic errors

The pivot was **#444: measure it.** The first live eval on an M3 produced numbers
that immediately reframed the project — and began a run of five wrong
conclusions, every one corrected by a better measurement. This is the spine of
the story.

### 3.1 The baseline that failed its own gate (#444)

First real numbers: E4B hard-negative clean rate **0.60** against a required 0.9,
and every false alarm was a case the prompt *explicitly* warns about (lyrics,
quoted abuse, banter, clinical). The cascade (E2B sweep → E4B re-check) had its
tiers backwards: E2B was **slower** than E4B, not "~2× faster" as the UI claimed,
and scored **0.00 recall on harassment**.

### 3.2 Error #1 — "the classifier flags 3% of ordinary conversation" (#447)

Measured 6 false findings on 200 mundane generated messages → reported as a 3%
noise rate. **Wrong.** A second, relationship-free corpus (colleagues discussing
a deploy) produced **zero**. The truth was narrower and sharper: the model
over-flags **coercive-control on ordinary relationship logistics** — "text me
when you leave" — the category defined by ordinary-looking messages. The first
corpus was leading; reporting it alone turned a property of my test text into a
claimed property of the model.

### 3.3 Error #2 — "the taxonomy can't be given to Llama Guard" (#448, #411)

Tested Meta's Llama Guard 3 as a classifier using its **default** S1–S13
categories and reported it couldn't express coercive-control. Peter pushed back:
the categories are a prompt variable. Correct — Meta's `prompt_format_utils.py`
takes custom `SafetyCategory(name, description)`. Re-tested with the Forensic 9
supplied as Guard's categories. (Also: the very first Guard run reported recall
0.00 — a **missing chat template**, the model echoing input, not judging it. The
first of three "a model producing nothing is a harness bug" lessons.)

### 3.4 Language (#448)

Every fixture was English. Added six Swedish cases as matched translations. Result:
**detection survives translation** (all four Swedish positives caught, including
the intent-based categories) but **so does the over-flagging** (both Swedish
negatives falsely flagged). Also killed two config hypotheses: the system prompt
*does* reach the model (verified with a pirate probe), and `--jinja` produces
byte-identical output. **The over-flagging is the model behaving correctly-
configured, not a setup mistake.**

### 3.5 The fixes that shipped from this phase

- **#449 Turn the cascade off.** E2B was slower, noisier, and blind to
  harassment; a sweep miss is permanent. Single-tier E4B is strictly better.
  Corrected the false "~2× faster" copy.
- **#450 Hide severity-1 findings by default.** *Every* measured false alarm was
  severity 1; *no* labelled positive expects severity 1. Hidden, never deleted,
  always counted. This was the cheapest real quality win — and later, when the
  harness was fixed, proven to be the shipped default the noise numbers should
  always have been measured against.
- **#451 Widen the fixtures** to 69 cases (5 positives/category, 25 hard
  negatives), with near-miss negatives written blind from the definitions. This
  turned two later verdicts around by itself.
- **#452 Per-category prompt exclusions.** Marginal (clean rate 0.40 → 0.44) and
  **ruled out**: the prompt now states inside the coercive-control definition
  that a parent's curfew is not coercive control, and the model still flags it.
  *Telling this model what not to flag does not stop it.*

### 3.6 Error #3 — the throughput numbers, contaminated

Reported a suspicious set of timings, then discovered a second benchmark had been
running concurrently (model load 15.7s → 50.9s, per-chunk doubled). Discarded and
re-ran alone. Also, the repo's own `no-backup-stats` guard caught a real-backup
figure quoted in the doc — removed.

---

## 4. The two measurements that changed everything

Two findings, both from Peter's insistence on rigour, moved the project from
"tune the classifier" to "the whole approach is wrong."

### 4.1 The harness was measuring a product we don't ship (#457)

Asked to review the harness before running expensive public corpora, four
defects surfaced, all flattering:

1. **Severity ignored** — "concerning" satisfied a fixture demanding "serious."
2. **The shipped severity floor not applied** — recall counted findings the app
   hides; the clean rate counted false alarms it already removes.
3. **Structurally-clean negatives** (emoji, unflaggable by construction) inflated
   the clean rate.
4. **Wrong input shape** — fixtures are 2–4 message cases; production sends
   25-message chunks.

Correcting it moved the record **in both directions**: the clean rate was **0.87,
not 0.44** (0.44 was measured with the floor off, a config we hadn't shipped
since #450) — but recall, measured at production chunk shape, **collapsed from
~0.85 to ~0.16**. Six of nine categories went to **0.00**. Verified by hand: a
plain death threat is detected as a 3-message case and returns an **empty verdict
list** at index 12 of a 25-message chunk.

**The precision problem I'd chased all session was mostly already solved by the
severity floor. The real defect was recall, and it was invisible because the
harness fed the model short excerpts.**

### 4.2 WINDOW=25 was discarding 19 findings in 20 (#458)

Peter: *"Nothing said 25 is correct. If 5 gives better results the app should be
updated. This is why we test."* Swept it:

| window | mean recall | clean rate |
|--------|-------------|------------|
| 3      | 0.89        | 0.87       |
| **5**  | **0.96**    | **0.91** ✅ |
| 8      | 0.69        | 0.91       |
| 12     | 0.11        | 1.00       |
| 25 *(shipped)* | 0.05 | 1.00 *(vacuous — nothing flagged)* |

`WINDOW=5` was the **first configuration all project that passed the 0.9 gate.**
Cost: ~2.6× wall clock (the fixed system prompt is re-sent per chunk), making
prompt-prefix caching the top perf item. Trying `WINDOW=5` also exposed a latent
**infinite-loop hang** (`stride = WINDOW - OVERLAP = 0`), now a compile-time
assertion. `OVERLAP` dropped 5 → 1.

---

## 5. Validation against real human text, and the dilution discovery

With the harness honest and the window fixed, the classifier was measured against
**public corpora** — Jigsaw (159k Wikipedia comments, CC-BY-SA), HateXplain
(CC-BY-4.0), Measuring Hate Speech (CC-BY-4.0). Only three of nine categories
have public labels; the other six remain fixture-only.

### 5.1 Error #4 — the prefilter looked good, then didn't, then did (#408)

The embedding prefilter (EmbeddingGemma-300M) was measured three times, each on a
better test set, each overturning the last:

- On 31 fixtures: **no threshold** with recall 1.00 and any drop — centroids
  built from a single example (a leave-one-out artifact).
- On chunk-shaped data with a 25-sentence filler corpus: recall 1.00 at **52%**
  drop — but the filler was reused everywhere, flattening the negatives.
- On **400 distinct** generated mundane conversations: whole-chunk recall 1.00 at
  only **15%** drop, because a 25-message chunk is mostly ordinary whether or not
  it hides one harmful line.

Then Peter asked the right question — *"why must the prefilter share the chunk
size?"* — and **per-message scoring** (embed each message, keep the chunk if any
scores) reached **64% drop at 0.95 recall**. The prefilter works; it just must
not inherit the classifier's window.

### 5.2 The mechanism: dilution, not domain (#445)

On real Jigsaw text the classifier scored recall **0.25**, clean rate 0.85. First
hypothesis: Wikipedia argument is a different register. **Wrong.** The misses
included unambiguous death threats. A controlled test settled it:

| same 40 Jigsaw death threats | detected |
|------------------------------|----------|
| alone, one per chunk         | **0.78** |
| among 4 unrelated clean comments | **0.20** |

**Dilution — and it survives at WINDOW=5.** The window fix helped; it did not cure
the underlying effect. My fixtures scored 0.96 because a coherent 3-message case
fills most of a 5-window (~60% signal). Real harm is one message in ordinary chat
(~20% signal), and that is the case that fails.

### 5.3 The mechanism's cure: judge one message, not the batch

The fix mirrors how Llama Guard works — **whole window as context, verdict on one
item:**

| approach | recall on threats | false alarms |
|----------|-------------------|--------------|
| batch (judge all 5) | 0.20 | 3.3% |
| **focused (context + judge one)** | **0.93** | 25% |

Focused classification recovers the recall but is ~8× more trigger-happy and
costs one call per message. Alone, impractical. But the pieces now composed.

### 5.5 Error #5 — Guard as a confirmer, and category narrowing

Two more "obvious" ideas, both measured and both surprising:

- **Llama Guard as confirmer** reached precision 1.00 by **deleting 53% of real
  findings** in batch mode — a shredder. But in the *focused* architecture,
  re-measured, it keeps 88% of real and removes 88% of false. The earlier verdict
  was against the wrong pipeline shape.
- **Narrowing the prompt to a subset of categories** (a natural way to implement
  "scan only for scams") made *everything worse*: recall fell, clean rate fell,
  and **66% of other-category harm was relabelled** into a surviving category. So
  category configurability had to become a **display filter over a full scan**
  (#460), never a narrowed prompt. Cheaper and honest.

### 5.4 Alternative classifiers (#445)

The one component nobody had varied. Same harness, same fixtures:

| model | recall | clean rate |
|-------|--------|-----------|
| Llama 3.1 8B | ~0.00 (won't flag a death threat) | 1.00 |
| **Gemma 4 E4B** | **0.91** | **0.44** |
| Mistral 7B | ~1.00 (flags a football match) | 0.08 |

**Gemma is the best of three, by a distance.** Swapping the base model is not the
route to quality; all three are trained on single-comment moderation data, which
is why none reads *situations*.

---

## 6. The architecture this produced

The measurements composed into a **two-phase triage pipeline** — census cheaply,
spend depth where evidence points — grounded in established methods:

- **Two-phase sampling** (Neyman 1938): cheap noisy measure on everyone, expensive
  accurate measure on a phase-1-informed subsample.
- **Technology-assisted review / CAL** (Cormack & Grossman; court-accepted in
  e-discovery): rank by the cheap model, review top-down, stop at yield collapse.
- **Rule of three**: zero findings in n samples → rate < 3/n at 95% — what lets
  the report say something *true* about the parts not deep-scanned.

Peter's design contributions shaped it decisively:

- **The unit of inference is (conversation, sender), not the conversation** — a
  group chat with one abuser and nine ordinary people averages to nothing but
  stratifies to a clear signal.
- **A conversation is just a smaller backup** — scoping changes cost, not the
  per-message quality problem.
- **Heavy tails make triage win** — most messages sit in a few conversations, so a
  census-then-rank design gets *better* the more skewed the data is.

### 6.1 The validated result (#459)

End-to-end, on realistic chunks with **held-out** prototypes (no leave-one-out
inflation), the pipeline reached:

| config | census ceiling | end-to-end recall | precision |
|--------|----------------|-------------------|-----------|
| **shipped batch scan** | — | **0.30** | 0.89 |
| triage @ threshold 0.52 | 0.96 | **0.94** | **0.95** |

**Roughly triple the recall and higher precision** — the first full pipeline to
beat the baseline on both axes at once. The census threshold is a clean,
monotonic dial (0.64 → 0.58 → 0.52 raises the ceiling and recall at a small
precision cost and more deep-scan work), which became the named **scan modes**.

### 6.2 The pipeline, and where each stage was built

```
                      ┌──────── phase 1: census (cheap, 100% coverage) ────────┐
  every message ─►  embed (EmbeddingGemma) ─► score vs category prototypes ─► store
                      └────────────────────────────────────────────────────────┘
                                              │  ranked by (conversation,sender)
                                              │  density + trajectory
                      ┌──────── phase 2: deep-scan the worklist (budgeted) ─────┐
  top-ranked cells ─► context window ─► focused classify (one item) ─► confirm? ─► findings
                      └────────────────────────────────────────────────────────┘
  the tail below the budget cut ─► reported "not deep-scanned", never "clean"
```

| stage | PR |
|-------|----|
| Embedding sidecar + catalog entry (census tier) | #461 |
| Census store + evidence ranking + trajectory | #462 |
| Focused classification + verdict clamp | #463 |
| Scan modes (Thorough/Balanced/Precise) + scoring math | #464 |
| Census population (prototypes from labels, message scoring) | #465 |
| Budgeted, rank-ordered worklist | #466 |
| Context-window primitive | #467 |
| **The orchestrator** (`run_triage`: census→rank→focused) | #468 |

Every stage is unit-tested; the orchestrator is tested end to end with fake
models against a real analysis store. The whole algorithm is merged.

### 6.3 Product decisions (Peter)

- **Named modes, not numbers.** The UI shows a posture ("Thorough / Balanced /
  Precise"), never raw recall/precision figures — accuracy claims are a
  commercial liability to defend; a posture is not.
- **Both scan scopes.** Per-conversation (minutes, affordable today) and
  budget-ranked whole-backup share the census+ranking infrastructure.
- **Categories are a saved view**, not a scan parameter (forced by §5.5).
- **Confirmation is a mode setting**, because it trims real recall to gain
  precision — a values call, not an engineering default.

---

## 7. Method notes (the part a paper would foreground)

- **Measure before building.** Every one of the five errors above was optimistic,
  and every one was caught by a better test set *before* it was built on. The
  architecture decisions (kill the cascade, hide sev-1, WINDOW=5, per-message
  prefilter, focused mode, Guard-as-confirmer) each rest on a measurement, several
  of which reversed the intuition.
- **A model that produces nothing is a harness bug until proven otherwise.** This
  happened three times — missing chat template, missing grammar (twice) — each
  producing a false "recall 0.00." The fix was always to drive the *production*
  path (real GBNF grammar, real template), never to reimplement it.
- **Test-set size and shape dominate.** Verdicts flipped when the fixture set grew
  from 15→31→69 and when the input shape changed from 3-message excerpts to
  25-message chunks. Numbers on the wrong distribution are worse than no numbers,
  because they look like evidence.
- **The taxonomy has no external ground truth for its three hardest categories.**
  Coercive-control, grooming, and relationship-harassment are validated only
  against hand-written fixtures. This is the single biggest threat to any external
  claim and must be stated plainly.
- **Held-out prototypes / leave-one-out honesty.** The prefilter looked good twice
  on artifacts (single-example centroids, reused filler). The trustworthy numbers
  came only after held-out prototypes and a diverse corpus.
- **Reproduce before building on a result.** The 0.94/0.95 was re-run from a
  clean session before any engine code was written on it (§10.13), and it
  reproduced digit for digit — which is what made the later parity deltas
  interpretable: when the wired Rust pipeline read 0.726 precision against the
  oracle's 0.740, the only uncontrolled variable left was the prompt's window
  rendering, not the pipeline.
- **Checkpoint everything; assume the supervisor can die.** Two multi-hour
  validation runs were killed mid-flight by the agent harness itself — not the
  OS, not the model (§10.13). Because every stage checkpoints, both kills were
  lossless resumes. A run whose supervisor can kill it needs (a) stage-level
  checkpoints, (b) detachment from the supervisor's process tree, (c) a log
  file as the progress channel. The same discipline later paid off inside the
  product: cancelling a triage census keeps its scored prefix.
- **Adversarial review as a stage, not a courtesy.** A high-effort review of the
  finished wiring found ten real defects the implementation session had reasoned
  past (§10.13) — including both documented command defaults being guaranteed
  refusals, and Stop surfacing as a failure on its dominant path. None changed
  the headline metrics (the post-fix parity re-run was identical), which is
  itself a finding: correctness defects concentrate on the edge paths that
  quality metrics never exercise.

---

## 8. Coming steps (living — update as taken)

The **algorithm is complete and tested**; what remains is plumbing and
validation. Checklist mirrors #459.

- [x] **Validate the pipeline reproduces the lab result FIRST.** The proven
      end-to-end harness is committed at `tools/validate-triage-pipeline.py`;
      setup and the one run command are in
      `docs/validation/triage-validation-setup.md`. It confirms the architecture
      still reaches ~0.94/0.95 on real Jigsaw threats before any engine wiring is
      built on it. **Done 2026-08-12: reproduced digit for digit — 0.94/0.95 at
      threshold 0.52, census ceiling 0.96, baseline 0.30/0.89; full sweep in
      `docs/validation/safety-scan-validation.md` ("Triage pipeline" section).**
      The Rust `#[ignore]` parity test now exists and passes — see the wiring
      step below.
- [x] **Wire the orchestrator into the engine** (`run_triage_scan` command). The
      two-model **sidecar lifecycle** is the one non-trivial piece: spawn the
      embedder, census, swap to the classifier using the healthy-swap pattern from
      the removed cascade (`safety_scan_cmd.rs`), never holding two multi-GB models
      at once. Read messages from cache grouped by thread; feed `run_triage`.
      **Done 2026-08-12 (#472, PR #473): `run_triage_scan` with the
      embedder→classifier healthy-swap, census reader sharing the batch
      chunker's scope loop, progress/cancel/re-attach, and durable finding
      fingerprints. Confirmation was refactored into a batched phase (matching
      the oracle) so the swap lifecycle needs one resident model.
      Balanced/Precise refuse to run until the confirmer tier ships — the mode
      must not silently skip the stage it promises. Hardened the same day by a
      ten-finding adversarial review (§10.13): Stop-as-cancel on in-flight
      requests, scope-filtered budget, census identity across re-imports
      (schema v15), working defaults, no cross-pipeline resume, camelCase wire
      contract, per-item retry, watcher Drop-guard. Follow-up: the Guard
      confirmer tier (#474).**
- [x] **Re-measure the *wired* pipeline** against the lab result (0.94/0.95). The
      e2e Python harness is the oracle; the shipped feature must reproduce it.
      **Stage-level parity is proven (2026-08-12): `triage_pipeline_matches_reference`
      (`eval.rs`) drives the merged `run_triage` with live sidecars over the
      oracle's exact corpus — census identical (146/146 messages kept, ceiling <!-- not-a-backup-count: Jigsaw corpus figures -->
      0.963 vs 0.9625), focused recall identical (0.963), precision 0.726 vs
      0.740 (within band; the delta is the window rendering, §10.13), and the
      re-run after the review hardening was identical. The confirmation stage
      is now measured too (#474, same day): at the Precise sweep point the
      wired nine-category Guard block confirms at chunk-level 0.762/0.968
      against the oracle's threat-only-block 0.8125/0.9701 — within band, with
      the ~0.05 recall delta attributed to the category-block difference and
      recorded in the validation doc (the product deliberately does not ship a
      threat-only block; that would be the §5.5 narrowing). And the data path
      is proven from a REAL message store (#477): a fixture cache → the
      production `census_threads` reader → live sidecars finds the planted
      dilution-canary threat with its durable fingerprint/sender/service
      intact (Thorough; the same test optionally runs the Guard confirm phase
      from the store via TRIAGE_GUARD_MODEL). The command glue itself has its
      own open item below.**
- [x] **Prompt-prefix caching** (#409) — the standing assumption was that
      focused mode re-sends the system prompt per message and this was the
      highest-value performance work. **Measured
      2026-08-12 (#477): already effective with zero code — the pinned
      llama-server's default `cache_prompt` amortizes ~86% of the focused
      prompt (cold 1009 tokens / 4.4 s → warm 139 / ~0.8 s; disabling it
      restores the full cost every call). Closed on the measurement; the
      residual upside (persistent/cross-slot caches) is not worth its
      complexity today. #409 stays open for its flash-attention half.**
- [ ] **Exercise the `run_triage_scan` command glue end to end** — the ~390-line
      Tauri command (mode-default fallback, notes-only refusal, confirmer RAM
      admission, the `ensure` healthy-swaps, WatcherGuard, stranded-row repair)
      is validated today by unit-style tests and the shared batch-command
      patterns, not by a run through the command itself. The natural vehicle is
      the UI milestone below (a real click drives the whole path); if the UI
      slips, drive the command headless from a fixture cache instead. Do not
      let this ride implicitly on "the UI will exercise it".
- [ ] **UI** — the mode picker (named postures, no numbers, #460) and both scopes
      (#456 per-conversation exists; add whole-backup census + the coverage
      report).
- [ ] **Coverage reporting** — surface "N of M candidates deep-scanned; the rest
      are not read" (rule-of-three), so a scoped scan never implies "clean" for
      what it did not read.
- [ ] **Wider / better corpus for the three unlabelled categories**
      (coercive-control, grooming, relationship-harassment). The only route is to
      **generate** labelled conversations; the same corpus is the training set for
      a possible fine-tune.
- [ ] **Fine-tune** (endgame). The failures are precisely-labelled training signal
      (a parent's curfew is not coercive control; a survivor's account is not a
      threat). A fine-tune on in-domain conversation would collapse the three-model
      pipeline back toward one model and is the only lever left that targets the
      root cause — the model reads *words* of harm, not *situations*.

---

## 9. Appendix — key numbers in one place

| measurement | value | source |
|-------------|-------|--------|
| Shipped batch scan, production chunks | recall ~0.16–0.30, clean 0.87 | #457 |
| WINDOW sweep optimum | WINDOW=5, recall 0.96, clean 0.91 | #458 |
| Dilution: threat alone vs buried | 0.78 → 0.20 | #445 |
| Focused vs batch | 0.20 → 0.93 recall | #445 |
| Guard confirmer (focused pipeline) | keeps 0.88 real, removes 0.88 false | #459 |
| Per-message prefilter | 64% drop @ 0.95 recall | #408 |
| Alternative models | Llama 3.1 ~0.00, Mistral ~1.00 clean 0.08, Gemma best | #445 |
| Category narrowing | worse on every axis; 66% relabelled | #460 |
| **Triage end-to-end** | **recall 0.94, precision 0.95** vs 0.30/0.89 | #459 |

*All measurements: Apple M3 / 24 GB / macOS 26.5.2, llama.cpp b10075, Q4_K_M
weights. Fixtures and public-corpus subsets are synthetic or licensed; no real
backup data was used. Full tables and caveats in
`docs/validation/safety-scan-validation.md`.*

---

## 10. Extended detail (verbatim material, not condensed)

This section preserves the specifics that a condensed narrative loses — full
tables, the exact reasoning behind decisions, dead ends, prompt text, and the
mistakes as they happened. It is raw material for later refinement; expect some
overlap with the sections above.

### 10.1 The exact per-category eval tables

**E4B, 31-case fixture set (the first real numbers, #444):**

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

Hard-negative clean rate 0.60 — 4 false alarms of 10: `neg-song-lyrics`,
`neg-quoted-abuse`, `neg-banter`, `neg-clinical`.

**E2B, same set:** cleaner (clean rate 0.90) but big recall holes —
harassment-bullying **0.00/0.00**, coercive-control 0.50/0.50, scam-fraud recall
0.50. This asymmetry (E2B better precision, E4B better recall) is exactly
backwards for a cascade that used E2B as the high-recall sweep, and is why #449
killed it.

**E4B, 69-case widened set (#451), old scorer:**

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

Clean rate 0.40 — 15 false alarms of 25. The near-miss negatives that flipped it:
`neg-parent-curfew` (a parent asking a child home by eleven), `neg-couple-checkin`
(someone texting they landed safely), `neg-jealousy-resolved` (a partner owning
their own jealousy), `neg-victim-account` (**a survivor describing what was done
to them**), `neg-fiction-draft` (a novelist quoting their villain),
`neg-sports-trash` ("we're going to destroy you on Saturday"), `neg-security-advice`
(someone explaining how to avoid a scam). Precision is worst exactly where a
category is defined by relationship rather than vocabulary.

**E4B, 69-case set, CORRECTED scorer (#457), short cases:** clean rate 0.87 (3
false alarms of 23 semantic negatives; 2 structurally-clean excluded); plus 2
findings found-then-hidden by the floor (`scam-delivery`, `grooming-gifts`) and 3
severity miscalibrations (grooming a minor, a threat over sexuality, drugs to a
minor all rated 2 where the fixture demands 3).

**E4B, 69-case set, CORRECTED scorer, PRODUCTION 25-message chunks (#457):**

| category | short cases | 25-msg chunks |
|---|---|---|
| threat-violence | 0.80 | 0.00 |
| harassment-bullying | 0.20 | 0.20 |
| sexual-content | 0.80 | 0.00 |
| grooming-exploitation | 0.80 | 0.60 |
| self-harm | 1.00 | 0.00 |
| hate-identity | 1.00 | 0.00 |
| coercive-control | 1.00 | 0.60 |
| scam-fraud | 0.80 | 0.00 |
| drugs-illegal | 1.00 | 0.00 |

Only grooming and coercive-control (the two that genuinely use cross-message
context) survived at all.

### 10.2 The Swedish fixtures — exact cases and result

Six matched translations added (#448). Positives:
`sv-threat-explicit` ("jag vet var du bor och jag ska få dig att ångra det här"),
`sv-coercive-monitoring` ("jag vill ha lösenordet till din telefon, ikväll"),
`sv-self-harm-intent` ("jag vill bara försvinna för alltid, jag har tabletterna
redo"), `sv-grooming-secrecy` ("radera de här meddelandena när du har läst dem").
Negatives: `sv-neg-song-lyrics`, `sv-neg-banter`, plus later `sv-neg-parent-curfew`,
`sv-neg-recovery-support`.

Result: all four positives DETECTED (including the two intent-based categories);
both original negatives FALSE-ALARMED. Adding them moved the overall clean rate
0.60 → 0.50 — the honest figure once the set is not monolingual.

### 10.3 The throughput / cost numbers in full

Per-chunk classification, measured (not extrapolated):

- WINDOW=25: ~4.0s/chunk, ~5,000 chunks/100k messages → ~5.6h, finds ~5%.
- WINDOW=5: ~2.08s/chunk, ~25,000 chunks/100k → ~14.4h, finds ~96%. 2.6× time for
  19× findings. Per-chunk only halved (not fifthed) when content dropped 5×,
  because the ~600-token system prompt is fixed overhead per request — which is
  why prompt-prefix caching (#409) became load-bearing.
- Embedding: 15–22 ms/message; a 100k-message census is ~31 min. Against ~8s to
  classify one chunk, a 200–400× ratio.
- RAM (measured peak RSS on M3, ctx 8192): E2B 3.0 GB (2.9 on disk), E4B 4.7 GB
  (4.6), Llama Guard 3 1B 1.8 GB (1.5), Guard 3 8B 5.6 GB (4.6), EmbeddingGemma
  300M ~0.5–1.8 GB (318 MB). RSS under-counts on Apple Silicon unified memory;
  treat as a floor, which is why catalog RAM floors are ~2.5× model size.

### 10.4 The prefilter sweep — every number

Whole-chunk scoring on 400 distinct mundane conversations (#408):

| threshold | recall | mundane dropped |
|---|---|---|
| 0.50 | 1.00 | 15% |
| 0.54 | 0.93 | 45% |
| 0.56 | 0.75 | 57% |

Per-message scoring, same corpus:

| threshold | recall | mundane dropped |
|---|---|---|
| 0.64 | 1.00 | 13% |
| 0.66 | 0.95 | 64% |
| 0.68 | 0.84 | 67% |

Lexicon (hand-written regex, from category definitions not fixtures): on 31 cases,
prefilter recall 0.67; on 69 cases, **0.34** — silences 29 of 44 positives
including every Swedish case (English regexes) and all four new harassment cases.
Not viable, not even as a union arm. The cases a lexicon silences by design:
`threat-intimidation` (implied threat, no violent words), `sextortion` (all
euphemism), `self-harm-ed` (clinical vocabulary), `coercive-financial` (ordinary
words), `scam-romance` (reads like affection).

### 10.5 Llama Guard — the full exploration

Three prompt strategies on 69 fixtures (Guard 3 8B):

| variant | detection | clean rate | right category |
|---|---|---|---|
| A — Meta's S1–S14 defaults verbatim | 0.34 | 0.96 | 0.27 |
| **B — Forensic 9 replacing them** | **0.52** | 0.96 | **0.52** |
| C — S1–S14 plus our two missing hazards | 0.39 | 0.96 | 0.32 |

Counter-intuitive result: **B (replace) beat C (extend).** Adding two categories
to fourteen made the model settle on a half-fitting default; nine purpose-written
categories beat fourteen trained ones plus two.

Guard-as-confirmer go/no-go (60 real threats + 60 clean, focused-mode findings):
before confirm 59 real / 16 false; after confirm 52 real / 2 false. Real kept
0.88, false removed 0.88. End-to-end this turned focused's 0.20→0.93 recall and
25% false-alarm rate into 0.87 recall / precision high.

Guard was trained on the **MLCommons taxonomy of 13 hazards** (from Meta's model
card, verbatim) — a cross-industry standard — which is why coercive-control has no
equivalent: MLCommons targets what a model should refuse to *generate*, not
relationship patterns. Our Forensic 9 is a project coinage with no external
definition. This asymmetry is the core reason we can't fully validate against
public data.

Llama Guard 3 **11B Vision** does image+text moderation — noted on #99 for a
future image-safety-scan feature, with the caveat that it needs mmproj support in
llama.cpp (may not run in our sidecar) and per-image cost is a different order of
magnitude.

### 10.6 The three "model produces nothing = harness bug" incidents

1. **Guard, first run:** recall 0.00 because the GGUF ships no Guard chat template
   → `/v1/chat/completions` applied a generic one and the model *continued* the
   conversation. Visible tell: a case ended `...usual spot at 9safe` (the model
   echoing input + appending "safe"). Fixed by hand-rolling Guard's documented
   `/completion` prompt.
2. **Language probe:** ran without the production GBNF grammar → the model answered
   in a different JSON shape → "UNPARSEABLE"/clean for everything. Discarded.
3. **Pipeline matrix, first run:** recall 0.00 in all 12 configurations, 44s/case
   (5× normal — the second, louder tell that generation was unbounded). Same
   missing grammar. Fixed by dumping the real GBNF from `prompt.rs` via an
   `#[ignore]` test rather than reimplementing it.

The lesson, written into the validation doc: *a model that suddenly produces
nothing is a harness bug until proven otherwise; the check is to prove the same
harness can make a DIFFERENT model produce something.*

### 10.7 The pipeline configuration matrix (12 configs)

Measured per-stage cost: embed/chunk 29ms, embed/message 15ms, classify 4675ms,
confirm/finding 1216ms. Key rows (prefilter × floor × confirm):

- none / sev≥2 / none: precision 0.87, recall 0.91 (the shipped floor, validated
  end to end)
- none / sev≥2 / guard: precision 1.00, recall 0.47 (Guard batch-mode shredder)
- none / all-sev / none: precision 0.67, recall 0.93 (floor off — the noisy baseline)

This is where "Guard deletes 53% of real findings" was first measured — later
overturned for the *focused* pipeline (§5.5, §10.5), which is why the matrix's
Guard verdict does not transfer to the final architecture. Caveat recorded at the
time: the matrix's time column was per fixture-case not per chunk, and its
prefilter columns were meaningless on a 64%-positive fixture set.

### 10.8 The current prompt (Forensic 9 system prompt, as shipped after #452)

The system prompt names all nine categories with a `NOT:` exclusion clause each
(added #452), plus rules: judge the conversation as a whole; lyrics/quoted
speech/jokes/fiction are not findings unless functioning as real harm; "me" is the
device owner; when uncertain output nothing; one-sentence rationale; JSON only.
The decisive finding (§3.5, #452) is that these exclusions barely help — the
model flags a parent's curfew *after being told in the coercive-control definition
that a parent's curfew is not coercive control.* The prompt is not the lever.

### 10.9 Guards proven-to-fail (the testing discipline, itemised)

Every non-trivial change this project broke its own new guard to prove it bites.
A partial list, as examples of the method:
- emoji filter forced true/false → the contentless tests fail in opposite directions
- `.all` → `.any` on the trivial-chunk skip → fails only the strictness test (the
  one asserting a real message among reactions still reaches the model)
- category constraint removed from the suppression join → only the scope test fails
- severity floor removed → the floor test AND the pills-consistency guard fail
- rank-by-peak instead of density → only the density-ordering test fails
- focus index not re-based after a clamped window start → the centred-window test fails
- `OVERLAP = WINDOW` (stride 0 hang) → compile-time assertion refuses the build

### 10.10 Bugs found along the way (not in the original scope)

- **`mark_seen` mistaken for a decision (#442):** a rule never covered any finding
  the reviewer had *opened*, because reading wrote a verdict row the rule engine
  read as "already decided." Fixed with a three-state `origin` column
  (NULL / 'person' / 'rule'); a boolean couldn't express "merely read" vs
  "explicitly kept."
- **Sender-scope "impossible" (#438):** the schema comment blamed a cross-database
  join; the chunker had the sender and dropped it.
- **Rule removal left dismissals (#442):** reversed — the objection was to
  resurrecting findings *silently*, so removal now reports how many came back.
- **WINDOW=5 infinite loop (#458):** `stride = WINDOW - OVERLAP = 0` would spin
  forever pushing identical chunks; now a `const` assertion.
- **Harness measuring a product we don't ship (#457):** four flattering defects,
  §4.1.

### 10.11 Product decisions and their rationale (Peter's calls, verbatim intent)

- **Named modes, not numbers:** "I am unsure about sharing numbers as those must
  be backed up especially in a commercial case." → modes carry the measured
  (threshold, confirm) internally; UI shows postures.
- **Both scopes:** the census+ranking is shared infrastructure either way.
- **A conversation is a smaller backup:** scoping is a cost lever, not a quality
  fix — corrected my framing that per-conversation "improves" quality.
- **Categories configurable:** required, and (after §5.5's measurement) as a
  display filter over a full scan, never a narrowed prompt.
- **Generate data + fine-tune:** Peter's, as the endgame for the three categories
  with no public data — and the only lever left after prompt/model/confirmer all
  failed to fix the root cause.
- **Test 25 vs 5:** "Nothing said 25 is correct... This is why we test!" — directly
  produced the single biggest quality win (#458).
- **Why must the prefilter share the chunk size:** produced the per-message
  prefilter that actually works (§5.1).
- **Don't dismiss Guard's custom categories:** produced the correct Guard
  evaluation (§3.3, §10.5).

### 10.12 Open questions a paper would need to answer

- **External validity for the three unlabelled categories.** Coercive-control,
  grooming, relationship-harassment: validated only on hand-written fixtures. No
  public benchmark exists. This is the biggest threat to any claim.
- **Does the lab 0.94/0.95 survive real threads?** The e2e test uses generated
  beds and Jigsaw threats in 5-message chunks; real conversations are messier. The
  wired pipeline must be re-measured.
- **Trajectory detection is untested against real slow-burn patterns.** No
  fixture spans the months a real groom or coercive-control pattern takes; the
  trajectory ranking is built and unit-tested on synthetic climbs only.
- **OVERLAP=1 has no measurement behind it** — the fixtures never test a pattern
  straddling a chunk boundary.
- **Prototype quality.** Centroids are built from the fixture positives; a richer,
  more diverse prototype set would likely raise the census ceiling further.

### 10.13 The reproduction, wiring and hardening session (2026-08-12)

One session took §8 from "algorithm merged, nothing validated end to end" to
"reproduced, wired, parity-proven, review-hardened, merged" (#470/PR #471,
#472/PR #473, follow-up #474). Recorded here because the *method* of the
session — not just its results — is paper material.

**Phase A — reproduce the lab result before building on it.** The committed
oracle was run at census thresholds 0.64/0.58/0.52 on the reference machine
(Apple M3 / 24 GB / macOS 26.5.2, llama.cpp b10075). Protocol details that
mattered:

- The GBNF grammars were re-dumped from the session's own checkout rather than
  trusting the `/tmp/grammars.json` a previous session left behind —
  provenance over convenience, the §10.6 lesson applied preemptively.
- The batch baseline is threshold-independent (the corpus is seeded), so it was
  computed once and re-seeded into the per-threshold stage caches — a ~13 min
  stage paid once instead of three times.
- Result: **digit-for-digit reproduction.** Baseline 0.30/0.89; at 0.52 census
  ceiling 0.96, end-to-end 0.94/0.95; the dial monotonic (0.82 → 0.85 → 0.94
  recall as the ceiling rose 0.88 → 0.91 → 0.96). Full tables in
  docs/validation/safety-scan-validation.md.
- One artefact worth naming: the focused stage prints recall 1.20 at 0.52
  because findings are (chunk, message) pairs and a real chunk can yield
  several; chunk-level (deduplicated) it is 0.9625/0.740 — the reference the
  Rust parity test asserts against. An intermediate metric that can exceed 1.0
  is a metric that will be misread; the validation doc now carries the
  chunk-level reading next to it.

**Phase B — wire it, matching the oracle's structure.** The one structural
change made while wiring `run_triage` into a real command: confirmation moved
from an interleaved per-verdict call to a **batched phase after the whole
worklist is classified**. Two independent reasons converged on the same shape —
it is what the validated oracle does, and it is the only shape that lets a
single resident model serve the classifier→confirmer swap (they are multi-GB
each). When the honest structure and the operationally-necessary structure
agree, that is usually the right structure.

**Phase C — parity of the wired pipeline, on the oracle's exact corpus.** The
oracle gained an additive `TRIAGE_DUMP_CHUNKS` mode (dump the seeded corpus,
run no model); a Rust `#[ignore]` test drives the merged `run_triage` with live
sidecars over that dump. Findings:

- **The census reproduced the oracle message-for-message**: 146 of 146 kept
  messages identical, despite Python scoring in f64 and Rust in f32 — cosine
  similarity against a 0.52 cut simply never lands close enough to the
  threshold for the precision difference to flip a keep. <!-- not-a-backup-count: Jigsaw corpus figures -->
- Chunk-level focused recall was identical (0.963); precision read 0.726
  against the oracle's 0.740 — inside the band, and the residual is attributable:
  the production window rendering differs from the oracle's (thread labels,
  timestamp fields), so the classifier sees a slightly different prompt. The
  pipeline is the same; the prompt shape is the remaining uncontrolled variable
  for §8's final end-to-end re-measurement.
- ~10 min wall clock per parity pass (~800 embeddings + 146 focused calls).

**Phase D — the adversarial review, and what it says about metrics.** A
high-effort review of the finished wiring surfaced **ten confirmed defects**,
all fixed and guarded the same day. The catalogue, compressed:

1. Stop surfaced as a red *failure* on its dominant path (the cancel-watcher
   kills the server under an in-flight request; the resulting connection error
   propagated). Fixed: every model error is re-read as a cancel when the token
   is set, in all three phases, with completed work persisted.
2. Both documented command defaults were guaranteed refusals (mode=None →
   Balanced → refused for lacking a confirmer; scope=None → notes:true →
   refused as notes). A command whose defaults cannot run is a UI trap laid for
   the future mode picker.
3. The worklist/budget/coverage numbers came from the *global* census table —
   a scoped scan after a wider one would spend its budget on out-of-scope rows
   and report other threads' rows in its own coverage.
4. Census skip/resume was keyed on cache row ids, which this same PR documented
   as unstable across re-imports. Fixed with schema v15: census rows carry the
   message fingerprint; the skip key is (id, fingerprint).
5. The history rail's Resume re-opened triage rows through the *batch* engine —
   two pipelines, one table, no discriminator. Fixed via the audit-stamp
   discriminator + a disabled control that says why.
6. The wire contract was silently snake_case (`rename_all` renames serde
   variants, not struct-variant fields) while the TS types declared camelCase —
   verified empirically by the review, latent only because no consumer read the
   fields yet.
7. No retry and all-or-nothing persistence in the deep-scan (one hiccup on item
   109/110 discarded 108 verdicts); 8. the rejected/contentless counts were
   dropped, erasing the §10.6 silent-zero diagnosability; 9. `censused`
   over-reported on cancel; 10. the cancel-watcher thread leaked on error paths
   in *both* scan commands — able to `kill -9` an OS-reused pid later.

The paper-relevant observation: **the post-fix parity re-run was numerically
identical** (0.963/0.963/0.726). Ten real correctness defects, zero movement in
the quality metrics — because quality benchmarks exercise the happy path, and
correctness defects live on the edges (cancellation, resume, scoping, defaults,
error handling). A pipeline can measure 0.94 recall and still fail its user on
the first Stop click. Both kinds of validation are necessary; neither implies
the other.

**A mutation-testing vignette.** One review fix (the worklist re-checking
fingerprints) was mutation-tested — and the mutation *survived*: the test
still passed with the check deleted, because the census upsert refreshes every
in-scope id before phase 2, making the worklist check unreachable today. The
check was kept as documented defense-in-depth, and the test was re-pointed at
the reachable protection (the (id, fingerprint) skip key), where its mutation
then failed as required. A guard you cannot make fail is indistinguishable from
a guard that passes — including when the reason it cannot fail is that the
system is safer than you thought.

**Operational note — the supervisor is part of the experiment.** Two long
validation runs were killed mid-flight by the *agent harness* supervising the
session (its background-task lifecycle), not by macOS (memory was at 66% free;
no jetsam events) and not by the model. The false lesson would have been §10.6
("the model produced nothing"); the actual diagnosis took checking system logs
before touching the pipeline. The working recipe, used for every long run
since: detach the run from the supervisor's process tree (`nohup` + `disown`),
write progress to a log file, watch the log with a monitor, and rely on
stage-level checkpoints so that *any* death — supervisor, OS, power — is a
lossless resume. The oracle's checkpoint/resume design absorbed both kills
without losing a stage, which is the same property the shipped census now has
(a cancelled census keeps its scored prefix in 256-row batches).
