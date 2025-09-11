//! # Utility Functions Module
//!
//! This module contains common utility functions used throughout the engine.
//! These utilities provide helper functionality for:
//! - JSON value manipulation and path navigation
//! - Value truthiness evaluation
//! - Nested data structure access and modification

use serde_json::Value;

/// Helper function to check if a value is truthy
///
/// This function determines truthiness based on JavaScript-like semantics:
/// - `false`, `null`, `0`, `""`, `[]`, `{}` are falsy
/// - Everything else is truthy
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

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
pub fn get_nested_value<'b>(data: &'b Value, path: &str) -> Option<&'b Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = data;

    for part in parts {
        match current {
            Value::Object(map) => {
                current = map.get(part)?;
            }
            Value::Array(arr) => {
                let index = part.parse::<usize>().ok()?;
                current = arr.get(index)?;
            }
            _ => return None,
        }
    }

    Some(current)
}

/// Set nested value in JSON using dot notation path
///
/// Creates intermediate objects as needed when navigating the path.
/// Supports setting values in nested objects but not arrays.
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
            if let Value::Object(map) = current {
                map.insert(part.to_string(), value);
            }
            return;
        }

        // Navigate to the next level, creating objects as needed
        match current {
            Value::Object(map) => {
                current = map
                    .entry(part.to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
            }
            _ => return, // Can't navigate further
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
    fn test_is_truthy() {
        // Boolean values
        assert_eq!(is_truthy(&json!(true)), true);
        assert_eq!(is_truthy(&json!(false)), false);

        // Null
        assert_eq!(is_truthy(&json!(null)), false);

        // Numbers
        assert_eq!(is_truthy(&json!(0)), false);
        assert_eq!(is_truthy(&json!(0.0)), false);
        assert_eq!(is_truthy(&json!(1)), true);
        assert_eq!(is_truthy(&json!(-1)), true);
        assert_eq!(is_truthy(&json!(0.1)), true);

        // Strings
        assert_eq!(is_truthy(&json!("")), false);
        assert_eq!(is_truthy(&json!("hello")), true);
        assert_eq!(is_truthy(&json!(" ")), true); // Whitespace is truthy

        // Arrays
        assert_eq!(is_truthy(&json!([])), false);
        assert_eq!(is_truthy(&json!([1])), true);
        assert_eq!(is_truthy(&json!([false])), true); // Non-empty array is truthy

        // Objects
        assert_eq!(is_truthy(&json!({})), false);
        assert_eq!(is_truthy(&json!({"key": "value"})), true);
        assert_eq!(is_truthy(&json!({"key": null})), true); // Non-empty object is truthy
    }

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

        // Note: Current implementation doesn't support setting array elements
        // This is a known limitation that could be addressed in future
        set_nested_value(&mut data, "items.0", json!(10));
        // Array remains unchanged because we can't navigate into arrays for setting
        assert_eq!(data["items"], json!([1, 2, 3]));
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
}
