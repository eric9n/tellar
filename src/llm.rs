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
) -> anyhow::Result<String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let payload = json!({
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
    
    let content = res_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Gemini returned invalid or empty content"))?;
    
    Ok(content.to_string())
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


