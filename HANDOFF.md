# Session Handoff — 2026-04-12 (session 2)

## Last Updated
2026-04-12

## Project Status
🟡 claudio-mux v1 functional — shell rendering works, tiling layout redesign needed

## What Was Done This Session

### Implementation (19 tasks, all complete)
- Wrote full implementation plan at `docs/superpowers/plans/2026-04-12-claudio-mux.md`
- Executed all 19 tasks via subagent-driven development on branch `feat/claudio-mux`
- Created 3 new crates: terminal-core (29 tests), terminal-fb, terminal-ansi (4 tests)
- Migrated kernel dashboard from PrefixState to terminal-core InputRouter
- Built claudio-mux binary with full tokio event loop

### Bug Fixes (6 found via code review + 3 found via interactive testing)
- **DSR response** — shells send ESC[6n and block; we now respond with cursor position
- **Ctrl+char encoding** — guard on is_ascii_alphabetic(), use lowercase
- **Host::Drop order** — disable_raw_mode → leave alt screen → show cursor
- **F5-F12 keys** — added VT escape sequences
- **Ctrl+C forwarding** — removed tokio ctrl_c handler, Ctrl+C goes to shell
- **Dead pane cleanup** — auto-close exited panes, all_exited() check
- **Double key events** — crossterm 0.28 sends Press+Release; now filter Press only
- **Shifted command keys** — " and % require Shift; router now strips SHIFT before matching
- **Shell default** — auto-detect pwsh.exe vs cmd.exe

### Interactive Testing Results
- Shell spawns and renders (cmd.exe banner, copyright, prompt visible)
- Ctrl+B s (spawn shell) works — creates horizontal split with new shell
- Ctrl+B n/p (focus next/prev) works
- Ctrl+B q (quit) works, terminal restores cleanly
- Ctrl+B " now works (was broken by SHIFT modifier)
- Status bar renders with session name and pane count

## Current State

### Working
- terminal-core: 29 tests, full VTE + layout + input routing
- terminal-ansi: 4 tests, diff rendering
- claudio-mux: launches, spawns shells, renders output, prefix commands, splits, focus, quit
- Kernel: compiles with InputRouter migration

### Known Issues
- **Tiling layout degenerates** — binary split halving produces tiny slivers after 3-4 splits. Need dwm/awesome-style tiling (master+stack or fair grid). User confirmed this is the #1 priority for next session.
- --layout flag not wired to layouts::load_layout
- No scrollback, no mouse support (v2)

## Blocking Issues
- MSVC linker on this machine (msvcrt.lib) — tests/builds use GNU target workaround
- Prior blockers (AHCI DMA, SSH pipe wiring) unrelated and open

## What's Next
1. **Brainstorm tiling layout strategy** — dwm master+stack vs awesome fair vs i3 manual. This is a terminal-core Layout redesign. Use superpowers:brainstorming skill.
2. **Implement chosen tiling strategy** in terminal-core Layout
3. **Wire --layout flag** for named layouts
4. **Merge to main** once tiling is solid
5. **v2 planning** — daemon mode, session persistence

## Notes for Next Session
- Branch: `feat/claudio-mux` (19 commits, pushed to origin)
- Build: `cd tools/claudio-mux && rustup run stable-x86_64-pc-windows-gnu cargo build --target x86_64-pc-windows-gnu`
- Run: `& "J:\baremetal claude\tools\claudio-mux\target\x86_64-pc-windows-gnu\debug\claudio-mux.exe"`
- Logs: `%LOCALAPPDATA%\ridge-cell\claudio-mux\data\logs\`
- claudio-mux is EXCLUDED from workspace (like image-builder) — has own .cargo/config.toml
- DSR response (ESC[6n → ESC[row;colR) is critical — without it shells produce no output
- The tiling redesign changes terminal-core's Layout — may need LayoutPolicy abstraction
- Design spec: `docs/superpowers/specs/2026-04-11-claudio-mux-design.md`
- Memory saved: feedback_tiling_layout.md has the user's requirements and candidate strategies
- User prefers subagent-driven development (feedback_subagents.md)
