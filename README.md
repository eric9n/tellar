# 🕯️ Tellar — The Cyber Steward

> "In the dance between the digital and the physical, Tellar is the silent partner who keeps the rhythm."

Tellar is a **Minimalist, Document-Driven Cyber Steward** for Discord servers (Guilds). Built with a **Reactive Blackboard Architecture**, Tellar blurs the line between a filesystem and a social space, treating every Discord channel as a living parchment and every thread as a collaborative ritual.

---

## 🏛️ Core Philosophy

Tellar is built on the principle of **Intelligent Minimalism**. It doesn't aim to be a multi-functional bot with a thousand commands. Instead, it provides the core cognitive primitives—**Perception, Persistence, and Action**—allowing a guild to grow organic intelligence through documents.

- **The Blackboard is the State**: No hidden databases. If Tellar knows it, it's written in a file.
- **Agentic Collaboration**: Tellar doesn't just respond; it observes, maintains, and proposes.
- **Ritualistic Execution**: Tasks aren't just "jobs"; they are rituals synchronized between Discord and the Guild Foundation.

---

## 🏗️ Architecture

### 1. The Guild Foundation (会馆)
The local root of Tellar's consciousness. It mirrors your Discord Server structure, organizing knowledge and state into a predictable hierarchy.

### 2. Decentralized Knowledge
Knowledge is not a monolith. Each channel maintains its own `KNOWLEDGE.md`, allowing for context-aware intelligence that stays relevant to the conversation it grew from.

### 3. The Steward (管家)
The reactive role entrypoint. The Steward responds to channel and ritual events, while the underlying runtime is split into focused layers for context, tools, session assembly, and native tool-calling turns.

### 4. The Guardian (守护者)
The proactive soul. While the Steward is reactive, the Guardian is observational—auditing health, distilling history into knowledge, and ensuring the foundations remain solid.

### 5. Runtime Layers
The runtime is intentionally split into narrow modules instead of one monolithic agent file:

- **`tools.rs`**: Core tool definitions, tool dispatch, and local safety boundaries.
- **`context.rs`**: Prompt loading, blackboard parsing, steering detection, and image extraction.
- **`agent_loop.rs`**: Native tool-calling turn loop and batch execution policy.
- **`session.rs`**: Session assembly from prompts, local memory, and multimodal context.
- **`thread_runtime.rs`**: Thread-file execution, result persistence, and archival flow.
- **`steward.rs` / `guardian.rs`**: Role entrypoints, not shared infrastructure containers.

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

### Runtime Guardrails

Tellar uses native tool calling with a few explicit runtime constraints to keep long-running guild automation predictable:

- **Read-only tools batch together**: `ls`, `find`, `grep`, and `read` can run in the same turn.
- **Write tools force reevaluation**: `write` and `edit` end the current batch immediately.
- **Read-only budget**: each turn allows at most 4 read-only tool calls before forcing a new reasoning step.
- **Duplicate and dead-end detection**: repeated tool calls, repeated errors, and no-new-information loops are cut short.
- **Hard stop safety fuse**: the default agent loop stops after 16 turns if softer convergence rules fail.

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
