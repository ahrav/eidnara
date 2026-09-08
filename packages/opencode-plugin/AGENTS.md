# OpenCode plugin

Edit TUI source under `src/tui/`, not generated files under `src/tui-compiled/`. Regenerate and commit the compiled tree with `bun run --cwd packages/opencode-plugin build:tui`.

After TUI or bundle changes, run `bun run --cwd packages/opencode-plugin smoke`. `bun test` loads source modules and does not prove the shipped compiled TUI imports successfully.
