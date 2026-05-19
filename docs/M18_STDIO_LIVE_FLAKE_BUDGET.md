# M18 Stdio Live Flake Budget

`scripts/run-m18-stdio-live-tmux-soak.sh` provides the TUI-side repeat-run
scaffold for octos-tui#50. It does not synthesize live evidence: `run-once`
launches `octos-tui` in tmux against a real `octos serve --stdio` child, and
`repeat` records each child command exit code, duration, log path, and artifact
directory.

Quick local smoke:

```sh
cargo build --bin octos-tui
scripts/run-m18-stdio-live-tmux-soak.sh run-once
```

Repeated local or nightly command:

```sh
OCTOS_TUI_M18_STDIO_REPEAT_COUNT=10 \
OCTOS_TUI_M18_STDIO_FAILURE_BUDGET=0 \
scripts/run-m18-stdio-live-tmux-soak.sh repeat
```

The repeat report is written under
`e2e/test-results-m18-stdio-live-tmux/<run-id>-repeat-report.json`. Failed
runs keep their per-run artifact directory and repeat log. Use
`OCTOS_TUI_M18_STDIO_RUN_COMMAND` only when CI already has a stricter live
stdio tmux command; the repeat harness will still count the command's real exit
status and preserve the configured artifact directory.

Harness-only self-test:

```sh
scripts/run-m18-stdio-live-tmux-soak.sh self-test
```

The self-test validates report accounting with synthetic child commands. It is
not live M18 evidence.
