//! # Retry Handling Module
//!
//! This module provides configurable retry mechanisms for handling transient failures
//! in workflow task execution. It supports both fixed delay and exponential backoff
//! strategies to handle various failure scenarios effectively.

use std::thread;
use std::time::Duration;

/// Configuration for retry behavior with support for exponential backoff.
///
/// The `RetryConfig` allows fine-tuning of retry behavior for tasks that may
/// experience transient failures. It provides:
///
/// - Configurable maximum retry attempts
/// - Fixed or exponential backoff strategies
/// - Customizable base delay between retries
///
/// ## Example
///
/// ```rust
/// use dataflow_rs::engine::RetryConfig;
///
/// // Create config with exponential backoff
/// let config = RetryConfig {
///     max_retries: 5,
///     retry_delay_ms: 100,
///     use_backoff: true,
/// };
///
/// // First retry: 100ms * 2^0 = 100ms
/// // Second retry: 100ms * 2^1 = 200ms
/// // Third retry: 100ms * 2^2 = 400ms
/// ```
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries (0 means no retries)
    pub max_retries: u32,
    /// Base delay between retries in milliseconds
    pub retry_delay_ms: u64,
    /// Whether to use exponential backoff (doubles delay each retry)
    pub use_backoff: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay_ms: 1000,
            use_backoff: true,
        }
    }
}

impl RetryConfig {
    /// Calculate delay for a given retry attempt
    pub fn calculate_delay(&self, retry_count: u32) -> Duration {
        let delay = if self.use_backoff {
            self.retry_delay_ms * (2_u64.pow(retry_count))
        } else {
            self.retry_delay_ms
        };
        Duration::from_millis(delay)
    }

    /// Sleep for the appropriate delay
    pub fn sleep(&self, retry_count: u32) {
        thread::sleep(self.calculate_delay(retry_count));
    }
}
