//! JSON Schema validation (M4 structured output).
//!
//! Validates agent output against a JSON Schema. Uses the `jsonschema` crate
//! for full schema compliance. Designed for structured output validation
//! where agents must return results matching a defined schema.

use serde_json::Value;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SchemaError {
    #[error("schema is not a valid JSON Schema: {0}")]
    InvalidSchema(String),
    #[error("output does not match schema: {0}")]
    ValidationFailed(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Validates a JSON value against the given JSON Schema.
///
/// # Arguments
/// * `output` - The agent's output to validate.
/// * `schema` - The JSON Schema to validate against.
///
/// # Returns
/// * `Ok(())` if validation passes.
/// * `Err(SchemaError)` with a description of why validation failed.
pub fn validate_output(output: &Value, schema: &Value) -> Result<(), SchemaError> {
    // First, validate that the schema itself is a valid JSON Schema object
    match schema {
        Value::Object(_) => {}
        Value::Bool(b) => {
            // JSON Schema allows boolean schemas: true = anything, false = nothing
            if !b {
                return Err(SchemaError::ValidationFailed(
                    "schema is 'false' (rejects all)".to_string(),
                ));
            }
            return Ok(());
        }
        _ => {
            return Err(SchemaError::InvalidSchema(
                "schema must be an object or boolean".to_string(),
            ));
        }
    }

    // Create a JSON Schema validator using jsonschema crate
    let validator = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|e| SchemaError::InvalidSchema(format!("failed to compile schema: {}", e)))?;

    if let Err(errors) = validator.validate(output) {
        // Collect the first few validation errors (owned strings, no borrow of validator)
        let details: Vec<String> = errors
            .take(5)
            .map(|e| format!("instance {}: {}", e.instance_path, e))
            .collect();
        return Err(SchemaError::ValidationFailed(details.join("; ")));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_valid_output_matches_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "result": { "type": "string" },
                "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
            },
            "required": ["result"]
        });

        let output = json!({
            "result": "success",
            "confidence": 0.95
        });

        assert!(validate_output(&output, &schema).is_ok());
    }

    #[test]
    fn test_invalid_output_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "result": { "type": "string" }
            },
            "required": ["result"]
        });

        let output = json!({"result": 42});
        let err = validate_output(&output, &schema).unwrap_err();
        assert!(matches!(err, SchemaError::ValidationFailed(_)));
    }

    #[test]
    fn test_missing_required_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "result": { "type": "string" }
            },
            "required": ["result"]
        });

        let output = json!({"other": "value"});
        let err = validate_output(&output, &schema).unwrap_err();
        assert!(matches!(err, SchemaError::ValidationFailed(_)));
    }

    #[test]
    fn test_boolean_schema_false() {
        let err = validate_output(&json!("anything"), &json!(false)).unwrap_err();
        assert!(matches!(err, SchemaError::ValidationFailed(_)));
    }

    #[test]
    fn test_boolean_schema_true() {
        assert!(validate_output(&json!("anything"), &json!(true)).is_ok());
        assert!(validate_output(&json!({"key": "value"}), &json!(true)).is_ok());
    }

    #[test]
    fn test_invalid_schema_not_object() {
        let err = validate_output(&json!("value"), &json!("not_a_schema")).unwrap_err();
        assert!(matches!(err, SchemaError::InvalidSchema(_)));
    }

    #[test]
    fn test_nested_object_validation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["id"]
                }
            },
            "required": ["data"]
        });

        let valid = json!({
            "data": {
                "id": 1,
                "tags": ["a", "b"]
            }
        });
        assert!(validate_output(&valid, &schema).is_ok());

        let invalid = json!({
            "data": {
                "id": "not_an_int",
                "tags": [1, 2]
            }
        });
        assert!(validate_output(&invalid, &schema).is_err());
    }
}
