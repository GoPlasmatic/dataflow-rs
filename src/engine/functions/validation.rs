use crate::engine::error::{DataflowError, Result};
use serde::Deserialize;
use serde_json::Value;

/// Pre-parsed configuration for validation function
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationConfig {
    pub rules: Vec<ValidationRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidationRule {
    pub logic: Value,
    pub path: String,
    pub message: String,
    #[serde(skip)]
    pub logic_index: Option<usize>,
}

impl ValidationConfig {
    pub fn from_json(input: &Value) -> Result<Self> {
        let rules = input.get("rules").ok_or_else(|| {
            DataflowError::Validation("Missing 'rules' array in input".to_string())
        })?;

        let rules_arr = rules
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'rules' must be an array".to_string()))?;

        let mut parsed_rules = Vec::new();

        for rule in rules_arr {
            let logic = rule
                .get("logic")
                .ok_or_else(|| DataflowError::Validation("Missing 'logic' in rule".to_string()))?
                .clone();

            let path = rule
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("data")
                .to_string();

            let message = rule
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Validation failed")
                .to_string();

            parsed_rules.push(ValidationRule {
                logic,
                path,
                message,
                logic_index: None,
            });
        }

        Ok(ValidationConfig {
            rules: parsed_rules,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validation_config_from_json() {
        let input = json!({
            "rules": [
                {
                    "logic": {"!!": [{"var": "data.required_field"}]},
                    "path": "data",
                    "message": "Required field is missing"
                },
                {
                    "logic": {">": [{"var": "data.age"}, 18]},
                    "message": "Must be over 18"
                }
            ]
        });

        let config = ValidationConfig::from_json(&input).unwrap();
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.rules[0].path, "data");
        assert_eq!(config.rules[0].message, "Required field is missing");
        assert_eq!(config.rules[1].path, "data"); // Default path
        assert_eq!(config.rules[1].message, "Must be over 18");
    }

    #[test]
    fn test_validation_config_missing_rules() {
        let input = json!({});
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_invalid_rules() {
        let input = json!({
            "rules": "not_an_array"
        });
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_missing_logic() {
        let input = json!({
            "rules": [
                {
                    "path": "data",
                    "message": "Some error"
                }
            ]
        });
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_defaults() {
        let input = json!({
            "rules": [
                {
                    "logic": {"var": "data.field"}
                }
            ]
        });

        let config = ValidationConfig::from_json(&input).unwrap();
        assert_eq!(config.rules[0].path, "data");
        assert_eq!(config.rules[0].message, "Validation failed");
    }
}
