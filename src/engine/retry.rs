use std::thread;
use std::time::Duration;

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Delay between retries in milliseconds
    pub retry_delay_ms: u64,
    /// Whether to use exponential backoff
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
