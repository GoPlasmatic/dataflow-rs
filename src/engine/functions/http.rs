use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::AsyncFunctionHandler;
use crate::engine::message::{Change, Message};
use async_trait::async_trait;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, Method,
};
use serde_json::{json, Value};
use std::convert::TryFrom;
use std::str::FromStr;
use std::time::Duration;

/// An HTTP task function for making API requests asynchronously.
///
/// This implementation uses the async reqwest client for efficient non-blocking HTTP requests.
/// It supports different HTTP methods, headers, and parsing responses
/// into the message payload.
pub struct HttpFunction {
    client: Client,
}

impl HttpFunction {
    pub fn new(timeout_secs: u64) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self { client }
    }
}

#[async_trait]
impl AsyncFunctionHandler for HttpFunction {
    async fn execute(&self, message: &mut Message, input: &Value) -> Result<(usize, Vec<Change>)> {
        // Extract URL
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation("URL is required".to_string()))?;

        // Extract method (default to GET)
        let method_str = input.get("method").and_then(Value::as_str).unwrap_or("GET");

        let method = Method::from_str(method_str)
            .map_err(|e| DataflowError::Validation(format!("Invalid HTTP method: {e}")))?;

        // Determine whether this method supports a body
        let supports_body = method != Method::GET && method != Method::HEAD;

        // Build the request
        let mut request = self.client.request(method, url);

        // Add headers if present
        if let Some(headers) = input.get("headers").and_then(Value::as_object) {
            let mut header_map = HeaderMap::new();

            for (key, value) in headers {
                if let Some(value_str) = value.as_str() {
                    let header_name = HeaderName::try_from(key).map_err(|e| {
                        DataflowError::Validation(format!("Invalid header name '{key}': {e}"))
                    })?;

                    let header_value = HeaderValue::try_from(value_str).map_err(|e| {
                        DataflowError::Validation(format!(
                            "Invalid header value '{value_str}': {e}"
                        ))
                    })?;

                    header_map.insert(header_name, header_value);
                }
            }

            request = request.headers(header_map);
        }

        // Add body if present for methods that support it
        if let Some(body) = input.get("body") {
            if supports_body {
                request = request.json(body);
            }
        }

        // Make the request asynchronously
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                DataflowError::Timeout(format!("HTTP request timed out: {e}"))
            } else if e.is_connect() {
                DataflowError::Http {
                    status: 0,
                    message: format!("Connection error: {e}"),
                }
            } else {
                DataflowError::Http {
                    status: e.status().map_or(0, |s| s.as_u16()),
                    message: format!("HTTP request failed: {e}"),
                }
            }
        })?;

        // Get status code
        let status = response.status();
        let status_code = status.as_u16() as usize;

        // Parse the response asynchronously
        let response_body = response.text().await.map_err(|e| DataflowError::Http {
            status: status.as_u16(),
            message: format!("Failed to read response body: {e}"),
        })?;

        // Try to parse as JSON, but fall back to string if it's not valid JSON
        let response_value =
            serde_json::from_str::<Value>(&response_body).unwrap_or_else(|_| json!(response_body));

        // Store response in message temp data
        message.temp_data = json!({
            "status": status_code,
            "body": response_value,
            "success": status.is_success(),
        });

        // Return changes
        Ok((
            status_code,
            vec![Change {
                path: "temp_data".to_string(),
                old_value: Value::Null,
                new_value: message.temp_data.clone(),
            }],
        ))
    }
}
