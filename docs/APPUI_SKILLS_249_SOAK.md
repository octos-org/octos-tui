# AppUI Skills 249 Soak

Manual tmux smoke for profile skill management:

1. Start the backend in one pane:
   `cargo run -p octos-cli -- serve --stdio`
2. Start the TUI in another pane:
   `cargo run -- --mode protocol --stdio-command "cargo run -p octos-cli -- serve --stdio" --profile coding`
3. In the TUI, run:
   `/skills list`
   `/skills search research`
   `/skills install <github-shorthand-or-local-skill-path> --force`
   `/skills remove <installed-skill-name>`
4. Open `/skills` and verify the menu is capability-gated, shows cached installed skills and registry packages, and all install/remove actions travel over AppUI methods only.

Expected: no REST calls from the TUI, no local profile directory inspection, and mutation results refresh `profile/skills/list`.
