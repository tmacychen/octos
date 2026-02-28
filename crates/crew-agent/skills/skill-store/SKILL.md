---
name: skill-store
description: Browse and install community skills from the skill registry.
---

# Skill Store

When the user asks to browse skills, install skills, find available skills, show skill store, or similar (including Chinese: 技能商店, 安装技能, 查看技能, 浏览技能):

## Browse Available Skills

Run this command to show all available skill packages:

```bash
crew skills search --cwd {{CWD}}
```

To search for a specific skill:

```bash
crew skills search <query> --cwd {{CWD}}
```

Show the output to the user so they can see what's available.

## Install a Skill Package

When the user wants to install a specific package, use the repo path from the search results:

```bash
crew skills install <user/repo> --cwd {{CWD}}
```

For example:
```bash
# Install all mofa skills
crew skills install mofa-org/mofa-skills --cwd {{CWD}}

# Install a single skill from a package
crew skills install mofa-org/mofa-skills/mofa-slides --cwd {{CWD}}
```

## Options

- Add `--force` to overwrite existing skills
- Add `--branch <tag>` to install a specific version (default: main)

## After Install

Tell the user:
1. Skills are installed. Run `crew skills list --cwd {{CWD}}` to verify.
2. Check individual skill requirements (e.g. API keys) in the skill's documentation.
