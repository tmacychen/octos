# Crew App Skill Development Guide

This guide covers everything you need to build, register, and deploy a new app skill for crew-rs.

---

## Architecture Overview

An app skill is a **standalone executable binary** that communicates with the crew gateway via a simple **stdin/stdout JSON protocol**. The gateway spawns the skill binary as a child process for each tool call, passes JSON arguments on stdin, and reads JSON results from stdout.

```
User message → LLM → tool_use("get_weather", {"city": "Paris"})
                         ↓
              Gateway spawns: ~/.crew/skills/weather/main get_weather
                         ↓
              Stdin:  {"city": "Paris"}
              Stdout: {"output": "Paris, France\nClear sky\n...", "success": true}
                         ↓
              LLM sees result → generates natural language response
```

---

## Skill Directory Structure

Each skill lives in its own crate under `crates/app-skills/`:

```
crates/app-skills/my-skill/
├── Cargo.toml          # Crate config, binary name
├── manifest.json       # Tool definitions (JSON Schema)
├── SKILL.md            # Documentation + frontmatter metadata
└── src/
    └── main.rs         # Binary entry point
```

After bootstrapping, the skill is installed at:

```
~/.crew/skills/my-skill/
├── main                # Executable binary (copied from target/)
├── manifest.json       # Tool definitions
└── SKILL.md            # Documentation
```

---

## Step-by-Step: Create a New Skill

### 1. Create the Crate

```bash
mkdir -p crates/app-skills/my-skill/src
```

### 2. Cargo.toml

```toml
[package]
name = "my-skill"
version = "1.0.0"
edition = "2021"
description = "Short description of what this skill does"
authors = ["your-name"]

[[bin]]
name = "my_skill"          # Binary name (used in bundled_app_skills.rs)
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# Add other deps as needed:
# reqwest = { version = "0.12", features = ["blocking", "rustls-tls", "json"], default-features = false }
# chrono = "0.4"
```

**Important:** The `[[bin]] name` must match the `binary_name` in `bundled_app_skills.rs`.

### 3. manifest.json

Defines the tools the LLM can call. Uses JSON Schema for input validation.

```json
{
  "name": "my-skill",
  "version": "1.0.0",
  "author": "your-name",
  "description": "What this skill does",
  "timeout_secs": 15,
  "requires_network": false,
  "tools": [
    {
      "name": "my_tool",
      "description": "Clear description for the LLM. What does this tool do? When should it be used?",
      "input_schema": {
        "type": "object",
        "properties": {
          "param1": {
            "type": "string",
            "description": "What this parameter means"
          },
          "param2": {
            "type": "integer",
            "description": "Optional numeric parameter (default: 10)"
          }
        },
        "required": ["param1"]
      }
    }
  ]
}
```

**Manifest fields:**

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | Yes | — | Skill identifier |
| `version` | Yes | — | Semantic version |
| `author` | No | — | Author name |
| `description` | No | — | Human-readable description |
| `timeout_secs` | No | 30 | Max execution time per tool call (1-600) |
| `requires_network` | No | false | Informational flag |
| `sha256` | No | — | Binary integrity check (hex hash) |
| `tools` | Yes | — | Array of tool definitions |

**Tool definition fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Tool name (snake_case, globally unique) |
| `description` | Yes | Description shown to LLM — be specific about when to use |
| `input_schema` | Yes | JSON Schema for input parameters |

### 4. SKILL.md

Documentation with YAML frontmatter. The LLM reads this to understand when and how to use the skill.

```markdown
---
name: my-skill
description: Short description. Triggers: keyword1, keyword2, 关键词, trigger phrase.
version: 1.0.0
author: your-name
always: false
---

# My Skill

Detailed description of what this skill does and when to use it.

## Tools

### my_tool

Explain what this tool does with examples.

\```json
{"param1": "example value", "param2": 5}
\```

**Parameters:**
- `param1` (required): What it means
- `param2` (optional): What it controls. Default: 10
```

**Frontmatter fields:**

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | Yes | — | Skill identifier |
| `description` | Yes | — | One-line description. Include trigger keywords after "Triggers:" |
| `version` | Yes | — | Semantic version |
| `author` | No | — | Author name |
| `always` | No | `false` | If `true`, skill docs are always included in system prompt |
| `requires_bins` | No | — | Comma-separated binaries that must exist (checked via `which`) |
| `requires_env` | No | — | Comma-separated env vars that must be set |

**Trigger keywords** help the agent decide when to activate the skill. Include terms in multiple languages if your users are multilingual.

### 5. src/main.rs

The binary implements the stdin/stdout protocol.

**Minimal template:**

```rust
use std::io::Read;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct MyToolInput {
    param1: String,
    #[serde(default = "default_param2")]
    param2: i32,
}

fn default_param2() -> i32 { 10 }

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tool_name = args.get(1).map(|s| s.as_str()).unwrap_or("unknown");

    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        fail(&format!("Failed to read stdin: {e}"));
    }

    match tool_name {
        "my_tool" => handle_my_tool(&buf),
        _ => fail(&format!("Unknown tool '{tool_name}'. Expected: my_tool")),
    }
}

fn fail(msg: &str) -> ! {
    println!("{}", json!({"output": msg, "success": false}));
    std::process::exit(1);
}

fn handle_my_tool(input_json: &str) {
    let input: MyToolInput = match serde_json::from_str(input_json) {
        Ok(v) => v,
        Err(e) => fail(&format!("Invalid input: {e}")),
    };

    // ... your logic here ...

    let result = format!("Processed {} with param2={}", input.param1, input.param2);
    println!("{}", json!({"output": result, "success": true}));
}
```

**Protocol rules:**

1. **argv[1]** = tool name (e.g., `get_weather`, `get_forecast`)
2. **stdin** = JSON object matching the tool's `input_schema`
3. **stdout** = JSON object with:
   - `output` (string): Human-readable result text
   - `success` (bool): `true` for success, `false` for failure
4. **exit code**: 0 for success, non-zero for failure
5. **stderr**: Ignored by the gateway (use for debug logging)

---

## Register the Skill

### 6. Add to Workspace

In the root `Cargo.toml`, add to `members`:

```toml
[workspace]
members = [
    # ... existing members ...
    "crates/app-skills/my-skill",
]
```

### 7. Register in bundled_app_skills.rs

In `crates/crew-agent/src/bundled_app_skills.rs`, add to `BUNDLED_APP_SKILLS`:

```rust
pub const BUNDLED_APP_SKILLS: &[(&str, &str, &str, &str)] = &[
    // ... existing skills ...
    (
        "my-skill",                                          // dir_name (skill directory name)
        "my_skill",                                          // binary_name (must match [[bin]] name)
        include_str!("../../app-skills/my-skill/SKILL.md"),  // embedded docs
        include_str!("../../app-skills/my-skill/manifest.json"), // embedded manifest
    ),
];
```

**Tuple format:** `(dir_name, binary_name, skill_md, manifest_json)`

- `dir_name`: Name of the directory under `~/.crew/skills/`
- `binary_name`: Name of the binary in `target/release/` (must match `[[bin]] name` in Cargo.toml)
- `skill_md`: Embedded SKILL.md content
- `manifest_json`: Embedded manifest.json content

---

## Build & Test

### 8. Build

```bash
# Build just your skill
cargo build -p my-skill

# Build everything
cargo build --workspace
```

### 9. Test Standalone

```bash
# Test your tool directly
echo '{"param1": "hello", "param2": 5}' | ./target/debug/my_skill my_tool

# Expected output:
# {"output":"Processed hello with param2=5","success":true}

# Test error handling
echo '{}' | ./target/debug/my_skill my_tool
echo '{"param1": "test"}' | ./target/debug/my_skill unknown_tool
```

### 10. Test with Gateway

```bash
# Build release + install
cargo build --release --workspace

# Start gateway (skills are bootstrapped automatically)
crew gateway

# Check skill was loaded
ls ~/.crew/skills/my-skill/
# main  manifest.json  SKILL.md

# Ask the agent to use your skill
```

---

## Examples

### Example 1: Local-Only Skill (Clock)

No network, no env vars. Uses `chrono` + `chrono-tz`.

```
crates/app-skills/time/
├── Cargo.toml          # deps: chrono, chrono-tz, serde, serde_json
├── manifest.json       # 1 tool: get_time, timeout_secs: 5
├── SKILL.md            # Triggers: time, clock, 几点
└── src/main.rs         # Reads system clock, formats with timezone
```

**Key pattern:** Default to local time when no timezone given.

### Example 2: Network Skill (Weather)

Calls external API, needs network. Uses `reqwest` (blocking).

```
crates/app-skills/weather/
├── Cargo.toml          # deps: reqwest (blocking, rustls-tls), serde, serde_json
├── manifest.json       # 2 tools: get_weather, get_forecast, timeout_secs: 15
├── SKILL.md            # Triggers: weather, forecast, 天气
└── src/main.rs         # Geocode city → fetch weather from Open-Meteo
```

**Key patterns:**
- Build HTTP client with timeouts
- Handle API errors gracefully (return `success: false`)
- URL-encode user input
- Multiple tools in one binary (match on `argv[1]`)

### Example 3: Env-Var Skill (Send Email)

Requires credentials from environment variables.

```
crates/app-skills/send-email/
├── Cargo.toml          # deps: lettre, serde, serde_json, reqwest
├── manifest.json       # 1 tool: send_email
├── SKILL.md            # requires_env: SMTP_HOST,SMTP_USERNAME,SMTP_PASSWORD
└── src/main.rs         # Reads SMTP_* env vars, sends via SMTP
```

**Key pattern:** Check env vars early, fail with clear error message.

```rust
fn get_smtp_config() -> SmtpConfig {
    let host = std::env::var("SMTP_HOST")
        .unwrap_or_else(|_| fail("SMTP_HOST env var not set"));
    // ...
}
```

---

## Advanced Topics

### Multiple Tools in One Skill

A single skill binary can implement multiple tools. The tool name is passed as `argv[1]`:

```rust
match tool_name {
    "get_weather" => handle_get_weather(&buf),
    "get_forecast" => handle_get_forecast(&buf),
    _ => fail(&format!("Unknown tool '{tool_name}'")),
}
```

Each tool must be declared in `manifest.json`:

```json
{
  "tools": [
    { "name": "get_weather", "description": "...", "input_schema": { ... } },
    { "name": "get_forecast", "description": "...", "input_schema": { ... } }
  ]
}
```

### Environment Variables

Skills inherit the gateway's environment (minus blocked vars). To use API keys:

```rust
let api_key = std::env::var("MY_API_KEY")
    .unwrap_or_else(|_| fail("MY_API_KEY not set"));
```

Declare requirements in SKILL.md frontmatter so the skill is marked unavailable when env vars are missing:

```yaml
---
requires_env: MY_API_KEY
---
```

### Timeout Configuration

Set appropriate timeouts in `manifest.json`:

| Skill Type | Recommended Timeout |
|------------|-------------------|
| Local computation | 5s |
| Single API call | 15s |
| Multi-step API calls | 30-60s |
| Long-running research | 300-600s |

### Security

The gateway automatically sanitizes these env vars before spawning skill processes:

- `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`
- `NODE_OPTIONS`, `PYTHONPATH`, `PERL5LIB`
- `RUSTFLAGS`, `RUST_LOG`
- And 10+ others (see `BLOCKED_ENV_VARS` in `sandbox.rs`)

Skills should also:
- Validate all input (never trust `city`, `path`, etc.)
- Use timeouts on HTTP requests
- Avoid shell injection (don't pass user input to shell commands)

### Platform Skills vs App Skills

| | App Skills | Platform Skills |
|---|---|---|
| **Location** | `crates/app-skills/` | `crates/platform-skills/` |
| **Array** | `BUNDLED_APP_SKILLS` | `PLATFORM_SKILLS` |
| **Bootstrap** | Every gateway startup | Admin bot only |
| **Scope** | Per-gateway | Shared across all gateways |
| **Use when** | Always available, self-contained | Requires external service |

### Updating Skills Without Full Rebuild

Skills can be rebuilt and deployed independently:

```bash
# Build just the skill
cargo build --release -p weather

# Copy to remote server
scp target/release/weather remote:~/.crew/skills/weather/main

# No gateway restart needed — next tool call uses the new binary
```

Note: If you change `SKILL.md` or `manifest.json`, you must rebuild the `crew` binary too (they're embedded via `include_str!`).

---

## Checklist

- [ ] Create `crates/app-skills/<name>/` with Cargo.toml, manifest.json, SKILL.md, src/main.rs
- [ ] `[[bin]] name` in Cargo.toml matches `binary_name` in bundled_app_skills.rs
- [ ] manifest.json has valid JSON Schema for all tool inputs
- [ ] SKILL.md has frontmatter with trigger keywords
- [ ] Binary reads `argv[1]` for tool name, stdin for JSON input
- [ ] Binary writes `{"output": "...", "success": true/false}` to stdout
- [ ] Error cases return `success: false` with clear message
- [ ] Add to workspace `Cargo.toml` members
- [ ] Add to `BUNDLED_APP_SKILLS` in `bundled_app_skills.rs`
- [ ] `cargo build --workspace` succeeds
- [ ] Standalone test: `echo '{"param": "value"}' | ./target/debug/my_skill my_tool`
- [ ] Gateway test: skill appears in `~/.crew/skills/` and agent can use it
