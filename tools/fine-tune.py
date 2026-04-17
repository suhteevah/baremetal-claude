#!/usr/bin/env python3
"""QLoRA fine-tune Qwen2.5-Coder-7B on ClaudioOS training data.

Runs on a single RTX 3070 Ti (8GB VRAM). Uses 4-bit quantization for
the base model and LoRA adapters for parameter-efficient fine-tuning.

Prerequisites:
    pip install torch transformers peft trl bitsandbytes accelerate datasets

Usage:
    # Generate training data first:
    python tools/generate-training-data.py

    # Then fine-tune:
    python tools/fine-tune.py

    # Export to GGUF after training:
    python tools/export-gguf.py
"""

import argparse
import json
import os
import sys
from pathlib import Path

# Check dependencies before importing
try:
    import torch
    from transformers import (
        AutoModelForCausalLM,
        AutoTokenizer,
        BitsAndBytesConfig,
        TrainingArguments,
    )
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
    from trl import SFTTrainer, SFTConfig
    from datasets import Dataset
except ImportError as e:
    print(f"Missing dependency: {e}")
    print("Install with: pip install torch transformers peft trl bitsandbytes accelerate datasets")
    sys.exit(1)

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

# Base model — selected via --size flag (default 7B, best coding model in 8GB w/ QLoRA)
SIZE_CONFIG = {
    "1.5b": {
        "base_model": "Qwen/Qwen2.5-Coder-1.5B",
        "output_subdir": "claudio-coder-1.5b-lora",
        "merged_subdir": "claudio-coder-1.5b-merged",
    },
    "7b": {
        "base_model": "Qwen/Qwen2.5-Coder-7B",
        "output_subdir": "claudio-coder-7b-lora",
        "merged_subdir": "claudio-coder-7b-merged",
    },
}

# Module-level defaults; overridden in main() after arg parsing
BASE_MODEL = SIZE_CONFIG["7b"]["base_model"]
REPO_ROOT = Path(__file__).parent.parent
TRAINING_DATA = REPO_ROOT / "tools" / "training-data.jsonl"
OUTPUT_DIR = REPO_ROOT / "models" / SIZE_CONFIG["7b"]["output_subdir"]
MERGED_DIR = REPO_ROOT / "models" / SIZE_CONFIG["7b"]["merged_subdir"]

# Training hyperparameters (tuned for 3070 Ti 8GB)
LORA_R = 16                    # LoRA rank — 16 is good quality/memory tradeoff
LORA_ALPHA = 32                # LoRA scaling factor (usually 2x rank)
LORA_DROPOUT = 0.05            # Small dropout to prevent overfitting
MAX_SEQ_LEN = 512              # Dropped 1024 → 512 after VRAM spill (44s→147s climb, 2026-04-08)
                               # samples were spilling VRAM→sysRAM and BSOD'd
                               # the machine at ~step 3 of the 7B run.
BATCH_SIZE = 1                 # Must be 1 for 8GB VRAM
GRADIENT_ACCUM = 8             # Effective batch size = 8
LEARNING_RATE = 2e-4           # Standard for QLoRA
NUM_EPOCHS = 4                 # Bumped to 4 for better recall on 7B model
WARMUP_RATIO = 0.03            # Warm up for 3% of training
SAVE_STEPS = 25                # Frequent checkpoints — last run BSOD'd before
                               # the first save and we lost everything.

# LoRA target modules for Qwen2.5
TARGET_MODULES = [
    "q_proj", "k_proj", "v_proj", "o_proj",  # Attention
    "gate_proj", "up_proj", "down_proj",       # FFN (SwiGLU)
]


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------

def load_training_data() -> Dataset:
    """Load the JSONL training data into a HuggingFace Dataset."""
    if not TRAINING_DATA.exists():
        print(f"Training data not found at {TRAINING_DATA}")
        print("Run: python tools/generate-training-data.py")
        sys.exit(1)

    examples = []
    with open(TRAINING_DATA, "r", encoding="utf-8") as f:
        for line in f:
            ex = json.loads(line)
            # Convert ShareGPT format to a single text string
            text = format_conversation(ex["conversations"])
            examples.append({"text": text})

    print(f"Loaded {len(examples)} training examples")
    return Dataset.from_list(examples)


def format_conversation(messages: list[dict]) -> str:
    """Format a ShareGPT conversation into a single training string.

    Uses Qwen's chat template format:
    <|im_start|>system\n...<|im_end|>
    <|im_start|>user\n...<|im_end|>
    <|im_start|>assistant\n...<|im_end|>
    """
    parts = []
    for msg in messages:
        role = msg["from"]
        value = msg["value"]
        # Map ShareGPT roles to Qwen chat roles
        if role == "system":
            parts.append(f"<|im_start|>system\n{value}<|im_end|>")
        elif role == "human":
            parts.append(f"<|im_start|>user\n{value}<|im_end|>")
        elif role == "gpt":
            parts.append(f"<|im_start|>assistant\n{value}<|im_end|>")
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Model setup
# ---------------------------------------------------------------------------

def setup_model():
    """Load the base model in 4-bit and apply LoRA."""
    print(f"Loading {BASE_MODEL} in 4-bit quantization...")

    # 4-bit quantization config for QLoRA
    bnb_config = BitsAndBytesConfig(
        load_in_4bit=True,
        bnb_4bit_quant_type="nf4",          # Normal Float 4 — best for QLoRA
        bnb_4bit_compute_dtype=torch.float16, # fp16 — works with bnb 0.43.x
        bnb_4bit_use_double_quant=True,       # Double quantization saves ~0.4GB
    )

    # Pin every weight to GPU 0. device_map="auto" sees ~6.7 GB free and
    # decides 7B-NF4 won't fit, then tries CPU offload, which BnB rejects.
    # Forcing {"": 0} skips the planner — if it OOMs we surface a real
    # OOM instead of a confusing "modules dispatched on CPU" error.
    model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL,
        quantization_config=bnb_config,
        device_map={"": 0},
        trust_remote_code=True,
        torch_dtype=torch.float16,
    )

    # Load tokenizer
    tokenizer = AutoTokenizer.from_pretrained(
        BASE_MODEL,
        trust_remote_code=True,
    )
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    # Prepare for k-bit training
    model = prepare_model_for_kbit_training(model)

    # Apply LoRA
    lora_config = LoraConfig(
        r=LORA_R,
        lora_alpha=LORA_ALPHA,
        target_modules=TARGET_MODULES,
        lora_dropout=LORA_DROPOUT,
        bias="none",
        task_type="CAUSAL_LM",
    )
    model = get_peft_model(model, lora_config)

    # Cast any bf16 parameters to fp16 to avoid gradient scaler crash on 3070 Ti
    for name, param in model.named_parameters():
        if param.dtype == torch.bfloat16:
            param.data = param.data.to(torch.float16)

    # Print trainable parameters
    trainable, total = model.get_nb_trainable_parameters()
    print(f"Trainable: {trainable:,} / {total:,} parameters ({100*trainable/total:.2f}%)")

    return model, tokenizer


# ---------------------------------------------------------------------------
# Training
# ---------------------------------------------------------------------------

def train(model, tokenizer, dataset, num_epochs: int, resume: bool):
    """Run QLoRA fine-tuning.

    num_epochs: total epochs for THIS run (e.g. 1 for a per-epoch session).
    resume:     if True and a checkpoint exists in OUTPUT_DIR, resume from it.
                Trainer restores optimizer state, LR schedule position, and
                dataloader position — mathematically identical to an
                uninterrupted run, just spread across sessions.
    """
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    training_args = SFTConfig(
        output_dir=str(OUTPUT_DIR),
        num_train_epochs=num_epochs,
        per_device_train_batch_size=BATCH_SIZE,
        gradient_accumulation_steps=GRADIENT_ACCUM,
        learning_rate=LEARNING_RATE,
        warmup_ratio=WARMUP_RATIO,
        lr_scheduler_type="cosine",
        fp16=False,                         # Disable AMP entirely — bnb still promotes to bf16
        bf16=False,                         # internally, breaking AMP's grad scaler. Without AMP,
                                            # bnb_4bit_compute_dtype=fp16 + LoRA-fp16 trains natively.
        logging_steps=10,
        save_steps=SAVE_STEPS,
        save_total_limit=3,                 # Keep last 3 checkpoints
        max_length=MAX_SEQ_LEN,
        gradient_checkpointing=True,        # Saves ~2GB VRAM at cost of speed
        gradient_checkpointing_kwargs={"use_reentrant": False},
        optim="paged_adamw_8bit",           # 8-bit optimizer saves VRAM
        max_grad_norm=0.3,                  # Tighter clip — keeps grad tensors small
        dataloader_pin_memory=False,        # Don't pin — we need the sysRAM
        report_to="none",                   # No wandb/tensorboard
        dataset_text_field="text",
    )

    trainer = SFTTrainer(
        model=model,
        train_dataset=dataset,
        processing_class=tokenizer,
        args=training_args,
    )

    # Detect existing checkpoint for resume
    has_checkpoint = any(
        p.name.startswith("checkpoint-") for p in OUTPUT_DIR.iterdir()
    ) if OUTPUT_DIR.exists() else False
    should_resume = resume and has_checkpoint

    print(f"\nStarting training: {num_epochs} epoch(s), {len(dataset)} examples")
    print(f"Effective batch size: {BATCH_SIZE * GRADIENT_ACCUM}")
    print(f"Output: {OUTPUT_DIR}")
    print(f"Resume from checkpoint: {should_resume}\n")

    trainer.train(resume_from_checkpoint=should_resume)
    trainer.save_model(str(OUTPUT_DIR))
    tokenizer.save_pretrained(str(OUTPUT_DIR))

    print(f"\nTraining complete! LoRA adapters saved to: {OUTPUT_DIR}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def parse_args():
    p = argparse.ArgumentParser(
        description="QLoRA fine-tune Qwen2.5-Coder on ClaudioOS data."
    )
    p.add_argument(
        "--size",
        choices=list(SIZE_CONFIG.keys()),
        default="7b",
        help="Model size to train. 1.5b trains fast, 7b is stronger. Default: 7b.",
    )
    p.add_argument(
        "--output-suffix",
        default="",
        help="Optional suffix on the output LoRA dir (e.g. '-v2'). "
             "Lets you train a fresh LoRA without clobbering a previous run.",
    )
    p.add_argument(
        "--epochs",
        type=int,
        default=NUM_EPOCHS,
        help=f"Epochs to run THIS session (default: {NUM_EPOCHS}). "
             "For per-epoch sessions use --epochs 1 and rely on auto-resume.",
    )
    p.add_argument(
        "--no-resume",
        action="store_true",
        help="Ignore existing checkpoints and start fresh. "
             "Default is to always resume if a checkpoint exists.",
    )
    return p.parse_args()


def main():
    args = parse_args()

    # Resolve per-size paths, allowing an output-dir suffix for v2/v3/etc.
    global BASE_MODEL, OUTPUT_DIR, MERGED_DIR
    cfg = SIZE_CONFIG[args.size]
    BASE_MODEL = cfg["base_model"]
    OUTPUT_DIR = REPO_ROOT / "models" / f"{cfg['output_subdir']}{args.output_suffix}"
    MERGED_DIR = REPO_ROOT / "models" / f"{cfg['merged_subdir']}{args.output_suffix}"

    print("=" * 60)
    print("ClaudioOS Model Fine-Tuning")
    print(f"Size: {args.size}   Base model: {BASE_MODEL}")
    print(f"Training data: {TRAINING_DATA}")
    print(f"Output LoRA:   {OUTPUT_DIR}")
    print(f"Target GPU: RTX 3070 Ti (8GB VRAM)")
    print(f"Epochs this session: {args.epochs}  |  Resume: {not args.no_resume}")
    print("=" * 60)

    # Check CUDA
    if not torch.cuda.is_available():
        print("ERROR: CUDA not available. Need an NVIDIA GPU.")
        sys.exit(1)

    # bitsandbytes 0.43.x is the last release before NF4 params get
    # internally promoted to bf16 — using fp16 compute + AMP fp16 scaler
    # works correctly on the 3070 Ti at this version. 0.49+ broke this.
    print("Note: Using fp16 compute (bitsandbytes pinned at 0.43.x)")

    gpu_name = torch.cuda.get_device_name(0)
    gpu_mem = torch.cuda.get_device_properties(0).total_memory / (1024**3)
    print(f"GPU: {gpu_name} ({gpu_mem:.1f} GB)")

    dataset = load_training_data()
    model, tokenizer = setup_model()
    train(model, tokenizer, dataset, num_epochs=args.epochs, resume=not args.no_resume)

    print(f"\nNext steps:")
    print(f"  1. Merge LoRA weights:  python tools/export-gguf.py")
    print(f"  2. Serve locally:       llama-server -m models/claudio-coder-7b.gguf --port 8080 -ngl 99")
    print(f"  3. Test:                curl localhost:8080/v1/chat/completions -d '{{\"messages\": [...]}}'")


if __name__ == "__main__":
    main()
