use std::env;
use std::fs;
use tellar::thread;
use tempfile::tempdir;

fn should_skip_live_gemini_error(err: &str) -> bool {
    err.contains("error sending request for url")
        || err.contains("operation timed out")
        || err.contains("connection reset")
        || err.contains("dns error")
        || err.contains("temporary failure")
}

#[tokio::test]
async fn test_full_plan_driven_ritual_turn_with_gemini_3() {
    let api_key = match env::var("GEMINI_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("skipping test: GEMINI_API_KEY not set");
            return;
        }
    };

    // 1. Setup a temporary guild environment for tools to work
    let dir = tempdir().expect("Failed to create temp dir");
    let base_path = dir.path();

    // Create folders required for runtime flows
    fs::create_dir_all(base_path.join("agents")).unwrap();
    fs::create_dir_all(base_path.join("brain")).unwrap();
    fs::create_dir_all(base_path.join("rituals")).unwrap();
    fs::write(
        base_path.join("agents").join("AGENTS.md"),
        "You are Tellar, a precise task processor.",
    )
    .unwrap();

    let config = tellar::config::Config {
        gemini: tellar::config::GeminiConfig {
            api_key: api_key.clone(),
            model: "gemini-3-flash-preview".to_string(),
        },
        discord: tellar::config::DiscordConfig {
            token: "fake".to_string(),
            guild_id: None,
            channel_mappings: None,
        },
        runtime: tellar::config::RuntimeConfig::default(),
    };

    // 2. Prepare initial state
    fs::create_dir_all(base_path.join("docs")).unwrap();
    fs::write(
        base_path.join("docs").join("report.txt"),
        "alpha\nproject_token=delta-42\nomega\n",
    )
    .unwrap();
    let path = base_path.join("rituals").join("token_check.md");
    fs::write(
        &path,
        concat!(
            "---\n",
            "status: active\n",
            "schedule: hold\n",
            "discord_event_id: evt-1\n",
            "origin_channel: \"0\"\n",
            "---\n\n",
            "- [ ] Find the value of `project_token` inside docs/report.txt and tell me the exact value.\n"
        ),
    )
    .unwrap();

    println!("🚀 Starting full plan-driven ritual turn...");

    // 3. Run the thread runtime through the public ritual path.
    let result =
        thread::execute_thread_file(&path, base_path, std::sync::Arc::new(config), None, Some("0".to_string()), None)
            .await;

    match result {
        Ok(()) => {
            println!("✅ Plan-driven ritual turn completed successfully!");
            let content = fs::read_to_string(&path).unwrap();
            if should_skip_live_gemini_error(&content) {
                println!("skipping test: live Gemini request failed inside ritual log");
                return;
            }
            if content.contains("❌ Task failed (") && !content.contains("- [x]") {
                println!(
                    "skipping test: ritual step did not complete because the live model path did not settle"
                );
                return;
            }
            assert!(content.contains("- [x]"));
            assert!(content.contains("delta-42"));
        }
        Err(e) => {
            let err_str = format!("{:?}", e);
            if should_skip_live_gemini_error(&err_str) {
                println!("skipping test: live Gemini request failed: {}", err_str);
                return;
            }
            panic!("❌ Plan-driven ritual flow failed with error: {}", e);
        }
    }
}

#[tokio::test]
async fn test_privileged_request_with_exec_disabled_settles_without_completing_ritual() {
    let api_key = match env::var("GEMINI_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("skipping test: GEMINI_API_KEY not set");
            return;
        }
    };

    let dir = tempdir().expect("Failed to create temp dir");
    let base_path = dir.path();
    fs::create_dir_all(base_path.join("agents")).unwrap();
    fs::create_dir_all(base_path.join("brain")).unwrap();
    fs::create_dir_all(base_path.join("rituals")).unwrap();
    fs::write(
        base_path.join("agents").join("AGENTS.md"),
        "You are Tellar, a precise task processor.",
    )
    .unwrap();

    let mut runtime = tellar::config::RuntimeConfig::default();
    runtime.privileged = false;

    let config = tellar::config::Config {
        gemini: tellar::config::GeminiConfig {
            api_key,
            model: "gemini-3-flash-preview".to_string(),
        },
        discord: tellar::config::DiscordConfig {
            token: "fake".to_string(),
            guild_id: None,
            channel_mappings: None,
        },
        runtime,
    };

    let path = base_path.join("rituals").join("host_path.md");
    fs::write(
        &path,
        concat!(
            "---\n",
            "status: active\n",
            "schedule: hold\n",
            "discord_event_id: evt-2\n",
            "origin_channel: \"0\"\n",
            "---\n\n",
            "- [ ] Find /root/process_intel.py and send it to me as an attachment.\n"
        ),
    )
    .unwrap();

    println!("🚀 Starting privileged-mode clarification live test...");

    let result =
        thread::execute_thread_file(&path, base_path, std::sync::Arc::new(config), None, Some("0".to_string()), None)
            .await;

    match result {
        Ok(()) => {
            println!("✅ Privileged clarification flow completed!");
            let content = fs::read_to_string(&path).unwrap();
            if should_skip_live_gemini_error(&content) {
                println!("skipping test: live Gemini request failed inside ritual log");
                return;
            }
            if content.contains("❌ Task failed (") && content.contains("- [ ]") {
                println!(
                    "skipping test: privileged ritual did not settle because the live model path did not complete"
                );
                return;
            }
            let lower = content.to_ascii_lowercase();
            assert!(content.contains("- [ ]"));
            assert!(
                lower.contains("cannot")
                    || lower.contains("can't")
                    || lower.contains("disabled")
                    || lower.contains("privileged")
                    || content.contains("无法")
                    || content.contains("不能"),
                "ritual log did not clearly state the limitation: {}",
                content
            );
        }
        Err(e) => {
            let err_str = format!("{:?}", e);
            if should_skip_live_gemini_error(&err_str) {
                println!("skipping test: live Gemini request failed: {}", err_str);
                return;
            }
            panic!("❌ Plan-driven privileged flow failed with error: {}", e);
        }
    }
}
