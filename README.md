# 🕯️ Tellar — The Discord Steward

<p align="center">
  <strong>Reactive Blackboard Architecture for Collaborative Stewardship.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg?style=for-the-badge" alt="MIT License"></a>
  <a href="https://rust-lang.org"><img src="https://img.shields.io/badge/Language-Rust-orange?style=for-the-badge&logo=rust" alt="Rust"></a>
</p>

**Tellar** is a minimalist, reactive AI steward designed for Discord servers (Guilds). Unlike traditional bots, Tellar operates on a **Blackboard Architecture** where the Discord channel history and a local filesystem (the "Guild Foundation") are used as a shared workspace for perception and action.

It observes Discord messages, updates its local state (Knowledge, Rituals, Threads), and reacts as an agentic collaborator.

## 🏛️ Core Concepts

- **Guild (会馆)**: The local root of Tellar's consciousness. It mirrors your Discord Server structure.
- **Channels**: Discord channels are projected into local markdown files. Knowledge is decentralized: each channel folder maintains its own `KNOWLEDGE.md`.
- **Brain**: A dedicated `brain/` directory handles system metadata, attachments, and global memory, decoupled from user channels.
- **Rituals**: Scheduled or reactive task threads. Rituals are isolated task loops that Tellar performs to manage the guild.
- **Watchman**: A filesystem-reactive engine that awakens the Steward whenever local foundations are touched.

## 🛠️ The Minimalist Core
Tellar adheres to the **pi-mono** standard of tool-calling, providing only the essential primitives:
1. `read`: Perception of foundations.
2. `write`: Persistence of memories.
3. `edit`: Precision surgery on state.
4. `bash`: Direct action on the environment.

Advanced capabilities (`notify`, `draw`, etc.) are implemented as **Pluggable Skills**.

## 🚀 Getting Started

### Prerequisites
- [Rust](https://rust-lang.org) (stable)
- A Discord Bot Token (with Message Content Intent enabled)

### Installation
```bash
git clone https://github.com/dagow/tellar.git
cd tellar
cargo install --path .
```

### Setup
Tellar is **single-binary portable**. On first run, it detects if your configuration is empty and guides you through an **Interactive Setup**:

```bash
tellar --guild ./my-guild
```

1. Enter your Gemini API Key when prompted.
2. Select a model (e.g., `gemini-3-flash-preview`) from the list.
3. Tellar will automatically prepare your **`tellar.yml`**.
4. Configure your Steward's personality in `./my-guild/agents/AGENTS.md`.
5. Start Tellar and invite the bot to your server.


## ⚖️ License
Distributed under the MIT License. See `LICENSE` for more information.

---
*Built with ❤️ for those who treat their Discord servers as temples of knowledge.*
