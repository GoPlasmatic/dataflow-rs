use crate::engine::AsyncFunctionHandler;
use crate::engine::error::{DataflowError, Result};
use crate::engine::message::{Change, Message};
use async_trait::async_trait;
use datalogic_rs::DataLogic;
use log::error;
use serde_json::{Value, json};

/// A mapping function that transforms data using JSONLogic expressions.
///
/// This function allows mapping data from one location to another within
/// a message, applying transformations using JSONLogic expressions.
pub struct MapFunction;

impl Default for MapFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl MapFunction {
    /// Create a new MapFunction
    pub fn new() -> Self {
        Self
    }

    /// Set a value at the specified path within the target object
    fn set_value_at_path(&self, target: &mut Value, path: &str, value: &Value) -> Result<Value> {
        let mut current = target;
        let mut old_value = Value::Null;
        let path_parts: Vec<&str> = path.split('.').collect();

        // Helper function to check if a string is a valid array index
        fn is_numeric_index(s: &str) -> bool {
            s.parse::<usize>().is_ok()
        }

        // Navigate to the parent of the target location
        for (i, part) in path_parts.iter().enumerate() {
            let is_numeric = is_numeric_index(part);

            if i == path_parts.len() - 1 {
                // We're at the last part, so set the value
                if is_numeric {
                    // Handle numeric index - ensure current is an array
                    if !current.is_array() {
                        // Convert to array if it's not already
                        *current = Value::Array(vec![]);
                    }

                    if let Ok(index) = part.parse::<usize>() {
                        if let Value::Array(arr) = current {
                            // Extend array if needed
                            while arr.len() <= index {
                                arr.push(Value::Null);
                            }
                            // Save old value
                            old_value = arr[index].clone();
                            arr[index] = value.clone();
                        }
                    } else {
                        error!("Invalid array index: {part}");
                        return Err(DataflowError::Validation(format!(
                            "Invalid array index: {part}"
                        )));
                    }
                } else {
                    // Handle object property
                    if !current.is_object() {
                        // Convert to object if it's not already
                        *current = Value::Object(serde_json::Map::new());
                    }

                    if let Value::Object(map) = current {
                        // Save the old value before replacing
                        let mut key = part.to_string();
                        if key.starts_with("#") {
                            key = key.strip_prefix("#").unwrap_or(&key).to_string();
                        }
                        old_value = map.get(&key).cloned().unwrap_or(Value::Null);
                        // Merge objects if both old and new values are objects
                        let value_to_insert = if old_value.is_object() && value.is_object() {
                            let mut merged_map = old_value.as_object().unwrap().clone();
                            if let Some(new_map) = value.as_object() {
                                // New values override old values
                                for (k, v) in new_map {
                                    merged_map.insert(k.clone(), v.clone());
                                }
                            }
                            Value::Object(merged_map)
                        } else {
                            value.clone()
                        };
                        map.insert(key, value_to_insert);
                    }
                }
            } else {
                // We need to navigate deeper
                if is_numeric {
                    // Handle numeric index - ensure current is an array
                    if !current.is_array() {
                        *current = Value::Array(vec![]);
                    }

                    if let Ok(index) = part.parse::<usize>() {
                        if let Value::Array(arr) = current {
                            // Extend array if needed
                            while arr.len() <= index {
                                arr.push(Value::Null);
                            }
                            // Ensure the indexed element exists and is ready for further navigation
                            if arr[index].is_null() {
                                // Look ahead to see if next part is numeric to decide what to create
                                let next_part = path_parts.get(i + 1).unwrap_or(&"");
                                if is_numeric_index(next_part) {
                                    arr[index] = Value::Array(vec![]);
                                } else {
                                    arr[index] = json!({});
                                }
                            }
                            current = &mut arr[index];
                        }
                    } else {
                        error!("Invalid array index: {part}");
                        return Err(DataflowError::Validation(format!(
                            "Invalid array index: {part}"
                        )));
                    }
                } else {
                    // Handle object property
                    if !current.is_object() {
                        *current = Value::Object(serde_json::Map::new());
                    }

                    if let Value::Object(map) = current {
                        let mut key = part.to_string();
                        if key.starts_with("#") {
                            key = key.strip_prefix("#").unwrap_or(&key).to_string();
                        }
                        if !map.contains_key(&key) {
                            // Look ahead to see if next part is numeric to decide what to create
                            let next_part = path_parts.get(i + 1).unwrap_or(&"");
                            if is_numeric_index(next_part) {
                                map.insert(part.to_string(), Value::Array(vec![]));
                            } else {
                                map.insert(key.clone(), json!({}));
                            }
                        }
                        current = map.get_mut(&key).unwrap();
                    }
                }
            }
        }

        Ok(old_value)
    }
}

#[async_trait]
impl AsyncFunctionHandler for MapFunction {
    async fn execute(
        &self,
        message: &mut Message,
        input: &Value,
        data_logic: &mut DataLogic,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract mappings array from input
        let mappings = input.get("mappings").ok_or_else(|| {
            DataflowError::Validation("Missing 'mappings' array in input".to_string())
        })?;

        let mappings_arr = mappings
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'mappings' must be an array".to_string()))?;

        let mut changes = Vec::new();

        // Process each mapping
        for mapping in mappings_arr {
            // Get path where to store the result
            let target_path = mapping.get("path").and_then(Value::as_str).ok_or_else(|| {
                DataflowError::Validation("Missing 'path' in mapping".to_string())
            })?;

            // Get the logic to evaluate
            let logic = mapping.get("logic").ok_or_else(|| {
                DataflowError::Validation("Missing 'logic' in mapping".to_string())
            })?;

            // Clone message data for evaluation context - do this for each iteration
            // to ensure subsequent mappings see changes from previous mappings
            let data_clone = message.data.clone();
            let metadata_clone = message.metadata.clone();
            let temp_data_clone = message.temp_data.clone();

            // Create a combined data object with message fields for evaluation
            let data_for_eval = json!({
                "data": data_clone,
                "metadata": metadata_clone,
                "temp_data": temp_data_clone,
            });

            // Determine target object based on path prefix
            let (target_object, adjusted_path) =
                if let Some(path) = target_path.strip_prefix("data.") {
                    (&mut message.data, path)
                } else if let Some(path) = target_path.strip_prefix("metadata.") {
                    (&mut message.metadata, path)
                } else if let Some(path) = target_path.strip_prefix("temp_data.") {
                    (&mut message.temp_data, path)
                } else if target_path == "data" {
                    (&mut message.data, "")
                } else if target_path == "metadata" {
                    (&mut message.metadata, "")
                } else if target_path == "temp_data" {
                    (&mut message.temp_data, "")
                } else {
                    // Default to data
                    (&mut message.data, target_path)
                };

            // Evaluate the logic using provided DataLogic
            data_logic.reset_arena();
            let result = data_logic
                .evaluate_json(logic, &data_for_eval, None)
                .map_err(|e| {
                    error!("Failed to evaluate logic: {e} for {logic}, {data_for_eval}");
                    DataflowError::LogicEvaluation(format!("Failed to evaluate logic: {e}"))
                })?;

            if result.is_null() {
                continue;
            }

            // Set the result at the target path
            if adjusted_path.is_empty() {
                // Replace the entire object
                let old_value = if target_object.is_object() && result.is_object() {
                    let mut merged_map = target_object.as_object().unwrap().clone();
                    if let Some(new_map) = result.as_object() {
                        // New values override old values
                        for (k, v) in new_map {
                            merged_map.insert(k.clone(), v.clone());
                        }
                    }
                    std::mem::replace(target_object, Value::Object(merged_map))
                } else {
                    std::mem::replace(target_object, result.clone())
                };
                changes.push(Change {
                    path: target_path.to_string(),
                    old_value,
                    new_value: result,
                });
            } else {
                // Set a specific path
                let old_value = self.set_value_at_path(target_object, adjusted_path, &result)?;
                changes.push(Change {
                    path: target_path.to_string(),
                    old_value,
                    new_value: result,
                });
            }
        }

        Ok((200, changes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::message::Message;
    use datalogic_rs::DataLogic;
    use serde_json::json;

    #[tokio::test]
    async fn test_array_notation_simple() {
        let map_fn = MapFunction::new();

        // Test simple array notation: data.items.0.name
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.items.0.name",
                    "logic": "Test Item"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "items": [
                {
                    "name": "Test Item"
                }
            ]
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_array_notation_complex_path() {
        let map_fn = MapFunction::new();

        // Test complex path like the original example: data.MX.FIToFICstmrCdtTrf.CdtTrfTxInf.0.PmtId.InstrId
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.MX.FIToFICstmrCdtTrf.CdtTrfTxInf.0.PmtId.InstrId",
                    "logic": "INSTR123"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "MX": {
                "FIToFICstmrCdtTrf": {
                    "CdtTrfTxInf": [
                        {
                            "PmtId": {
                                "InstrId": "INSTR123"
                            }
                        }
                    ]
                }
            }
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_multiple_array_indices() {
        let map_fn = MapFunction::new();

        // Test multiple array indices in the same path: data.matrix.0.1.value
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.matrix.0.1.value",
                    "logic": "cell_01"
                },
                {
                    "path": "data.matrix.1.0.value",
                    "logic": "cell_10"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "matrix": [
                [
                    null,
                    {
                        "value": "cell_01"
                    }
                ],
                [
                    {
                        "value": "cell_10"
                    }
                ]
            ]
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_array_extension() {
        let map_fn = MapFunction::new();

        // Test that arrays are extended when accessing high indices
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.items.5.name",
                    "logic": "Item at index 5"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());

        // Should create an array with 6 elements (indices 0-5)
        assert!(message.data["items"].is_array());
        let items_array = message.data["items"].as_array().unwrap();
        assert_eq!(items_array.len(), 6);

        // First 5 elements should be null
        for i in 0..5 {
            assert_eq!(items_array[i], json!(null));
        }

        // Element at index 5 should have our value
        assert_eq!(items_array[5], json!({"name": "Item at index 5"}));
    }

    #[tokio::test]
    async fn test_mixed_array_and_object_notation() {
        let map_fn = MapFunction::new();

        // Test mixing array and object notation: data.users.0.profile.addresses.1.city
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.users.0.profile.addresses.1.city",
                    "logic": "New York"
                },
                {
                    "path": "data.users.0.profile.name",
                    "logic": "John Doe"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "users": [
                {
                    "profile": {
                        "name": "John Doe",
                        "addresses": [
                            null,
                            {
                                "city": "New York"
                            }
                        ]
                    }
                }
            ]
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_overwrite_existing_value() {
        let map_fn = MapFunction::new();

        // Test overwriting an existing value in an array
        let mut message = Message::new(&json!({}));
        message.data = json!({
            "items": [
                {"name": "Old Value"},
                {"name": "Another Item"}
            ]
        });

        let input = json!({
            "mappings": [
                {
                    "path": "data.items.0.name",
                    "logic": "New Value"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "items": [
                {"name": "New Value"},
                {"name": "Another Item"}
            ]
        });
        assert_eq!(message.data, expected);

        // Check that changes are recorded
        let (_, changes) = result.unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "data.items.0.name");
        assert_eq!(changes[0].old_value, json!("Old Value"));
        assert_eq!(changes[0].new_value, json!("New Value"));
    }

    #[tokio::test]
    async fn test_array_notation_with_jsonlogic() {
        let map_fn = MapFunction::new();

        // Test array notation with JSONLogic expressions
        let mut message = Message::new(&json!({}));
        message.temp_data = json!({
            "transactions": [
                {"id": "tx1", "amount": 100},
                {"id": "tx2", "amount": 200}
            ]
        });
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.processed.0.transaction_id",
                    "logic": {"var": "temp_data.transactions.0.id"}
                },
                {
                    "path": "data.processed.0.amount_cents",
                    "logic": {"*": [{"var": "temp_data.transactions.0.amount"}, 100]}
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "processed": [
                {
                    "transaction_id": "tx1",
                    "amount_cents": 10000
                }
            ]
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_convert_object_to_array() {
        let map_fn = MapFunction::new();

        // Test converting an existing object to an array when numeric index is encountered
        let mut message = Message::new(&json!({}));
        message.data = json!({
            "items": {
                "existing_key": "existing_value"
            }
        });

        let input = json!({
            "mappings": [
                {
                    "path": "data.items.0.new_value",
                    "logic": "array_item"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        // The object should be converted to an array
        assert!(message.data["items"].is_array());
        let expected = json!({
            "items": [
                {
                    "new_value": "array_item"
                }
            ]
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_non_numeric_index_handling() {
        let map_fn = MapFunction::new();

        // Test that non-numeric strings are treated as object keys, not array indices
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.items.invalid_index.name",
                    "logic": "test"
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        // This should succeed and create an object structure
        assert!(result.is_ok());
        let expected = json!({
            "items": {
                "invalid_index": {
                    "name": "test"
                }
            }
        });
        assert_eq!(message.data, expected);

        // Verify that "invalid_index" was treated as an object key, not array index
        assert!(message.data["items"].is_object());
        assert!(!message.data["items"].is_array());
    }

    #[tokio::test]
    async fn test_object_merge_on_mapping() {
        let map_fn = MapFunction::new();

        // Test that when mapping to an existing object, the values are merged
        let mut message = Message::new(&json!({}));
        message.data = json!({
            "config": {
                "database": {
                    "host": "localhost",
                    "port": 5432,
                    "username": "admin"
                }
            }
        });

        // First mapping - add new fields to existing object
        let input = json!({
            "mappings": [
                {
                    "path": "data.config.database",
                    "logic": {
                        "password": "secret123",
                        "ssl": true,
                        "port": 5433  // This should override the existing port
                    }
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "config": {
                "database": {
                    "host": "localhost",
                    "port": 5433,  // Updated
                    "username": "admin",
                    "password": "secret123",  // Added
                    "ssl": true  // Added
                }
            }
        });
        assert_eq!(message.data, expected);

        // Check that changes are recorded correctly
        let (_, changes) = result.unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "data.config.database");
        assert_eq!(
            changes[0].old_value,
            json!({
                "host": "localhost",
                "port": 5432,
                "username": "admin"
            })
        );
        assert_eq!(
            changes[0].new_value,
            json!({
                "password": "secret123",
                "ssl": true,
                "port": 5433
            })
        );
    }

    #[tokio::test]
    async fn test_object_merge_with_nested_path() {
        let map_fn = MapFunction::new();

        // Test object merging with a nested path
        let mut message = Message::new(&json!({}));
        message.data = json!({
            "user": {
                "profile": {
                    "name": "John Doe",
                    "age": 30
                }
            }
        });

        let input = json!({
            "mappings": [
                {
                    "path": "data.user.profile",
                    "logic": {
                        "email": "john@example.com",
                        "age": 31,  // Override
                        "city": "New York"
                    }
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "user": {
                "profile": {
                    "name": "John Doe",  // Preserved
                    "age": 31,  // Updated
                    "email": "john@example.com",  // Added
                    "city": "New York"  // Added
                }
            }
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_non_object_replacement() {
        let map_fn = MapFunction::new();

        // Test that non-object values are replaced, not merged
        let mut message = Message::new(&json!({}));
        message.data = json!({
            "settings": {
                "value": "old_string"
            }
        });

        let input = json!({
            "mappings": [
                {
                    "path": "data.settings.value",
                    "logic": {"new": "object"}
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        // String should be replaced with object, not merged
        let expected = json!({
            "settings": {
                "value": {"new": "object"}
            }
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_parent_child_mapping_issue_fix() {
        let map_fn = MapFunction::new();

        // Test case for GitHub issue #1: Multiple mappings where parent overwrites child
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        // First map to parent, then to child - child should preserve parent's other fields
        let input = json!({
            "mappings": [
                {
                    "path": "data.Parent.Child",
                    "logic": {
                        "Field1": "Value1",
                        "Field2": "Value2"
                    }
                },
                {
                    "path": "data.Parent.Child.NestedObject",
                    "logic": {
                        "NestedField": "NestedValue"
                    }
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "Parent": {
                "Child": {
                    "Field1": "Value1",
                    "Field2": "Value2",
                    "NestedObject": {
                        "NestedField": "NestedValue"
                    }
                }
            }
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_multiple_mappings_with_dependencies() {
        let map_fn = MapFunction::new();

        // Test that later mappings can use values set by earlier mappings
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.config.database.host",
                    "logic": "localhost"
                },
                {
                    "path": "data.config.database.port",
                    "logic": 5432
                },
                {
                    // This mapping references the previously set values
                    "path": "data.config.connectionString",
                    "logic": {
                        "cat": [
                            "postgresql://",
                            {"var": "data.config.database.host"},
                            ":",
                            {"var": "data.config.database.port"}
                        ]
                    }
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "config": {
                "database": {
                    "host": "localhost",
                    "port": 5432
                },
                "connectionString": "postgresql://localhost:5432"
            }
        });
        assert_eq!(message.data, expected);
    }

    #[tokio::test]
    async fn test_nested_path_after_parent_mapping() {
        let map_fn = MapFunction::new();

        // Test complex scenario: map to parent object, then add nested fields
        let mut message = Message::new(&json!({}));
        message.data = json!({});

        let input = json!({
            "mappings": [
                {
                    "path": "data.transaction",
                    "logic": {
                        "id": "TX-001",
                        "amount": 1000,
                        "currency": "USD"
                    }
                },
                {
                    "path": "data.transaction.metadata",
                    "logic": {
                        "timestamp": "2024-01-01T12:00:00Z",
                        "source": "API"
                    }
                },
                {
                    "path": "data.transaction.metadata.tags",
                    "logic": ["urgent", "international"]
                }
            ]
        });

        let mut data_logic = DataLogic::with_preserve_structure();
        let result = map_fn.execute(&mut message, &input, &mut data_logic).await;

        assert!(result.is_ok());
        let expected = json!({
            "transaction": {
                "id": "TX-001",
                "amount": 1000,
                "currency": "USD",
                "metadata": {
                    "timestamp": "2024-01-01T12:00:00Z",
                    "source": "API",
                    "tags": ["urgent", "international"]
                }
            }
        });
        assert_eq!(message.data, expected);
    }
}
