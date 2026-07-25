---
"gthings": minor
---

- Feat: simplify commands — remove `gthings init`, merge shell setup into `gthings update` as all-in-one command (binary update + shell PATH config + skill install)
- Feat: add shell detection utility (bash, zsh, fish) with auto-PATH configuration to shell config files
- Feat: add `gthings skill add --opencode/--agents/--all` command for standalone skill installation
- Chore: strip emoji from all CLI output
- Chore: update embedded skill documentation for simplified command structure
