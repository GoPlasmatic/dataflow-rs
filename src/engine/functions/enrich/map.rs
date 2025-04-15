use crate::engine::message::{Change, Message};
use crate::engine::FunctionHandler;
use datalogic_rs::DataLogic;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Function that maps data from one location to another using JSONLogic rules
pub struct MapFunction {
    /// DataLogic instance for evaluating JSONLogic expressions
    data_logic_mutex: Arc<Mutex<DataLogic>>,
}

impl MapFunction {
    /// Creates a new MapFunction with a thread-safe DataLogic instance
    pub fn new_with_mutex(data_logic: Arc<Mutex<DataLogic>>) -> Self {
        Self {
            data_logic_mutex: data_logic,
        }
    }

    /// Ensures the path starts with either "data." or "metadata."
    fn validate_and_normalize_path(path: &str) -> Result<String, String> {
        if path.is_empty() {
            return Err("Path cannot be empty".to_string());
        }

        // Check if path starts with data. or metadata.
        if path.starts_with("data.") || path.starts_with("metadata.") {
            return Ok(path.to_string());
        }

        // Check if path is exactly "data" or "metadata"
        if path == "data" || path == "metadata" {
            return Ok(path.to_string());
        }

        // Otherwise, default to adding data. prefix
        Ok(format!("data.{}", path))
    }

    /// Extract the root field and remaining path segments
    fn parse_path(path: &str) -> Result<(&str, Vec<&str>), String> {
        let segments: Vec<&str> = path.split('.').collect();
        if segments.is_empty() {
            return Err("Path cannot be empty".to_string());
        }

        let root = segments[0];
        if root != "data" && root != "metadata" {
            return Err(format!(
                "Cannot modify '{}'. Only 'data' and 'metadata' fields can be modified",
                root
            ));
        }

        Ok((root, segments))
    }

    /// Get a reference to the appropriate field in the message
    fn get_target_field<'a>(message: &'a mut Message, root: &str) -> &'a mut Value {
        match root {
            "data" => &mut message.data,
            "metadata" => &mut message.metadata,
            _ => unreachable!(),
        }
    }

    /// Set a value at a path in the message structure
    fn apply_value_to_message(
        message: &mut Message,
        path: &str,
        value: Value,
    ) -> Result<(), String> {
        let validated_path = Self::validate_and_normalize_path(path)?;
        let (root, segments) = Self::parse_path(&validated_path)?;

        // Get a reference to the target field
        let target = Self::get_target_field(message, root);

        // Initialize target if needed
        if !target.is_object() && segments.len() > 1 {
            *target = json!({});
        }

        // If we're replacing the entire field
        if segments.len() == 1 {
            if !value.is_object() {
                return Err(format!("Can only replace '{}' with an object value", root));
            }
            *target = value;
            return Ok(());
        }

        // Navigate to the target location
        let mut current = target;
        for i in 1..segments.len() {
            let segment = segments[i];

            // If this is the last segment, set the value
            if i == segments.len() - 1 {
                match current {
                    Value::Object(obj) => {
                        obj.insert(segment.to_string(), value);
                        return Ok(());
                    }
                    _ => {
                        return Err(format!(
                            "Cannot set value at '{}' - parent is not an object",
                            validated_path
                        ))
                    }
                }
            }

            // Otherwise, navigate to next level
            match current {
                Value::Object(obj) => {
                    // Create empty objects for missing segments
                    if !obj.contains_key(segment) {
                        obj.insert(segment.to_string(), json!({}));
                    }
                    current = obj.get_mut(segment).unwrap();
                }
                _ => {
                    return Err(format!(
                        "Cannot navigate to '{}' - parent is not an object",
                        validated_path
                    ))
                }
            }
        }

        Ok(())
    }

    /// Get a value from a path in the message structure for old value recording
    fn get_value_from_message(message: &Message, path: &str) -> Value {
        let validated_path = match Self::validate_and_normalize_path(path) {
            Ok(p) => p,
            Err(_) => return Value::Null,
        };

        let (root, segments) = match Self::parse_path(&validated_path) {
            Ok(p) => p,
            Err(_) => return Value::Null,
        };

        // Get the source field
        let source = match root {
            "data" => &message.data,
            "metadata" => &message.metadata,
            _ => return Value::Null,
        };

        // If we're getting the entire field
        if segments.len() == 1 {
            return source.clone();
        }

        // Navigate to the target location
        let path_parts = &segments[1..];
        let pointer_path = "/".to_string() + &path_parts.join("/");
        match source.pointer(&pointer_path) {
            Some(val) => val.clone(),
            None => Value::Null,
        }
    }

    /// Evaluate a JSONLogic rule with the DataLogic instance
    fn evaluate_rule(&self, rule: &Value, context: &Value) -> Result<Value, String> {
        let data_logic = self
            .data_logic_mutex
            .lock()
            .map_err(|e| format!("Failed to acquire lock on DataLogic: {}", e))?;

        data_logic
            .evaluate_json(rule, context, None)
            .map_err(|e| format!("Failed to evaluate rule: {}", e))
    }

    /// Process a single mapping and update the message
    fn process_mapping(
        &self,
        message: &mut Message,
        context: &Value,
        path_str: &str,
        logic: &Value,
    ) -> Result<Change, String> {
        // Validate the path
        let validated_path = Self::validate_and_normalize_path(path_str)?;

        // Evaluate the rule
        let result = self.evaluate_rule(logic, context)?;

        // Get the old value for auditing
        let old_value = Self::get_value_from_message(message, &validated_path);

        // Apply the new value
        Self::apply_value_to_message(message, &validated_path, result.clone())?;

        Ok(Change {
            path: validated_path,
            old_value,
            new_value: result,
        })
    }
}

// Mark as threadsafe
unsafe impl Send for MapFunction {}
unsafe impl Sync for MapFunction {}

impl FunctionHandler for MapFunction {
    fn execute(
        &self,
        message: &mut Message,
        input: &Value,
    ) -> Result<(usize, Vec<Change>), String> {
        // Prepare a context for evaluating JSONLogic expressions
        let context: Value = json!({
            "temp_data": message.temp_data.clone(),
            "data": message.data.clone(),
            "metadata": message.metadata.clone(),
            "payload": message.payload.clone(),
            "input": input.clone()
        });

        // Process mappings
        let mut changes = Vec::new();

        // Process array-format mappings (preferred format)
        if let Some(mappings) = input.get("mappings").and_then(Value::as_array) {
            for mapping in mappings {
                if let (Some(path), Some(logic)) = (mapping.get("path"), mapping.get("logic")) {
                    if let Some(path_str) = path.as_str() {
                        let change = self.process_mapping(message, &context, path_str, logic)?;
                        changes.push(change);
                    }
                }
            }
        }

        // Log warning if no changes were made
        if changes.is_empty() {
            eprintln!("Warning: No mappings found in input for map function");
        }

        Ok((200, changes))
    }
}
