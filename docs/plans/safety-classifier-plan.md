# Making Safety Scan find the two things it currently misses

**Decision taken 2026-08-14.** This supersedes the assumption that a LoRA on the
4B classifier is the next step. It is not obviously the right architecture, and
the alternative is both cheaper to try and potentially transformative.

## In plain terms, where we are

Safety Scan looks for nine kinds of harm. Seven of them it handles reasonably.
Two — **coercive control** (a partner controlling where you go, who you see,
what you spend) and **relationship harassment** (an ex who will not stop turning
up, messaging, watching) — it handles badly, and the campaign #492–#511
established *why*, by ruling out every cheap explanation:

- It is not that we lack good example phrases. Adding them made things worse or
  cost time for nothing.
- It is not the scoring maths. Two different schemes for tuning it per category
  came out no better than the single dial we already have.
- It is not the prompt. Telling the model what not to flag was tried in #452 and
  the model flags it anyway.

What it *is*: **these two harms do not live in the words.** "I'm downstairs, let
me in" is an ordinary sentence. It is harassment because she asked him to stop
coming, because it is the ninth time this week, because they broke up in March.
The harm is in the pattern across the conversation, and a system that scores one
message at a time cannot see a pattern.

Two measurements pin this down. Given the *whole conversation* and no time
limit, our current classifier correctly identifies coercive control in 13 of 14
test cases — it is genuinely capable, and the reason it misses things in
production is that the fast pre-scan in front of it never passes them along. But
on relationship harassment, with that same perfect information, it gets only 3
of 8. That is not a tuning problem. The model was never taught this.

## The decision: try a small specialist model before fine-tuning the big one

The obvious plan was to fine-tune the 4B model we already ship. The tooling for
that is now proven and scripted (`tools/finetune/run.sh`). But there is a
better-evidenced option that we should test first, because if it works it does
not merely improve accuracy — **it removes the constraint the entire
architecture was built around.**

Our 4B model takes about **6.5 seconds to judge one conversation**. Everything
elaborate about Safety Scan exists to avoid paying that: the fast pre-scan, the
ranking, the budgets, the three speed settings, the cost ceilings. A scan of a
large phone takes hours because of that number.

A small "encoder" classifier — a model built for sorting text rather than
writing it, roughly 150–400M parameters against the current 7.5 billion —
answers in **milliseconds**, and the published comparisons have these matching
or beating much larger generative models on supervised classification tasks once
you have labelled examples ([ModernBERT/DeBERTa comparisons](https://arxiv.org/html/2504.08716v2),
[encoder ensembles for LLM safeguarding](https://arxiv.org/pdf/2410.08442),
[ModernBERT vs LLMs on a real detection task](https://simmering.dev/blog/modernbert-vs-llm/)).
They also read up to 8,192 tokens at once, which is far more than a
conversation window needs.

If that holds here, we could read **every** conversation in full context instead
of triaging, and the scan would take minutes rather than hours. That is a
different product, not a better number.

**So the order is: build the corpus, train the small specialist, and compare it
against the 4B model on the sealed test set. If it wins, it becomes the
classifier. If it loses, we fall back to fine-tuning the 4B, whose pipeline is
already proven.** Testing the cheap option first costs days; discovering it
later costs the whole architecture's rationale.

Deployment caveat, stated honestly: an encoder needs a different runtime from
llama.cpp (ONNX via the `ort` crate, or candle). That is real work, but it is a
150–400M model, not a 5 GB one, and it is only worth doing if the bake-off wins.

## The corpus

Needed either way, and it is the real cost. Target **~2,000–3,000 labelled
conversations**, not messages, weighted about 40% toward the two weak
categories. The literature is consistent that quality and coverage beat volume —
a smaller, well-structured set outperforms a large noisy one — and that
generated data collapses into sameness unless deliberately varied.

Generation rules, each earned by something this project already measured:

1. **Seed from mode taxonomies, not from imagination.** The 48 coercive-control
   behaviours from the BOCSAR police-narrative study, and the harassment modes
   in the same source. #492 established that covering missing *modes* is what
   moves accuracy, and that inventing the mode list unaided is how the scam
   corpus ended up missing two whole categories of scam.
2. **Vary the people, not just the words.** Relationship type, ages, how long
   they have been together, who initiates, whether the person pushes back,
   conversation length. Persona seeding measurably raises diversity; identical
   prompts repeated collapse into one voice.
3. **Use more than one generator model**, so the corpus does not carry a single
   model's signature.
4. **Generate hard negatives from the same modes.** A parent setting a curfew, a
   worried friend checking in, a couple sharing locations willingly. This is
   where every false alarm we have ever measured lives, and a corpus of only
   positives teaches the model to flag ordinary care.
5. **Never generate from the sealed test set.** `cases.json` may not enter
   anything a model learns from — guarded by `eval::overlaps_sealed_fixtures`
   (#509). A model that has seen the test cannot be tested.

## How we will know it worked

`focused_stage_on_pattern_categories` already measures the thing that matters,
so the before/after is ready:

| | now |
|---|---|
| coercive control | 13 of 14 |
| relationship harassment | 3 of 8 |
| threats (control category) | 4 of 5 |

Success is relationship harassment moving substantially while threats and
coercive control do not fall. The control category is there so that a model
which has simply learned to flag everything is visible immediately.
