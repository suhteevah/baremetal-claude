#!/usr/bin/env python3
"""Fine-tune Qwen2.5-Coder-1.5B for ClaudioOS offline manual, Maxwell-friendly.

Variant of fine-tune.py for the GTX 980 on cnc-server. The 980 is SM 5.2
(Maxwell) — bitsandbytes 4-bit requires SM 7.5+, so we drop QLoRA and
instead freeze the base model in fp16, attach LoRA adapters, and train
only the LoRA params with vanilla AdamW.

Memory budget:
    1.5B fp16 weights             ≈ 3.0 GB   frozen, no grad
    LoRA adapters (r=16)          ≈  25 MB   trainable, fp16
    LoRA grads (fp16)             ≈  25 MB
    AdamW state (fp32 for LoRA)   ≈ 100 MB
    Activations (w/ checkpoint)   ≈ 300 MB   at seq=512 batch=1
    -----------------------------------------
    Total                         ≈ 3.5 GB   fits in 3.9 GB free

Prereqs (already installed in /opt/ml-venv):
    torch==2.5.1+cu121 transformers peft trl datasets accelerate

Usage:
    /opt/ml-venv/bin/python tools/fine-tune-980.py
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import LoraConfig, get_peft_model
from trl import SFTConfig, SFTTrainer
from datasets import Dataset

BASE_MODEL = "Qwen/Qwen2.5-Coder-1.5B"
REPO_ROOT = Path(__file__).parent.parent

# Task config — selects which dataset + adapter dir to use
TASK_CONFIG = {
    "manual": {
        "data": "manual-training-data.jsonl",
        "lora_prefix": "claudio-manual",
    },
    "code": {
        "data": "training-data.jsonl",
        "lora_prefix": "claudio-coder",
    },
}
# These get set in main() from --task flag
TRAINING_DATA = REPO_ROOT / "tools" / TASK_CONFIG["manual"]["data"]
OUTPUT_DIR = REPO_ROOT / "models" / f"{TASK_CONFIG['manual']['lora_prefix']}-1.5b-lora-980"

# LoRA — smaller rank to fit 4 GB Maxwell
LORA_R = 8
LORA_ALPHA = 16
LORA_DROPOUT = 0.05
TARGET_MODULES = [
    "q_proj", "k_proj", "v_proj", "o_proj",
    "gate_proj", "up_proj", "down_proj",
]

# Training — tuned for GTX 980 4 GB VRAM
MAX_SEQ_LEN = 256
BATCH_SIZE = 1
GRADIENT_ACCUM = 16       # effective batch = 16
LEARNING_RATE = 2e-4
NUM_EPOCHS = 3
WARMUP_RATIO = 0.03
SAVE_STEPS = 50
LOGGING_STEPS = 10


def format_conversation(messages: list[dict]) -> str:
    parts = []
    for msg in messages:
        role = msg["from"]
        value = msg["value"]
        if role == "system":
            parts.append(f"<|im_start|>system\n{value}<|im_end|>")
        elif role == "human":
            parts.append(f"<|im_start|>user\n{value}<|im_end|>")
        elif role == "gpt":
            parts.append(f"<|im_start|>assistant\n{value}<|im_end|>")
    return "\n".join(parts)


def load_training_data() -> Dataset:
    if not TRAINING_DATA.exists():
        sys.exit(f"training data missing: {TRAINING_DATA}")
    examples = []
    with TRAINING_DATA.open("r", encoding="utf-8") as f:
        for line in f:
            ex = json.loads(line)
            examples.append({"text": format_conversation(ex["conversations"])})
    print(f"[data] {len(examples)} examples loaded from {TRAINING_DATA.name}")
    return Dataset.from_list(examples)


def setup_model():
    print(f"[model] loading {BASE_MODEL} in fp16 (no bnb, no device_map)…")
    t0 = time.time()
    model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL,
        torch_dtype=torch.float16,
        low_cpu_mem_usage=True,
        trust_remote_code=True,
    )
    model = model.to("cuda:0")
    print(f"[model] base on GPU in {time.time()-t0:.1f}s. "
          f"VRAM used: {torch.cuda.memory_allocated()/1e9:.2f} GB")

    # Freeze ALL base parameters (LoRA will add trainable ones on top)
    for p in model.parameters():
        p.requires_grad = False

    # Enable grad checkpointing on the frozen base — saves activation memory.
    # Must also re-enable input grads so gradient can flow into LoRA layers
    # that sit on top of frozen base matrices.
    model.gradient_checkpointing_enable()
    model.enable_input_require_grads()

    tok = AutoTokenizer.from_pretrained(BASE_MODEL, trust_remote_code=True)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token

    lora_config = LoraConfig(
        r=LORA_R,
        lora_alpha=LORA_ALPHA,
        target_modules=TARGET_MODULES,
        lora_dropout=LORA_DROPOUT,
        bias="none",
        task_type="CAUSAL_LM",
    )
    model = get_peft_model(model, lora_config)

    # Cast LoRA params to fp32 for stable Adam updates — base is still fp16
    for name, param in model.named_parameters():
        if param.requires_grad:
            param.data = param.data.to(torch.float32)

    trainable, total = model.get_nb_trainable_parameters()
    print(f"[lora] trainable={trainable:,} / total={total:,} "
          f"({100*trainable/total:.2f}%)")
    return model, tok


def train(model, tok, dataset, resume: bool) -> int:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    args = SFTConfig(
        output_dir=str(OUTPUT_DIR),
        num_train_epochs=NUM_EPOCHS,
        per_device_train_batch_size=BATCH_SIZE,
        gradient_accumulation_steps=GRADIENT_ACCUM,
        learning_rate=LEARNING_RATE,
        warmup_ratio=WARMUP_RATIO,
        lr_scheduler_type="cosine",
        fp16=True,                          # AMP — base is fp16, LoRA is fp32
        bf16=False,                         # Maxwell has no bf16 hardware path
        logging_steps=LOGGING_STEPS,
        save_steps=SAVE_STEPS,
        save_total_limit=3,
        max_length=MAX_SEQ_LEN,
        gradient_checkpointing=True,
        gradient_checkpointing_kwargs={"use_reentrant": False},
        optim="adamw_torch",                # bnb-free optimizer
        max_grad_norm=0.3,
        dataloader_pin_memory=False,
        report_to="none",
        dataset_text_field="text",
    )
    trainer = SFTTrainer(
        model=model,
        train_dataset=dataset,
        processing_class=tok,
        args=args,
    )
    has_ckpt = any(p.name.startswith("checkpoint-") for p in OUTPUT_DIR.iterdir()) if OUTPUT_DIR.exists() else False
    should_resume = resume and has_ckpt
    print(f"[train] {NUM_EPOCHS} epoch(s), {len(dataset)} examples, "
          f"eff batch={BATCH_SIZE*GRADIENT_ACCUM}, seq_len<={MAX_SEQ_LEN}")
    print(f"[train] resume={should_resume}")
    t0 = time.time()
    trainer.train(resume_from_checkpoint=should_resume)
    print(f"[train] done in {(time.time()-t0)/60:.1f} min")
    trainer.save_model()
    tok.save_pretrained(OUTPUT_DIR)
    print(f"[save] adapter saved → {OUTPUT_DIR}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--task", choices=list(TASK_CONFIG), default="manual",
                    help="Which dataset to train on")
    ap.add_argument("--resume", action="store_true", help="Resume from checkpoint if present")
    args = ap.parse_args()
    global TRAINING_DATA, OUTPUT_DIR
    tcfg = TASK_CONFIG[args.task]
    TRAINING_DATA = REPO_ROOT / "tools" / tcfg["data"]
    OUTPUT_DIR = REPO_ROOT / "models" / f"{tcfg['lora_prefix']}-1.5b-lora-980"
    print(f"[task] {args.task}  data={TRAINING_DATA.name}  out={OUTPUT_DIR.name}")
    if not torch.cuda.is_available():
        sys.exit("CUDA not available")
    print(f"[gpu] {torch.cuda.get_device_name(0)}  "
          f"cap={torch.cuda.get_device_capability(0)}  "
          f"vram={torch.cuda.get_device_properties(0).total_memory/1e9:.1f} GB")
    dataset = load_training_data()
    model, tok = setup_model()
    return train(model, tok, dataset, args.resume)


if __name__ == "__main__":
    sys.exit(main())
