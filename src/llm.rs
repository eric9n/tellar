use serde::Serialize;
use serde_json::json;
use once_cell::sync::Lazy;

static POOLED_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent("google-cloud-sdk vscode_cloudshelleditor/0.1")
        .build()
        .expect("Failed to create pooled reqwest client")
});

#[derive(Debug, Serialize, Clone)]
pub struct MultimodalPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<InlineData>,
}

#[derive(Debug, Serialize, Clone)]
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
        }
    }

    pub fn image(mime_type: impl Into<String>, base64_data: impl Into<String>) -> Self {
        Self {
            text: None,
            inline_data: Some(InlineData {
                mime_type: mime_type.into(),
                data: base64_data.into(),
            }),
        }
    }
}

/// Call Gemini API to generate content with multiple parts (text + images)
pub async fn generate_multimodal(
    system_prompt: &str,
    user_parts: Vec<MultimodalPart>,
    api_key: &str,
    model: &str,
    temperature: f32,
    tools: Option<serde_json::Value>,
) -> anyhow::Result<String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let mut payload = json!({
        "systemInstruction": {
            "parts": [{ "text": system_prompt }]
        },
        "contents": [
            {
                "role": "user",
                "parts": user_parts
            }
        ],
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
        let mut function_call: Option<&serde_json::Value> = None;

        for part in parts.as_array().unwrap() {
            if let Some(text) = part["text"].as_str() {
                text_acc.push_str(text);
            }
            if part.get("functionCall").is_some() {
                function_call = Some(&part["functionCall"]);
                break; // Gemini usually sends one function call at the end of parts
            }
        }

        if let Some(call) = function_call {
            let name = call["name"].as_str().unwrap_or("unknown");
            let args = call["args"].clone();
            
            // Translate native call to our internal JSON ReAct format
            let react_json = json!({
                "thought": if text_acc.is_empty() { "Natural function call triggered." } else { text_acc.trim() },
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


