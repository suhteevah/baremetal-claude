#!/usr/bin/env python3
"""Generate fine-tuning training data from the ClaudioOS codebase.

Walks every .rs file, extracts function-level instruction/output pairs,
and formats as ShareGPT JSON for QLoRA fine-tuning.

Output: tools/training-data.jsonl (one JSON object per line)

Usage:
    python tools/generate-training-data.py
"""

import os
import re
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent
OUTPUT = REPO_ROOT / "tools" / "training-data.jsonl"

# ClaudioOS internal crates
TARGET_CRATES = [
    "kernel/src",
    "crates/api-client/src",
    "crates/agent/src",
    "crates/terminal/src",
    "crates/net/src",
    "crates/auth/src",
    "crates/editor/src",
    "crates/python-lite/src",
    "crates/rustc-lite/src",
    "crates/llm/src",
    "crates/gpu/src",
    "crates/vulkan/src",
    "crates/wraith-dom/src",
    "crates/wraith-transport/src",
    "crates/wraith-render/src",
    "crates/elf-loader/src",
    "crates/linux-compat/src",
    "crates/fs-persist/src",
    "crates/js-lite/src",
    "crates/vfs/src",
    "crates/shell/src",
    "crates/sshd/src",
]

# External Rust projects on J: drive — all built with Claude
EXTERNAL_PROJECTS = [
    Path("J:/kalshi-trader-v7/src"),
    Path("J:/kalshi-trader-v7/src-tauri/src"),
    Path("J:/kalshi-weather-trader"),
    Path("J:/claudearbitrage/src"),
    Path("J:/wraith-browser/src"),
    Path("J:/wraith-browser/sevro/src"),
    Path("J:/wraith-dom/src"),
    Path("J:/wraith-render/src"),
    Path("J:/wraith-transport/src"),
    Path("J:/icognito cad tool/src"),
    Path("J:/world-mode/src"),
    Path("J:/world/src"),
    Path("J:/antcolony/crates/antcolony-sim/src"),
    Path("J:/antcolony/crates/antcolony-game/src"),
    Path("J:/antcolony/crates/antcolony-render/src"),
    Path("J:/antcolony/src"),
    Path("J:/wages of war/src"),
    Path("J:/gpu-compute-nostd/src"),
    Path("J:/vulkan-nostd/src"),
    Path("J:/acpi-nostd/src"),
    Path("J:/ahci-nostd/src"),
    Path("J:/bluetooth-nostd/src"),
    Path("J:/btrfs-nostd/src"),
    Path("J:/dotnet-clr-nostd/src"),
    Path("J:/dxvk-bridge-nostd/src"),
    Path("J:/editor-nostd/src"),
    Path("J:/elf-loader-nostd/src"),
    Path("J:/ext4-rw/src"),
    Path("J:/hda-nostd/src"),
    Path("J:/intel-nic-nostd/src"),
    Path("J:/js-lite/src"),
    Path("J:/linux-compat-nostd/src"),
    Path("J:/ntfs-rw/src"),
    Path("J:/nvme-nostd/src"),
    Path("J:/pe-loader-nostd/src"),
    Path("J:/python-lite/src"),
    Path("J:/rustc-lite/src"),
    Path("J:/shell-nostd/src"),
    Path("J:/smp-nostd/src"),
    Path("J:/sshd-pqc/src"),
    Path("J:/usb-storage-nostd/src"),
    Path("J:/vfs-nostd/src"),
    Path("J:/wifi-nostd/src"),
    Path("J:/win32-nostd/src"),
    Path("J:/winrt-nostd/src"),
    Path("J:/xhci-nostd/src"),
    Path("J:/distcc for claw project/openclaw-rust-agents/src"),
]

# System prompt that will be prepended to every training example
SYSTEM_PROMPT = """You are a Rust systems programmer working on ClaudioOS, a bare-metal operating system that runs AI coding agents directly on x86_64 hardware.

Key constraints:
- All code is #![no_std] with extern crate alloc
- No Linux kernel, no POSIX, no JavaScript runtime
- Uses spin::Mutex for synchronization (no std Mutex)
- Uses smoltcp for networking, embedded-tls for TLS 1.3
- Cranelift for JIT compilation
- Raw HTTP/1.1 over TLS byte streams (no reqwest/hyper)
- Single address space, async executor, interrupt-driven

Write clean, well-commented Rust code that compiles for x86_64-unknown-none."""


def extract_functions(source: str, filepath: str) -> list[dict]:
    """Extract function definitions with their doc comments and bodies."""
    examples = []
    lines = source.split("\n")
    i = 0

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        # Look for function definitions
        fn_match = re.match(
            r"^(\s*)(pub\s+)?(unsafe\s+)?(async\s+)?fn\s+(\w+)",
            line,
        )
        if fn_match:
            indent = len(fn_match.group(1) or "")
            fn_name = fn_match.group(5)

            # Collect doc comments above the function
            doc_lines = []
            j = i - 1
            while j >= 0:
                prev = lines[j].strip()
                if prev.startswith("///") or prev.startswith("//!"):
                    doc_lines.insert(0, prev)
                    j -= 1
                elif prev.startswith("#[") or prev == "":
                    j -= 1
                else:
                    break

            # Collect the function body (brace counting)
            fn_lines = []
            brace_count = 0
            started = False
            k = i
            while k < len(lines):
                fn_lines.append(lines[k])
                brace_count += lines[k].count("{") - lines[k].count("}")
                if "{" in lines[k]:
                    started = True
                if started and brace_count <= 0:
                    break
                k += 1

            fn_body = "\n".join(fn_lines)

            # Skip tiny functions (getters, one-liners)
            if len(fn_body) < 80:
                i = k + 1
                continue

            # Skip test functions
            if fn_name.startswith("test_") or fn_name == "tests":
                i = k + 1
                continue

            # Build instruction from doc comment or function signature
            doc_text = "\n".join(doc_lines) if doc_lines else ""
            rel_path = os.path.relpath(filepath, REPO_ROOT).replace("\\", "/")

            if doc_text:
                instruction = (
                    f"In `{rel_path}`, implement the function `{fn_name}`. "
                    f"Here is the documentation:\n{doc_text}"
                )
            else:
                # Use the signature as the instruction
                sig_line = lines[i].strip()
                instruction = (
                    f"In `{rel_path}`, implement this function:\n```rust\n{sig_line}\n```"
                )

            examples.append(
                {
                    "conversations": [
                        {"from": "system", "value": SYSTEM_PROMPT},
                        {"from": "human", "value": instruction},
                        {"from": "gpt", "value": f"```rust\n{fn_body}\n```"},
                    ]
                }
            )

            i = k + 1
        else:
            i += 1

    return examples


def extract_module_docs(source: str, filepath: str) -> list[dict]:
    """Extract module-level documentation as a Q&A pair."""
    lines = source.split("\n")
    doc_lines = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("//!"):
            doc_lines.append(stripped[3:].strip())
        elif stripped.startswith("#![") or stripped == "" or stripped.startswith("extern"):
            continue
        else:
            break

    if len(doc_lines) < 3:
        return []

    doc_text = "\n".join(doc_lines)
    rel_path = os.path.relpath(filepath, REPO_ROOT).replace("\\", "/")

    return [
        {
            "conversations": [
                {"from": "system", "value": SYSTEM_PROMPT},
                {
                    "from": "human",
                    "value": f"Explain the purpose and architecture of `{rel_path}`.",
                },
                {"from": "gpt", "value": doc_text},
            ]
        }
    ]


def extract_struct_impls(source: str, filepath: str) -> list[dict]:
    """Extract struct/enum definitions with their impl blocks."""
    examples = []
    lines = source.split("\n")
    rel_path = os.path.relpath(filepath, REPO_ROOT).replace("\\", "/")
    i = 0

    while i < len(lines):
        line = lines[i]
        # Match struct or enum definitions
        match = re.match(
            r"^(pub\s+)?(struct|enum)\s+(\w+)",
            line.strip(),
        )
        if match:
            type_kind = match.group(2)
            type_name = match.group(3)

            # Collect the definition (brace counting)
            def_lines = []
            brace_count = 0
            started = False
            k = i
            while k < len(lines):
                def_lines.append(lines[k])
                brace_count += lines[k].count("{") - lines[k].count("}")
                if "{" in lines[k]:
                    started = True
                if started and brace_count <= 0:
                    break
                k += 1

            body = "\n".join(def_lines)
            if len(body) > 100:  # skip trivial structs
                examples.append(
                    {
                        "conversations": [
                            {"from": "system", "value": SYSTEM_PROMPT},
                            {
                                "from": "human",
                                "value": f"In `{rel_path}`, define the `{type_name}` {type_kind} with appropriate fields and derive macros for a bare-metal OS.",
                            },
                            {"from": "gpt", "value": f"```rust\n{body}\n```"},
                        ]
                    }
                )
            i = k + 1
        else:
            i += 1

    return examples


NO_STD_PORTING_PROMPT = """You are an expert at porting Rust crates from std to no_std for bare-metal targets.

Key transformations:
- Replace `use std::` with `use core::` or `use alloc::`
- Add `#![no_std]` and `extern crate alloc`
- Replace `std::collections::HashMap` with `hashbrown::HashMap` or `alloc::collections::BTreeMap`
- Replace `std::sync::Mutex` with `spin::Mutex`
- Replace `std::io` with custom error types
- Replace `std::time::Instant` with platform-specific timers
- Guard `#[cfg(test)]` sections that need std
- Replace `println!` with `log::info!`
- Use `alloc::string::String`, `alloc::vec::Vec`, `alloc::boxed::Box` explicitly"""


def extract_nostd_porting_examples(repo_root: Path) -> list[dict]:
    """Extract no_std porting patterns from the forked Cranelift crates.

    Finds files that contain both `core::` and patterns suggesting they were
    ported from std, and generates instruction/output pairs teaching the
    std -> no_std transformation.
    """
    examples = []
    fork_dirs = [
        repo_root / "crates" / "cranelift-codegen-nostd" / "src",
        repo_root / "crates" / "cranelift-frontend-nostd" / "src",
        repo_root / "crates" / "cranelift-codegen-shared-nostd" / "src",
        repo_root / "crates" / "cranelift-control-nostd" / "src",
        repo_root / "crates" / "rustc-hash-nostd" / "src",
        repo_root / "crates" / "arbitrary-stub" / "src",
    ]

    for fork_dir in fork_dirs:
        if not fork_dir.exists():
            continue

        for rs_file in sorted(fork_dir.rglob("*.rs")):
            if "target" in str(rs_file):
                continue
            try:
                source = rs_file.read_text(encoding="utf-8", errors="replace")
            except Exception:
                continue

            # Only include files that show porting patterns
            has_alloc = "use alloc::" in source or "extern crate alloc" in source
            has_core = "use core::" in source
            has_nostd = "#![no_std]" in source
            has_cfg_guard = "#[cfg(not(" in source or "#[cfg(feature" in source

            if not (has_alloc or (has_core and has_cfg_guard)):
                continue

            # Extract the interesting lines showing the transformation
            rel_path = os.path.relpath(rs_file, repo_root).replace("\\", "/")

            # Collect the imports and cfg-guarded sections
            porting_lines = []
            lines = source.split("\n")
            for i, line in enumerate(lines):
                stripped = line.strip()
                if any(
                    pat in stripped
                    for pat in [
                        "use alloc::",
                        "use core::",
                        "extern crate alloc",
                        "#![no_std]",
                        "#[cfg(not(",
                        "#[cfg(feature",
                        "// no_std",
                        "// std",
                        "hashbrown",
                        "spin::",
                    ]
                ):
                    # Include surrounding context (2 lines before/after)
                    start = max(0, i - 2)
                    end = min(len(lines), i + 3)
                    chunk = "\n".join(lines[start:end])
                    if chunk not in porting_lines:
                        porting_lines.append(chunk)

            if not porting_lines:
                continue

            # Build the training example
            ported_code = "\n\n// ---\n\n".join(porting_lines[:15])  # cap at 15 chunks

            crate_name = rel_path.split("/")[1]  # e.g. "cranelift-codegen-nostd"
            original_name = crate_name.replace("-nostd", "")

            examples.append(
                {
                    "conversations": [
                        {"from": "system", "value": NO_STD_PORTING_PROMPT},
                        {
                            "from": "human",
                            "value": f"Port `{rel_path}` from the `{original_name}` crate to work in a #![no_std] bare-metal environment. Show the key import and configuration changes needed.",
                        },
                        {
                            "from": "gpt",
                            "value": f"Here are the key no_std porting changes for `{rel_path}`:\n\n```rust\n{ported_code}\n```\n\nKey transformations applied:\n- Replaced `std::` imports with `core::` and `alloc::` equivalents\n- Added `extern crate alloc` for heap types (Vec, String, Box, BTreeMap)\n- Guarded std-only features behind `#[cfg]` attributes\n- Used `alloc::collections::BTreeMap` instead of `HashMap` where needed",
                        },
                    ]
                }
            )

    # Also add a general "how to port" meta-example
    examples.append(
        {
            "conversations": [
                {"from": "system", "value": NO_STD_PORTING_PROMPT},
                {
                    "from": "human",
                    "value": "How do you port a large Rust crate like cranelift-codegen from std to no_std for a bare-metal OS?",
                },
                {
                    "from": "gpt",
                    "value": """The process for porting cranelift-codegen to no_std (which we did for ClaudioOS):

1. **Add `#![no_std]` and `extern crate alloc`** to lib.rs

2. **Replace std imports systematically:**
   - `std::fmt` → `core::fmt`
   - `std::mem` → `core::mem`
   - `std::ops` → `core::ops`
   - `std::collections::HashMap` → `hashbrown::HashMap` (with ahash)
   - `std::collections::BTreeMap` → `alloc::collections::BTreeMap`
   - `std::vec::Vec` → `alloc::vec::Vec`
   - `std::string::String` → `alloc::string::String`
   - `std::boxed::Box` → `alloc::boxed::Box`

3. **Handle std::io:** Create stub error types or use custom Result types. Most codegen code doesn't actually need real I/O.

4. **Guard timing code:** `std::time::Instant` doesn't exist. Wrap timing behind `#[cfg(feature = "timing")]` or replace with no-op.

5. **Guard unwinding:** `std::panic::catch_unwind` doesn't work. Use `panic = "abort"` in Cargo.toml.

6. **Build script post-processing:** Some crates generate code with `std::` references. Add a build.rs step that replaces `std::` with `core::` in generated files.

7. **Fork dependencies:** If a dependency uses std, you may need to fork it too (we forked 6 crates total for Cranelift).

8. **Use `libm`** for math functions (exp, sin, cos, sqrt) that live in std on normal targets.""",
                },
            ]
        }
    )

    return examples


def add_claude_md_example(examples: list[dict]):
    """Add the CLAUDE.md as a project knowledge example."""
    claude_md = REPO_ROOT / "CLAUDE.md"
    if claude_md.exists():
        content = claude_md.read_text(encoding="utf-8", errors="replace")
        examples.append(
            {
                "conversations": [
                    {"from": "system", "value": SYSTEM_PROMPT},
                    {
                        "from": "human",
                        "value": "Describe the ClaudioOS project architecture, build system, and development phases.",
                    },
                    {"from": "gpt", "value": content[:8000]},  # truncate if huge
                ]
            }
        )


def main():
    all_examples = []
    files_processed = 0

    for crate_path in TARGET_CRATES:
        src_dir = REPO_ROOT / crate_path
        if not src_dir.exists():
            continue

        for rs_file in sorted(src_dir.rglob("*.rs")):
            try:
                source = rs_file.read_text(encoding="utf-8", errors="replace")
            except Exception as e:
                print(f"  skip {rs_file}: {e}", file=sys.stderr)
                continue

            files_processed += 1

            # Extract different types of examples
            all_examples.extend(extract_module_docs(source, str(rs_file)))
            all_examples.extend(extract_functions(source, str(rs_file)))
            all_examples.extend(extract_struct_impls(source, str(rs_file)))

    # Process external Rust projects on J: drive
    for ext_dir in EXTERNAL_PROJECTS:
        if not ext_dir.exists():
            continue

        for rs_file in sorted(ext_dir.rglob("*.rs")):
            # Skip build artifacts
            if "target" in str(rs_file) or ".claude" in str(rs_file):
                continue
            try:
                source = rs_file.read_text(encoding="utf-8", errors="replace")
            except Exception as e:
                print(f"  skip {rs_file}: {e}", file=sys.stderr)
                continue

            files_processed += 1
            all_examples.extend(extract_module_docs(source, str(rs_file)))
            all_examples.extend(extract_functions(source, str(rs_file)))
            all_examples.extend(extract_struct_impls(source, str(rs_file)))

    # Extract no_std porting patterns from forked Cranelift crates
    nostd_examples = extract_nostd_porting_examples(REPO_ROOT)
    all_examples.extend(nostd_examples)
    print(f"  no_std porting examples: {len(nostd_examples)}")

    # Add CLAUDE.md
    add_claude_md_example(all_examples)

    # Write output
    with open(OUTPUT, "w", encoding="utf-8") as f:
        for ex in all_examples:
            f.write(json.dumps(ex, ensure_ascii=False) + "\n")

    print(f"Generated {len(all_examples)} training examples from {files_processed} files")
    print(f"Output: {OUTPUT}")
    print(f"Size: {OUTPUT.stat().st_size / 1024:.1f} KB")

    # Stats
    fn_count = sum(
        1 for ex in all_examples if "implement" in ex["conversations"][1]["value"].lower()
    )
    doc_count = sum(
        1 for ex in all_examples if "explain" in ex["conversations"][1]["value"].lower()
    )
    struct_count = sum(
        1 for ex in all_examples if "define" in ex["conversations"][1]["value"].lower()
    )
    print(f"  Functions: {fn_count}")
    print(f"  Module docs: {doc_count}")
    print(f"  Structs/enums: {struct_count}")


if __name__ == "__main__":
    main()
