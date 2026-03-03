# Account Manager Skill

version: 1.0.0
author: hagency

## Overview

This skill provides the `manage_account` tool for managing sub-accounts under the current profile. Sub-accounts share the parent profile's LLM provider configuration and API keys (same billing) but have their own data directory, memory, sessions, skills, and messaging channels.

## When to Use

Use this tool when the user asks to:
- List their sub-accounts ("show my sub-accounts", "what accounts do I have")
- Create a new sub-account ("create a work bot", "set up a new assistant called X")
- Delete a sub-account ("remove the work bot", "delete sub-account X")
- Check sub-account details ("show info about work bot", "what's the status of X")

## Usage

### List sub-accounts

```json
{ "action": "list" }
```

### Create a sub-account

```json
{
  "action": "create",
  "name": "work bot",
  "system_prompt": "You are a professional work assistant.",
  "telegram_token": "123456:ABC-DEF...",
  "enable": true
}
```

Only `name` is required. Other fields are optional.

### Delete a sub-account

```json
{
  "action": "delete",
  "sub_account_id": "parent-id--work-bot"
}
```

### Get sub-account info

```json
{
  "action": "info",
  "sub_account_id": "parent-id--work-bot"
}
```

## Environment Variables

This tool reads `CREW_HOME` and `CREW_PROFILE_ID` from the environment (set automatically by the gateway). No manual configuration is needed.
