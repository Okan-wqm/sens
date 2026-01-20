//! Common utilities for PLC programming protocols
//!
//! Shared functionality across all PLC programming implementations.

use anyhow::{Result, anyhow};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{info, warn};

/// Default connection timeout
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default operation timeout
pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Default program upload timeout (longer for large programs)
pub const DEFAULT_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum program size (10 MB)
pub const MAX_PROGRAM_SIZE: usize = 10 * 1024 * 1024;

/// Validate program source code
pub fn validate_program_source(source: &str) -> Result<()> {
    if source.is_empty() {
        return Err(anyhow!("Program source cannot be empty"));
    }

    if source.len() > MAX_PROGRAM_SIZE {
        return Err(anyhow!(
            "Program size {} bytes exceeds maximum {} bytes",
            source.len(),
            MAX_PROGRAM_SIZE
        ));
    }

    // Basic ST syntax validation
    let source_upper = source.to_uppercase();

    // Check for common ST keywords
    let has_var = source_upper.contains("VAR") || source_upper.contains("PROGRAM");
    if !has_var {
        warn!("Program may not contain valid IEC 61131-3 declarations");
    }

    Ok(())
}

/// Parse Structured Text variable declarations
pub fn parse_st_variables(source: &str) -> Vec<(String, String)> {
    let mut variables = Vec::new();

    // Simple regex-free parser for VAR blocks
    let lines: Vec<&str> = source.lines().collect();
    let mut in_var_block = false;

    for line in lines {
        let trimmed = line.trim();
        let upper = trimmed.to_uppercase();

        if upper.starts_with("VAR") && !upper.starts_with("VAR_") {
            in_var_block = true;
            continue;
        }

        if upper == "END_VAR" {
            in_var_block = false;
            continue;
        }

        if in_var_block && trimmed.contains(':') {
            // Parse variable declaration: name : type [:= initial];
            if let Some(colon_pos) = trimmed.find(':') {
                let name = trimmed[..colon_pos].trim().to_string();
                let rest = &trimmed[colon_pos + 1..];

                // Extract type (before := or ;)
                let type_end = rest
                    .find(":=")
                    .or_else(|| rest.find(';'))
                    .unwrap_or(rest.len());
                let data_type = rest[..type_end].trim().to_string();

                if !name.is_empty() && !data_type.is_empty() {
                    variables.push((name, data_type));
                }
            }
        }
    }

    variables
}

/// Generate ST program header
pub fn generate_st_header(program_name: &str, author: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    format!(
        r#"(*
 * Program: {}
 * Author: {}
 * Generated: {}
 * Generator: Suderra Edge Agent v1.3.0
 *)

"#,
        program_name, author, timestamp
    )
}

/// Sanitize program name for PLC compatibility
pub fn sanitize_program_name(name: &str) -> String {
    let mut result = String::with_capacity(name.len());

    for (i, c) in name.chars().enumerate() {
        if i == 0 {
            // First character must be letter or underscore
            if c.is_ascii_alphabetic() || c == '_' {
                result.push(c);
            } else {
                result.push('_');
            }
        } else {
            // Subsequent characters: letter, digit, or underscore
            if c.is_ascii_alphanumeric() || c == '_' {
                result.push(c);
            } else {
                result.push('_');
            }
        }
    }

    // Truncate to reasonable length (most PLCs limit to 32-64 chars)
    if result.len() > 32 {
        result.truncate(32);
    }

    // Ensure not empty
    if result.is_empty() {
        result = "Program1".to_string();
    }

    result
}

/// Convert IEC 61131-3 data type to byte size
pub fn data_type_size(data_type: &str) -> usize {
    match data_type.to_uppercase().as_str() {
        "BOOL" => 1,
        "BYTE" | "SINT" | "USINT" => 1,
        "WORD" | "INT" | "UINT" => 2,
        "DWORD" | "DINT" | "UDINT" | "REAL" => 4,
        "LWORD" | "LINT" | "ULINT" | "LREAL" => 8,
        "TIME" | "DATE" | "TOD" | "DT" => 4,
        "STRING" => 256, // Default string length
        "WSTRING" => 512,
        _ => 4, // Default to DWORD size
    }
}

/// Async operation with timeout wrapper
pub async fn with_timeout<T, E, F>(
    operation: F,
    timeout_duration: Duration,
    operation_name: &str,
) -> Result<T>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    match timeout(timeout_duration, operation).await {
        Ok(result) => result.map_err(|e| anyhow!(e)),
        Err(_) => Err(anyhow!(
            "Operation '{}' timed out after {:?}",
            operation_name,
            timeout_duration
        )),
    }
}

/// Log program upload audit event
pub fn audit_program_upload(
    protocol: &str,
    plc_address: &str,
    program_name: &str,
    success: bool,
    details: &str,
) {
    if success {
        info!(
            target: "audit",
            protocol = %protocol,
            plc = %plc_address,
            program = %program_name,
            action = "program_upload",
            status = "success",
            details = %details,
            "PLC program uploaded successfully"
        );
    } else {
        warn!(
            target: "audit",
            protocol = %protocol,
            plc = %plc_address,
            program = %program_name,
            action = "program_upload",
            status = "failed",
            details = %details,
            "PLC program upload failed"
        );
    }
}

/// Connection state management
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Connected => write!(f, "Connected"),
            Self::Error => write!(f, "Error"),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_program_source() {
        assert!(validate_program_source("").is_err());
        assert!(validate_program_source("VAR x : INT; END_VAR").is_ok());
    }

    #[test]
    fn test_parse_st_variables() {
        let source = r#"
            VAR
                counter : INT := 0;
                temperature : REAL;
                active : BOOL := FALSE;
            END_VAR
        "#;

        let vars = parse_st_variables(source);
        assert_eq!(vars.len(), 3);
        assert_eq!(vars[0], ("counter".to_string(), "INT".to_string()));
        assert_eq!(vars[1], ("temperature".to_string(), "REAL".to_string()));
        assert_eq!(vars[2], ("active".to_string(), "BOOL".to_string()));
    }

    #[test]
    fn test_sanitize_program_name() {
        assert_eq!(sanitize_program_name("MyProgram"), "MyProgram");
        assert_eq!(sanitize_program_name("123Start"), "_23Start");
        assert_eq!(sanitize_program_name("Test-Program.1"), "Test_Program_1");
        assert_eq!(sanitize_program_name(""), "Program1");
    }

    #[test]
    fn test_data_type_size() {
        assert_eq!(data_type_size("BOOL"), 1);
        assert_eq!(data_type_size("INT"), 2);
        assert_eq!(data_type_size("DINT"), 4);
        assert_eq!(data_type_size("LREAL"), 8);
    }
}
