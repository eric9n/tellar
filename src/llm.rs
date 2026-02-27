use serde::{Serialize, Deserialize};
use serde_json::json;
use once_cell::sync::Lazy;

static POOLED_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent("google-cloud-sdk vscode_cloudshelleditor/0.1")
        .build()
        .expect("Failed to create pooled reqwest client")
});

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    #[serde(rename = "function")] // Gemini uses 'function' for tool results in some contexts, but 'model' for assistant. 
    ToolResult,                   // We'll map this carefully in generate_multimodal.
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub parts: Vec<MultimodalPart>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MultimodalPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,
    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    pub function_call: Option<serde_json::Value>,
    #[serde(rename = "functionResponse", skip_serializing_if = "Option::is_none")]
    pub function_response: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InlineData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub data: String, // Base64
}

impl MultimodalPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            inline_data: None,
            function_call: None,
            function_response: None,
        }
    }

    pub fn image(mime_type: impl Into<String>, base64_data: impl Into<String>) -> Self {
        Self {
            text: None,
            inline_data: Some(InlineData {
                mime_type: mime_type.into(),
                data: base64_data.into(),
            }),
            function_call: None,
            function_response: None,
        }
    }
    
    pub fn function_call(name: &str, args: serde_json::Value) -> Self {
        Self {
            text: None,
            inline_data: None,
            function_call: Some(json!({ "name": name, "args": args })),
            function_response: None,
        }
    }

    pub fn function_response(name: &str, response: serde_json::Value) -> Self {
        Self {
            text: None,
            inline_data: None,
            function_call: None,
            function_response: Some(json!({ "name": name, "response": response })),
        }
    }
}

/// Call Gemini API with full message history (pi-mono style)
pub async fn generate_multimodal(
    system_prompt: &str,
    history: Vec<Message>,
    api_key: &str,
    model: &str,
    temperature: f32,
    tools: Option<serde_json::Value>,
) -> anyhow::Result<String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    // Map MessageRole to Gemini roles
    let contents: Vec<serde_json::Value> = history.into_iter().map(|msg| {
        let gemini_role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "model",
            MessageRole::ToolResult => "function", // In the history, results use 'function' role
            MessageRole::System => "user",         // System instructions are handled separately
        };
        json!({
            "role": gemini_role,
            "parts": msg.parts
        })
    }).collect();

    let mut payload = json!({
        "systemInstruction": {
            "parts": [{ "text": system_prompt }]
        },
        "contents": contents,
        "generationConfig": {
            "temperature": temperature
        }
    });

    if let Some(t) = tools {
        payload["tools"] = t;
    }

    let response = POOLED_CLIENT.post(url)
        .header("X-Goog-Api-Client", "google-cloud-sdk vscode_cloudshelleditor/0.1")
        .json(&payload)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow::anyhow!("Gemini API Error (Model: {}): {}", model, error_text));
    }

    let res_json: serde_json::Value = response.json().await?;
    let parts = &res_json["candidates"][0]["content"]["parts"];
    
    if parts.is_array() {
        let mut text_acc = String::new();
        let mut function_calls = Vec::new();

        for part in parts.as_array().unwrap() {
            if let Some(text) = part["text"].as_str() {
                text_acc.push_str(text);
            }
            if let Some(call) = part.get("functionCall") {
                function_calls.push(call);
            }
        }

        // If there are function calls, translate to our ReAct JSON for the loop to handle
        if !function_calls.is_empty() {
            let call = function_calls[0]; // Currently Tellar handles one at a time
            let name = call["name"].as_str().unwrap_or("unknown");
            let args = call["args"].clone();
            
            let react_json = json!({
                "thought": if text_acc.is_empty() { "Tool call triggered." } else { text_acc.trim() },
                "tool": name,
                "args": args
            });
            return Ok(serde_json::to_string(&react_json).unwrap());
        }

        if !text_acc.is_empty() {
            return Ok(text_acc);
        }
    }

    // Fallback if no text or function call was found
    let reason = res_json["candidates"][0]["finishReason"].as_str().unwrap_or("UNKNOWN");
    let msg = if reason == "SAFETY" {
        format!("Gemini blocked the response due to SAFETY filters. Check your prompt or history context.")
    } else {
        format!("Gemini returned no content. Finish Reason: {}. Response: {}", reason, res_json)
    };
    eprintln!("🔴 [LLM ERROR] {}", msg);
    Err(anyhow::anyhow!(msg))
}

/// Fetch available models from Gemini API
pub async fn list_models(api_key: &str) -> anyhow::Result<Vec<String>> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models?key={}",
        api_key
    );

    let response = POOLED_CLIENT.get(url)
        .header("X-Goog-Api-Client", "google-cloud-sdk vscode_cloudshelleditor/0.1")
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        return Err(anyhow::anyhow!("Gemini API Error: {}", error_text));
    }

    let res_json: serde_json::Value = response.json().await?;
    
    let mut models = Vec::new();
    if let Some(list) = res_json["models"].as_array() {
        for m in list {
            if let Some(name) = m["name"].as_str() {
                // Return short name (e.g. models/gemini-pro -> gemini-pro)
                let short_name = name.strip_prefix("models/").unwrap_or(name);
                
                // Filter for models that support generateContent
                if let Some(methods) = m["supportedGenerationMethods"].as_array() {
                    if methods.iter().any(|v| v.as_str() == Some("generateContent")) {
                        models.push(short_name.to_string());
                    }
                }
            }
        }
    }
    
    Ok(models)
}


