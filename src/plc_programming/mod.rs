//! PLC Programming Module
//!
//! Provides capabilities to upload IEC 61131-3 programs (Structured Text, Ladder, FBD)
//! to various PLC platforms.
//!
//! ## Supported Protocols
//!
//! | Protocol | PLCs | Port |
//! |----------|------|------|
//! | Codesys Gateway | Codesys-based PLCs | 1217 |
//! | S7comm | Siemens S7-300/400/1200/1500 | 102 |
//! | OPC UA | IEC 62541 compliant PLCs | 4840 |
//! | EtherNet/IP CIP | Allen-Bradley, Rockwell | 44818 |
//! | ADS/AMS | Beckhoff TwinCAT | 48898 |
//!
//! ## Security
//!
//! - All protocols support TLS/encryption where available
//! - Authentication required for program uploads
//! - Audit logging for all program changes
//! - IEC 62443 SL2 compliance
//!
//! ## v1.3.0 Feature

pub mod codesys;
pub mod s7comm;
pub mod opcua;
pub mod ethernet_ip;
pub mod ads;
pub mod common;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use codesys::CodesysClient;
pub use s7comm::S7Client;
pub use opcua::OpcUaClient;
pub use ethernet_ip::EtherNetIpClient;
pub use ads::AdsClient;

// ============================================================================
// Common Types
// ============================================================================

/// PLC Program representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlcProgram {
    /// Program name
    pub name: String,
    /// Program language
    pub language: ProgramLanguage,
    /// Source code
    pub source: String,
    /// Variables/tags
    #[serde(default)]
    pub variables: Vec<PlcVariable>,
    /// Function blocks
    #[serde(default)]
    pub function_blocks: Vec<PlcFunctionBlock>,
    /// Program metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// IEC 61131-3 Programming Languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramLanguage {
    /// Structured Text
    St,
    /// Ladder Diagram
    Ld,
    /// Function Block Diagram
    Fbd,
    /// Instruction List
    Il,
    /// Sequential Function Chart
    Sfc,
}

impl Default for ProgramLanguage {
    fn default() -> Self {
        Self::St
    }
}

/// PLC Variable definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlcVariable {
    /// Variable name
    pub name: String,
    /// Data type
    pub data_type: PlcDataType,
    /// Initial value
    #[serde(default)]
    pub initial_value: Option<String>,
    /// Memory address (optional)
    #[serde(default)]
    pub address: Option<String>,
    /// Variable scope
    #[serde(default)]
    pub scope: VariableScope,
    /// Retain on power loss
    #[serde(default)]
    pub retain: bool,
    /// Description
    #[serde(default)]
    pub description: String,
}

/// PLC Data Types (IEC 61131-3)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PlcDataType {
    Bool,
    Byte,
    Word,
    Dword,
    Lword,
    Sint,
    Int,
    Dint,
    Lint,
    Usint,
    Uint,
    Udint,
    Ulint,
    Real,
    Lreal,
    Time,
    Date,
    Tod,      // Time of Day
    Dt,       // Date and Time
    String,
    Wstring,
    Array(Box<PlcDataType>, usize), // Array with element type and size
    Struct(String),                  // User-defined struct name
}

impl Default for PlcDataType {
    fn default() -> Self {
        Self::Int
    }
}

/// Variable Scope
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VariableScope {
    /// Global variable
    Global,
    /// Local to program
    #[default]
    Local,
    /// Input variable
    Input,
    /// Output variable
    Output,
    /// In/Out variable
    InOut,
}

/// PLC Function Block instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlcFunctionBlock {
    /// Instance name
    pub name: String,
    /// Function block type
    pub fb_type: String,
    /// Input connections
    #[serde(default)]
    pub inputs: HashMap<String, String>,
    /// Output connections
    #[serde(default)]
    pub outputs: HashMap<String, String>,
}

/// Program upload result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResult {
    /// Upload successful
    pub success: bool,
    /// Program ID on PLC
    pub program_id: Option<String>,
    /// Compilation warnings
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Compilation errors (if failed)
    #[serde(default)]
    pub errors: Vec<String>,
    /// Upload timestamp
    pub timestamp: String,
    /// PLC response data
    #[serde(default)]
    pub plc_response: HashMap<String, serde_json::Value>,
}

/// PLC Connection Status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlcStatus {
    /// Connected to PLC
    pub connected: bool,
    /// PLC run mode
    pub run_mode: PlcRunMode,
    /// PLC model/type
    pub model: String,
    /// Firmware version
    pub firmware: String,
    /// Current program name
    pub current_program: Option<String>,
    /// Last program change
    pub last_modified: Option<String>,
}

/// PLC Run Mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum PlcRunMode {
    #[default]
    Unknown,
    Run,
    Stop,
    Program,
    Fault,
    Test,
}

// ============================================================================
// Unified PLC Programming Interface
// ============================================================================

/// Trait for PLC programming clients
#[async_trait::async_trait]
pub trait PlcProgrammer: Send + Sync {
    /// Get protocol name
    fn protocol_name(&self) -> &'static str;

    /// Connect to PLC
    async fn connect(&mut self) -> Result<()>;

    /// Disconnect from PLC
    async fn disconnect(&mut self) -> Result<()>;

    /// Check if connected
    fn is_connected(&self) -> bool;

    /// Get PLC status
    async fn get_status(&self) -> Result<PlcStatus>;

    /// Upload program to PLC
    async fn upload_program(&self, program: &PlcProgram) -> Result<UploadResult>;

    /// Download program from PLC
    async fn download_program(&self, program_name: &str) -> Result<PlcProgram>;

    /// Start PLC (RUN mode)
    async fn start(&self) -> Result<()>;

    /// Stop PLC (STOP mode)
    async fn stop(&self) -> Result<()>;

    /// Get list of programs on PLC
    async fn list_programs(&self) -> Result<Vec<String>>;

    /// Delete program from PLC
    async fn delete_program(&self, program_name: &str) -> Result<()>;

    /// Compile program (without upload)
    async fn compile(&self, program: &PlcProgram) -> Result<UploadResult>;
}

// ============================================================================
// Configuration
// ============================================================================

/// PLC Programming configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlcProgrammingConfig {
    /// Codesys connections
    #[serde(default)]
    pub codesys: Vec<codesys::CodesysConfig>,

    /// Siemens S7 connections
    #[serde(default)]
    pub s7: Vec<s7comm::S7Config>,

    /// OPC UA connections
    #[serde(default)]
    pub opcua: Vec<opcua::OpcUaConfig>,

    /// Allen-Bradley connections
    #[serde(default)]
    pub ethernet_ip: Vec<ethernet_ip::EtherNetIpConfig>,

    /// Beckhoff ADS connections
    #[serde(default)]
    pub ads: Vec<ads::AdsConfig>,
}

impl Default for PlcProgrammingConfig {
    fn default() -> Self {
        Self {
            codesys: Vec::new(),
            s7: Vec::new(),
            opcua: Vec::new(),
            ethernet_ip: Vec::new(),
            ads: Vec::new(),
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
    fn test_program_language_default() {
        assert_eq!(ProgramLanguage::default(), ProgramLanguage::St);
    }

    #[test]
    fn test_plc_program_serialization() {
        let program = PlcProgram {
            name: "TestProgram".to_string(),
            language: ProgramLanguage::St,
            source: r#"
                VAR
                    counter : INT := 0;
                END_VAR

                counter := counter + 1;
            "#.to_string(),
            variables: vec![
                PlcVariable {
                    name: "counter".to_string(),
                    data_type: PlcDataType::Int,
                    initial_value: Some("0".to_string()),
                    address: None,
                    scope: VariableScope::Local,
                    retain: false,
                    description: "Test counter".to_string(),
                }
            ],
            function_blocks: vec![],
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string_pretty(&program).unwrap();
        assert!(json.contains("TestProgram"));
        assert!(json.contains("counter"));
    }

    #[test]
    fn test_data_type_array() {
        let arr_type = PlcDataType::Array(Box::new(PlcDataType::Int), 10);
        let json = serde_json::to_string(&arr_type).unwrap();
        assert!(json.contains("INT"));
    }
}
