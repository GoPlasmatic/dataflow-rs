pub mod cron;
pub mod http;

use crate::engine::message::Message;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait SourceFunctionHandler: Send + Sync {
    /// Starts the source function listener.
    ///
    /// The provided `message_processor` callback is called with a constructed Message
    /// whenever an event is received from the source.
    async fn start(
        &self,
        message_processor: Arc<dyn Fn(Message) + Send + Sync>,
    ) -> Result<(), Box<dyn std::error::Error>>;
}
