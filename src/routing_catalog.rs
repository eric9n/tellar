/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/routing_catalog.rs
 * Responsibility: Build the routing-time tool catalog exposed to the router.
 */

use crate::config::Config;
use crate::skills::skill_discovery_stamp;
use crate::tools::get_routing_tool_definitions;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

static HOST_ABSOLUTE_PATH_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(^|[\s`'"(])/(?:[^/\s]+/)*[^/\s]+"#).expect("valid host path regex")
});

pub(crate) struct RoutingToolCatalog {
    pub(crate) allowed_tools: HashSet<String>,
    pub(crate) rendered_specs: String,
}

#[derive(Clone)]
struct CachedRoutingCatalog {
    skill_stamp: crate::skills::SkillDiscoveryStamp,
    allowed_tools: HashSet<String>,
    specs: Value,
    rendered_specs: String,
}

fn has_host_absolute_path(text: &str) -> bool {
    HOST_ABSOLUTE_PATH_RE.is_match(text)
}

static ROUTING_TOOL_CACHE: Lazy<RwLock<HashMap<PathBuf, CachedRoutingCatalog>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

fn render_tool_specs(specs: &Value) -> String {
    serde_json::to_string_pretty(specs).unwrap_or_else(|_| "[]".to_string())
}

fn collect_uncached_routing_catalog(base_path: &Path) -> CachedRoutingCatalog {
    let specs = get_routing_tool_definitions(base_path);
    let rendered_specs = render_tool_specs(&specs);
    let allowed_tools = specs
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<HashSet<_>>();

    CachedRoutingCatalog {
        skill_stamp: skill_discovery_stamp(base_path),
        allowed_tools,
        specs,
        rendered_specs,
    }
}

fn base_routing_catalog(base_path: &Path) -> CachedRoutingCatalog {
    let cache_key = base_path.to_path_buf();
    let skill_stamp = skill_discovery_stamp(base_path);

    if let Some(cached) = ROUTING_TOOL_CACHE
        .read()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
        && cached.skill_stamp == skill_stamp {
            return cached;
        }

    let fresh = collect_uncached_routing_catalog(base_path);
    if let Ok(mut cache) = ROUTING_TOOL_CACHE.write() {
        cache.insert(cache_key, fresh.clone());
    }
    fresh
}

pub(crate) fn collect_routing_tool_catalog(
    base_path: &Path,
    config: &Config,
    text: &str,
) -> RoutingToolCatalog {
    let cached = base_routing_catalog(base_path);
    let mut tool_specs = cached.specs.clone();
    let mut rendered_specs = cached.rendered_specs.clone();

    if has_host_absolute_path(text) {
        let allowed_host_tools: &[&str] = if config.runtime.privileged {
            &["exec", "send_attachment", "send_attachments"]
        } else {
            &[]
        };

        let filtered = tool_specs
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| allowed_host_tools.contains(&name))
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        tool_specs = Value::Array(filtered);
        rendered_specs = render_tool_specs(&tool_specs);
    }

    let allowed_tools = if has_host_absolute_path(text) {
        tool_specs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect::<HashSet<_>>()
    } else {
        cached.allowed_tools.clone()
    };

    RoutingToolCatalog {
        allowed_tools,
        rendered_specs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DiscordConfig, GeminiConfig, RuntimeConfig};
    use crate::skills::SkillMetadata;
    use std::fs;
    use tempfile::tempdir;

    fn write_skill(base: &Path, dir_name: &str, body: &str) {
        let skill_dir = base.join("skills").join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), body).unwrap();
    }

    fn test_config() -> Config {
        Config {
            gemini: GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake".to_string(),
            },
            discord: DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: RuntimeConfig::default(),
        }
    }

    #[test]
    fn test_collect_routing_tool_catalog_reads_installed_skills() {
        let tmp = tempdir().unwrap();
        write_skill(
            tmp.path(),
            "snapshot",
            r#"---
name: snapshot
description: Test snapshot skill.
tools:
  snapshot:
    description: Snapshot test tool.
    shell: ./scripts/snapshot.sh
    parameters:
      type: object
---
"#,
        );

        let config = test_config();
        let catalog = collect_routing_tool_catalog(tmp.path(), &config, "use snapshot");

        assert!(catalog.allowed_tools.contains("ls"));
        assert!(catalog.allowed_tools.contains("snapshot"));

        let specs: Value = serde_json::from_str(&catalog.rendered_specs).unwrap();
        let specs = specs.as_array().unwrap();
        assert!(
            specs
                .iter()
                .any(|entry| entry.get("name") == Some(&Value::String("snapshot".to_string())))
        );
        assert_eq!(SkillMetadata::discover_skills(tmp.path()).len(), 1);
    }

    #[test]
    fn test_collect_routing_tool_catalog_limits_host_path_requests_to_host_capable_tools() {
        let tmp = tempdir().unwrap();
        let mut config = test_config();
        config.runtime.privileged = true;
        let catalog = collect_routing_tool_catalog(
            tmp.path(),
            &config,
            "find and send /root/process_intel.py",
        );

        assert!(catalog.allowed_tools.contains("exec"));
        assert!(catalog.allowed_tools.contains("send_attachment"));
        assert!(catalog.allowed_tools.contains("send_attachments"));
        assert!(!catalog.allowed_tools.contains("find"));
        assert!(!catalog.allowed_tools.contains("read"));
        assert!(!catalog.allowed_tools.contains("ls"));
    }

    #[test]
    fn test_collect_routing_tool_catalog_hides_host_tools_when_not_privileged() {
        let tmp = tempdir().unwrap();
        let config = test_config();
        let catalog = collect_routing_tool_catalog(
            tmp.path(),
            &config,
            "find and send /root/process_intel.py",
        );

        assert!(!catalog.allowed_tools.contains("exec"));
        assert!(!catalog.allowed_tools.contains("send_attachment"));
        assert!(!catalog.allowed_tools.contains("send_attachments"));
        assert!(catalog.allowed_tools.is_empty());
        let specs: Value = serde_json::from_str(&catalog.rendered_specs).unwrap();
        assert_eq!(specs.as_array().map(Vec::len), Some(0));
    }
}
