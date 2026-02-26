# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-02-25

### Added
- **Embedded Assets**: The binary now embeds its own `guild/` template, making it fully portable.
- **Unified Initialization**: Data-driven setup logic using `extract_embedded_assets`.
- **Guild Foundation Assets**: Pre-initialized `KNOWLEDGE.md` and `WELCOME.md` for new server setups.

### Changed
- **Conceptual Alignment**: Renamed all "Workspace" terminology to **"Guild"** to match Discord's model.
- **Tool Refactor**: Aligned core tools (`read`, `write`, `edit`, `bash`) with the **pi-mono** standard.
- **Skill-ization**: Moved `notify` and `draw` tools from core logic to pluggable external skills.
- **Path Resolution**: Improved directory discovery with support for local `guild/` folders and `TELLAR_GUILD` env var.

### Removed
- Unused Rust dependencies for base64 and image generation (now handled by Python skills).
