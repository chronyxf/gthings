---
gthings: minor
---

- Feat: add `gthings update` command — runs `cargo install gthings`
- Feat: add `gthings skill add --opencode/--agents/--all` command — installs embedded skill files
- Refactor: consolidate from 6 opencode skills to 1, merge reference files into SKILL.md
- Test: 9 unit tests for embedded skill structure, 5 integration tests for skill install
- Chore: remove old scripts/install-skills.sh, delete merged reference files (errors/agent-trace/agent-prompt)
