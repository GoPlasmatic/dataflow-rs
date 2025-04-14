use crate::engine::functions::source::SourceFunctionHandler;
use crate::engine::message::Message;
use async_trait::async_trait;
use datalogic_rs::arena::DataArena;
use datalogic_rs::{DataValue, FromJson};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use once_cell::sync::OnceCell;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread_local;
use uuid::Uuid;

// Create a thread-local instance of DataArena
thread_local! {
    static THREAD_ARENA: OnceCell<DataArena> = const { OnceCell::new() };
}

pub struct HttpSourceFunctionHandler {
    pub address: String,
    pub path: String,
    pub methods: Vec<Method>,
    // DataArena initialization function
    pub init_arena: Arc<dyn Fn() -> DataArena + Send + Sync>,
}

impl HttpSourceFunctionHandler {
    pub fn new(
        address: String,
        path: String,
        init_arena: Arc<dyn Fn() -> DataArena + Send + Sync>,
    ) -> Self {
        Self {
            address,
            path,
            methods: vec![Method::GET, Method::POST],
            init_arena,
        }
    }

    pub fn with_methods(mut self, methods: Vec<Method>) -> Self {
        self.methods = methods;
        self
    }
}

#[async_trait]
impl SourceFunctionHandler for HttpSourceFunctionHandler {
    async fn start(
        &self,
        message_processor: Arc<dyn Fn(Message) + Send + Sync>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let addr: SocketAddr = self.address.parse()?;
        let path = self.path.clone();
        let methods = self.methods.clone();
        let init_arena = self.init_arena.clone();

        let make_svc = make_service_fn(move |_| {
            let path = path.clone();
            let methods = methods.clone();
            let init_arena = init_arena.clone();
            let message_processor = message_processor.clone();

            async move {
                Ok::<_, Infallible>(service_fn(move |req: Request<Body>| {
                    let path = path.clone();
                    let methods = methods.clone();
                    let init_arena = init_arena.clone();
                    let message_processor = message_processor.clone();

                    async move {
                        // Check if path matches
                        if req.uri().path() != path {
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(StatusCode::NOT_FOUND)
                                    .body(Body::from("Not Found"))
                                    .unwrap(),
                            );
                        }

                        // Check if method is allowed
                        if !methods.contains(req.method()) {
                            return Ok::<_, Infallible>(
                                Response::builder()
                                    .status(StatusCode::METHOD_NOT_ALLOWED)
                                    .body(Body::from("Method Not Allowed"))
                                    .unwrap(),
                            );
                        }

                        // Extract HTTP request metadata before consuming the request
                        let method = req.method().clone();
                        let uri_path = req.uri().path().to_string();
                        let uri_query = req.uri().query().unwrap_or_default().to_string();

                        // Create headers object
                        let mut headers_obj = json!({});
                        for (name, value) in req.headers() {
                            if let Ok(v) = value.to_str() {
                                headers_obj[name.as_str()] = json!(v);
                            }
                        }

                        // Extract payload data based on method
                        let payload = match method {
                            Method::GET => {
                                // Extract query parameters
                                let params: Value = serde_urlencoded::from_str(&uri_query)
                                    .unwrap_or_else(|_| json!({"raw": uri_query}));
                                params
                            }
                            Method::POST => {
                                // Extract body as JSON
                                let bytes = hyper::body::to_bytes(req.into_body())
                                    .await
                                    .unwrap_or_default();
                                serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                                    let raw = String::from_utf8_lossy(&bytes).to_string();
                                    json!({"raw": raw})
                                })
                            }
                            _ => json!({}),
                        };

                        // Get thread-local DataArena instance or initialize it
                        THREAD_ARENA.with(|cell| {
                            let arena = cell.get_or_init(|| (init_arena)());

                            // Construct the message
                            let message = Message {
                                id: Uuid::new_v4().to_string(),
                                data: DataValue::from_json(&json!({}), arena),
                                payload: DataValue::from_json(&payload, arena),
                                metadata: DataValue::from_json(
                                    &json!({
                                        "source": "http",
                                        "method": method.as_str(),
                                        "path": uri_path,
                                        "headers": headers_obj,
                                        "timestamp": chrono::Utc::now().to_rfc3339()
                                    }),
                                    arena,
                                ),
                                temp_data: DataValue::from_json(&json!({}), arena),
                                audit_trail: Vec::new(),
                            };

                            // Process the message using the provided processor function
                            message_processor(message);
                        });

                        // Return success response
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .body(Body::from("Request processed successfully"))
                                .unwrap(),
                        )
                    }
                }))
            }
        });

        println!(
            "HTTP Source Function listening on http://{}{}",
            addr, self.path
        );
        let server = Server::bind(&addr).serve(make_svc);

        server.await?;
        Ok(())
    }
}
