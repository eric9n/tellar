use tellar::{agent_loop, llm};
use std::env;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_full_agent_turn_with_gemini_3() {
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
    
    // Create folders required for steward/guardian
    fs::create_dir_all(base_path.join("agents")).unwrap();
    fs::create_dir_all(base_path.join("brain")).unwrap();
    fs::write(base_path.join("agents").join("AGENTS.md"), "You are Tellar, a helpful assistant.").unwrap();
    
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
        guardian: None,
    };

    // 2. Prepare initial state
    let system_prompt = "You are a helpful assistant.";
    fs::create_dir_all(base_path.join("docs")).unwrap();
    fs::write(
        base_path.join("docs").join("report.txt"),
        "alpha\nproject_token=delta-42\nomega\n",
    )
    .unwrap();
    let path = base_path.join("test.md");
    fs::write(&path, "User: Please find the token in docs/report.txt").unwrap();

    let initial_messages = vec![
        llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(
                "Find the value of `project_token` inside the guild and tell me the exact value. Use the available tools."
            )],
        }
    ];

    println!("🚀 Starting full multi-turn agent loop...");
    
    // 3. Run the actual agent loop
    // We expect the model to use built-in tools (ls/grep/read) and then answer.
    let result = agent_loop::run_agent_loop(
        initial_messages,
        &path,
        base_path,
        &config,
        "0",
        system_prompt
    ).await;

    match result {
        Ok(final_answer) => {
            println!("✅ Multi-turn loop completed successfully!");
            println!("📥 Final Answer: {}", final_answer);
            assert!(final_answer.contains("delta-42"));
        },
        Err(e) => {
            let err_str = format!("{:?}", e);
            // Check if this is the signature error
            if err_str.contains("thought_signature") || err_str.contains("INVALID_ARGUMENT") {
                panic!("❌ Gemini 3 rejected history in Turn 2! Error: {}", err_str);
            }
            panic!("❌ Agent loop failed with error: {}", e);
        }
    }
}

#[tokio::test]
async fn test_privileged_request_with_exec_disabled_settles_without_search_loop() {
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
    fs::write(base_path.join("agents").join("AGENTS.md"), "You are Tellar, a helpful assistant.").unwrap();

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
        guardian: None,
    };

    let system_prompt = "You are a helpful assistant.";
    let path = base_path.join("test.md");
    fs::write(&path, "User: Find /root/process_intel.py and send it as an attachment").unwrap();

    let initial_messages = vec![
        llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(
                "Find /root/process_intel.py and send it to me as an attachment. Use the available tools if they help.",
            )],
        },
        llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(
                "### Execution Boundary\nThis request targets a host path. Call `exec` first; it will reject immediately because privileged mode is disabled, then explain the limitation instead of searching with guild file tools.\nYou cannot send file attachments directly. If this request depends on unsupported capabilities, say so directly and finish instead of continuing to search.",
            )],
        },
    ];

    println!("🚀 Starting privileged-mode refusal live test...");

    let result = agent_loop::run_agent_loop(
        initial_messages,
        &path,
        base_path,
        &config,
        "0",
        system_prompt,
    )
    .await;

    match result {
        Ok(final_answer) => {
            println!("✅ Privileged refusal flow completed!");
            println!("📥 Final Answer: {}", final_answer);
            let lower = final_answer.to_ascii_lowercase();
            assert!(
                lower.contains("cannot")
                    || lower.contains("can't")
                    || lower.contains("disabled")
                    || lower.contains("privileged")
                    || final_answer.contains("无法")
                    || final_answer.contains("不能"),
                "final answer did not clearly state the limitation: {}",
                final_answer
            );
        }
        Err(e) => panic!("❌ Agent loop failed with error: {}", e),
    }
}
