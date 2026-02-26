//! OpenAI (GPT) provider implementation.

use async_trait::async_trait;
use crew_core::{Message, MessageRole};
use eyre::{Result, WrapErr};
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use secrecy::{ExposeSecret, SecretString};

use crate::vision;

use crate::config::ChatConfig;
use crate::provider::LlmProvider;
use crate::sse::SseEvent;
use crate::types::{ChatResponse, ChatStream, StopReason, StreamEvent, TokenUsage, ToolSpec};

/// OpenAI GPT provider.
pub struct OpenAIProvider {
    client: Client,
    api_key: SecretString,
    model: String,
    base_url: String,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            api_key: SecretString::from(api_key.into()),
            model: model.into(),
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }

    /// Create a provider using the OPENAI_API_KEY environment variable.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .wrap_err("OPENAI_API_KEY environment variable not set")?;
        Ok(Self::new(api_key, "gpt-4o"))
    }

    /// Set a custom base URL (for Azure, local proxies, etc.).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Build the shared request struct used by both chat() and chat_stream().
    fn build_request<'a>(
        &'a self,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
        config: &ChatConfig,
    ) -> OpenAIRequest<'a> {
        let openai_messages: Vec<OpenAIMessage> = messages
            .iter()
            .map(|m| {
                let role = m.role.as_str();
                // Convert tool_calls from crew_core format to OpenAI format
                let tool_calls = m.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| OpenAIToolCall {
                            id: tc.id.clone(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: tc.name.clone(),
                                arguments: tc.arguments.to_string(),
                            },
                        })
                        .collect()
                });
                OpenAIMessage {
                    role,
                    content: build_openai_content(m),
                    reasoning_content: m.reasoning_content.as_deref(),
                    tool_call_id: m.tool_call_id.as_deref(),
                    tool_calls,
                }
            })
            .collect();
        // Merge consecutive system messages into one (some providers like
        // MiniMax reject multiple system messages with error 2013).
        let openai_messages = merge_system_messages(openai_messages);

        let openai_tools: Option<Vec<OpenAITool>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| OpenAITool {
                        r#type: "function",
                        function: OpenAIFunction {
                            name: &t.name,
                            description: &t.description,
                            parameters: &t.input_schema,
                        },
                    })
                    .collect(),
            )
        };

        // GPT-5+, o1, o3, o4 models use max_completion_tokens instead of max_tokens
        let uses_completion_tokens = self.model.starts_with("gpt-5")
            || self.model.starts_with("o1")
            || self.model.starts_with("o3")
            || self.model.starts_with("o4");

        // Some models (o1, o3, kimi-k2.5) don't support custom temperature
        let fixed_temperature = self.model.starts_with("o1")
            || self.model.starts_with("o3")
            || self.model.contains("k2.5");
        let temperature = if fixed_temperature {
            None
        } else {
            config.temperature
        };

        OpenAIRequest {
            model: &self.model,
            messages: openai_messages,
            max_tokens: if uses_completion_tokens {
                None
            } else {
                config.max_tokens
            },
            max_completion_tokens: if uses_completion_tokens {
                config.max_tokens.or(Some(4096))
            } else {
                None
            },
            temperature,
            tools: openai_tools,
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatResponse> {
        let request = self.build_request(messages, tools, config);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .wrap_err("failed to send request to OpenAI")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            eyre::bail!(
                "OpenAI API error: {status} - {}",
                crate::provider::truncate_error_body(&body)
            );
        }

        let api_response: OpenAIResponse = response
            .json()
            .await
            .wrap_err("failed to parse OpenAI response")?;

        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| eyre::eyre!("no choices in OpenAI response"))?;

        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| crew_core::ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
                metadata: None,
            })
            .collect();

        let stop_reason = match choice.finish_reason.as_str() {
            "stop" => StopReason::EndTurn,
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        Ok(ChatResponse {
            content: choice.message.content,
            reasoning_content: choice.message.reasoning_content,
            tool_calls,
            stop_reason,
            usage: TokenUsage {
                input_tokens: api_response.usage.prompt_tokens,
                output_tokens: api_response.usage.completion_tokens,
            },
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        config: &ChatConfig,
    ) -> Result<ChatStream> {
        let request = self.build_request(messages, tools, config);

        let mut body =
            serde_json::to_value(&request).wrap_err("failed to serialize OpenAI request")?;
        let obj = body
            .as_object_mut()
            .ok_or_else(|| eyre::eyre!("failed to build OpenAI request body"))?;
        obj.insert("stream".into(), true.into());
        obj.insert(
            "stream_options".into(),
            serde_json::json!({"include_usage": true}),
        );

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .wrap_err("failed to send streaming request to OpenAI")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            eyre::bail!(
                "OpenAI API error: {status} - {}",
                crate::provider::truncate_error_body(&text)
            );
        }

        let sse_stream = crate::sse::parse_sse_response(response);
        let event_stream =
            sse_stream.flat_map(|event| futures::stream::iter(parse_openai_sse_events(&event)));

        Ok(Box::pin(event_stream))
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    fn provider_name(&self) -> &str {
        "openai"
    }
}

#[derive(Serialize)]
struct OpenAIRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAIMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool<'a>>>,
}

#[derive(Serialize)]
struct OpenAIMessage<'a> {
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

/// Content can be plain text or multipart (text + images).
#[derive(Serialize)]
#[serde(untagged)]
enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

#[derive(Serialize)]
struct OpenAIImageUrl {
    url: String,
}

/// Merge consecutive system messages into a single system message.
///
/// Some OpenAI-compatible providers (e.g. MiniMax) reject requests with
/// multiple system messages. This combines their text content with a newline
/// separator while preserving all other messages in order.
fn merge_system_messages(messages: Vec<OpenAIMessage<'_>>) -> Vec<OpenAIMessage<'_>> {
    let mut result: Vec<OpenAIMessage> = Vec::with_capacity(messages.len());
    for msg in messages {
        if msg.role == "system" {
            if let Some(last) = result.last_mut() {
                if last.role == "system" {
                    // Merge content: extract text from both and combine
                    let existing = match &last.content {
                        Some(OpenAIContent::Text(t)) => t.clone(),
                        _ => String::new(),
                    };
                    let new_text = match &msg.content {
                        Some(OpenAIContent::Text(t)) => t.as_str(),
                        _ => "",
                    };
                    last.content = Some(OpenAIContent::Text(format!("{existing}\n\n{new_text}")));
                    continue;
                }
            }
        }
        result.push(msg);
    }
    result
}

fn build_openai_content(msg: &Message) -> Option<OpenAIContent> {
    let images: Vec<_> = msg.media.iter().filter(|p| vision::is_image(p)).collect();

    if images.is_empty() {
        if msg.content.is_empty() {
            // Tool messages require a content string (OpenAI spec).
            // User messages must not be empty (many providers reject them).
            // Assistant messages can have null content when tool_calls are present;
            // some providers (Moonshot/kimi) reject empty-string content on assistant msgs.
            return match msg.role {
                MessageRole::Tool => Some(OpenAIContent::Text(String::new())),
                MessageRole::User => Some(OpenAIContent::Text("[empty message]".to_string())),
                _ => None,
            };
        }
        return Some(OpenAIContent::Text(msg.content.clone()));
    }

    let mut parts = Vec::new();
    for path in images {
        if let Ok((mime, data)) = vision::encode_image(path) {
            parts.push(OpenAIContentPart::ImageUrl {
                image_url: OpenAIImageUrl {
                    url: format!("data:{mime};base64,{data}"),
                },
            });
        }
    }
    if !msg.content.is_empty() {
        parts.push(OpenAIContentPart::Text {
            text: msg.content.clone(),
        });
    }
    Some(OpenAIContent::Parts(parts))
}

#[derive(Serialize)]
struct OpenAITool<'a> {
    r#type: &'a str,
    function: OpenAIFunction<'a>,
}

#[derive(Serialize)]
struct OpenAIFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: String,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type", default = "default_function_type")]
    call_type: String,
    function: FunctionCall,
}

fn default_function_type() -> String {
    "function".to_string()
}

#[derive(Serialize, Deserialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

// --- Streaming SSE helpers (shared with OpenRouter) ---

pub(crate) fn parse_openai_sse_events(event: &SseEvent) -> Vec<StreamEvent> {
    if event.data == "[DONE]" {
        return vec![];
    }

    let data: serde_json::Value = match serde_json::from_str(&event.data) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut events = Vec::new();

    if let Some(choices) = data["choices"].as_array() {
        for choice in choices {
            // Reasoning/thinking content (kimi-k2.5, o1, etc.)
            if let Some(reasoning) = choice["delta"]["reasoning_content"].as_str() {
                if !reasoning.is_empty() {
                    events.push(StreamEvent::ReasoningDelta(reasoning.to_string()));
                }
            }

            if let Some(content) = choice["delta"]["content"].as_str() {
                if !content.is_empty() {
                    events.push(StreamEvent::TextDelta(content.to_string()));
                }
            }

            if let Some(tool_calls) = choice["delta"]["tool_calls"].as_array() {
                for tc in tool_calls {
                    events.push(StreamEvent::ToolCallDelta {
                        index: tc["index"].as_u64().unwrap_or(0) as usize,
                        id: tc["id"].as_str().map(String::from),
                        name: tc["function"]["name"].as_str().map(String::from),
                        arguments_delta: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or("")
                            .to_string(),
                    });
                }
            }

            if let Some(reason) = choice["finish_reason"].as_str() {
                events.push(StreamEvent::Done(match reason {
                    "stop" => StopReason::EndTurn,
                    "tool_calls" => StopReason::ToolUse,
                    "length" => StopReason::MaxTokens,
                    _ => StopReason::EndTurn,
                }));
            }
        }
    }

    if let Some(usage) = data.get("usage").filter(|u| !u.is_null()) {
        events.push(StreamEvent::Usage(TokenUsage {
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
        }));
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChatConfig;
    use crate::provider::LlmProvider;
    use crew_core::{Message, MessageRole};

    fn msg(content: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: content.to_string(),
            media: vec![],
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Real API test: NVIDIA NIM with Llama 3.3 70B.
    /// Run with: NVIDIA_API_KEY=... cargo test -p crew-llm -- --ignored test_nvidia_nim_llama
    #[tokio::test]
    #[ignore]
    async fn test_nvidia_nim_llama() {
        let api_key = std::env::var("NVIDIA_API_KEY").expect("NVIDIA_API_KEY must be set");
        let provider = OpenAIProvider::new(&api_key, "meta/llama-3.3-70b-instruct")
            .with_base_url("https://integrate.api.nvidia.com/v1");

        assert_eq!(provider.model_id(), "meta/llama-3.3-70b-instruct");

        let messages = vec![msg("What is 2+2? Reply with just the number.")];
        let config = ChatConfig {
            max_tokens: Some(64),
            ..Default::default()
        };
        let response = provider.chat(&messages, &[], &config).await.unwrap();

        eprintln!("NVIDIA Llama response: {:?}", response.content);
        eprintln!("Tokens: {:?}", response.usage);

        assert!(response.content.is_some());
        let content = response.content.unwrap();
        assert!(content.contains('4'), "Expected '4' in response: {content}");
        assert!(response.usage.input_tokens > 0);
        assert!(response.usage.output_tokens > 0);
    }

    /// Real API test: NVIDIA NIM with Mistral Small.
    /// Run with: NVIDIA_API_KEY=... cargo test -p crew-llm -- --ignored test_nvidia_nim_mistral
    #[tokio::test]
    #[ignore]
    async fn test_nvidia_nim_mistral() {
        let api_key = std::env::var("NVIDIA_API_KEY").expect("NVIDIA_API_KEY must be set");
        let provider =
            OpenAIProvider::new(&api_key, "mistralai/mistral-small-3.1-24b-instruct-2503")
                .with_base_url("https://integrate.api.nvidia.com/v1");

        let messages = vec![msg("Name the capital of France in one word.")];
        let config = ChatConfig {
            max_tokens: Some(32),
            ..Default::default()
        };
        let response = provider.chat(&messages, &[], &config).await.unwrap();

        eprintln!("NVIDIA Mistral response: {:?}", response.content);
        let content = response.content.unwrap();
        assert!(
            content.to_lowercase().contains("paris"),
            "Expected 'Paris' in response: {content}"
        );
    }

    /// Real API test: NVIDIA NIM streaming.
    /// Run with: NVIDIA_API_KEY=... cargo test -p crew-llm -- --ignored test_nvidia_nim_streaming
    #[tokio::test]
    #[ignore]
    async fn test_nvidia_nim_streaming() {
        let api_key = std::env::var("NVIDIA_API_KEY").expect("NVIDIA_API_KEY must be set");
        let provider = OpenAIProvider::new(&api_key, "meta/llama-3.3-70b-instruct")
            .with_base_url("https://integrate.api.nvidia.com/v1");

        let messages = vec![msg("Count from 1 to 5, one number per line.")];
        let config = ChatConfig {
            max_tokens: Some(64),
            ..Default::default()
        };
        let mut stream = provider.chat_stream(&messages, &[], &config).await.unwrap();

        let mut chunks = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::TextDelta(text) => chunks.push(text),
                StreamEvent::Done(_) => break,
                _ => {}
            }
        }

        let full_text = chunks.join("");
        eprintln!("NVIDIA streaming result: {full_text}");
        assert!(!full_text.is_empty(), "Stream should produce text");
        assert!(full_text.contains('1'), "Should contain '1': {full_text}");
        assert!(full_text.contains('5'), "Should contain '5': {full_text}");
    }
}
