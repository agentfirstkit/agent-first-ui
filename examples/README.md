# AFUI Data Examples

These files are AFUI snapshots. They are protocol data, not HTML fragments, terminal escape sequences, frontend projects, package manifests, or downloadable components.

## Files

- `operator_console.afui.json` - dense commerce operator surface with stats, tables, details, inputs, logs, progress, and risk-aware actions.
- `go_game.afui.json` - graphical Go operation surface using an open `go_board` semantic kind plus deterministic `record:` action arguments.
- `tui_ops.afui.json` - terminal-style deployment operation surface with panes, logs, command input data, key-help hints, and risk-aware actions.

Each file is data-only: a trusted host renders it. Validate any of them against the schema in `../spec/` using the SDK in `../rust/`.
