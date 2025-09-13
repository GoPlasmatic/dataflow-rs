//! # Utility Functions Module
//!
//! This module contains common utility functions used throughout the engine.
//! These utilities provide helper functionality for:
//! - JSON value manipulation and path navigation
//! - Value truthiness evaluation
//! - Nested data structure access and modification

use serde_json::Value;

/// Get nested value from JSON using dot notation path
///
/// Supports both object property access and array indexing:
/// - `"user.name"` - Access object property
/// - `"items.0"` - Access array element by index
/// - `"user.addresses.0.city"` - Combined object and array access
///
/// # Arguments
/// * `data` - The JSON value to navigate
/// * `path` - Dot-separated path to the target value
///
/// # Returns
/// * `Option<&Value>` - Reference to the value if found, None otherwise
///
/// # Safety
/// Returns None for invalid array indices or missing keys rather than panicking
pub fn get_nested_value<'b>(data: &'b Value, path: &str) -> Option<&'b Value> {
    if path.is_empty() {
        return Some(data);
    }

    let parts: Vec<&str> = path.split('.').collect();
    let mut current = data;

    for part in parts {
        match current {
            Value::Object(map) => {
                current = map.get(part)?;
            }
            Value::Array(arr) => {
                // Safely parse and validate array index
                let index = match part.parse::<usize>() {
                    Ok(idx) => idx,
                    Err(_) => return None, // Invalid index format
                };

                // Bounds check before access
                if index >= arr.len() {
                    return None; // Index out of bounds
                }

                current = arr.get(index)?;
            }
            _ => return None, // Can't navigate further
        }
    }

    Some(current)
}

/// Set nested value in JSON using dot notation path
///
/// Creates intermediate objects or arrays as needed when navigating the path.
/// Supports setting values in nested objects and arrays with automatic expansion.
///
/// # Arguments
/// * `data` - The JSON value to modify
/// * `path` - Dot-separated path to the target location
/// * `value` - The value to set at the target location
///
/// # Example
/// ```
/// use serde_json::json;
/// use dataflow_rs::engine::utils::set_nested_value;
///
/// let mut data = json!({});
/// set_nested_value(&mut data, "user.name", json!("Alice"));
/// assert_eq!(data, json!({"user": {"name": "Alice"}}));
/// ```
pub fn set_nested_value(data: &mut Value, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = data;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last part - set the value
            match current {
                Value::Object(map) => {
                    map.insert(part.to_string(), value);
                }
                Value::Array(arr) => {
                    // Try to parse as array index
                    if let Ok(index) = part.parse::<usize>() {
                        // Expand array if necessary (fill with nulls)
                        while arr.len() <= index {
                            arr.push(Value::Null);
                        }
                        if index < arr.len() {
                            arr[index] = value;
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Navigate to the next level
        // Check if next part is a number (array index)
        let next_is_array = parts
            .get(i + 1)
            .and_then(|p| p.parse::<usize>().ok())
            .is_some();

        match current {
            Value::Object(map) => {
                // Check if current part is meant to be an array index
                if let Ok(_index) = part.parse::<usize>() {
                    // This shouldn't happen in a well-formed path
                    return;
                }

                // Create the appropriate structure for the next level
                current = map.entry(part.to_string()).or_insert_with(|| {
                    if next_is_array {
                        Value::Array(Vec::new())
                    } else {
                        Value::Object(serde_json::Map::new())
                    }
                });
            }
            Value::Array(arr) => {
                // Parse current part as array index
                if let Ok(index) = part.parse::<usize>() {
                    // Expand array if necessary
                    while arr.len() <= index {
                        arr.push(Value::Null);
                    }

                    // Ensure the element at index exists and is the right type
                    if arr[index].is_null() {
                        arr[index] = if next_is_array {
                            Value::Array(Vec::new())
                        } else {
                            Value::Object(serde_json::Map::new())
                        };
                    }

                    current = &mut arr[index];
                } else {
                    // Can't use string key on array
                    return;
                }
            }
            _ => {
                // Current value is neither object nor array, can't navigate
                return;
            }
        }
    }
}

/// Clone a value at a nested path
///
/// Combines `get_nested_value` with cloning for convenience.
///
/// # Arguments
/// * `data` - The JSON value to read from
/// * `path` - Dot-separated path to the target value
///
/// # Returns
/// * `Option<Value>` - Cloned value if found, None otherwise
pub fn get_nested_value_cloned(data: &Value, path: &str) -> Option<Value> {
    get_nested_value(data, path).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_get_nested_value() {
        let data = json!({
            "user": {
                "name": "John",
                "age": 30,
                "addresses": [
                    {"city": "New York", "zip": "10001"},
                    {"city": "San Francisco", "zip": "94102"}
                ],
                "preferences": {
                    "theme": "dark",
                    "notifications": true
                }
            },
            "items": [1, 2, 3]
        });

        // Object property access
        assert_eq!(get_nested_value(&data, "user.name"), Some(&json!("John")));
        assert_eq!(get_nested_value(&data, "user.age"), Some(&json!(30)));

        // Nested object access
        assert_eq!(
            get_nested_value(&data, "user.preferences.theme"),
            Some(&json!("dark"))
        );
        assert_eq!(
            get_nested_value(&data, "user.preferences.notifications"),
            Some(&json!(true))
        );

        // Array element access
        assert_eq!(get_nested_value(&data, "items.0"), Some(&json!(1)));
        assert_eq!(get_nested_value(&data, "items.2"), Some(&json!(3)));

        // Combined object and array access
        assert_eq!(
            get_nested_value(&data, "user.addresses.0.city"),
            Some(&json!("New York"))
        );
        assert_eq!(
            get_nested_value(&data, "user.addresses.1.zip"),
            Some(&json!("94102"))
        );

        // Non-existent paths
        assert_eq!(get_nested_value(&data, "user.missing"), None);
        assert_eq!(get_nested_value(&data, "items.10"), None);
        assert_eq!(get_nested_value(&data, "user.addresses.2.city"), None);
        assert_eq!(get_nested_value(&data, "nonexistent.path"), None);
    }

    #[test]
    fn test_set_nested_value() {
        let mut data = json!({});

        // Set simple property
        set_nested_value(&mut data, "name", json!("Alice"));
        assert_eq!(data, json!({"name": "Alice"}));

        // Set nested property (creates intermediate objects)
        set_nested_value(&mut data, "user.email", json!("alice@example.com"));
        assert_eq!(
            data,
            json!({
                "name": "Alice",
                "user": {"email": "alice@example.com"}
            })
        );

        // Overwrite existing value
        set_nested_value(&mut data, "name", json!("Bob"));
        assert_eq!(
            data,
            json!({
                "name": "Bob",
                "user": {"email": "alice@example.com"}
            })
        );

        // Set deeply nested property
        set_nested_value(&mut data, "settings.theme.mode", json!("dark"));
        assert_eq!(data["settings"]["theme"]["mode"], json!("dark"));

        // Add to existing nested object
        set_nested_value(&mut data, "user.age", json!(25));
        assert_eq!(data["user"]["age"], json!(25));
        assert_eq!(data["user"]["email"], json!("alice@example.com"));
    }

    #[test]
    fn test_set_nested_value_with_arrays() {
        let mut data = json!({
            "items": [1, 2, 3]
        });

        // Test setting existing array element
        set_nested_value(&mut data, "items.0", json!(10));
        assert_eq!(data["items"], json!([10, 2, 3]));

        // Test setting array element beyond current length (should expand)
        set_nested_value(&mut data, "items.5", json!(50));
        assert_eq!(data["items"], json!([10, 2, 3, null, null, 50]));

        // Test creating nested array structure
        let mut data2 = json!({});
        set_nested_value(&mut data2, "matrix.0.0", json!(1));
        set_nested_value(&mut data2, "matrix.0.1", json!(2));
        set_nested_value(&mut data2, "matrix.1.0", json!(3));
        assert_eq!(
            data2,
            json!({
                "matrix": [[1, 2], [3]]
            })
        );
    }

    #[test]
    fn test_set_nested_value_array_expansion() {
        let mut data = json!({});

        // Create array and set element at index 2 (should create nulls for 0 and 1)
        set_nested_value(&mut data, "array.2", json!("value"));
        assert_eq!(
            data,
            json!({
                "array": [null, null, "value"]
            })
        );

        // Test deeply nested array creation
        let mut data2 = json!({});
        set_nested_value(&mut data2, "deep.nested.0.field", json!("test"));
        assert_eq!(
            data2,
            json!({
                "deep": {
                    "nested": [{"field": "test"}]
                }
            })
        );
    }

    #[test]
    fn test_get_nested_value_cloned() {
        let data = json!({
            "user": {
                "profile": {
                    "name": "Alice",
                    "settings": {"theme": "dark"}
                }
            }
        });

        // Test successful cloning
        let cloned = get_nested_value_cloned(&data, "user.profile.name");
        assert_eq!(cloned, Some(json!("Alice")));

        // Test cloning complex object
        let cloned = get_nested_value_cloned(&data, "user.profile.settings");
        assert_eq!(cloned, Some(json!({"theme": "dark"})));

        // Test non-existent path
        let cloned = get_nested_value_cloned(&data, "user.missing");
        assert_eq!(cloned, None);
    }

    #[test]
    fn test_get_nested_value_bounds_checking() {
        let data = json!({
            "items": [1, 2, 3],
            "nested": {
                "array": [
                    {"id": 1},
                    {"id": 2}
                ]
            }
        });

        // Test valid array access
        assert_eq!(get_nested_value(&data, "items.0"), Some(&json!(1)));
        assert_eq!(get_nested_value(&data, "items.2"), Some(&json!(3)));

        // Test out-of-bounds array access (should return None, not panic)
        assert_eq!(get_nested_value(&data, "items.10"), None);
        assert_eq!(get_nested_value(&data, "items.999999"), None);

        // Test invalid array index format
        assert_eq!(get_nested_value(&data, "items.abc"), None);
        assert_eq!(get_nested_value(&data, "items.-1"), None);
        assert_eq!(get_nested_value(&data, "items.2.5"), None);

        // Test nested array bounds
        assert_eq!(
            get_nested_value(&data, "nested.array.0.id"),
            Some(&json!(1))
        );
        assert_eq!(get_nested_value(&data, "nested.array.5.id"), None);

        // Test empty path
        assert_eq!(get_nested_value(&data, ""), Some(&data));
    }

    #[test]
    fn test_set_nested_value_bounds_safety() {
        let mut data = json!({});

        // Test creating arrays with large indices (should create nulls in between)
        set_nested_value(&mut data, "large.10", json!("value"));
        assert_eq!(data["large"].as_array().unwrap().len(), 11);
        assert_eq!(data["large"][10], json!("value"));
        for i in 0..10 {
            assert_eq!(data["large"][i], json!(null));
        }

        // Test setting nested array values
        let mut data2 = json!({"matrix": []});
        set_nested_value(&mut data2, "matrix.2.1", json!(5));
        assert_eq!(data2["matrix"][0], json!(null));
        assert_eq!(data2["matrix"][1], json!(null));
        assert_eq!(data2["matrix"][2][0], json!(null));
        assert_eq!(data2["matrix"][2][1], json!(5));

        // Test overwriting array elements
        let mut data3 = json!({"arr": [1, 2, 3]});
        set_nested_value(&mut data3, "arr.1", json!("replaced"));
        assert_eq!(data3["arr"], json!([1, "replaced", 3]));
    }
}
