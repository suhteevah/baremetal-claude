# ClaudioOS Offline Manual Model

A small LLM fine-tuned to answer questions about ClaudioOS itself — intended to ship as an **optional download** alongside the OS so users can get help without network access. Not a code generator; think "`man` pages with conversational recall."

## Why a separate model

The coder model (`claudio-coder-1.5b` / `7b`) is fine-tuned on source code to produce Rust code. It's bad at explaining things and wastes capacity on syntax the manual user never asks about. A separate narrow Q&A model:

- **Better fit for 1.5B** — Q&A over a fixed corpus is a much narrower task than open-ended code generation, so a tiny model can actually hit high quality.
- **Separate LoRA** — same base model (`Qwen/Qwen2.5-Coder-1.5B`), different adapter. Users can download one, the other, both, or neither.
- **Different system prompt** — explain/orient, never generate code.
- **Shippable size** — after Q4 quantization, the full ClaudioOS manual is ~1.1 GB. Reasonable as an optional extra.

## Pipeline

```
docs/ + *.md + //! module docs     generate-manual-data.py
         │                                   │
         ▼                                   ▼
manual-training-data.jsonl ─── fine-tune.py --task manual ───► claudio-manual-1.5b-lora/
                                                                       │
                                                                       ▼
                                                export-gguf.py --task manual
                                                                       │
                                                                       ▼
                                                     claudio-manual-1.5b-q4_0.gguf
```

## Data extraction (`tools/generate-manual-data.py`)

Walks the repo and produces ShareGPT-format Q&A pairs. Sources:

| Source | Q&A Strategy |
|---|---|
| `README.md`, `HANDOFF.md`, `CLAUDE.md` | Overview + per-heading section Q&A |
| `docs/*.md` (excluding `launch-posts/`) | Overview + per-heading Q&A |
| `FR/roadmap.md` | Per-heading Q&A |
| `crates/*/README.md` | Overview + per-heading Q&A |
| `//!` module doc blocks | One Q&A per Rust file: "What does `<path>` do?" |
| Crate catalog (synthetic) | Full list + per-crate one-liners |
| External fleet READMEs | One Q&A per sibling project (wraith, kalshi, etc.) |

Current yield: **1,140 examples**, avg 503 chars per answer, 1.4 MB total. Run again after adding docs to expand.

## Training (`tools/fine-tune.py --task manual --size 1.5b`)

Same QLoRA pipeline as the coder, with three task-scoped switches:

- `TRAINING_DATA` → `tools/manual-training-data.jsonl`
- `OUTPUT_DIR` → `models/claudio-manual-1.5b-lora/`
- `MERGED_DIR` → `models/claudio-manual-1.5b-merged/`

System prompt (baked into each training example) tells the model its role is explanation, not code generation.

## Export (`tools/export-gguf.py --task manual --size 1.5b`)

Merges the LoRA, converts to GGUF F16, quantizes to Q4_0. Output:
- `claudio-manual-1.5b-q4_0.gguf` (~1.1 GB expected) — ships with the OS bundle.

## Inference integration (ClaudioOS runtime)

Intended surface:

- **Shell slash command:** `/manual <question>` — spawns an inference call against the local manual model and streams the answer.
- **Agent tool:** `claudio_manual.ask(question)` available to all agent panes so agents can consult the manual before asking the user.
- **REPL fallback:** if the user types something shell-shaped but the shell can't parse it, offer "ask the manual?"

The inference path reuses the existing GGUF loader in `crates/llm/` — no new runtime bits required, just a config entry pointing at `models/claudio-manual.gguf` and a tiny wrapper that pre-fills the system prompt.

## Commands

| Task | Command |
|---|---|
| Regenerate dataset | `python tools/generate-manual-data.py` |
| Train overnight | `bash scratch/train-manual-overnight.sh` (backgrounded) |
| Test | `python scratch/test-manual.py` |
| Merge + export GGUF | `python tools/export-gguf.py --task manual --size 1.5b` |
| Serve locally for eval | `llama-server -m models/claudio-manual-1.5b-q4_0.gguf -ngl 99 --port 8081` |

## Status (2026-04-17)

- ✅ Data extractor written and run: 1,140 Q&A examples
- ✅ `fine-tune.py` parameterized for `--task manual`
- ✅ `export-gguf.py` parameterized for `--task manual`
- ✅ Overnight training wrapper at `scratch/train-manual-overnight.sh`
- ✅ Test harness at `scratch/test-manual.py`
- ⏸ Training not yet run — ready to kick off whenever GPU is free
- ⏸ Runtime integration (slash command, agent tool) not yet scaffolded

## Open design questions

1. **Multi-turn?** Current data is single-turn Q&A. Should conversations carry context? For offline-manual use I think single-turn is fine — each question is self-contained.
2. **Citation?** Should the model cite the doc path it's drawing from? Would help users verify. Could be added by amending the training data with `(source: docs/X.md)` suffix on answers.
3. **Scope:** should we include the sibling fleet (wraith, kalshi) at all, or keep it tightly ClaudioOS-only? Currently included as high-level "what's Matt's fleet look like" knowledge. Easy to strip.
4. **Update cadence:** when docs change materially, re-extract + fine-tune. Could automate via a sweep-style cron that triggers when `docs/` churn exceeds a threshold.
