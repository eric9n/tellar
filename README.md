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
The reactive heart. The Steward observes the channels and rituals, fulfilling intent inscribed on the blackboards using iterative **ReAct loops**.

### 4. The Guardian (守护者)
The proactive soul. While the Steward is reactive, the Guardian is observational—auditing health, distilling history into knowledge, and ensuring the foundations remain solid.

---

## 🛠️ Primitive Capabilities

Tellar adheres to the **pi-mono** standard, providing the essential tools:
- **`read`**: Perception of the foundations with offset/limit precision.
- **`write`**: Persistence of intent and memory.
- **`edit`**: Surgical, safe modification of existing state.
- **`bash`**: Direct, scoped action on the environment.

Advanced capabilities like **Image Generation** or **Notifications** are implemented as pluggable **Skills**.

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
tellar --guild ./my-guild
```

1. **Inscribe Keys**: Provide your Gemini API Key and Discord Token.
2. **Select a Brain**: Choose from available Gemini models.
3. **Define Identity**: Edit `agents/AGENTS.md` to shape your Steward's personality.

### Per-Channel Customization
Tellar supports unique identities for different channels. Place `<CHANNEL_ID>.AGENTS.md` in your `agents/` directory to supplement the base instructions for specific contexts.

---

## 🎭 Ritual Mode
To execute complex tasks, create a **Ritual** in the `rituals/` directory. Rituals support:
- **Schedules**: Use cron expressions for recurring maintenance.
- **Status Tracking**: Move tasks from `[ ]` to `[x]` as the Steward progresses.
- **Shared Vision**: Attach images or context that the Steward can perceive and act upon.

---

## 🖥️ Service Management (Ubuntu / systemd)

To run Tellar as a persistent service on Ubuntu using `systemd --user`:

1. **Install Binary**: Run `cargo install --path .` to install Tellar to `~/.cargo/bin/tellar`.
2. **Setup Unit**: Create `~/.config/systemd/user/tellar.service` with the content from `scripts/tellar.service`.
3. **Configure Paths**: Edit the service file to point `WorkingDirectory` and `ExecStart` to your actual guild and binary paths (usually `ExecStart=%h/.cargo/bin/tellar`).
4. **Control Commands**:
   ```bash
   systemctl --user daemon-reload
   systemctl --user enable tellar
   systemctl --user start tellar
   systemctl --user status tellar
   ```
5. **Persistence**: Enable "lingering" so the service starts on boot and runs without an active session:
   ```bash
   loginctl enable-linger $USER
   ```

---

## ⚖️ License
Distributed under the MIT License. See `LICENSE` for more information.

---
*Built for those who treat their Discord servers as temples of knowledge.*
