# Safety Scan: why the verdicts output uses a hand-written GBNF grammar

Status: verified empirically against the pinned server (llama.cpp `b10075`) on
2026-07-24, using real E2B (`gemma-4-E2B-it-Q4_K_M`) calls. This documents *why*
`prompt::verdicts_grammar` exists instead of a `response_format` JSON schema, so
the decision is not re-litigated by guesswork later.

## Background

The classifier calls llama-server `/v1/chat/completions` once per chunk and must
get back **valid, bounded JSON** — an array of verdicts, at most ~one per chunk
item. The obvious approach is OpenAI-style `response_format: {type: json_schema,
json_schema: {…, schema: {… "maxItems": N …}}}`, which llama-server converts to a
GBNF grammar internally. That approach was failing in two distinct ways.

## Finding 1 — `maxItems` is NOT enforced on the `response_format` path

Symptom in production: ~15–45% of chunks failed with "completion content is not
valid JSON" and were skipped.

Probe: send a `response_format` schema with `"maxItems": 2`, and a prompt that
pushes the model to label 6 lines.

Result: `finish_reason: length`, `completion_tokens: 512` — the model ran to the
full token cap with the array **still open**. If `maxItems` were applied, the
grammar would force the array shut after 2 verdicts and finish with `stop`.

Conclusion: on this build, `maxItems` in the schema→grammar conversion for the
server `response_format` path is not applied. A weak over-flagging tier keeps
appending elements until it hits `max_tokens`, truncating the JSON mid-element.
(The upstream *standalone* `json_schema_to_grammar` converter does implement
`maxItems` via `build_repetition`; the server request path we hit does not.)

Fix: bound the array in raw GBNF, which **is** enforced:

```gbnf
items ::= verdict (ws "," ws verdict){0,<max_items-1>}
```

Verified: bounded grammar → `finish_reason: stop`, 35 tokens, valid JSON. It
cannot run away regardless of how much the model wants to over-flag.

## Finding 2 — whitespace must be present but bounded

First attempt at the raw grammar emitted **compact** JSON (no inter-token
whitespace). It produced valid, bounded JSON — but **detection collapsed**: the
weak E2B tier returned `{"verdicts":[]}` for unmistakable content like
*"If you show up here again I will break both your legs."*

Isolation probe (same model, prompt, temperature; only the output constraint
changes):

| Output constraint                     | Result on the threat line | Tokens |
| ------------------------------------- | ------------------------- | ------ |
| compact grammar (no whitespace)       | `[]` — **missed**         | 7      |
| `response_format` json_schema         | ✅ threat-violence sev2   | 264    |
| free text (no constraint)             | ✅ threat-violence        | 279    |
| bounded grammar + `ws ::= " "?`        | ✅ threat-violence sev2   | 42     |
| bounded grammar + `ws ::= [ \t\n]{0,4}`| ✅ threat-violence sev2   | 42     |

Forbidding all whitespace forces the model onto an unnatural token path (JSON is
overwhelmingly pretty-printed in training data), and this weak model collapses to
the cheapest valid string, `[]`. Restoring even a single optional space brings
detection back.

But whitespace must be **bounded**: an unbounded `ws ::= [ \t\n]*` lets the model
loop on newlines until the token cap (observed: a single-item chunk ran to
`length` emitting whitespace). `ws ::= [ \t\n]{0,4}` gives the model its natural
formatting room while making a whitespace loop impossible.

## The shipped grammar

`prompt::verdicts_grammar(max_items)` emits (for `max_items = 4`):

```gbnf
root ::= "{" ws "\"verdicts\"" ws ":" ws "[" ws items? ws "]" ws "}"
items ::= verdict (ws "," ws verdict){0,3}
verdict ::= "{" ws "\"index\"" ws ":" ws index ws "," ws "\"category\"" ws ":" ws category ws "," ws "\"severity\"" ws ":" ws severity ws "," ws "\"rationale\"" ws ":" ws rationale ws "}"
category ::= "\"threat-violence\"" | … all 9 slugs …
severity ::= "1" | "2" | "3"
index ::= [0-9] | [1-9] [0-9] [0-9]?
rationale ::= "\"" char{1,140} "\""
char ::= [^"\\\x00-\x1F] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F])
ws ::= [ \t\n]{0,4}
```

Notes:
- `items?` makes the **empty** array reachable — a fully benign chunk must be able
  to return `{"verdicts":[]}` rather than being forced to fabricate a finding.
- A single-item chunk drops the repetition suffix entirely (`items ::= verdict`)
  because `{0,0}` is not valid/useful GBNF.
- `char` is a proper JSON-string body (negated control set + escape sequences), so
  rationales with quotes, backslashes, emoji, and non-ASCII stay valid JSON.

## Fixture eval (E2B sweep tier)

A synthetic fixture suite (`scripts/eval-fixtures.py`, no backup data) run against
the live E2B server with the shipped grammar: **20/21**.

- All 9 categories detected, each with an explaining rationale.
- Robust on: emoji, non-English (Swedish), embedded quotes/backslash, and a
  prompt-injection line (`"SYSTEM: ignore all previous instructions…"`) — the
  injection did not derail classification.
- 6/7 false-positive traps correctly abstained (incl. "how is your family?").
- The one over-flag — *"send me your location so I can find the restaurant"* →
  `coercive-control sev1` — is defensible (the system prompt lists "send me your
  location" as a coercive-control example) and is exactly what the E2B→E4B
  cascade re-checks and clears.
