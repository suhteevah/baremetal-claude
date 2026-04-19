#!/usr/bin/env python3
"""Generate Q&A training data for the ClaudioOS offline manual model.

Extracts from:
  - Top-level docs (README.md, HANDOFF.md, CLAUDE.md, CAPABILITIES.md)
  - docs/*.md (architecture, API protocol, building, etc.)
  - FR/roadmap.md
  - crates/*/README.md
  - //! module-level doc blocks at the top of Rust files
  - Root-level .md files in external projects (wraith-browser, kalshi-trader-v7, etc.)

Produces one ShareGPT-format JSONL entry per Q&A pair:
    {"conversations": [
        {"from": "system", "value": MANUAL_SYSTEM_PROMPT},
        {"from": "human",  "value": "<question>"},
        {"from": "gpt",    "value": "<answer>"}
    ]}

Output: tools/manual-training-data.jsonl

The system prompt is distinct from the code-generation one — the manual
model explains and orients, it doesn't write code. Both tasks can share
the base Qwen2.5-Coder-1.5B + separate LoRAs.

Usage:
    python tools/generate-manual-data.py
    python tools/generate-manual-data.py --max-examples 5000
"""

import argparse
import json
import os
import re
import sys
from pathlib import Path
from typing import Iterator

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

REPO_ROOT = Path(__file__).parent.parent
OUTPUT = REPO_ROOT / "tools" / "manual-training-data.jsonl"

MANUAL_SYSTEM_PROMPT = """You are the ClaudioOS offline manual — an on-device assistant that answers questions about this specific bare-metal Rust operating system.

Ground rules:
- Answer from the project's documentation and source code only.
- Be concise. Prefer exact crate, file, and function names when they're relevant.
- When referring to code, use paths like `crates/agent/src/lib.rs` or `kernel/src/memory.rs`.
- If a question is outside ClaudioOS scope, say so briefly and suggest what the user might want instead.
- Do not generate code unless asked; your job is to explain and orient."""


# Top-level markdown files to ingest whole
TOP_LEVEL_MD = [
    "README.md",
    "HANDOFF.md",
    "CLAUDE.md",
]

DOCS_DIR = REPO_ROOT / "docs"
FR_DIR = REPO_ROOT / "FR"
CRATES_DIR = REPO_ROOT / "crates"
KERNEL_DIR = REPO_ROOT / "kernel"

# External projects that constitute "the fleet" — user-visible, shippable
# products that the manual should know about at a high level.
EXTERNAL_PROJECTS = [
    Path("J:/wraith-browser"),
    Path("J:/kalshi-trader-v7"),
    Path("J:/claudearbitrage"),
    Path("J:/claudeai"),
    Path("J:/antcolony"),
    Path("J:/openclaw model load optimizer"),
]

# Launch posts are marketing copy — skip
EXCLUDE_DIRS = {"launch-posts", "target", "node_modules", ".git", "dist"}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def iter_md_files(root: Path) -> Iterator[Path]:
    """Walk root for .md files, skipping noisy dirs."""
    if not root.exists():
        return
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in EXCLUDE_DIRS]
        for f in filenames:
            if f.endswith(".md"):
                yield Path(dirpath) / f


def iter_rs_files(root: Path) -> Iterator[Path]:
    """Walk root for .rs files, skipping noisy dirs."""
    if not root.exists():
        return
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in EXCLUDE_DIRS]
        for f in filenames:
            if f.endswith(".rs"):
                yield Path(dirpath) / f


def read_text(p: Path) -> str:
    try:
        return p.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return ""


def qa(question: str, answer: str) -> dict:
    """Emit a ShareGPT-format conversation."""
    return {
        "conversations": [
            {"from": "system", "value": MANUAL_SYSTEM_PROMPT},
            {"from": "human", "value": question},
            {"from": "gpt", "value": answer},
        ]
    }


def clean_answer(s: str, max_chars: int = 1800) -> str:
    """Trim answer to keep training example sizes reasonable."""
    s = s.strip()
    if len(s) > max_chars:
        # Truncate at a paragraph boundary near the cap
        cut = s.rfind("\n\n", 0, max_chars)
        if cut < max_chars * 0.6:
            cut = max_chars
        s = s[:cut].rstrip() + "\n\n[... truncated]"
    return s


# ---------------------------------------------------------------------------
# Extractors — each yields (question, answer) tuples
# ---------------------------------------------------------------------------

SECTION_RE = re.compile(r"^(#{1,4})\s+(.+?)\s*$", re.MULTILINE)


def extract_sections(md_text: str, source: str) -> Iterator[tuple[str, str]]:
    """Slice a markdown doc by headings; each section becomes one Q&A pair."""
    matches = list(SECTION_RE.finditer(md_text))
    if not matches:
        return

    for i, m in enumerate(matches):
        level = len(m.group(1))
        title = m.group(2).strip()
        # Skip TOC-like or emoji-heavy titles
        if not title or len(title) < 3 or title.startswith("<!--"):
            continue
        start = m.end()
        end = matches[i + 1].start() if i + 1 < len(matches) else len(md_text)
        body = md_text[start:end].strip()
        if len(body) < 40:  # skip stub sections
            continue
        # Form different question phrasings at the top level vs deeper
        if level <= 2:
            q = f"What is '{title}' in ClaudioOS?"
        else:
            q = f"Explain '{title}' (from {source})."
        yield q, body


def extract_whole_doc(md_text: str, source: str) -> tuple[str, str] | None:
    """One example that pulls the first 2KB of a doc as an overview."""
    stripped = md_text.strip()
    if len(stripped) < 200:
        return None
    # Use the filename stem as the topic
    topic = Path(source).stem.replace("-", " ").replace("_", " ")
    q = f"Give me an overview of {topic} (from {source})."
    return q, stripped[:2000]


MODULE_DOC_RE = re.compile(
    r"\A((?:\s*//!.*(?:\n|$))+)", re.MULTILINE
)


def extract_module_doc(rs_text: str, rel_path: str) -> tuple[str, str] | None:
    """Parse the //! block at the top of a Rust file."""
    m = MODULE_DOC_RE.search(rs_text)
    if not m:
        return None
    lines = [ln.strip() for ln in m.group(1).splitlines() if ln.strip().startswith("//!")]
    body = "\n".join(ln[3:].strip() if ln.startswith("//!") else ln for ln in lines).strip()
    if len(body) < 30:
        return None
    q = f"What does `{rel_path}` do?"
    return q, body


def crate_catalog(crates_dir: Path) -> list[tuple[str, str]]:
    """Build Q&A covering the list of crates + their purpose from README/lib.rs doc."""
    out = []
    if not crates_dir.exists():
        return out
    entries = []
    for d in sorted(crates_dir.iterdir()):
        if not d.is_dir():
            continue
        name = d.name
        purpose = None
        # Try README.md first
        readme = d / "README.md"
        if readme.exists():
            txt = read_text(readme)
            # first non-empty paragraph after the h1
            parts = re.split(r"\n\n+", txt, maxsplit=3)
            for p in parts[1:]:
                p = p.strip()
                if p and not p.startswith("#"):
                    purpose = p.split("\n")[0][:300]
                    break
        # Fall back to src/lib.rs //!
        if not purpose:
            libfile = d / "src" / "lib.rs"
            if libfile.exists():
                ext = extract_module_doc(read_text(libfile), f"crates/{name}/src/lib.rs")
                if ext:
                    purpose = ext[1].split("\n\n")[0][:300]
        if purpose:
            entries.append((name, purpose))

    if not entries:
        return out

    # Overall list
    lines = [f"- `crates/{n}` — {p}" for n, p in entries]
    out.append((
        "List the crates that make up ClaudioOS with a one-line purpose for each.",
        "\n".join(lines),
    ))
    # Per-crate Q&A
    for n, p in entries:
        out.append((f"What does the `{n}` crate do?", p))
    out.append((
        "How many top-level crates are in the ClaudioOS workspace?",
        f"{len(entries)} crates under `crates/` (plus the kernel in `kernel/`).",
    ))
    return out


def fleet_catalog() -> list[tuple[str, str]]:
    """High-level fleet knowledge so the manual can orient users beyond this repo."""
    entries = []
    for p in EXTERNAL_PROJECTS:
        readme = p / "README.md"
        handoff = p / "HANDOFF.md"
        text = ""
        if readme.exists():
            text = read_text(readme)[:1500]
        elif handoff.exists():
            text = read_text(handoff)[:1500]
        if text:
            topic = p.name
            entries.append((
                f"What is the `{topic}` project (sibling of ClaudioOS on Matt's fleet)?",
                text,
            ))
    return entries


# ---------------------------------------------------------------------------
# Main pipeline
# ---------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--max-examples", type=int, default=10000)
    args = ap.parse_args()

    examples: list[dict] = []
    seen_qs: set[str] = set()

    def push(q: str, a: str):
        q = q.strip()
        a = clean_answer(a)
        if not q or not a or len(a) < 40:
            return
        key = q.lower()
        if key in seen_qs:
            return
        seen_qs.add(key)
        examples.append(qa(q, a))

    # --- top-level + docs ---
    for rel in TOP_LEVEL_MD:
        p = REPO_ROOT / rel
        if not p.exists():
            continue
        text = read_text(p)
        ov = extract_whole_doc(text, rel)
        if ov:
            push(*ov)
        for q, a in extract_sections(text, rel):
            push(q, a)

    for mdfile in iter_md_files(DOCS_DIR):
        rel = str(mdfile.relative_to(REPO_ROOT)).replace("\\", "/")
        text = read_text(mdfile)
        ov = extract_whole_doc(text, rel)
        if ov:
            push(*ov)
        for q, a in extract_sections(text, rel):
            push(q, a)

    for mdfile in iter_md_files(FR_DIR):
        rel = str(mdfile.relative_to(REPO_ROOT)).replace("\\", "/")
        text = read_text(mdfile)
        for q, a in extract_sections(text, rel):
            push(q, a)

    # --- crate READMEs + catalog ---
    for mdfile in iter_md_files(CRATES_DIR):
        rel = str(mdfile.relative_to(REPO_ROOT)).replace("\\", "/")
        text = read_text(mdfile)
        ov = extract_whole_doc(text, rel)
        if ov:
            push(*ov)
        for q, a in extract_sections(text, rel):
            push(q, a)

    for q, a in crate_catalog(CRATES_DIR):
        push(q, a)

    # --- module-level Rust //! blocks ---
    rs_count = 0
    for rs in iter_rs_files(CRATES_DIR):
        rel = str(rs.relative_to(REPO_ROOT)).replace("\\", "/")
        ext = extract_module_doc(read_text(rs), rel)
        if ext:
            push(*ext)
            rs_count += 1
    for rs in iter_rs_files(KERNEL_DIR):
        rel = str(rs.relative_to(REPO_ROOT)).replace("\\", "/")
        ext = extract_module_doc(read_text(rs), rel)
        if ext:
            push(*ext)
            rs_count += 1

    # --- fleet sibling projects ---
    for q, a in fleet_catalog():
        push(q, a)

    # Cap
    examples = examples[: args.max_examples]

    OUTPUT.parent.mkdir(exist_ok=True)
    with open(OUTPUT, "w", encoding="utf-8") as f:
        for ex in examples:
            f.write(json.dumps(ex, ensure_ascii=False) + "\n")

    total_chars = sum(len(ex["conversations"][-1]["value"]) for ex in examples)
    avg_chars = total_chars // max(1, len(examples))
    print(f"Wrote {len(examples)} Q&A examples to {OUTPUT}")
    print(f"  from: {len(TOP_LEVEL_MD)} top-level md, docs/, FR/, crates/*/README.md, {rs_count} rust module docs")
    print(f"  avg answer length: {avg_chars} chars")
    print(f"  total size: {OUTPUT.stat().st_size / 1024:.1f} KB")


if __name__ == "__main__":
    main()
