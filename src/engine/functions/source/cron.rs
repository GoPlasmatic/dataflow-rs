use crate::engine::functions::source::SourceFunctionHandler;
use crate::engine::message::Message;
use async_trait::async_trait;
use chrono::Utc;
use datalogic_rs::arena::DataArena;
use datalogic_rs::{DataValue, FromJson};
use once_cell::sync::OnceCell;
use serde_json::json;
use std::sync::Arc;
use std::thread_local;
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

// Create a thread-local instance of DataArena
thread_local! {
    static THREAD_ARENA: OnceCell<DataArena> = const { OnceCell::new() };
}

pub struct CronSourceFunctionHandler {
    pub cron_expression: String,
    pub interval_seconds: u64,
    pub payload_generator: Box<dyn Fn() -> serde_json::Value + Send + Sync>,
    pub init_arena: Arc<dyn Fn() -> DataArena + Send + Sync>,
}

impl CronSourceFunctionHandler {
    pub fn new(
        interval_seconds: u64,
        init_arena: Arc<dyn Fn() -> DataArena + Send + Sync>,
        payload_generator: Box<dyn Fn() -> serde_json::Value + Send + Sync>,
    ) -> Self {
        Self {
            cron_expression: String::new(), // Not implemented yet, using simple interval
            interval_seconds,
            payload_generator,
            init_arena,
        }
    }
}

#[async_trait]
impl SourceFunctionHandler for CronSourceFunctionHandler {
    async fn start(
        &self,
        message_processor: Arc<dyn Fn(Message) + Send + Sync>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let interval_seconds = self.interval_seconds;
        let init_arena = self.init_arena.clone();
        let payload_generator = &self.payload_generator;

        println!(
            "Starting Cron Source Function with interval of {} seconds",
            interval_seconds
        );

        let interval = Duration::from_secs(interval_seconds);
        let mut next_tick = Instant::now() + interval;

        loop {
            let now = Instant::now();

            if now >= next_tick {
                let current_time = Utc::now();
                let payload = payload_generator();

                THREAD_ARENA.with(|cell| {
                    let arena = cell.get_or_init(|| (init_arena)());

                    // Construct the message
                    let message = Message {
                        id: Uuid::new_v4().to_string(),
                        data: DataValue::from_json(&json!({}), arena),
                        payload: DataValue::from_json(&payload, arena),
                        metadata: DataValue::from_json(
                            &json!({
                                "source": "cron",
                                "timestamp": current_time.to_rfc3339(),
                                "interval_seconds": interval_seconds
                            }),
                            arena,
                        ),
                        temp_data: DataValue::from_json(&json!({}), arena),
                        audit_trail: Vec::new(),
                    };

                    // Process the message
                    message_processor(message);
                });

                // Calculate next tick
                next_tick = now + interval;
            }

            // Sleep until next tick
            let sleep_duration = next_tick.saturating_duration_since(now);
            sleep(sleep_duration).await;
        }
    }
}
