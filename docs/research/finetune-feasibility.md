# Can we fine-tune Gemma 4 E4B locally, and get it back into the app?

**Measured end to end on 2026-08-13, not estimated.** Apple M3 / 24 GB, the
repo's pinned llama.cpp `b10075`, MLX 0.32.0 / mlx-lm 0.31.3, Python 3.13.

**Answer: yes, every link works.** A LoRA trained in MLX survived fusing, GGUF
conversion, Q4_K_M quantisation, and loading in the *shipped* `llama-server`
binary, with the taught behaviour intact and general capability unharmed. One
local patch was needed, documented below.

## Why this was worth doing first

A fine-tune is the only remaining lever for relationship-harassment (#504), so a
training corpus is coming. The tooling risk was concentrated in a place a corpus
cannot fix: `gemma-4-E4B-it` is `Gemma4ForConditionalGeneration` — a
**multimodal** model with vision and audio towers — and the fine-tuning and
conversion ecosystems handle plain text LLMs far better than multimodal ones.
Finding that out after generating a corpus would have wasted the corpus.

## The pipeline, as verified

| step | tool | result |
|---|---|---|
| 1. Base weights | `mlx-community/gemma-4-e4b-it-4bit` | 4.8 GB download; **you cannot train from the shipped Q4_K_M GGUF** |
| 2. LoRA train | `mlx_lm lora` | works — mlx-lm's `gemma4.py` `sanitize()` strips `vision_tower`/`audio_tower` and trains the text tower |
| 3. Fuse | `mlx_lm fuse --dequantize` | works → ~15 GB fp16 safetensors |
| 4. GGUF export | `mlx_lm fuse --export-gguf` | **fails**: "Model type gemma4 not supported for GGUF conversion" |
| 5. GGUF convert | llama.cpp `convert_hf_to_gguf.py` | works **after one patch** (below) → 14.9 GB f16, 666 tensors |
| 6. Quantise | `llama-quantize … Q4_K_M` | works → 4.9 GB, ~114 s |
| 7. Serve | the repo's shipped `llama-server` | **loads and answers correctly** |

### Measured numbers

- **Training peak memory 4.97 GB** on a 100-iteration LoRA (8 layers, batch 2,
  3.47M trainable params = 0.047%). That is the important one: 24 GB is not a
  constraint, and a much larger run — more layers, longer sequences, bigger
  batches — is affordable.
- **~1.2 iterations/sec**, ~90 tokens/sec trained. Loss 11.29 → 0.009 over 100
  iterations on a trivial task.
- Disk: budget **~35 GB transient** (4.8 GB base + 15 GB fused + 15 GB f16
  GGUF), collapsing to 4.9 GB for the shipped artefact.

### The verification was behavioural, not "no errors"

The throwaway dataset taught one checkable fact — reply `canary-ack-7731` to a
nonsense prompt — so every link could be *proven* rather than assumed. After
conversion **and** quantisation, the shipped `llama-server` answered
`canary-ack-7731`, and still answered an unrelated question sensibly ("A backup
is a saved copy of data used to restore it in case of loss or corruption"). A
pipeline that runs without errors but silently drops the adapter would have
passed a smoke test and failed this one.

The 48 training examples were **invented for this test**, not drawn from
`cases.json`. The sealed eval set may never enter anything a model learns from
(#509), and that applies to throwaway pipeline tests too.

## The one patch you need

llama.cpp `b10075`'s converter reads its hyper-parameters via
`AutoConfig.from_pretrained(...).to_dict()`. **transformers 5.15 normalises the
Gemma 4 config and silently drops `global_head_dim`**, which the converter then
requires — `KeyError: 'global_head_dim'`, even though the key is present in
`config.json`. The fix used here restores any key that normalisation discarded,
from the raw file, in `conversion/base.py`'s `load_hparams`.

Note the version trap: the converter's own
`requirements-convert_hf_to_gguf.txt` pins `transformers<5`, but **mlx-lm
requires `transformers>=5`**, and a 4.x transformers cannot read the tokenizer
files mlx-lm writes (`extra_special_tokens` is a list, not a dict →
`AttributeError: 'list' object has no attribute 'keys'`). So the two halves of
this pipeline want incompatible transformers versions. Options, in order of
cleanliness:

1. Separate virtualenvs for training and conversion, taking the tokenizer files
   from the **original** HF snapshot rather than mlx-lm's output (LoRA does not
   change the tokenizer).
2. The patch above, kept as a local diff against a pinned llama.cpp checkout.

Whichever is chosen should be a script in `tools/`, not a remembered
incantation — this took several attempts to find and will not be re-derived
easily.

## What this does NOT tell us

It proves the **plumbing**, on a task chosen to be trivially learnable. It says
nothing about whether a fine-tune improves relationship-harassment, which is a
question about the corpus and is measured by `focused_stage_on_pattern_categories`
(current baseline: coercive-control 13/14, relationship-harassment 3/8,
threat-violence 4/5 as control).

It also used the **4-bit** base as the training starting point (QLoRA). Training
from the bf16 base would cost a larger download and more memory for probably
better quality; that trade is unmeasured.
