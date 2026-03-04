# 🕯️ Tellar — The Cyber Steward

> "In the dance between the digital and the physical, Tellar is the silent partner who keeps the rhythm."

Tellar is a **Minimalist, Document-Driven Task Processor** for Discord servers (Guilds). Built with a **Reactive Blackboard Architecture**, Tellar treats every Discord channel as a filesystem-backed task surface: it identifies task intent, generates a bounded execution plan, executes it precisely, and records the outcome in durable documents.

---

## 🏛️ Core Philosophy

Tellar is built on the principle of **Intelligent Minimalism**. It is not a casual chat bot. It is a bounded task processor that should recognize what the user wants, convert it into a precise task, execute the smallest safe plan, and clearly report success, failure, or missing input.

- **The Blackboard is the State**: No hidden databases. If Tellar knows it, it's written in a file.
- **Task-First Routing**: Every request is classified as executable, missing input, or not supported.
- **Explicit Failure**: If a task cannot be completed, Tellar should say so directly and explain why.
- **Ritualistic Execution**: Tasks aren't just "jobs"; they are filesystem-backed rituals and thread steps with durable state.

---

## 🏗️ Architecture

### 1. The Guild Foundation (会馆)
The local root of Tellar's consciousness. It mirrors your Discord Server structure, organizing knowledge and state into a predictable hierarchy.

### 2. Decentralized Knowledge
Knowledge is not a monolith. Each channel maintains its own `KNOWLEDGE.md`, allowing task execution to stay anchored to the local context that produced it.

### 3. The Steward (管家)
The reactive role entrypoint. The Steward responds to channel and ritual events, while the underlying runtime is split into focused layers for context, routing, finite plan execution, and tool dispatch.

### 4. Runtime Layers
The runtime is intentionally split into narrow modules instead of one monolithic agent file:

- **Architecture diagram**: See [`docs/architecture-overview.svg`](docs/architecture-overview.svg) for a current high-level module and execution flow map.

- **`tools.rs`**: Core tool definitions, tool dispatch, and local safety boundaries.
- **`prompt_context.rs`**: System prompt loading and prompt-related test helpers.
- **`thread/doc.rs`**: Thread document parsing and task-thread metadata inspection.
- **`session.rs`**: Session assembly and plan-first request execution.
- **`plan_executor.rs`**: The main deterministic execution core for conversational and ritual flows.
- **`thread/mod.rs` / `thread/store.rs`**: Thread-file execution, result persistence, and archival flow.
- **`watch.rs` / `rhythm.rs`**: File-watch and ritual scheduling triggers that feed thread execution.

---

## 🛠️ Primitive Capabilities

 Tellar adheres to a small, orthogonal core toolset:
- **`ls`**: Discover files and directories.
- **`find`**: Locate files or directories by name when the path is unknown.
- **`grep`**: Search for relevant text before opening files.
- **`read`**: Perception of the foundations with offset/limit precision.
- **`write`**: Persistence of intent and memory.
- **`edit`**: Surgical, safe modification of existing state.

Everything outside local cognition should be modeled as a **Skill**. Core tools inspect and modify durable workspace state; skills handle domain-specific or external capabilities and should preferably write their results back into the guild filesystem.

Skills are treated as **user-installed, trusted local extensions**. Tellar starts each skill in that skill's directory for predictable relative paths, but it does **not** sandbox the skill to that directory. A skill can run host commands and access any location available to the Tellar process. Tellar does not attempt to provide partial or misleading isolation here: if you install a skill, you are responsible for reviewing and trusting its behavior.

### Discord Delivery Tools

Tellar also exposes a dedicated Discord delivery layer for returning artifacts back to the active channel:

- **`send_message`**: Send plain text, with automatic newline-aware chunking for long output.
- **`send_reply`**: Reply to a specific Discord message ID.
- **`send_embed`**: Send a simple rich embed with title, description, and optional color.
- **`send_attachment`**: Send one local file as an attachment.
- **`send_attachments`**: Send multiple local files as separate attachments.
- **`send_image`**: Send an image file with image-focused delivery semantics.
- **`send_code_block`**: Send formatted code or logs in a fenced block.
- **`send_text_file`**: Materialize large text into a file and send it as an attachment.

This keeps execution and delivery separate: `exec` or the local cognition tools produce results, while delivery tools decide how those results are sent back to Discord.

### Runtime Guardrails

Tellar now runs through explicit finite plans instead of open-ended agent loops. The main guardrails are therefore task-centric:

- **Plan-first execution**: requests are routed into `plan`, `needs_input`, or `reject`.
- **Bounded steps**: execution is limited to explicit `CallTool`, `Respond`, and `AskForMissing` plan steps.
- **Explicit outcomes**: every task ends as `Completed`, `NeedsInput`, `Failed`, or `Rejected`.
- **No silent fallback**: unsupported or blocked work is surfaced directly instead of drifting into exploratory behavior.

---

## 🚀 Getting Started

### Installation
Tellar is a single-binary portable engine written in Rust.

```bash
git clone https://github.com/eric9n/tellar.git
cd tellar
cargo install --path .
```

### Setup
Run Tellar to enter the **Interactive Setup**:

```bash
tellar                # Defaults to ~/.tellar/guild
# OR
tellar --guild ./my-guild
```

1. **Inscribe Keys**: Provide your Gemini API Key and Discord Token.
2. **Select a Brain**: Choose from available Gemini models.
3. **Define Identity**: Edit your Steward's personality in `agents/AGENTS.md`.

### Minimal `tellar.yml`

The generated config can stay small. A typical baseline looks like this:

```yaml
gemini:
  api_key: YOUR_GEMINI_API_KEY
  model: gemini-3-flash-preview
discord:
  token: YOUR_DISCORD_BOT_TOKEN
runtime:
  max_turns: 16
  read_only_budget: 4
  max_tool_output_bytes: 5000
```

`runtime` controls the main safety and convergence limits for the native tool-calling loop.

### Per-Channel Customization
Tellar supports unique identities for different channels. Place `<CHANNEL_ID>.AGENTS.md` in your `agents/` directory to supplement the base instructions for specific contexts.

### Recommended Guild Layout

Tellar works best when the guild filesystem follows a stable, predictable layout:

```text
guild/
├── tellar.yml
├── agents/
│   ├── AGENTS.md
│   └── GUARDIAN.md
├── brain/
│   ├── KNOWLEDGE.md
│   └── events/
├── channels/
│   └── <channel-folder>/
│       ├── KNOWLEDGE.md
│       ├── 2026-02-27.md
│       └── history/
├── rituals/
│   └── ...
└── skills/
    └── ...
```

- **`tellar.yml`**: local runtime configuration.
- **`agents/`**: role prompts and channel-specific identity overrides.
- **`brain/KNOWLEDGE.md`**: global distilled memory shared across the guild.
- **`brain/events/`**: optional system-wide or cross-channel event records.
- **`channels/<channel>/KNOWLEDGE.md`**: long-lived memory for one Discord channel.
- **`channels/<channel>/YYYY-MM-DD.md`**: day log / conversation blackboard for that channel.
- **`channels/<channel>/history/`**: archived completed thread files.
- **`rituals/`**: scheduled or longer-running task documents.
- **`skills/`**: external or domain-specific capabilities beyond the core local tools.

The core tools are designed around this layout: use `find` to locate paths, `ls` to inspect structure, `grep` to narrow content, and `read` before `write` or `edit`.

### Installing Skills

Tellar treats `SKILL.md` as the canonical skill source. `SKILL.json` is an optional compiled cache that Tellar can generate for faster, more predictable runtime loading.

Install a local skill by compiling its `SKILL.md` into `SKILL.json`:

```bash
tellarctl install-skill /path/to/skill
```

This command uses your configured Gemini model to compile the skill into machine-readable metadata, validates the result, and writes `SKILL.json` next to `SKILL.md`.

Runtime behavior:

- If `SKILL.json` exists and is valid, Tellar uses it as a cache for runtime loading.
- If `SKILL.json` is missing, Tellar reads `SKILL.md` directly.
- If `SKILL.json` exists but is invalid, Tellar falls back to `SKILL.md`.

`tellarctl install-skill` is therefore a build step for runtime speed and determinism, not a requirement for a skill to exist.

---

## 🎭 Ritual Mode
To execute complex tasks, create a **Ritual** in the `rituals/` directory. Rituals support:
- **Schedules**: Use cron expressions for recurring maintenance.
- **Status Tracking**: Move tasks from `[ ]` to `[x]` as the Steward progresses.
- **Shared Vision**: Attach images or context that the Steward can perceive and act upon.

---

## 🖥️ Service Management (Ubuntu / systemd)

To run Tellar as a persistent service on Ubuntu, use the provided `tellarctl` CLI:

1. **Install Tellar**: Run `cargo install --path .` (this installs both `tellar` and `tellarctl` to `~/.cargo/bin/`).
2. **Setup Workspace**: Run the management command with the **absolute path** to your guild (defaults to `~/.tellar/guild` if omitted):
   ```bash
   tellarctl setup                # Defaults to ~/.tellar/guild
   # OR
   tellarctl setup --guild /path/to/custom/guild
   ```
3. **Install Service**:
   ```bash
   tellarctl install-service      # Linux systemd user service
   ```
4. **Control Commands**:
   - **Start**: `tellarctl start`
   - **Stop**: `tellarctl stop`
   - **Restart**: `tellarctl restart`
   - **Status**: `tellarctl status`
   - **Logs**: `tellarctl logs` (Follow real-time output)

---

## ⚖️ License
Distributed under the MIT License. See `LICENSE` for more information.

---
*Built for those who treat their Discord servers as temples of knowledge.*
