# Session Handoff — 2026-04-12

## What was completed

- Full brainstorming session for **claudio-mux** — a tmux-like terminal multiplexer for Windows that shares core logic with ClaudioOS's framebuffer dashboard
- Design spec written, reviewed, and committed: `docs/superpowers/specs/2026-04-11-claudio-mux-design.md` (~850 lines, 10 sections)
- All architecture decisions locked:
  - **Approach**: share actual code between ClaudioOS and Windows (not mirror by convention)
  - **Crate structure**: workspace split into `terminal-core` (no_std) + `terminal-fb` + `terminal-ansi` + `tools/claudio-mux`
  - **Phasing**: v1 = splits/focus/status bar/layouts (single-process), v2 = daemon + session persistence, v3 = multi-client (pair programming)
  - **Key vocab**: Ctrl+B prefix, same bindings as ClaudioOS dashboard, InputRouter state machine lifted into shared core
  - **Windows runtime**: tokio + crossterm + portable-pty (ConPTY)
  - **Config**: TOML at `%APPDATA%\ridge-cell\claudio-mux\`
- Resume checkpoint saved: `scratch/claudio-mux-brainstorm-resume.md`

## Current state

- **Design spec**: committed in `df8c0fd`, fully reviewed (4 self-review fixes applied)
- **Implementation plan**: NOT YET WRITTEN — was starting the writing-plans skill when session ended
- **No code changes to existing crates** — the refactor hasn't started
- **ClaudioOS kernel**: unaffected so far; prior session's work (mount safety, logger VFS, etc.) is intact

## Blockers

- None for claudio-mux itself. The design is approved and ready for planning.
- Prior blockers (AHCI DMA fix, SSH pipe wiring) are unrelated to claudio-mux and remain open.

## Next steps

1. **Write the implementation plan** — invoke `superpowers:writing-plans` with the spec at `docs/superpowers/specs/2026-04-11-claudio-mux-design.md`. Plan goes to `docs/superpowers/plans/2026-04-12-claudio-mux.md`.
2. **Execute the plan** — roughly 15 tasks:
   - Tasks 1-6: Create `terminal-core` crate (extract from `crates/terminal/`)
   - Task 7: Create `terminal-fb` crate (framebuffer renderer)
   - Task 8: Migrate `kernel/src/dashboard.rs` to use terminal-core's InputRouter
   - Tasks 9-15: Create `terminal-ansi` crate + `tools/claudio-mux` binary
3. Key files to watch during refactor:
   - `crates/terminal/src/pane.rs` — pixel math at lines 77-78, 177-178, 267-320 must be stripped
   - `crates/terminal/src/layout.rs` — `Viewport` (pixel) becomes `CellViewport` (cells), `SEPARATOR_PX` moves to terminal-fb
   - `kernel/src/dashboard.rs` — `PrefixState` + `handle_prefix_command` (line 691, 1199) get replaced by `InputRouter`

## Uncommitted changes

- Only change: `.claude/scheduled_tasks.lock` deleted (irrelevant, not committing)
- All design work was auto-committed in `df8c0fd`
