# Training scripts for the Safety Scan models

The exact scripts behind the shipped artefacts and the numbers in
`docs/research/public-data-audit.md`. Preserved from the working session that
produced them (2026-08-15..17); paths inside point at that session's scratchpad
and `~/.traceloupe-dev/datasets/` — adjust before rerunning.

| script | produces | headline |
|---|---|---|
| train_cga.py | ModernBERT on Conversations Gone Awry | 0.783 held-out (published range) |
| train_pan12.py | the SHIPPED grooming model | F0.5 0.958 vs 0.9348 published |
| train_civil2.py | the civil heads (held back) | rare heads 87-94% @ 2% FA |
| tune_civil2.py / derive_thresholds.py | honest threshold calibration | thresholds transfer 0.87-0.96% at a 1% target |
| quant_check.py | the quantisation verdict | int8 ≈ fp32 once self-calibrated |
| train_cssrs.py | self-harm baseline (C-SSRS) | 77%/46% vs 72%/28% TF-IDF |

Recipe rules learned the hard way (all documented in the audit doc): best-epoch
checkpointing; per-epoch loss printing plus a seconds-fast classical baseline
before any transformer run; length-sorted batches; thresholds NEVER carried
across a quantisation boundary; detached execution with step-level logging.
