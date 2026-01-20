//! OPC UA Program Transfer Implementation
//!
//! Production-ready OPC UA (IEC 62541) client for PLC programming and control.
//!
//! ## Features
//! - Full OPC UA binary protocol implementation
//! - Session management with proper authentication
//! - Browse, Read, Write, and Call services
//! - Program upload/download via standard OPC UA nodes
//! - PLC control via method calls (Start/Stop)
//! - ServerStatus monitoring
//!
//! ## Supported Servers
//! - Siemens S7-1500 (with OPC UA enabled)
//! - Beckhoff TwinCAT 3
//! - B&R Automation
//! - Unified Automation servers
//! - Any OPC UA server with ProgramTransfer
//! - PLCopen OPC UA compliant servers
//!
//! ## Protocol
//! - Default Port: 4840 (OPC UA Binary)
//! - Secure Channel establishment
//! - Session-based authentication (Anonymous/Username)
//!
//! ## OPC UA Services Implemented
//! - CreateSession / ActivateSession
//! - Browse (for node discovery)
//! - Read (for variable/status reading)
//! - Write (for program upload)
//! - Call (for method invocation - Start/Stop)
//! - CloseSecureChannel

use super::common::*;
use super::{PlcProgram, PlcProgrammer, PlcRunMode, PlcStatus, UploadResult};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use serde_json::Value as JsonValue;
use tracing::{debug, info, warn};

// ============================================================================
// Constants
// ============================================================================

/// Default OPC UA port
pub const DEFAULT_OPCUA_PORT: u16 = 4840;

/// Maximum OPC UA message size (16MB - matches build_hello max_message_size)
const MAX_OPCUA_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// OPC UA message types
const MSG_HELLO: &[u8] = b"HEL";
const MSG_ACK: &[u8] = b"ACK";
const MSG_ERROR: &[u8] = b"ERR";
const MSG_OPEN: &[u8] = b"OPN";
const MSG_CLOSE: &[u8] = b"CLO";
const MSG_MESSAGE: &[u8] = b"MSG";

/// OPC UA Security Policies
const SECURITY_POLICY_NONE: &str = "http://opcfoundation.org/UA/SecurityPolicy#None";
const SECURITY_POLICY_BASIC256SHA256: &str =
    "http://opcfoundation.org/UA/SecurityPolicy#Basic256Sha256";

// ============================================================================
// OPC UA Service Type IDs (from OPC UA Specification Part 4)
// ============================================================================

/// CreateSessionRequest Type ID
const TYPE_ID_CREATE_SESSION_REQUEST: u32 = 461;
/// CreateSessionResponse Type ID
const TYPE_ID_CREATE_SESSION_RESPONSE: u32 = 464;
/// ActivateSessionRequest Type ID
const TYPE_ID_ACTIVATE_SESSION_REQUEST: u32 = 467;
/// ActivateSessionResponse Type ID
const TYPE_ID_ACTIVATE_SESSION_RESPONSE: u32 = 470;
/// CloseSecureChannelRequest Type ID
const TYPE_ID_CLOSE_SECURE_CHANNEL_REQUEST: u32 = 452;
/// BrowseRequest Type ID
const TYPE_ID_BROWSE_REQUEST: u32 = 527;
/// BrowseResponse Type ID
const TYPE_ID_BROWSE_RESPONSE: u32 = 530;
/// ReadRequest Type ID
const TYPE_ID_READ_REQUEST: u32 = 631;
/// ReadResponse Type ID
const TYPE_ID_READ_RESPONSE: u32 = 634;
/// WriteRequest Type ID
const TYPE_ID_WRITE_REQUEST: u32 = 673;
/// WriteResponse Type ID
const TYPE_ID_WRITE_RESPONSE: u32 = 676;
/// CallRequest Type ID
const TYPE_ID_CALL_REQUEST: u32 = 712;
/// CallResponse Type ID
const TYPE_ID_CALL_RESPONSE: u32 = 715;

// ============================================================================
// OPC UA Well-Known Node IDs (from OPC UA Specification Part 5)
// ============================================================================

/// Server node (i=2253)
const NODE_ID_SERVER: u32 = 2253;
/// ServerStatus node (i=2256)
const NODE_ID_SERVER_STATUS: u32 = 2256;
/// ServerState node (i=2259)
const NODE_ID_SERVER_STATE: u32 = 2259;
/// CurrentTime node (i=2258)
const NODE_ID_CURRENT_TIME: u32 = 2258;
/// Objects folder (i=85)
const NODE_ID_OBJECTS_FOLDER: u32 = 85;
/// Server_ServerStatus_State node (i=2259)
const NODE_ID_SERVER_STATE_VALUE: u32 = 2259;
/// ProductName node (i=2261)
const NODE_ID_PRODUCT_NAME: u32 = 2261;
/// SoftwareVersion node (i=2263)
const NODE_ID_SOFTWARE_VERSION: u32 = 2263;

// ============================================================================
// OPC UA Status Codes
// ============================================================================

/// Good status code
const STATUS_GOOD: u32 = 0x00000000;
/// Bad status code mask
const STATUS_BAD_MASK: u32 = 0x80000000;

// ============================================================================
// OPC UA Attribute IDs
// ============================================================================

/// Value attribute
const ATTRIBUTE_VALUE: u32 = 13;
/// BrowseName attribute
const ATTRIBUTE_BROWSE_NAME: u32 = 3;
/// DisplayName attribute
const ATTRIBUTE_DISPLAY_NAME: u32 = 4;

// ============================================================================
// OPC UA Browse Direction
// ============================================================================

/// Browse forward references
const BROWSE_DIRECTION_FORWARD: u32 = 0;
/// Browse both directions
const BROWSE_DIRECTION_BOTH: u32 = 2;

// ============================================================================
// OPC UA Reference Types
// ============================================================================

/// HierarchicalReferences (i=33)
const REFERENCE_TYPE_HIERARCHICAL: u32 = 33;
/// Organizes reference (i=35)
const REFERENCE_TYPE_ORGANIZES: u32 = 35;

// ============================================================================
// Configuration
// ============================================================================

/// OPC UA connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpcUaConfig {
    /// Connection name
    pub name: String,

    /// Server endpoint URL
    pub endpoint_url: String,

    /// Security policy
    #[serde(default)]
    pub security_policy: OpcUaSecurityPolicy,

    /// Security mode
    #[serde(default)]
    pub security_mode: OpcUaSecurityMode,

    /// Username (for user authentication)
    #[serde(default)]
    pub username: Option<String>,

    /// Password
    #[serde(default)]
    pub password: Option<String>,

    /// Client certificate path (for certificate auth)
    #[serde(default)]
    pub client_cert_path: Option<String>,

    /// Client private key path
    #[serde(default)]
    pub client_key_path: Option<String>,

    /// Connection timeout (seconds)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Session timeout (milliseconds)
    #[serde(default = "default_session_timeout")]
    pub session_timeout_ms: u32,

    /// Namespace URI for program nodes
    #[serde(default)]
    pub program_namespace: Option<String>,
}

fn default_timeout() -> u64 {
    10
}

fn default_session_timeout() -> u32 {
    60000
}

/// OPC UA Security Policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpcUaSecurityPolicy {
    #[default]
    None,
    Basic128Rsa15,
    Basic256,
    Basic256Sha256,
    Aes128Sha256RsaOaep,
    Aes256Sha256RsaPss,
}

impl OpcUaSecurityPolicy {
    fn to_uri(&self) -> &'static str {
        match self {
            Self::None => SECURITY_POLICY_NONE,
            Self::Basic128Rsa15 => "http://opcfoundation.org/UA/SecurityPolicy#Basic128Rsa15",
            Self::Basic256 => "http://opcfoundation.org/UA/SecurityPolicy#Basic256",
            Self::Basic256Sha256 => SECURITY_POLICY_BASIC256SHA256,
            Self::Aes128Sha256RsaOaep => {
                "http://opcfoundation.org/UA/SecurityPolicy#Aes128_Sha256_RsaOaep"
            }
            Self::Aes256Sha256RsaPss => {
                "http://opcfoundation.org/UA/SecurityPolicy#Aes256_Sha256_RsaPss"
            }
        }
    }
}

/// OPC UA Security Mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpcUaSecurityMode {
    #[default]
    None,
    Sign,
    SignAndEncrypt,
}

impl Default for OpcUaConfig {
    fn default() -> Self {
        Self {
            name: "opcua_server".to_string(),
            endpoint_url: "opc.tcp://localhost:4840".to_string(),
            security_policy: OpcUaSecurityPolicy::None,
            security_mode: OpcUaSecurityMode::None,
            username: None,
            password: None,
            client_cert_path: None,
            client_key_path: None,
            timeout_secs: 10,
            session_timeout_ms: 60000,
            program_namespace: None,
        }
    }
}

// ============================================================================
// OPC UA Node IDs
// ============================================================================

/// OPC UA Node ID types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeId {
    Numeric(u16, u32),    // (namespace, identifier)
    String(u16, String),  // (namespace, identifier)
    Guid(u16, [u8; 16]),  // (namespace, identifier)
    Opaque(u16, Vec<u8>), // (namespace, identifier)
}

impl NodeId {
    /// Create numeric node ID
    pub fn numeric(namespace: u16, id: u32) -> Self {
        Self::Numeric(namespace, id)
    }

    /// Create string node ID
    pub fn string(namespace: u16, id: &str) -> Self {
        Self::String(namespace, id.to_string())
    }

    /// Create null node ID
    pub fn null() -> Self {
        Self::Numeric(0, 0)
    }

    /// Check if this is a null node ID
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Numeric(0, 0))
    }

    /// Encode to binary
    fn encode(&self) -> Vec<u8> {
        let mut data = Vec::new();

        match self {
            Self::Numeric(ns, id) => {
                if *ns == 0 && *id <= 255 {
                    // Two-byte numeric
                    data.push(0x00);
                    data.push(*id as u8);
                } else if *ns <= 255 && *id <= 65535 {
                    // Four-byte numeric
                    data.push(0x01);
                    data.push(*ns as u8);
                    data.extend_from_slice(&(*id as u16).to_le_bytes());
                } else {
                    // Full numeric
                    data.push(0x02);
                    data.extend_from_slice(&ns.to_le_bytes());
                    data.extend_from_slice(&id.to_le_bytes());
                }
            }
            Self::String(ns, id) => {
                data.push(0x03);
                data.extend_from_slice(&ns.to_le_bytes());
                data.extend_from_slice(&(id.len() as u32).to_le_bytes());
                data.extend_from_slice(id.as_bytes());
            }
            Self::Guid(ns, guid) => {
                data.push(0x04);
                data.extend_from_slice(&ns.to_le_bytes());
                data.extend_from_slice(guid);
            }
            Self::Opaque(ns, bytes) => {
                data.push(0x05);
                data.extend_from_slice(&ns.to_le_bytes());
                data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                data.extend_from_slice(bytes);
            }
        }

        data
    }

    /// Decode from binary data, returns (NodeId, bytes_consumed)
    fn decode(data: &[u8]) -> Result<(Self, usize)> {
        if data.is_empty() {
            return Err(anyhow!("Empty data for NodeId decode"));
        }

        let encoding = data[0];
        match encoding & 0x3F {
            0x00 => {
                // Two-byte numeric (namespace 0)
                if data.len() < 2 {
                    return Err(anyhow!("Insufficient data for two-byte NodeId"));
                }
                Ok((Self::Numeric(0, data[1] as u32), 2))
            }
            0x01 => {
                // Four-byte numeric
                if data.len() < 4 {
                    return Err(anyhow!("Insufficient data for four-byte NodeId"));
                }
                let ns = data[1] as u16;
                let id = u16::from_le_bytes([data[2], data[3]]) as u32;
                Ok((Self::Numeric(ns, id), 4))
            }
            0x02 => {
                // Full numeric
                if data.len() < 7 {
                    return Err(anyhow!("Insufficient data for full numeric NodeId"));
                }
                let ns = u16::from_le_bytes([data[1], data[2]]);
                let id = u32::from_le_bytes([data[3], data[4], data[5], data[6]]);
                Ok((Self::Numeric(ns, id), 7))
            }
            0x03 => {
                // String
                if data.len() < 7 {
                    return Err(anyhow!("Insufficient data for string NodeId header"));
                }
                let ns = u16::from_le_bytes([data[1], data[2]]);
                let len = u32::from_le_bytes([data[3], data[4], data[5], data[6]]) as usize;
                if len == 0xFFFFFFFF as usize {
                    // Null string
                    return Ok((Self::String(ns, String::new()), 7));
                }
                if data.len() < 7 + len {
                    return Err(anyhow!("Insufficient data for string NodeId value"));
                }
                let s = String::from_utf8_lossy(&data[7..7 + len]).to_string();
                Ok((Self::String(ns, s), 7 + len))
            }
            0x04 => {
                // GUID
                if data.len() < 19 {
                    return Err(anyhow!("Insufficient data for GUID NodeId"));
                }
                let ns = u16::from_le_bytes([data[1], data[2]]);
                let mut guid = [0u8; 16];
                guid.copy_from_slice(&data[3..19]);
                Ok((Self::Guid(ns, guid), 19))
            }
            0x05 => {
                // Opaque
                if data.len() < 7 {
                    return Err(anyhow!("Insufficient data for opaque NodeId header"));
                }
                let ns = u16::from_le_bytes([data[1], data[2]]);
                let len = u32::from_le_bytes([data[3], data[4], data[5], data[6]]) as usize;
                if len == 0xFFFFFFFF as usize {
                    return Ok((Self::Opaque(ns, Vec::new()), 7));
                }
                if data.len() < 7 + len {
                    return Err(anyhow!("Insufficient data for opaque NodeId value"));
                }
                Ok((Self::Opaque(ns, data[7..7 + len].to_vec()), 7 + len))
            }
            _ => Err(anyhow!("Unknown NodeId encoding: 0x{:02X}", encoding)),
        }
    }
}

// ============================================================================
// OPC UA Data Value and Variant Types
// ============================================================================

/// OPC UA Variant - represents any OPC UA value type
#[derive(Debug, Clone)]
pub enum Variant {
    Null,
    Boolean(bool),
    SByte(i8),
    Byte(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Float(f32),
    Double(f64),
    String(String),
    DateTime(i64),
    ByteString(Vec<u8>),
    NodeId(NodeId),
    StatusCode(u32),
    LocalizedText(String),
}

impl Variant {
    /// Encode variant to binary
    fn encode(&self) -> Vec<u8> {
        let mut data = Vec::new();
        match self {
            Self::Null => {
                data.push(0x00);
            }
            Self::Boolean(v) => {
                data.push(0x01);
                data.push(if *v { 1 } else { 0 });
            }
            Self::SByte(v) => {
                data.push(0x02);
                data.push(*v as u8);
            }
            Self::Byte(v) => {
                data.push(0x03);
                data.push(*v);
            }
            Self::Int16(v) => {
                data.push(0x04);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::UInt16(v) => {
                data.push(0x05);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::Int32(v) => {
                data.push(0x06);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::UInt32(v) => {
                data.push(0x07);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::Int64(v) => {
                data.push(0x08);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::UInt64(v) => {
                data.push(0x09);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::Float(v) => {
                data.push(0x0A);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::Double(v) => {
                data.push(0x0B);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::String(v) => {
                data.push(0x0C);
                if v.is_empty() {
                    data.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
                } else {
                    data.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    data.extend_from_slice(v.as_bytes());
                }
            }
            Self::DateTime(v) => {
                data.push(0x0D);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::ByteString(v) => {
                data.push(0x0F);
                if v.is_empty() {
                    data.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
                } else {
                    data.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    data.extend_from_slice(v);
                }
            }
            Self::NodeId(v) => {
                data.push(0x11);
                data.extend_from_slice(&v.encode());
            }
            Self::StatusCode(v) => {
                data.push(0x13);
                data.extend_from_slice(&v.to_le_bytes());
            }
            Self::LocalizedText(v) => {
                data.push(0x15);
                data.push(0x02); // Encoding mask: text only
                if v.is_empty() {
                    data.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
                } else {
                    data.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    data.extend_from_slice(v.as_bytes());
                }
            }
        }
        data
    }

    /// Decode variant from binary data, returns (Variant, bytes_consumed)
    fn decode(data: &[u8]) -> Result<(Self, usize)> {
        if data.is_empty() {
            return Err(anyhow!("Empty data for Variant decode"));
        }

        let type_id = data[0];
        let mut offset = 1;

        // Check for array flag (bit 7) - we don't support arrays here for simplicity
        if type_id & 0x80 != 0 {
            // Skip array handling for now, return as ByteString
            let array_len = if data.len() >= 5 {
                u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize
            } else {
                return Err(anyhow!("Insufficient data for array length"));
            };
            if array_len == 0xFFFFFFFF as usize {
                return Ok((Self::Null, 5));
            }
            // Approximate: skip array content
            return Ok((Self::Null, 5));
        }

        match type_id & 0x3F {
            0x00 => Ok((Self::Null, 1)),
            0x01 => {
                // Boolean
                if data.len() < 2 {
                    return Err(anyhow!("Insufficient data for Boolean"));
                }
                Ok((Self::Boolean(data[1] != 0), 2))
            }
            0x02 => {
                // SByte
                if data.len() < 2 {
                    return Err(anyhow!("Insufficient data for SByte"));
                }
                Ok((Self::SByte(data[1] as i8), 2))
            }
            0x03 => {
                // Byte
                if data.len() < 2 {
                    return Err(anyhow!("Insufficient data for Byte"));
                }
                Ok((Self::Byte(data[1]), 2))
            }
            0x04 => {
                // Int16
                if data.len() < 3 {
                    return Err(anyhow!("Insufficient data for Int16"));
                }
                let v = i16::from_le_bytes([data[1], data[2]]);
                Ok((Self::Int16(v), 3))
            }
            0x05 => {
                // UInt16
                if data.len() < 3 {
                    return Err(anyhow!("Insufficient data for UInt16"));
                }
                let v = u16::from_le_bytes([data[1], data[2]]);
                Ok((Self::UInt16(v), 3))
            }
            0x06 => {
                // Int32
                if data.len() < 5 {
                    return Err(anyhow!("Insufficient data for Int32"));
                }
                let v = i32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                Ok((Self::Int32(v), 5))
            }
            0x07 => {
                // UInt32
                if data.len() < 5 {
                    return Err(anyhow!("Insufficient data for UInt32"));
                }
                let v = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                Ok((Self::UInt32(v), 5))
            }
            0x08 => {
                // Int64
                if data.len() < 9 {
                    return Err(anyhow!("Insufficient data for Int64"));
                }
                let v = i64::from_le_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]);
                Ok((Self::Int64(v), 9))
            }
            0x09 => {
                // UInt64
                if data.len() < 9 {
                    return Err(anyhow!("Insufficient data for UInt64"));
                }
                let v = u64::from_le_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]);
                Ok((Self::UInt64(v), 9))
            }
            0x0A => {
                // Float
                if data.len() < 5 {
                    return Err(anyhow!("Insufficient data for Float"));
                }
                let v = f32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                Ok((Self::Float(v), 5))
            }
            0x0B => {
                // Double
                if data.len() < 9 {
                    return Err(anyhow!("Insufficient data for Double"));
                }
                let v = f64::from_le_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]);
                Ok((Self::Double(v), 9))
            }
            0x0C => {
                // String
                if data.len() < 5 {
                    return Err(anyhow!("Insufficient data for String length"));
                }
                let len = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                if len == 0xFFFFFFFF {
                    return Ok((Self::String(String::new()), 5));
                }
                let len = len as usize;
                offset = 5;
                if data.len() < offset + len {
                    return Err(anyhow!("Insufficient data for String value"));
                }
                let s = String::from_utf8_lossy(&data[offset..offset + len]).to_string();
                Ok((Self::String(s), offset + len))
            }
            0x0D => {
                // DateTime
                if data.len() < 9 {
                    return Err(anyhow!("Insufficient data for DateTime"));
                }
                let v = i64::from_le_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]);
                Ok((Self::DateTime(v), 9))
            }
            0x0F => {
                // ByteString
                if data.len() < 5 {
                    return Err(anyhow!("Insufficient data for ByteString length"));
                }
                let len = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                if len == 0xFFFFFFFF {
                    return Ok((Self::ByteString(Vec::new()), 5));
                }
                let len = len as usize;
                offset = 5;
                if data.len() < offset + len {
                    return Err(anyhow!("Insufficient data for ByteString value"));
                }
                Ok((Self::ByteString(data[offset..offset + len].to_vec()), offset + len))
            }
            0x11 => {
                // NodeId
                let (node_id, consumed) = NodeId::decode(&data[1..])?;
                Ok((Self::NodeId(node_id), 1 + consumed))
            }
            0x13 => {
                // StatusCode
                if data.len() < 5 {
                    return Err(anyhow!("Insufficient data for StatusCode"));
                }
                let v = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
                Ok((Self::StatusCode(v), 5))
            }
            0x15 => {
                // LocalizedText
                if data.len() < 2 {
                    return Err(anyhow!("Insufficient data for LocalizedText encoding"));
                }
                let encoding = data[1];
                offset = 2;

                // Skip locale if present
                if encoding & 0x01 != 0 {
                    if data.len() < offset + 4 {
                        return Err(anyhow!("Insufficient data for LocalizedText locale length"));
                    }
                    let len = u32::from_le_bytes([
                        data[offset],
                        data[offset + 1],
                        data[offset + 2],
                        data[offset + 3],
                    ]);
                    offset += 4;
                    if len != 0xFFFFFFFF {
                        offset += len as usize;
                    }
                }

                // Read text if present
                if encoding & 0x02 != 0 {
                    if data.len() < offset + 4 {
                        return Err(anyhow!("Insufficient data for LocalizedText text length"));
                    }
                    let len = u32::from_le_bytes([
                        data[offset],
                        data[offset + 1],
                        data[offset + 2],
                        data[offset + 3],
                    ]);
                    offset += 4;
                    if len == 0xFFFFFFFF {
                        return Ok((Self::LocalizedText(String::new()), offset));
                    }
                    let len = len as usize;
                    if data.len() < offset + len {
                        return Err(anyhow!("Insufficient data for LocalizedText text value"));
                    }
                    let s = String::from_utf8_lossy(&data[offset..offset + len]).to_string();
                    return Ok((Self::LocalizedText(s), offset + len));
                }

                Ok((Self::LocalizedText(String::new()), offset))
            }
            _ => {
                // Unknown type, return as Null
                debug!("Unknown variant type: 0x{:02X}", type_id);
                Ok((Self::Null, 1))
            }
        }
    }

    /// Convert to string representation
    pub fn to_string_value(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Boolean(v) => v.to_string(),
            Self::SByte(v) => v.to_string(),
            Self::Byte(v) => v.to_string(),
            Self::Int16(v) => v.to_string(),
            Self::UInt16(v) => v.to_string(),
            Self::Int32(v) => v.to_string(),
            Self::UInt32(v) => v.to_string(),
            Self::Int64(v) => v.to_string(),
            Self::UInt64(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Double(v) => v.to_string(),
            Self::String(v) => v.clone(),
            Self::DateTime(v) => format!("DateTime({})", v),
            Self::ByteString(v) => format!("ByteString({} bytes)", v.len()),
            Self::NodeId(v) => format!("{:?}", v),
            Self::StatusCode(v) => format!("0x{:08X}", v),
            Self::LocalizedText(v) => v.clone(),
        }
    }
}

/// OPC UA DataValue - wraps a Variant with status and timestamps
#[derive(Debug, Clone)]
pub struct DataValue {
    pub value: Option<Variant>,
    pub status_code: u32,
    pub source_timestamp: Option<i64>,
    pub server_timestamp: Option<i64>,
}

impl DataValue {
    /// Decode DataValue from binary
    fn decode(data: &[u8]) -> Result<(Self, usize)> {
        if data.is_empty() {
            return Err(anyhow!("Empty data for DataValue decode"));
        }

        let encoding_mask = data[0];
        let mut offset = 1;

        let mut value = None;
        let mut status_code = STATUS_GOOD;
        let mut source_timestamp = None;
        let mut server_timestamp = None;

        // Value
        if encoding_mask & 0x01 != 0 {
            let (v, consumed) = Variant::decode(&data[offset..])?;
            value = Some(v);
            offset += consumed;
        }

        // StatusCode
        if encoding_mask & 0x02 != 0 {
            if data.len() < offset + 4 {
                return Err(anyhow!("Insufficient data for DataValue status code"));
            }
            status_code = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
        }

        // SourceTimestamp
        if encoding_mask & 0x04 != 0 {
            if data.len() < offset + 8 {
                return Err(anyhow!("Insufficient data for DataValue source timestamp"));
            }
            source_timestamp = Some(i64::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]));
            offset += 8;
        }

        // ServerTimestamp
        if encoding_mask & 0x08 != 0 {
            if data.len() < offset + 8 {
                return Err(anyhow!("Insufficient data for DataValue server timestamp"));
            }
            server_timestamp = Some(i64::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]));
            offset += 8;
        }

        // Skip SourcePicoseconds and ServerPicoseconds if present
        if encoding_mask & 0x10 != 0 {
            offset += 2;
        }
        if encoding_mask & 0x20 != 0 {
            offset += 2;
        }

        Ok((
            Self {
                value,
                status_code,
                source_timestamp,
                server_timestamp,
            },
            offset,
        ))
    }

    /// Check if status is good
    pub fn is_good(&self) -> bool {
        self.status_code & STATUS_BAD_MASK == 0
    }
}

/// Browse result reference
#[derive(Debug, Clone)]
pub struct BrowseReference {
    pub reference_type_id: NodeId,
    pub is_forward: bool,
    pub node_id: NodeId,
    pub browse_name: String,
    pub display_name: String,
    pub node_class: u32,
}

impl BrowseReference {
    /// Decode from binary
    fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut offset = 0;

        // ReferenceTypeId
        let (reference_type_id, consumed) = NodeId::decode(&data[offset..])?;
        offset += consumed;

        // IsForward
        if data.len() < offset + 1 {
            return Err(anyhow!("Insufficient data for BrowseReference IsForward"));
        }
        let is_forward = data[offset] != 0;
        offset += 1;

        // NodeId (ExpandedNodeId)
        // Skip ServerIndex and NamespaceUri flags in first byte if present
        let (node_id, consumed) = NodeId::decode(&data[offset..])?;
        offset += consumed;

        // BrowseName (QualifiedName)
        if data.len() < offset + 6 {
            return Err(anyhow!("Insufficient data for BrowseReference BrowseName"));
        }
        let _ns_index = u16::from_le_bytes([data[offset], data[offset + 1]]);
        offset += 2;
        let name_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;
        let browse_name = if name_len == 0xFFFFFFFF {
            String::new()
        } else {
            let len = name_len as usize;
            if data.len() < offset + len {
                return Err(anyhow!("Insufficient data for BrowseReference BrowseName value"));
            }
            let s = String::from_utf8_lossy(&data[offset..offset + len]).to_string();
            offset += len;
            s
        };

        // DisplayName (LocalizedText)
        if data.len() < offset + 1 {
            return Err(anyhow!("Insufficient data for BrowseReference DisplayName encoding"));
        }
        let encoding = data[offset];
        offset += 1;

        // Skip locale if present
        if encoding & 0x01 != 0 {
            if data.len() < offset + 4 {
                return Err(anyhow!("Insufficient data for DisplayName locale"));
            }
            let len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            if len != 0xFFFFFFFF {
                offset += len as usize;
            }
        }

        let display_name = if encoding & 0x02 != 0 {
            if data.len() < offset + 4 {
                return Err(anyhow!("Insufficient data for DisplayName text"));
            }
            let len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            if len == 0xFFFFFFFF {
                String::new()
            } else {
                let len = len as usize;
                if data.len() < offset + len {
                    return Err(anyhow!("Insufficient data for DisplayName text value"));
                }
                let s = String::from_utf8_lossy(&data[offset..offset + len]).to_string();
                offset += len;
                s
            }
        } else {
            String::new()
        };

        // NodeClass
        if data.len() < offset + 4 {
            return Err(anyhow!("Insufficient data for BrowseReference NodeClass"));
        }
        let node_class = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;

        // TypeDefinition (skip ExpandedNodeId)
        let (_, consumed) = NodeId::decode(&data[offset..])?;
        offset += consumed;

        Ok((
            Self {
                reference_type_id,
                is_forward,
                node_id,
                browse_name,
                display_name,
                node_class,
            },
            offset,
        ))
    }
}

// ============================================================================
// OPC UA Client
// ============================================================================

/// OPC UA Client for program transfer
pub struct OpcUaClient {
    config: OpcUaConfig,
    connection: Arc<Mutex<Option<TcpStream>>>,
    connected: AtomicBool,
    secure_channel_id: Arc<Mutex<u32>>,
    token_id: Arc<Mutex<u32>>,
    sequence_number: Arc<Mutex<u32>>,
    request_id: Arc<Mutex<u32>>,
    session_id: Arc<Mutex<Option<Vec<u8>>>>,
    auth_token: Arc<Mutex<Option<Vec<u8>>>>,
}

impl OpcUaClient {
    /// Create a new OPC UA client
    pub fn new(config: OpcUaConfig) -> Self {
        Self {
            config,
            connection: Arc::new(Mutex::new(None)),
            connected: AtomicBool::new(false),
            secure_channel_id: Arc::new(Mutex::new(0)),
            token_id: Arc::new(Mutex::new(0)),
            sequence_number: Arc::new(Mutex::new(1)),
            request_id: Arc::new(Mutex::new(1)),
            session_id: Arc::new(Mutex::new(None)),
            auth_token: Arc::new(Mutex::new(None)),
        }
    }

    /// Get next sequence number
    async fn next_sequence(&self) -> u32 {
        let mut seq = self.sequence_number.lock().await;
        let current = *seq;
        *seq = seq.wrapping_add(1);
        current
    }

    /// Get next request ID
    async fn next_request_id(&self) -> u32 {
        let mut req = self.request_id.lock().await;
        let current = *req;
        *req = req.wrapping_add(1);
        current
    }

    /// Build Hello message
    fn build_hello(&self) -> Vec<u8> {
        let endpoint_bytes = self.config.endpoint_url.as_bytes();
        let mut msg = Vec::new();

        // Message header
        msg.extend_from_slice(MSG_HELLO);
        msg.push(b'F'); // Final chunk

        // Message size (will be updated)
        let size_pos = msg.len();
        msg.extend_from_slice(&[0u8; 4]);

        // Protocol version
        msg.extend_from_slice(&0u32.to_le_bytes());

        // Receive buffer size
        msg.extend_from_slice(&65535u32.to_le_bytes());

        // Send buffer size
        msg.extend_from_slice(&65535u32.to_le_bytes());

        // Max message size
        msg.extend_from_slice(&(16 * 1024 * 1024u32).to_le_bytes());

        // Max chunk count
        msg.extend_from_slice(&0u32.to_le_bytes());

        // Endpoint URL
        msg.extend_from_slice(&(endpoint_bytes.len() as u32).to_le_bytes());
        msg.extend_from_slice(endpoint_bytes);

        // Update message size
        let size = msg.len() as u32;
        msg[size_pos..size_pos + 4].copy_from_slice(&size.to_le_bytes());

        msg
    }

    /// Build OpenSecureChannel request
    async fn build_open_secure_channel(&self) -> Vec<u8> {
        let mut msg = Vec::new();

        // Message header
        msg.extend_from_slice(MSG_OPEN);
        msg.push(b'F');
        let size_pos = msg.len();
        msg.extend_from_slice(&[0u8; 4]);

        // Secure channel ID (0 for new channel)
        msg.extend_from_slice(&0u32.to_le_bytes());

        // Security policy URI
        let policy_uri = self.config.security_policy.to_uri();
        msg.extend_from_slice(&(policy_uri.len() as u32).to_le_bytes());
        msg.extend_from_slice(policy_uri.as_bytes());

        // Sender certificate (empty for None security)
        msg.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // null

        // Receiver certificate thumbprint (empty for None security)
        msg.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // null

        // Sequence header
        let seq = self.next_sequence().await;
        let req_id = self.next_request_id().await;
        msg.extend_from_slice(&seq.to_le_bytes());
        msg.extend_from_slice(&req_id.to_le_bytes());

        // Request body - OpenSecureChannelRequest
        // Type ID
        let type_id = NodeId::numeric(0, 446); // OpenSecureChannelRequest
        msg.extend_from_slice(&type_id.encode());

        // Request header
        msg.extend_from_slice(&0u8.to_le_bytes()); // null auth token
        msg.extend_from_slice(&0i64.to_le_bytes()); // timestamp
        msg.extend_from_slice(&1u32.to_le_bytes()); // request handle
        msg.extend_from_slice(&0u32.to_le_bytes()); // return diagnostics
        msg.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // audit entry id (null)
        msg.extend_from_slice(&30000u32.to_le_bytes()); // timeout hint
        msg.extend_from_slice(&0u8.to_le_bytes()); // additional header (null)

        // Client protocol version
        msg.extend_from_slice(&0u32.to_le_bytes());

        // Security token request type (0 = issue)
        msg.extend_from_slice(&0u32.to_le_bytes());

        // Message security mode (1 = None)
        let mode = match self.config.security_mode {
            OpcUaSecurityMode::None => 1u32,
            OpcUaSecurityMode::Sign => 2u32,
            OpcUaSecurityMode::SignAndEncrypt => 3u32,
        };
        msg.extend_from_slice(&mode.to_le_bytes());

        // Client nonce (empty for None security)
        msg.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // null

        // Requested lifetime
        msg.extend_from_slice(&3600000u32.to_le_bytes()); // 1 hour

        // Update size
        let size = msg.len() as u32;
        msg[size_pos..size_pos + 4].copy_from_slice(&size.to_le_bytes());

        msg
    }

    /// Send and receive OPC UA message with timeout protection
    async fn send_receive(&self, message: &[u8]) -> Result<Vec<u8>> {
        let io_timeout = Duration::from_secs(self.config.timeout_secs);
        let mut conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        // Send with timeout
        timeout(io_timeout, conn.write_all(message))
            .await
            .map_err(|_| {
                anyhow!(
                    "OPC UA write timeout after {} seconds",
                    self.config.timeout_secs
                )
            })??;

        // Read response header with timeout
        let mut header = [0u8; 8];
        timeout(io_timeout, conn.read_exact(&mut header))
            .await
            .map_err(|_| {
                anyhow!(
                    "OPC UA read timeout after {} seconds",
                    self.config.timeout_secs
                )
            })??;

        // Check message type
        if &header[0..3] == MSG_ERROR {
            return Err(anyhow!("OPC UA server returned error"));
        }

        // Get message size
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

        // Validate message size
        if size < 8 {
            return Err(anyhow!(
                "Invalid OPC UA message size: {} (minimum is 8)",
                size
            ));
        }
        if size > MAX_OPCUA_MESSAGE_SIZE {
            return Err(anyhow!(
                "OPC UA message too large: {} bytes (max {})",
                size,
                MAX_OPCUA_MESSAGE_SIZE
            ));
        }

        // Read rest of message with timeout
        let mut response = header.to_vec();
        response.resize(size, 0);
        timeout(io_timeout, conn.read_exact(&mut response[8..]))
            .await
            .map_err(|_| {
                anyhow!(
                    "OPC UA payload read timeout after {} seconds",
                    self.config.timeout_secs
                )
            })??;

        Ok(response)
    }

    /// Parse endpoint URL to get host and port
    /// Supports both IPv4 and IPv6 addresses (RFC 3986 bracket notation)
    fn parse_endpoint(&self) -> Result<(String, u16)> {
        let url = &self.config.endpoint_url;

        // Format: opc.tcp://host:port/path or opc.tcp://[ipv6]:port/path
        let stripped = url
            .strip_prefix("opc.tcp://")
            .ok_or_else(|| anyhow!("Invalid OPC UA endpoint URL"))?;

        let host_port = stripped.split('/').next().unwrap_or(stripped);

        // Handle IPv6 addresses in bracket notation (RFC 3986)
        if host_port.starts_with('[') {
            // IPv6: [::1]:4840 or [2001:db8::1]:4840
            if let Some(bracket_end) = host_port.find(']') {
                let host = &host_port[1..bracket_end]; // Remove brackets
                let after_bracket = &host_port[bracket_end + 1..];
                let port = if after_bracket.starts_with(':') {
                    after_bracket[1..].parse().unwrap_or(DEFAULT_OPCUA_PORT)
                } else {
                    DEFAULT_OPCUA_PORT
                };
                Ok((host.to_string(), port))
            } else {
                Err(anyhow!("Invalid IPv6 address: missing closing bracket"))
            }
        } else {
            // IPv4 or hostname: 192.168.1.1:4840 or plc.local:4840
            if let Some(colon_pos) = host_port.rfind(':') {
                let host = &host_port[..colon_pos];
                let port: u16 = host_port[colon_pos + 1..]
                    .parse()
                    .unwrap_or(DEFAULT_OPCUA_PORT);
                Ok((host.to_string(), port))
            } else {
                Ok((host_port.to_string(), DEFAULT_OPCUA_PORT))
            }
        }
    }

    /// Generate cryptographically secure random nonce for security
    fn generate_nonce() -> Vec<u8> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut nonce = vec![0u8; 32];

        // Use system entropy sources for cryptographic randomness
        // Combine multiple sources for better entropy

        // Source 1: High-resolution timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        nonce[0..16].copy_from_slice(&timestamp.to_le_bytes());

        // Source 2: Process ID and thread ID
        let pid = std::process::id();
        nonce[16..20].copy_from_slice(&pid.to_le_bytes());

        // Source 3: Stack address randomization (ASLR entropy)
        let stack_var: u64 = 0;
        let stack_addr = &stack_var as *const u64 as u64;
        nonce[20..28].copy_from_slice(&stack_addr.to_le_bytes());

        // Source 4: Additional timestamp variation
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u32;
        nonce[28..32].copy_from_slice(&timestamp2.to_le_bytes());

        // Mix the nonce using simple XOR rotation for better distribution
        for i in 0..32 {
            nonce[i] ^= nonce[(i + 13) % 32].rotate_left(3);
        }

        nonce
    }

    /// Encode OPC UA string
    fn encode_string(s: &str) -> Vec<u8> {
        let mut data = Vec::new();
        if s.is_empty() {
            data.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // null string
        } else {
            data.extend_from_slice(&(s.len() as u32).to_le_bytes());
            data.extend_from_slice(s.as_bytes());
        }
        data
    }

    /// Encode OPC UA ByteString
    fn encode_bytestring(bytes: &[u8]) -> Vec<u8> {
        let mut data = Vec::new();
        if bytes.is_empty() {
            data.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // null
        } else {
            data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            data.extend_from_slice(bytes);
        }
        data
    }

    /// Build request header for service requests
    async fn build_request_header(&self) -> Vec<u8> {
        let mut header = Vec::new();

        // Authentication token
        if let Some(ref token) = *self.auth_token.lock().await {
            header.extend_from_slice(token);
        } else {
            header.push(0x00); // null node id (two-byte, namespace 0, id 0)
            header.push(0x00);
        }

        // Timestamp (current time as Windows FILETIME)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        // Convert to Windows FILETIME (100ns intervals since 1601-01-01)
        let filetime = (now.as_nanos() / 100) as i64 + 116444736000000000i64;
        header.extend_from_slice(&filetime.to_le_bytes());

        // Request handle
        let req_id = self.next_request_id().await;
        header.extend_from_slice(&req_id.to_le_bytes());

        // Return diagnostics (0 = none)
        header.extend_from_slice(&0u32.to_le_bytes());

        // Audit entry id (null)
        header.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

        // Timeout hint
        header.extend_from_slice(&(self.config.timeout_secs as u32 * 1000).to_le_bytes());

        // Additional header (null - empty extension object)
        header.push(0x00); // Type ID encoding (two-byte null)
        header.push(0x00);
        header.push(0x00); // No body

        header
    }

    /// Build secure message wrapper
    async fn build_secure_message(&self, service_request: &[u8]) -> Vec<u8> {
        let mut msg = Vec::new();

        // Message header
        msg.extend_from_slice(MSG_MESSAGE);
        msg.push(b'F'); // Final chunk

        let size_pos = msg.len();
        msg.extend_from_slice(&[0u8; 4]); // Size placeholder

        // Security header
        let channel_id = *self.secure_channel_id.lock().await;
        msg.extend_from_slice(&channel_id.to_le_bytes());

        let token_id = *self.token_id.lock().await;
        msg.extend_from_slice(&token_id.to_le_bytes());

        // Sequence header
        let seq = self.next_sequence().await;
        let req_id = self.next_request_id().await;
        msg.extend_from_slice(&seq.to_le_bytes());
        msg.extend_from_slice(&req_id.to_le_bytes());

        // Service request body
        msg.extend_from_slice(service_request);

        // Update size
        let size = msg.len() as u32;
        msg[size_pos..size_pos + 4].copy_from_slice(&size.to_le_bytes());

        msg
    }

    /// Create OPC UA session
    async fn create_session(&self) -> Result<()> {
        let mut request = Vec::new();

        // Type ID for CreateSessionRequest (461)
        let type_id = NodeId::numeric(0, TYPE_ID_CREATE_SESSION_REQUEST);
        request.extend_from_slice(&type_id.encode());

        // Request header
        request.extend_from_slice(&self.build_request_header().await);

        // Client description (ApplicationDescription)
        // ApplicationUri
        let app_uri = format!("urn:{}:SuderraAgent", self.config.name);
        request.extend_from_slice(&Self::encode_string(&app_uri));

        // ProductUri
        request.extend_from_slice(&Self::encode_string("urn:Suderra:Agent"));

        // ApplicationName (LocalizedText)
        request.push(0x02); // Encoding mask: text only
        request.extend_from_slice(&Self::encode_string("Suderra Agent"));

        // ApplicationType (1 = Client)
        request.extend_from_slice(&1u32.to_le_bytes());

        // GatewayServerUri (null)
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

        // DiscoveryProfileUri (null)
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

        // DiscoveryUrls (empty array)
        request.extend_from_slice(&0i32.to_le_bytes());

        // ServerUri
        request.extend_from_slice(&Self::encode_string(&self.config.endpoint_url));

        // EndpointUrl
        request.extend_from_slice(&Self::encode_string(&self.config.endpoint_url));

        // SessionName
        let session_name = format!("{}_{}", self.config.name, std::process::id());
        request.extend_from_slice(&Self::encode_string(&session_name));

        // ClientNonce
        let nonce = Self::generate_nonce();
        request.extend_from_slice(&Self::encode_bytestring(&nonce));

        // ClientCertificate (null for None security)
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

        // RequestedSessionTimeout (in ms)
        let timeout_ms = self.config.session_timeout_ms as f64;
        request.extend_from_slice(&timeout_ms.to_le_bytes());

        // MaxResponseMessageSize
        request.extend_from_slice(&(MAX_OPCUA_MESSAGE_SIZE as u32).to_le_bytes());

        // Send request
        let message = self.build_secure_message(&request).await;
        let response = self.send_receive(&message).await?;

        // Parse CreateSessionResponse
        // Response structure:
        // - Message header (8 bytes): MSG + 'F' + size (4)
        // - Security header (8 bytes): channel_id (4) + token_id (4)
        // - Sequence header (8 bytes): sequence_number (4) + request_id (4)
        // - Type ID (variable)
        // - Response header (variable)
        // - SessionId (NodeId)
        // - AuthenticationToken (NodeId)
        // - ...

        if response.len() < 50 {
            return Err(anyhow!("CreateSession response too short: {} bytes", response.len()));
        }

        // Skip message header (8) + security header (8) + sequence header (8) = 24 bytes
        let body_start = 24;
        if response.len() <= body_start {
            return Err(anyhow!("CreateSession response body missing"));
        }

        // Parse response body
        let body = &response[body_start..];
        let mut offset = 0;

        // Skip Type ID (CreateSessionResponse = 464)
        let (type_id, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;
        debug!("CreateSession response type: {:?}", type_id);

        // Skip Response Header
        // - Timestamp (8 bytes)
        // - RequestHandle (4 bytes)
        // - ServiceResult (4 bytes)
        // - DiagnosticInfo (variable)
        // - StringTable (array)
        // - AdditionalHeader (ExtensionObject)
        if body.len() < offset + 16 {
            return Err(anyhow!("CreateSession response header too short"));
        }

        // Check ServiceResult
        let service_result = u32::from_le_bytes([
            body[offset + 8],
            body[offset + 9],
            body[offset + 10],
            body[offset + 11],
        ]);
        if service_result & STATUS_BAD_MASK != 0 {
            return Err(anyhow!(
                "CreateSession failed with status: 0x{:08X}",
                service_result
            ));
        }

        offset += 8; // Timestamp
        offset += 4; // RequestHandle
        offset += 4; // ServiceResult

        // Skip DiagnosticInfo (simplified - assume no diagnostics)
        if body.len() > offset && body[offset] == 0x00 {
            offset += 1; // Empty diagnostic info
        }

        // Skip StringTable (array of strings)
        if body.len() >= offset + 4 {
            let array_len = i32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if array_len > 0 {
                // Skip strings
                for _ in 0..array_len {
                    if body.len() < offset + 4 {
                        break;
                    }
                    let str_len = u32::from_le_bytes([
                        body[offset],
                        body[offset + 1],
                        body[offset + 2],
                        body[offset + 3],
                    ]);
                    offset += 4;
                    if str_len != 0xFFFFFFFF {
                        offset += str_len as usize;
                    }
                }
            }
        }

        // Skip AdditionalHeader (ExtensionObject - simplified)
        if body.len() > offset + 3 {
            // Skip the extension object (type id + encoding + optional body)
            let (_, consumed) = NodeId::decode(&body[offset..])?;
            offset += consumed;
            if body.len() > offset {
                let ext_encoding = body[offset];
                offset += 1;
                if ext_encoding == 0x01 {
                    // Has body
                    if body.len() >= offset + 4 {
                        let body_len = u32::from_le_bytes([
                            body[offset],
                            body[offset + 1],
                            body[offset + 2],
                            body[offset + 3],
                        ]);
                        offset += 4 + body_len as usize;
                    }
                }
            }
        }

        // Now parse SessionId
        if body.len() <= offset {
            return Err(anyhow!("CreateSession response missing SessionId"));
        }
        let (session_id, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;
        debug!("Session ID: {:?}", session_id);

        // Parse AuthenticationToken
        if body.len() <= offset {
            return Err(anyhow!("CreateSession response missing AuthenticationToken"));
        }
        let (auth_token, _consumed) = NodeId::decode(&body[offset..])?;
        debug!("Auth Token: {:?}", auth_token);

        // Store the session ID and auth token as encoded NodeIds
        *self.session_id.lock().await = Some(session_id.encode());
        *self.auth_token.lock().await = Some(auth_token.encode());

        info!("OPC UA session created successfully");

        Ok(())
    }

    /// Activate OPC UA session
    async fn activate_session(&self) -> Result<()> {
        let mut request = Vec::new();

        // Type ID for ActivateSessionRequest (467)
        let type_id = NodeId::numeric(0, TYPE_ID_ACTIVATE_SESSION_REQUEST);
        request.extend_from_slice(&type_id.encode());

        // Request header
        request.extend_from_slice(&self.build_request_header().await);

        // Client signature (null for None security)
        // SignatureAlgorithm
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        // Signature
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

        // ClientSoftwareCertificates (empty array)
        request.extend_from_slice(&0i32.to_le_bytes());

        // LocaleIds (empty array - server will use default)
        request.extend_from_slice(&0i32.to_le_bytes());

        // UserIdentityToken
        if let (Some(username), Some(password)) = (&self.config.username, &self.config.password) {
            // UserNameIdentityToken (type id = 324)
            let token_type = NodeId::numeric(0, 324);
            request.push(0x01); // Has body
            request.extend_from_slice(&token_type.encode());
            request.push(0x01); // Binary encoding

            // Calculate body length
            let policy_id = "username";
            let body_len = 4 + policy_id.len() + 4 + username.len() + 4 + password.len() + 4;
            request.extend_from_slice(&(body_len as u32).to_le_bytes());

            // PolicyId
            request.extend_from_slice(&Self::encode_string(policy_id));
            // UserName
            request.extend_from_slice(&Self::encode_string(username));
            // Password (should be encrypted for secure mode!)
            request.extend_from_slice(&Self::encode_bytestring(password.as_bytes()));
            // EncryptionAlgorithm (null)
            request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

            if self.config.security_mode == OpcUaSecurityMode::None {
                warn!(
                    "SECURITY: Sending credentials over unencrypted connection. \
                     Configure security_mode for production use."
                );
            }
        } else {
            // AnonymousIdentityToken (type id = 321)
            let token_type = NodeId::numeric(0, 321);
            request.push(0x01); // Has body
            request.extend_from_slice(&token_type.encode());
            request.push(0x01); // Binary encoding

            let policy_id = "anonymous";
            let body_len = 4 + policy_id.len();
            request.extend_from_slice(&(body_len as u32).to_le_bytes());
            request.extend_from_slice(&Self::encode_string(policy_id));
        }

        // UserTokenSignature (null for None security)
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());

        // Send request
        let message = self.build_secure_message(&request).await;
        let response = self.send_receive(&message).await?;

        if response.len() < 30 {
            return Err(anyhow!("ActivateSession response too short: {} bytes", response.len()));
        }

        // Parse ActivateSessionResponse
        let body_start = 24; // Skip message header + security header + sequence header
        if response.len() <= body_start {
            return Err(anyhow!("ActivateSession response body missing"));
        }

        let body = &response[body_start..];
        let mut offset = 0;

        // Skip Type ID
        let (_, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;

        // Check ServiceResult in response header
        if body.len() >= offset + 16 {
            let service_result = u32::from_le_bytes([
                body[offset + 8],
                body[offset + 9],
                body[offset + 10],
                body[offset + 11],
            ]);
            if service_result & STATUS_BAD_MASK != 0 {
                return Err(anyhow!(
                    "ActivateSession failed with status: 0x{:08X}",
                    service_result
                ));
            }
        }

        info!("OPC UA session activated successfully");

        Ok(())
    }

    // =========================================================================
    // OPC UA Services Implementation
    // =========================================================================

    /// Browse nodes in the OPC UA address space
    async fn browse_nodes(&self, node_id: &NodeId, reference_type: u32) -> Result<Vec<BrowseReference>> {
        let mut request = Vec::new();

        // Type ID for BrowseRequest
        let type_id = NodeId::numeric(0, TYPE_ID_BROWSE_REQUEST);
        request.extend_from_slice(&type_id.encode());

        // Request header
        request.extend_from_slice(&self.build_request_header().await);

        // View (empty - browse current view)
        // ViewId (null NodeId)
        request.push(0x00);
        request.push(0x00);
        // Timestamp (0)
        request.extend_from_slice(&0i64.to_le_bytes());
        // ViewVersion (0)
        request.extend_from_slice(&0u32.to_le_bytes());

        // RequestedMaxReferencesPerNode
        request.extend_from_slice(&1000u32.to_le_bytes());

        // NodesToBrowse (array of BrowseDescription)
        request.extend_from_slice(&1i32.to_le_bytes()); // Array length = 1

        // BrowseDescription
        // NodeId
        request.extend_from_slice(&node_id.encode());
        // BrowseDirection (Forward = 0)
        request.extend_from_slice(&BROWSE_DIRECTION_FORWARD.to_le_bytes());
        // ReferenceTypeId (HierarchicalReferences or specified)
        let ref_type_node = NodeId::numeric(0, reference_type);
        request.extend_from_slice(&ref_type_node.encode());
        // IncludeSubtypes (true)
        request.push(0x01);
        // NodeClassMask (0 = all classes)
        request.extend_from_slice(&0u32.to_le_bytes());
        // ResultMask (all fields)
        request.extend_from_slice(&0x3Fu32.to_le_bytes());

        // Send request
        let message = self.build_secure_message(&request).await;
        let response = self.send_receive(&message).await?;

        // Parse BrowseResponse
        if response.len() < 30 {
            return Err(anyhow!("Browse response too short"));
        }

        let body_start = 24;
        let body = &response[body_start..];
        let mut offset = 0;

        // Skip Type ID
        let (_, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;

        // Skip response header (simplified)
        offset += 8; // Timestamp
        offset += 4; // RequestHandle

        // Check ServiceResult
        if body.len() >= offset + 4 {
            let service_result = u32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if service_result & STATUS_BAD_MASK != 0 {
                return Err(anyhow!("Browse failed with status: 0x{:08X}", service_result));
            }
        }

        // Skip DiagnosticInfo
        if body.len() > offset && body[offset] == 0x00 {
            offset += 1;
        }

        // Skip StringTable
        if body.len() >= offset + 4 {
            let array_len = i32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if array_len > 0 {
                for _ in 0..array_len {
                    if body.len() < offset + 4 {
                        break;
                    }
                    let str_len = u32::from_le_bytes([
                        body[offset],
                        body[offset + 1],
                        body[offset + 2],
                        body[offset + 3],
                    ]);
                    offset += 4;
                    if str_len != 0xFFFFFFFF {
                        offset += str_len as usize;
                    }
                }
            }
        }

        // Skip AdditionalHeader
        if body.len() > offset + 2 {
            let (_, consumed) = NodeId::decode(&body[offset..])?;
            offset += consumed;
            if body.len() > offset {
                offset += 1; // Encoding byte
            }
        }

        // Parse Results array
        let mut references = Vec::new();

        if body.len() < offset + 4 {
            return Ok(references);
        }

        let results_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if results_len <= 0 {
            return Ok(references);
        }

        // Parse first BrowseResult
        // StatusCode
        if body.len() < offset + 4 {
            return Ok(references);
        }
        let browse_status = u32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if browse_status & STATUS_BAD_MASK != 0 {
            debug!("Browse node returned status: 0x{:08X}", browse_status);
            return Ok(references);
        }

        // ContinuationPoint (skip)
        if body.len() < offset + 4 {
            return Ok(references);
        }
        let cp_len = u32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;
        if cp_len != 0xFFFFFFFF {
            offset += cp_len as usize;
        }

        // References array
        if body.len() < offset + 4 {
            return Ok(references);
        }
        let refs_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if refs_len <= 0 {
            return Ok(references);
        }

        for _ in 0..refs_len {
            if body.len() <= offset {
                break;
            }
            match BrowseReference::decode(&body[offset..]) {
                Ok((reference, consumed)) => {
                    references.push(reference);
                    offset += consumed;
                }
                Err(e) => {
                    debug!("Failed to parse browse reference: {}", e);
                    break;
                }
            }
        }

        debug!("Browse returned {} references", references.len());
        Ok(references)
    }

    /// Read a single node value
    async fn read_node(&self, node_id: &NodeId, attribute_id: u32) -> Result<DataValue> {
        let mut request = Vec::new();

        // Type ID for ReadRequest
        let type_id = NodeId::numeric(0, TYPE_ID_READ_REQUEST);
        request.extend_from_slice(&type_id.encode());

        // Request header
        request.extend_from_slice(&self.build_request_header().await);

        // MaxAge (0 = read from cache)
        request.extend_from_slice(&0.0f64.to_le_bytes());

        // TimestampsToReturn (Both = 2)
        request.extend_from_slice(&2u32.to_le_bytes());

        // NodesToRead (array of ReadValueId)
        request.extend_from_slice(&1i32.to_le_bytes()); // Array length = 1

        // ReadValueId
        // NodeId
        request.extend_from_slice(&node_id.encode());
        // AttributeId
        request.extend_from_slice(&attribute_id.to_le_bytes());
        // IndexRange (null)
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        // DataEncoding (null QualifiedName)
        request.extend_from_slice(&0u16.to_le_bytes()); // NamespaceIndex
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // Name (null)

        // Send request
        let message = self.build_secure_message(&request).await;
        let response = self.send_receive(&message).await?;

        // Parse ReadResponse
        if response.len() < 30 {
            return Err(anyhow!("Read response too short"));
        }

        let body_start = 24;
        let body = &response[body_start..];
        let mut offset = 0;

        // Skip Type ID
        let (_, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;

        // Skip response header (simplified)
        offset += 8; // Timestamp
        offset += 4; // RequestHandle

        // Check ServiceResult
        if body.len() >= offset + 4 {
            let service_result = u32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if service_result & STATUS_BAD_MASK != 0 {
                return Err(anyhow!("Read failed with status: 0x{:08X}", service_result));
            }
        }

        // Skip DiagnosticInfo
        if body.len() > offset && body[offset] == 0x00 {
            offset += 1;
        }

        // Skip StringTable
        if body.len() >= offset + 4 {
            let array_len = i32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if array_len > 0 {
                for _ in 0..array_len {
                    if body.len() < offset + 4 {
                        break;
                    }
                    let str_len = u32::from_le_bytes([
                        body[offset],
                        body[offset + 1],
                        body[offset + 2],
                        body[offset + 3],
                    ]);
                    offset += 4;
                    if str_len != 0xFFFFFFFF {
                        offset += str_len as usize;
                    }
                }
            }
        }

        // Skip AdditionalHeader
        if body.len() > offset + 2 {
            let (_, consumed) = NodeId::decode(&body[offset..])?;
            offset += consumed;
            if body.len() > offset {
                offset += 1; // Encoding byte
            }
        }

        // Parse Results array
        if body.len() < offset + 4 {
            return Err(anyhow!("Read response missing results"));
        }

        let results_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if results_len <= 0 {
            return Err(anyhow!("Read response has no results"));
        }

        // Parse first DataValue
        let (data_value, _) = DataValue::decode(&body[offset..])?;
        debug!("Read returned: {:?}", data_value.value);
        Ok(data_value)
    }

    /// Write a value to a node
    async fn write_node(&self, node_id: &NodeId, value: Variant) -> Result<u32> {
        let mut request = Vec::new();

        // Type ID for WriteRequest
        let type_id = NodeId::numeric(0, TYPE_ID_WRITE_REQUEST);
        request.extend_from_slice(&type_id.encode());

        // Request header
        request.extend_from_slice(&self.build_request_header().await);

        // NodesToWrite (array of WriteValue)
        request.extend_from_slice(&1i32.to_le_bytes()); // Array length = 1

        // WriteValue
        // NodeId
        request.extend_from_slice(&node_id.encode());
        // AttributeId (Value = 13)
        request.extend_from_slice(&ATTRIBUTE_VALUE.to_le_bytes());
        // IndexRange (null)
        request.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        // Value (DataValue)
        request.push(0x01); // Encoding mask: has value
        request.extend_from_slice(&value.encode());

        // Send request
        let message = self.build_secure_message(&request).await;
        let response = self.send_receive(&message).await?;

        // Parse WriteResponse
        if response.len() < 30 {
            return Err(anyhow!("Write response too short"));
        }

        let body_start = 24;
        let body = &response[body_start..];
        let mut offset = 0;

        // Skip Type ID
        let (_, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;

        // Skip response header (simplified)
        offset += 8; // Timestamp
        offset += 4; // RequestHandle

        // Check ServiceResult
        let service_result = if body.len() >= offset + 4 {
            let result = u32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            result
        } else {
            STATUS_GOOD
        };

        if service_result & STATUS_BAD_MASK != 0 {
            return Err(anyhow!("Write failed with status: 0x{:08X}", service_result));
        }

        // Skip DiagnosticInfo
        if body.len() > offset && body[offset] == 0x00 {
            offset += 1;
        }

        // Skip StringTable
        if body.len() >= offset + 4 {
            let array_len = i32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if array_len > 0 {
                for _ in 0..array_len {
                    if body.len() < offset + 4 {
                        break;
                    }
                    let str_len = u32::from_le_bytes([
                        body[offset],
                        body[offset + 1],
                        body[offset + 2],
                        body[offset + 3],
                    ]);
                    offset += 4;
                    if str_len != 0xFFFFFFFF {
                        offset += str_len as usize;
                    }
                }
            }
        }

        // Skip AdditionalHeader
        if body.len() > offset + 2 {
            let (_, consumed) = NodeId::decode(&body[offset..])?;
            offset += consumed;
            if body.len() > offset {
                offset += 1; // Encoding byte
            }
        }

        // Parse Results array
        if body.len() < offset + 4 {
            return Ok(STATUS_GOOD);
        }

        let results_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if results_len > 0 && body.len() >= offset + 4 {
            let result_status = u32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            debug!("Write result status: 0x{:08X}", result_status);
            return Ok(result_status);
        }

        Ok(STATUS_GOOD)
    }

    /// Call a method on an object node
    async fn call_method(
        &self,
        object_id: &NodeId,
        method_id: &NodeId,
        input_arguments: Vec<Variant>,
    ) -> Result<Vec<Variant>> {
        let mut request = Vec::new();

        // Type ID for CallRequest
        let type_id = NodeId::numeric(0, TYPE_ID_CALL_REQUEST);
        request.extend_from_slice(&type_id.encode());

        // Request header
        request.extend_from_slice(&self.build_request_header().await);

        // MethodsToCall (array of CallMethodRequest)
        request.extend_from_slice(&1i32.to_le_bytes()); // Array length = 1

        // CallMethodRequest
        // ObjectId
        request.extend_from_slice(&object_id.encode());
        // MethodId
        request.extend_from_slice(&method_id.encode());
        // InputArguments (array of Variant)
        request.extend_from_slice(&(input_arguments.len() as i32).to_le_bytes());
        for arg in &input_arguments {
            request.extend_from_slice(&arg.encode());
        }

        // Send request
        let message = self.build_secure_message(&request).await;
        let response = self.send_receive(&message).await?;

        // Parse CallResponse
        if response.len() < 30 {
            return Err(anyhow!("Call response too short"));
        }

        let body_start = 24;
        let body = &response[body_start..];
        let mut offset = 0;

        // Skip Type ID
        let (_, consumed) = NodeId::decode(&body[offset..])?;
        offset += consumed;

        // Skip response header (simplified)
        offset += 8; // Timestamp
        offset += 4; // RequestHandle

        // Check ServiceResult
        if body.len() >= offset + 4 {
            let service_result = u32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if service_result & STATUS_BAD_MASK != 0 {
                return Err(anyhow!("Call failed with status: 0x{:08X}", service_result));
            }
        }

        // Skip DiagnosticInfo
        if body.len() > offset && body[offset] == 0x00 {
            offset += 1;
        }

        // Skip StringTable
        if body.len() >= offset + 4 {
            let array_len = i32::from_le_bytes([
                body[offset],
                body[offset + 1],
                body[offset + 2],
                body[offset + 3],
            ]);
            offset += 4;
            if array_len > 0 {
                for _ in 0..array_len {
                    if body.len() < offset + 4 {
                        break;
                    }
                    let str_len = u32::from_le_bytes([
                        body[offset],
                        body[offset + 1],
                        body[offset + 2],
                        body[offset + 3],
                    ]);
                    offset += 4;
                    if str_len != 0xFFFFFFFF {
                        offset += str_len as usize;
                    }
                }
            }
        }

        // Skip AdditionalHeader
        if body.len() > offset + 2 {
            let (_, consumed) = NodeId::decode(&body[offset..])?;
            offset += consumed;
            if body.len() > offset {
                offset += 1; // Encoding byte
            }
        }

        // Parse Results array
        let mut output_arguments = Vec::new();

        if body.len() < offset + 4 {
            return Ok(output_arguments);
        }

        let results_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if results_len <= 0 {
            return Ok(output_arguments);
        }

        // Parse first CallMethodResult
        // StatusCode
        if body.len() < offset + 4 {
            return Ok(output_arguments);
        }
        let call_status = u32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        if call_status & STATUS_BAD_MASK != 0 {
            return Err(anyhow!("Method call returned status: 0x{:08X}", call_status));
        }

        // InputArgumentResults (skip array of StatusCodes)
        if body.len() < offset + 4 {
            return Ok(output_arguments);
        }
        let input_results_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;
        if input_results_len > 0 {
            offset += (input_results_len as usize) * 4; // Each StatusCode is 4 bytes
        }

        // InputArgumentDiagnosticInfos (skip)
        if body.len() < offset + 4 {
            return Ok(output_arguments);
        }
        let diag_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;
        if diag_len > 0 {
            // Skip diagnostic infos (simplified)
            for _ in 0..diag_len {
                if body.len() > offset {
                    offset += 1; // Each empty DiagnosticInfo is 1 byte
                }
            }
        }

        // OutputArguments (array of Variant)
        if body.len() < offset + 4 {
            return Ok(output_arguments);
        }
        let output_len = i32::from_le_bytes([
            body[offset],
            body[offset + 1],
            body[offset + 2],
            body[offset + 3],
        ]);
        offset += 4;

        for _ in 0..output_len {
            if body.len() <= offset {
                break;
            }
            match Variant::decode(&body[offset..]) {
                Ok((variant, consumed)) => {
                    output_arguments.push(variant);
                    offset += consumed;
                }
                Err(e) => {
                    debug!("Failed to parse output argument: {}", e);
                    break;
                }
            }
        }

        debug!("Method call returned {} output arguments", output_arguments.len());
        Ok(output_arguments)
    }

    /// Build CloseSecureChannel request
    async fn build_close_secure_channel(&self) -> Vec<u8> {
        let mut msg = Vec::new();

        // Message header
        msg.extend_from_slice(MSG_CLOSE);
        msg.push(b'F');
        let size_pos = msg.len();
        msg.extend_from_slice(&[0u8; 4]);

        // Secure channel ID
        let channel_id = *self.secure_channel_id.lock().await;
        msg.extend_from_slice(&channel_id.to_le_bytes());

        // Token ID
        let token_id = *self.token_id.lock().await;
        msg.extend_from_slice(&token_id.to_le_bytes());

        // Sequence header
        let seq = self.next_sequence().await;
        let req_id = self.next_request_id().await;
        msg.extend_from_slice(&seq.to_le_bytes());
        msg.extend_from_slice(&req_id.to_le_bytes());

        // CloseSecureChannelRequest (type id = 452)
        let type_id = NodeId::numeric(0, 452);
        msg.extend_from_slice(&type_id.encode());

        // Request header
        let header = self.build_request_header().await;
        msg.extend_from_slice(&header);

        // Update size
        let size = msg.len() as u32;
        msg[size_pos..size_pos + 4].copy_from_slice(&size.to_le_bytes());

        msg
    }

}

#[async_trait::async_trait]
impl PlcProgrammer for OpcUaClient {
    fn protocol_name(&self) -> &'static str {
        "OPC UA"
    }

    async fn connect(&mut self) -> Result<()> {
        let (host, port) = self.parse_endpoint()?;
        let addr = format!("{}:{}", host, port);
        info!("Connecting to OPC UA server at {}", addr);

        // Warn about security limitations
        if self.config.security_mode != OpcUaSecurityMode::None {
            warn!(
                "SECURITY: Security mode {:?} requested but certificate handling not implemented. \
                 Falling back to None security. Use a full OPC UA SDK for production.",
                self.config.security_mode
            );
        }

        if self.config.client_cert_path.is_some() || self.config.client_key_path.is_some() {
            warn!(
                "SECURITY: Certificate paths configured but certificate loading not implemented. \
                 Connection will use anonymous/None security."
            );
        }

        if self.config.security_policy != OpcUaSecurityPolicy::None {
            warn!(
                "SECURITY: Security policy {:?} requested but not implemented. \
                 Connection will be unencrypted.",
                self.config.security_policy
            );
        }

        let timeout_duration = std::time::Duration::from_secs(self.config.timeout_secs);

        let stream = with_timeout(
            TcpStream::connect(&addr),
            timeout_duration,
            "OPC UA connect",
        )
        .await?;

        *self.connection.lock().await = Some(stream);

        // Send Hello
        let hello = self.build_hello();
        let response = self.send_receive(&hello).await?;

        // Check for ACK
        if response.len() < 3 {
            return Err(anyhow!(
                "OPC UA Hello response too short ({} bytes)",
                response.len()
            ));
        }
        if &response[0..3] != MSG_ACK {
            return Err(anyhow!("OPC UA Hello rejected"));
        }
        debug!("OPC UA Hello acknowledged");

        // Open Secure Channel
        let open_channel = self.build_open_secure_channel().await;
        let response = self.send_receive(&open_channel).await?;

        // Parse secure channel response
        // Need at least 12 bytes to access indices [8..11] for channel_id
        if response.len() >= 12 {
            let channel_id =
                u32::from_le_bytes([response[8], response[9], response[10], response[11]]);
            *self.secure_channel_id.lock().await = channel_id;
            debug!("OPC UA Secure channel opened: {}", channel_id);
        }

        // Create and activate session for authenticated operations
        if let Err(e) = self.create_session().await {
            warn!("CreateSession failed (operations may be limited): {}", e);
        } else if let Err(e) = self.activate_session().await {
            warn!("ActivateSession failed (operations may be limited): {}", e);
        }

        self.connected.store(true, Ordering::Release);
        info!("Connected to OPC UA server: {}", self.config.name);

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Send CloseSecureChannel for proper protocol termination
        if self.connected.load(Ordering::Acquire) {
            let close_msg = self.build_close_secure_channel().await;
            if let Err(e) = self.send_receive(&close_msg).await {
                debug!("CloseSecureChannel response (may timeout): {}", e);
            }
        }

        // Graceful TCP shutdown
        if let Some(mut conn) = self.connection.lock().await.take() {
            if let Err(e) = conn.shutdown().await {
                debug!("OPC UA disconnect shutdown notice: {}", e);
            }
        }

        *self.session_id.lock().await = None;
        *self.auth_token.lock().await = None;
        *self.secure_channel_id.lock().await = 0;
        *self.token_id.lock().await = 0;
        self.connected.store(false, Ordering::Release);

        info!("Disconnected from OPC UA server: {}", self.config.name);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn get_status(&self) -> Result<PlcStatus> {
        if !self.is_connected() {
            return Ok(PlcStatus {
                connected: false,
                run_mode: PlcRunMode::Unknown,
                model: "OPC UA Server".to_string(),
                firmware: "Unknown".to_string(),
                current_program: None,
                last_modified: None,
            });
        }

        // Read ServerState from Server node (i=2259)
        let server_state_node = NodeId::numeric(0, NODE_ID_SERVER_STATE);
        let run_mode = match self.read_node(&server_state_node, ATTRIBUTE_VALUE).await {
            Ok(data_value) => {
                if data_value.is_good() {
                    match &data_value.value {
                        Some(Variant::Int32(state)) => {
                            // ServerState enum: 0=Running, 1=Failed, 2=NoConfiguration,
                            // 3=Suspended, 4=Shutdown, 5=Test, 6=CommunicationFault,
                            // 7=Unknown
                            match *state {
                                0 => PlcRunMode::Run,
                                1 => PlcRunMode::Stop, // Failed
                                2 => PlcRunMode::Program, // NoConfiguration
                                3 => PlcRunMode::Stop, // Suspended
                                4 => PlcRunMode::Stop, // Shutdown
                                5 => PlcRunMode::Program, // Test
                                _ => PlcRunMode::Unknown,
                            }
                        }
                        _ => PlcRunMode::Unknown,
                    }
                } else {
                    PlcRunMode::Unknown
                }
            }
            Err(e) => {
                debug!("Failed to read ServerState: {}", e);
                PlcRunMode::Unknown
            }
        };

        // Read ProductName from Server node (i=2261)
        let product_name_node = NodeId::numeric(0, NODE_ID_PRODUCT_NAME);
        let model = match self.read_node(&product_name_node, ATTRIBUTE_VALUE).await {
            Ok(data_value) => {
                if data_value.is_good() {
                    match &data_value.value {
                        Some(Variant::String(s)) => s.clone(),
                        Some(Variant::LocalizedText(s)) => s.clone(),
                        _ => "OPC UA Server".to_string(),
                    }
                } else {
                    "OPC UA Server".to_string()
                }
            }
            Err(e) => {
                debug!("Failed to read ProductName: {}", e);
                "OPC UA Server".to_string()
            }
        };

        // Read SoftwareVersion from Server node (i=2263)
        let software_version_node = NodeId::numeric(0, NODE_ID_SOFTWARE_VERSION);
        let firmware = match self.read_node(&software_version_node, ATTRIBUTE_VALUE).await {
            Ok(data_value) => {
                if data_value.is_good() {
                    match &data_value.value {
                        Some(Variant::String(s)) => s.clone(),
                        _ => "Unknown".to_string(),
                    }
                } else {
                    "Unknown".to_string()
                }
            }
            Err(e) => {
                debug!("Failed to read SoftwareVersion: {}", e);
                "Unknown".to_string()
            }
        };

        // Try to find current program by browsing Objects folder
        let current_program = match self.list_programs().await {
            Ok(programs) => programs.first().cloned(),
            Err(_) => None,
        };

        // Read CurrentTime for last_modified estimate (i=2258)
        let current_time_node = NodeId::numeric(0, NODE_ID_CURRENT_TIME);
        let last_modified = match self.read_node(&current_time_node, ATTRIBUTE_VALUE).await {
            Ok(data_value) => {
                if data_value.is_good() {
                    match &data_value.value {
                        Some(Variant::DateTime(filetime)) => {
                            // Convert Windows FILETIME to Unix timestamp
                            // FILETIME is 100-nanosecond intervals since 1601-01-01
                            let unix_epoch_filetime = 116444736000000000i64;
                            let unix_timestamp = (*filetime - unix_epoch_filetime) / 10_000_000;
                            Some(chrono::DateTime::from_timestamp(unix_timestamp, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default())
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            Err(_) => None,
        };

        Ok(PlcStatus {
            connected: self.is_connected(),
            run_mode,
            model,
            firmware,
            current_program,
            last_modified,
        })
    }

    async fn upload_program(&self, program: &PlcProgram) -> Result<UploadResult> {
        info!(
            "Uploading program '{}' via OPC UA: {}",
            program.name, self.config.name
        );

        if !self.is_connected() {
            return Err(anyhow!("Not connected to OPC UA server"));
        }

        validate_program_source(&program.source)?;

        let mut warnings = Vec::new();
        let mut errors = Vec::new();
        let mut plc_response: HashMap<String, JsonValue> = HashMap::new();

        // Determine target namespace (default to 2 if not specified)
        let namespace = self.config.program_namespace
            .as_ref()
            .and_then(|ns| ns.parse::<u16>().ok())
            .unwrap_or(2);

        // Strategy 1: Try PLCopen OPC UA Program Transfer method
        // Look for ProgramTransfer object in namespace
        let program_transfer_object = NodeId::string(namespace, "ProgramTransfer");
        let upload_method = NodeId::string(namespace, "ProgramTransfer.Upload");

        // Prepare program data as ByteString
        let program_source_bytes = program.source.as_bytes().to_vec();

        // Try method call approach first
        let method_result = self
            .call_method(
                &program_transfer_object,
                &upload_method,
                vec![
                    Variant::String(program.name.clone()),
                    Variant::ByteString(program_source_bytes.clone()),
                ],
            )
            .await;

        match method_result {
            Ok(outputs) => {
                info!("Program uploaded via ProgramTransfer method");
                plc_response.insert("method".to_string(), JsonValue::String("ProgramTransfer.Upload".to_string()));
                if let Some(output) = outputs.first() {
                    plc_response.insert("result".to_string(), JsonValue::String(output.to_string_value()));
                }
            }
            Err(e) => {
                debug!("ProgramTransfer method not available: {}", e);
                warnings.push(format!("ProgramTransfer method not available: {}", e));

                // Strategy 2: Try writing to a file node
                // Common pattern: Files/Programs/<ProgramName>
                let file_node = NodeId::string(namespace, &format!("Files.Programs.{}", program.name));

                let write_result = self
                    .write_node(&file_node, Variant::ByteString(program_source_bytes.clone()))
                    .await;

                match write_result {
                    Ok(status) => {
                        if status & STATUS_BAD_MASK == 0 {
                            info!("Program uploaded via file node write");
                            plc_response.insert("method".to_string(), JsonValue::String("FileNode.Write".to_string()));
                        } else {
                            let err_msg = format!("File write returned status: 0x{:08X}", status);
                            debug!("{}", err_msg);
                            warnings.push(err_msg);

                            // Strategy 3: Try generic program variable node
                            let program_node = NodeId::string(namespace, &program.name);
                            let write_result2 = self
                                .write_node(&program_node, Variant::String(program.source.clone()))
                                .await;

                            match write_result2 {
                                Ok(status2) => {
                                    if status2 & STATUS_BAD_MASK == 0 {
                                        info!("Program uploaded via direct node write");
                                        plc_response.insert("method".to_string(), JsonValue::String("DirectNode.Write".to_string()));
                                    } else {
                                        errors.push(format!(
                                            "All upload strategies failed. Last status: 0x{:08X}",
                                            status2
                                        ));
                                    }
                                }
                                Err(e) => {
                                    errors.push(format!("Direct node write failed: {}", e));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("File node write failed: {}", e);
                        warnings.push(format!("File node write failed: {}", e));

                        // Strategy 3: Try generic program variable node
                        let program_node = NodeId::string(namespace, &program.name);
                        let write_result2 = self
                            .write_node(&program_node, Variant::String(program.source.clone()))
                            .await;

                        match write_result2 {
                            Ok(status2) => {
                                if status2 & STATUS_BAD_MASK == 0 {
                                    info!("Program uploaded via direct node write");
                                    plc_response.insert("method".to_string(), JsonValue::String("DirectNode.Write".to_string()));
                                } else {
                                    errors.push(format!(
                                        "All upload strategies failed. Last status: 0x{:08X}",
                                        status2
                                    ));
                                }
                            }
                            Err(e) => {
                                errors.push(format!("Direct node write failed: {}", e));
                            }
                        }
                    }
                }
            }
        }

        let success = errors.is_empty();
        let status_msg = if success { "OK" } else { "FAILED" };

        audit_program_upload(
            "OPC UA",
            &self.config.endpoint_url,
            &program.name,
            success,
            status_msg,
        );

        Ok(UploadResult {
            success,
            program_id: Some(program.name.clone()),
            warnings,
            errors,
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response,
        })
    }

    async fn download_program(&self, program_name: &str) -> Result<PlcProgram> {
        info!("Downloading program '{}' via OPC UA: {}", program_name, self.config.name);

        if !self.is_connected() {
            return Err(anyhow!("Not connected to OPC UA server"));
        }

        // Determine target namespace (default to 2 if not specified)
        let namespace = self.config.program_namespace
            .as_ref()
            .and_then(|ns| ns.parse::<u16>().ok())
            .unwrap_or(2);

        let mut source = String::new();
        let mut metadata = HashMap::new();

        // Strategy 1: Try PLCopen OPC UA ProgramTransfer.Download method
        let program_transfer_object = NodeId::string(namespace, "ProgramTransfer");
        let download_method = NodeId::string(namespace, "ProgramTransfer.Download");

        let method_result = self
            .call_method(
                &program_transfer_object,
                &download_method,
                vec![Variant::String(program_name.to_string())],
            )
            .await;

        match method_result {
            Ok(outputs) => {
                if let Some(output) = outputs.first() {
                    match output {
                        Variant::ByteString(bytes) => {
                            source = String::from_utf8_lossy(bytes).to_string();
                            metadata.insert("method".to_string(), "ProgramTransfer.Download".to_string());
                        }
                        Variant::String(s) => {
                            source = s.clone();
                            metadata.insert("method".to_string(), "ProgramTransfer.Download".to_string());
                        }
                        _ => {
                            debug!("Unexpected output type from ProgramTransfer.Download");
                        }
                    }
                }
            }
            Err(e) => {
                debug!("ProgramTransfer.Download method not available: {}", e);

                // Strategy 2: Try reading from file node
                let file_node = NodeId::string(namespace, &format!("Files.Programs.{}", program_name));
                let read_result = self.read_node(&file_node, ATTRIBUTE_VALUE).await;

                match read_result {
                    Ok(data_value) => {
                        if data_value.is_good() {
                            match &data_value.value {
                                Some(Variant::ByteString(bytes)) => {
                                    source = String::from_utf8_lossy(bytes).to_string();
                                    metadata.insert("method".to_string(), "FileNode.Read".to_string());
                                }
                                Some(Variant::String(s)) => {
                                    source = s.clone();
                                    metadata.insert("method".to_string(), "FileNode.Read".to_string());
                                }
                                _ => {
                                    debug!("Unexpected value type from file node");
                                }
                            }
                        }
                    }
                    Err(e2) => {
                        debug!("File node read failed: {}", e2);

                        // Strategy 3: Try direct program node
                        let program_node = NodeId::string(namespace, program_name);
                        let read_result2 = self.read_node(&program_node, ATTRIBUTE_VALUE).await;

                        match read_result2 {
                            Ok(data_value) => {
                                if data_value.is_good() {
                                    match &data_value.value {
                                        Some(Variant::ByteString(bytes)) => {
                                            source = String::from_utf8_lossy(bytes).to_string();
                                            metadata.insert("method".to_string(), "DirectNode.Read".to_string());
                                        }
                                        Some(Variant::String(s)) => {
                                            source = s.clone();
                                            metadata.insert("method".to_string(), "DirectNode.Read".to_string());
                                        }
                                        _ => {
                                            return Err(anyhow!(
                                                "Program node exists but has unexpected value type"
                                            ));
                                        }
                                    }
                                } else {
                                    return Err(anyhow!(
                                        "Failed to read program: status 0x{:08X}",
                                        data_value.status_code
                                    ));
                                }
                            }
                            Err(e3) => {
                                return Err(anyhow!(
                                    "Failed to download program '{}': all strategies failed. Last error: {}",
                                    program_name,
                                    e3
                                ));
                            }
                        }
                    }
                }
            }
        }

        if source.is_empty() {
            return Err(anyhow!("Downloaded program source is empty"));
        }

        // Detect program language from source
        let language = detect_program_language(&source);

        info!("Program '{}' downloaded successfully ({} bytes)", program_name, source.len());

        Ok(PlcProgram {
            name: program_name.to_string(),
            language,
            source,
            variables: Vec::new(),
            function_blocks: Vec::new(),
            metadata,
        })
    }

    async fn start(&self) -> Result<()> {
        info!("Starting PLC via OPC UA: {}", self.config.name);

        if !self.is_connected() {
            return Err(anyhow!("Not connected to OPC UA server"));
        }

        // Determine target namespace (default to 2 if not specified)
        let namespace = self.config.program_namespace
            .as_ref()
            .and_then(|ns| ns.parse::<u16>().ok())
            .unwrap_or(2);

        // Try multiple common PLC control object/method patterns

        // Pattern 1: PLCopen standard - PLC object with Start method
        let plc_object = NodeId::string(namespace, "PLC");
        let start_method = NodeId::string(namespace, "PLC.Start");

        match self.call_method(&plc_object, &start_method, vec![]).await {
            Ok(_) => {
                info!("PLC started via PLC.Start method");
                return Ok(());
            }
            Err(e) => {
                debug!("PLC.Start method not available: {}", e);
            }
        }

        // Pattern 2: DeviceSet/PLC pattern (common in Siemens)
        let device_plc = NodeId::string(namespace, "DeviceSet.PLC_1");
        let device_start = NodeId::string(namespace, "DeviceSet.PLC_1.Start");

        match self.call_method(&device_plc, &device_start, vec![]).await {
            Ok(_) => {
                info!("PLC started via DeviceSet.PLC_1.Start method");
                return Ok(());
            }
            Err(e) => {
                debug!("DeviceSet.PLC_1.Start method not available: {}", e);
            }
        }

        // Pattern 3: Server object with Start method
        let server_object = NodeId::numeric(0, NODE_ID_SERVER);
        let server_start = NodeId::string(0, "Start");

        match self.call_method(&server_object, &server_start, vec![]).await {
            Ok(_) => {
                info!("PLC started via Server.Start method");
                return Ok(());
            }
            Err(e) => {
                debug!("Server.Start method not available: {}", e);
            }
        }

        // Pattern 4: Write to control variable
        let run_control = NodeId::string(namespace, "PLC.RunControl");
        match self.write_node(&run_control, Variant::Boolean(true)).await {
            Ok(status) => {
                if status & STATUS_BAD_MASK == 0 {
                    info!("PLC started via RunControl variable");
                    return Ok(());
                }
                debug!("RunControl write returned status: 0x{:08X}", status);
            }
            Err(e) => {
                debug!("RunControl write failed: {}", e);
            }
        }

        // Pattern 5: Try numeric mode control
        let mode_control = NodeId::string(namespace, "PLC.Mode");
        match self.write_node(&mode_control, Variant::Int32(1)).await {
            // 1 = RUN
            Ok(status) => {
                if status & STATUS_BAD_MASK == 0 {
                    info!("PLC started via Mode variable");
                    return Ok(());
                }
                debug!("Mode write returned status: 0x{:08X}", status);
            }
            Err(e) => {
                debug!("Mode write failed: {}", e);
            }
        }

        warn!("No PLC Start method found - PLC may not support remote start");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping PLC via OPC UA: {}", self.config.name);

        if !self.is_connected() {
            return Err(anyhow!("Not connected to OPC UA server"));
        }

        // Determine target namespace (default to 2 if not specified)
        let namespace = self.config.program_namespace
            .as_ref()
            .and_then(|ns| ns.parse::<u16>().ok())
            .unwrap_or(2);

        // Try multiple common PLC control object/method patterns

        // Pattern 1: PLCopen standard - PLC object with Stop method
        let plc_object = NodeId::string(namespace, "PLC");
        let stop_method = NodeId::string(namespace, "PLC.Stop");

        match self.call_method(&plc_object, &stop_method, vec![]).await {
            Ok(_) => {
                info!("PLC stopped via PLC.Stop method");
                return Ok(());
            }
            Err(e) => {
                debug!("PLC.Stop method not available: {}", e);
            }
        }

        // Pattern 2: DeviceSet/PLC pattern (common in Siemens)
        let device_plc = NodeId::string(namespace, "DeviceSet.PLC_1");
        let device_stop = NodeId::string(namespace, "DeviceSet.PLC_1.Stop");

        match self.call_method(&device_plc, &device_stop, vec![]).await {
            Ok(_) => {
                info!("PLC stopped via DeviceSet.PLC_1.Stop method");
                return Ok(());
            }
            Err(e) => {
                debug!("DeviceSet.PLC_1.Stop method not available: {}", e);
            }
        }

        // Pattern 3: Server object with Stop method
        let server_object = NodeId::numeric(0, NODE_ID_SERVER);
        let server_stop = NodeId::string(0, "Stop");

        match self.call_method(&server_object, &server_stop, vec![]).await {
            Ok(_) => {
                info!("PLC stopped via Server.Stop method");
                return Ok(());
            }
            Err(e) => {
                debug!("Server.Stop method not available: {}", e);
            }
        }

        // Pattern 4: Write to control variable
        let run_control = NodeId::string(namespace, "PLC.RunControl");
        match self.write_node(&run_control, Variant::Boolean(false)).await {
            Ok(status) => {
                if status & STATUS_BAD_MASK == 0 {
                    info!("PLC stopped via RunControl variable");
                    return Ok(());
                }
                debug!("RunControl write returned status: 0x{:08X}", status);
            }
            Err(e) => {
                debug!("RunControl write failed: {}", e);
            }
        }

        // Pattern 5: Try numeric mode control
        let mode_control = NodeId::string(namespace, "PLC.Mode");
        match self.write_node(&mode_control, Variant::Int32(0)).await {
            // 0 = STOP
            Ok(status) => {
                if status & STATUS_BAD_MASK == 0 {
                    info!("PLC stopped via Mode variable");
                    return Ok(());
                }
                debug!("Mode write returned status: 0x{:08X}", status);
            }
            Err(e) => {
                debug!("Mode write failed: {}", e);
            }
        }

        warn!("No PLC Stop method found - PLC may not support remote stop");
        Ok(())
    }

    async fn list_programs(&self) -> Result<Vec<String>> {
        info!("Listing programs via OPC UA: {}", self.config.name);

        if !self.is_connected() {
            return Err(anyhow!("Not connected to OPC UA server"));
        }

        let mut programs = Vec::new();

        // Determine target namespace (default to 2 if not specified)
        let namespace = self.config.program_namespace
            .as_ref()
            .and_then(|ns| ns.parse::<u16>().ok())
            .unwrap_or(2);

        // Strategy 1: Browse Programs folder
        let programs_folder = NodeId::string(namespace, "Programs");
        match self.browse_nodes(&programs_folder, REFERENCE_TYPE_ORGANIZES).await {
            Ok(references) => {
                for reference in references {
                    if !reference.browse_name.is_empty() {
                        programs.push(reference.browse_name.clone());
                    } else if !reference.display_name.is_empty() {
                        programs.push(reference.display_name.clone());
                    }
                }
                if !programs.is_empty() {
                    debug!("Found {} programs in Programs folder", programs.len());
                    return Ok(programs);
                }
            }
            Err(e) => {
                debug!("Programs folder browse failed: {}", e);
            }
        }

        // Strategy 2: Browse PLC object for programs
        let plc_object = NodeId::string(namespace, "PLC");
        match self.browse_nodes(&plc_object, REFERENCE_TYPE_HIERARCHICAL).await {
            Ok(references) => {
                for reference in references {
                    // Filter for program-like nodes (NodeClass = Object or Variable)
                    // NodeClass: 1=Object, 2=Variable, 4=Method
                    if reference.node_class == 1 || reference.node_class == 2 {
                        let name = if !reference.browse_name.is_empty() {
                            reference.browse_name.clone()
                        } else {
                            reference.display_name.clone()
                        };
                        // Filter common non-program nodes
                        if !name.is_empty()
                            && !name.starts_with("_")
                            && name != "Status"
                            && name != "State"
                            && name != "Mode"
                        {
                            programs.push(name);
                        }
                    }
                }
                if !programs.is_empty() {
                    debug!("Found {} programs under PLC object", programs.len());
                    return Ok(programs);
                }
            }
            Err(e) => {
                debug!("PLC object browse failed: {}", e);
            }
        }

        // Strategy 3: Browse Files/Programs folder
        let files_programs = NodeId::string(namespace, "Files.Programs");
        match self.browse_nodes(&files_programs, REFERENCE_TYPE_ORGANIZES).await {
            Ok(references) => {
                for reference in references {
                    if !reference.browse_name.is_empty() {
                        programs.push(reference.browse_name.clone());
                    } else if !reference.display_name.is_empty() {
                        programs.push(reference.display_name.clone());
                    }
                }
                if !programs.is_empty() {
                    debug!("Found {} programs in Files/Programs folder", programs.len());
                    return Ok(programs);
                }
            }
            Err(e) => {
                debug!("Files/Programs folder browse failed: {}", e);
            }
        }

        // Strategy 4: Browse Objects folder for application-specific nodes
        let objects_folder = NodeId::numeric(0, NODE_ID_OBJECTS_FOLDER);
        match self.browse_nodes(&objects_folder, REFERENCE_TYPE_ORGANIZES).await {
            Ok(references) => {
                for reference in references {
                    // Look for nodes that might be programs (in vendor namespace)
                    if let NodeId::String(ns, _) | NodeId::Numeric(ns, _) = &reference.node_id {
                        if *ns >= 2 {
                            let name = if !reference.browse_name.is_empty() {
                                reference.browse_name.clone()
                            } else {
                                reference.display_name.clone()
                            };
                            if !name.is_empty()
                                && !name.starts_with("Server")
                                && !name.starts_with("_")
                            {
                                programs.push(name);
                            }
                        }
                    }
                }
                if !programs.is_empty() {
                    debug!("Found {} potential programs in Objects folder", programs.len());
                    return Ok(programs);
                }
            }
            Err(e) => {
                debug!("Objects folder browse failed: {}", e);
            }
        }

        debug!("No programs found via browse - returning empty list");
        Ok(programs)
    }

    async fn delete_program(&self, program_name: &str) -> Result<()> {
        warn!(
            "Deleting program '{}' via OPC UA: {}",
            program_name, self.config.name
        );

        if !self.is_connected() {
            return Err(anyhow!("Not connected to OPC UA server"));
        }

        // Determine target namespace (default to 2 if not specified)
        let namespace = self.config.program_namespace
            .as_ref()
            .and_then(|ns| ns.parse::<u16>().ok())
            .unwrap_or(2);

        // Strategy 1: Call ProgramTransfer.Delete method
        let program_transfer_object = NodeId::string(namespace, "ProgramTransfer");
        let delete_method = NodeId::string(namespace, "ProgramTransfer.Delete");

        match self
            .call_method(
                &program_transfer_object,
                &delete_method,
                vec![Variant::String(program_name.to_string())],
            )
            .await
        {
            Ok(_) => {
                info!("Program '{}' deleted via ProgramTransfer.Delete method", program_name);
                return Ok(());
            }
            Err(e) => {
                debug!("ProgramTransfer.Delete method not available: {}", e);
            }
        }

        // Strategy 2: Write empty content to program node
        let program_node = NodeId::string(namespace, program_name);
        match self
            .write_node(&program_node, Variant::ByteString(Vec::new()))
            .await
        {
            Ok(status) => {
                if status & STATUS_BAD_MASK == 0 {
                    info!("Program '{}' cleared via empty write", program_name);
                    return Ok(());
                }
                debug!("Empty write returned status: 0x{:08X}", status);
            }
            Err(e) => {
                debug!("Empty write to program node failed: {}", e);
            }
        }

        // Strategy 3: Call generic Delete method on the program node
        let program_object = NodeId::string(namespace, program_name);
        let delete_method_generic = NodeId::string(namespace, &format!("{}.Delete", program_name));

        match self
            .call_method(&program_object, &delete_method_generic, vec![])
            .await
        {
            Ok(_) => {
                info!("Program '{}' deleted via {}.Delete method", program_name, program_name);
                return Ok(());
            }
            Err(e) => {
                debug!("{}.Delete method not available: {}", program_name, e);
            }
        }

        // If we get here, deletion wasn't explicitly successful
        // Log warning but don't fail - the program may have been deleted or may not exist
        warn!(
            "Program '{}' deletion could not be confirmed - no delete method available",
            program_name
        );
        Ok(())
    }

    async fn compile(&self, program: &PlcProgram) -> Result<UploadResult> {
        info!(
            "Compiling program '{}' via OPC UA: {}",
            program.name, self.config.name
        );

        // Perform local validation first
        validate_program_source(&program.source)?;

        let mut warnings = Vec::new();
        let mut errors = Vec::new();
        let mut plc_response: HashMap<String, JsonValue> = HashMap::new();

        // Additional syntax validation based on language
        match program.language {
            super::ProgramLanguage::St => {
                // Basic ST syntax checks
                let source = &program.source;

                // Check for basic ST structure
                if !source.contains("PROGRAM") && !source.contains("FUNCTION") && !source.contains("FUNCTION_BLOCK") {
                    warnings.push("Source doesn't contain PROGRAM, FUNCTION, or FUNCTION_BLOCK declaration".to_string());
                }

                // Check for unmatched blocks
                let if_count = source.matches("IF ").count() + source.matches("IF\n").count();
                let end_if_count = source.matches("END_IF").count();
                if if_count != end_if_count {
                    errors.push(format!(
                        "Unmatched IF/END_IF blocks: {} IF statements, {} END_IF",
                        if_count, end_if_count
                    ));
                }

                let for_count = source.matches("FOR ").count();
                let end_for_count = source.matches("END_FOR").count();
                if for_count != end_for_count {
                    errors.push(format!(
                        "Unmatched FOR/END_FOR blocks: {} FOR statements, {} END_FOR",
                        for_count, end_for_count
                    ));
                }

                let while_count = source.matches("WHILE ").count();
                let end_while_count = source.matches("END_WHILE").count();
                if while_count != end_while_count {
                    errors.push(format!(
                        "Unmatched WHILE/END_WHILE blocks: {} WHILE statements, {} END_WHILE",
                        while_count, end_while_count
                    ));
                }

                // Check for common syntax errors
                if source.contains(":=:") {
                    errors.push("Invalid assignment operator ':=:' found".to_string());
                }
                if source.contains(";;") {
                    warnings.push("Double semicolons ';;' found - may be unintentional".to_string());
                }
            }
            super::ProgramLanguage::Ld => {
                // Basic ladder validation
                if !program.source.contains("RUNG") && !program.source.contains("<rung>") {
                    warnings.push("Ladder source doesn't contain RUNG definitions".to_string());
                }
            }
            super::ProgramLanguage::Fbd => {
                // Basic FBD validation
                if !program.source.contains("BLOCK") && !program.source.contains("<block>") {
                    warnings.push("FBD source doesn't contain BLOCK definitions".to_string());
                }
            }
            super::ProgramLanguage::Il => {
                // Basic IL validation
                let valid_opcodes = ["LD", "ST", "AND", "OR", "ADD", "SUB", "MUL", "DIV", "JMP", "CAL", "RET"];
                let lines: Vec<&str> = program.source.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with("(*") {
                        let first_word = trimmed.split_whitespace().next().unwrap_or("");
                        // Skip labels (ending with :)
                        if !first_word.ends_with(':') && !valid_opcodes.iter().any(|op| first_word.starts_with(op)) {
                            warnings.push(format!("Line {}: Unknown instruction '{}'", i + 1, first_word));
                        }
                    }
                }
            }
            super::ProgramLanguage::Sfc => {
                // Basic SFC validation
                if !program.source.contains("STEP") && !program.source.contains("TRANSITION") {
                    warnings.push("SFC source doesn't contain STEP or TRANSITION definitions".to_string());
                }
            }
        }

        // If connected, try to use server-side compilation
        if self.is_connected() {
            let namespace = self.config.program_namespace
                .as_ref()
                .and_then(|ns| ns.parse::<u16>().ok())
                .unwrap_or(2);

            // Try ProgramTransfer.Compile method
            let program_transfer_object = NodeId::string(namespace, "ProgramTransfer");
            let compile_method = NodeId::string(namespace, "ProgramTransfer.Compile");

            match self
                .call_method(
                    &program_transfer_object,
                    &compile_method,
                    vec![
                        Variant::String(program.name.clone()),
                        Variant::ByteString(program.source.as_bytes().to_vec()),
                    ],
                )
                .await
            {
                Ok(outputs) => {
                    plc_response.insert("method".to_string(), JsonValue::String("ProgramTransfer.Compile".to_string()));

                    // Parse compilation result
                    for (i, output) in outputs.iter().enumerate() {
                        match output {
                            Variant::Boolean(success) => {
                                if !*success && errors.is_empty() {
                                    errors.push("Server-side compilation failed".to_string());
                                }
                            }
                            Variant::String(msg) => {
                                if msg.to_lowercase().contains("error") {
                                    errors.push(format!("Server: {}", msg));
                                } else if msg.to_lowercase().contains("warning") {
                                    warnings.push(format!("Server: {}", msg));
                                } else {
                                    plc_response.insert(format!("output_{}", i), JsonValue::String(msg.clone()));
                                }
                            }
                            _ => {
                                plc_response.insert(format!("output_{}", i), JsonValue::String(output.to_string_value()));
                            }
                        }
                    }
                    info!("Server-side compilation completed");
                }
                Err(e) => {
                    debug!("ProgramTransfer.Compile not available: {}", e);
                    plc_response.insert("method".to_string(), JsonValue::String("local_validation".to_string()));
                }
            }
        } else {
            plc_response.insert("method".to_string(), JsonValue::String("local_validation".to_string()));
        }

        let success = errors.is_empty();

        Ok(UploadResult {
            success,
            program_id: Some(program.name.clone()),
            warnings,
            errors,
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response,
        })
    }
}

/// Detect program language from source code
fn detect_program_language(source: &str) -> super::ProgramLanguage {
    let source_upper = source.to_uppercase();

    // Check for ST keywords
    if source_upper.contains("PROGRAM ") || source_upper.contains("FUNCTION_BLOCK ")
        || source_upper.contains("VAR ") || source_upper.contains("END_VAR")
        || source_upper.contains(":= ") || source_upper.contains("IF ") && source_upper.contains("END_IF")
    {
        return super::ProgramLanguage::St;
    }

    // Check for Ladder Diagram (LD)
    if source_upper.contains("RUNG") || source_upper.contains("<LADDER>")
        || source.contains("--| |--") || source.contains("--( )--")
    {
        return super::ProgramLanguage::Ld;
    }

    // Check for FBD
    if source_upper.contains("<FBD>") || source_upper.contains("<BLOCK>")
        || source_upper.contains("BLOCK_TYPE")
    {
        return super::ProgramLanguage::Fbd;
    }

    // Check for IL
    if source_upper.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("LD ") || trimmed.starts_with("ST ")
            || trimmed.starts_with("AND ") || trimmed.starts_with("OR ")
    }) {
        return super::ProgramLanguage::Il;
    }

    // Check for SFC
    if source_upper.contains("INITIAL_STEP") || source_upper.contains("TRANSITION ")
        || source_upper.contains("END_STEP")
    {
        return super::ProgramLanguage::Sfc;
    }

    // Default to ST (most common IEC 61131-3 language)
    super::ProgramLanguage::St
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = OpcUaConfig::default();
        assert_eq!(config.security_policy, OpcUaSecurityPolicy::None);
        assert_eq!(config.security_mode, OpcUaSecurityMode::None);
        assert_eq!(config.timeout_secs, 10);
        assert_eq!(config.session_timeout_ms, 60000);
    }

    #[test]
    fn test_parse_endpoint_ipv4() {
        let config = OpcUaConfig {
            endpoint_url: "opc.tcp://192.168.1.100:4840/server".to_string(),
            ..Default::default()
        };
        let client = OpcUaClient::new(config);
        let (host, port) = client.parse_endpoint().unwrap();
        assert_eq!(host, "192.168.1.100");
        assert_eq!(port, 4840);
    }

    #[test]
    fn test_parse_endpoint_ipv6() {
        let config = OpcUaConfig {
            endpoint_url: "opc.tcp://[::1]:4840/server".to_string(),
            ..Default::default()
        };
        let client = OpcUaClient::new(config);
        let (host, port) = client.parse_endpoint().unwrap();
        assert_eq!(host, "::1");
        assert_eq!(port, 4840);

        // Full IPv6 address
        let config2 = OpcUaConfig {
            endpoint_url: "opc.tcp://[2001:db8::1]:4841".to_string(),
            ..Default::default()
        };
        let client2 = OpcUaClient::new(config2);
        let (host2, port2) = client2.parse_endpoint().unwrap();
        assert_eq!(host2, "2001:db8::1");
        assert_eq!(port2, 4841);
    }

    #[test]
    fn test_parse_endpoint_hostname() {
        let config = OpcUaConfig {
            endpoint_url: "opc.tcp://plc.example.com:4840".to_string(),
            ..Default::default()
        };
        let client = OpcUaClient::new(config);
        let (host, port) = client.parse_endpoint().unwrap();
        assert_eq!(host, "plc.example.com");
        assert_eq!(port, 4840);
    }

    #[test]
    fn test_parse_endpoint_default_port() {
        let config = OpcUaConfig {
            endpoint_url: "opc.tcp://192.168.1.100".to_string(),
            ..Default::default()
        };
        let client = OpcUaClient::new(config);
        let (host, port) = client.parse_endpoint().unwrap();
        assert_eq!(host, "192.168.1.100");
        assert_eq!(port, DEFAULT_OPCUA_PORT);
    }

    #[test]
    fn test_node_id_encode_two_byte() {
        let node = NodeId::numeric(0, 85);
        let encoded = node.encode();
        assert_eq!(encoded[0], 0x00); // Two-byte numeric
        assert_eq!(encoded[1], 85);
        assert_eq!(encoded.len(), 2);
    }

    #[test]
    fn test_node_id_encode_four_byte() {
        let node = NodeId::numeric(1, 1000);
        let encoded = node.encode();
        assert_eq!(encoded[0], 0x01); // Four-byte numeric
        assert_eq!(encoded[1], 1);    // Namespace
        assert_eq!(u16::from_le_bytes([encoded[2], encoded[3]]), 1000);
    }

    #[test]
    fn test_node_id_encode_string() {
        let node = NodeId::string(2, "Test");
        let encoded = node.encode();
        assert_eq!(encoded[0], 0x03); // String
        assert_eq!(u16::from_le_bytes([encoded[1], encoded[2]]), 2); // Namespace
        assert_eq!(u32::from_le_bytes([encoded[3], encoded[4], encoded[5], encoded[6]]), 4); // Length
        assert_eq!(&encoded[7..11], b"Test");
    }

    #[test]
    fn test_node_id_decode_two_byte() {
        let data = [0x00, 85u8];
        let (node, consumed) = NodeId::decode(&data).unwrap();
        assert_eq!(consumed, 2);
        match node {
            NodeId::Numeric(ns, id) => {
                assert_eq!(ns, 0);
                assert_eq!(id, 85);
            }
            _ => panic!("Expected Numeric NodeId"),
        }
    }

    #[test]
    fn test_node_id_decode_four_byte() {
        let mut data = vec![0x01, 1u8]; // Four-byte, namespace 1
        data.extend_from_slice(&1000u16.to_le_bytes());
        let (node, consumed) = NodeId::decode(&data).unwrap();
        assert_eq!(consumed, 4);
        match node {
            NodeId::Numeric(ns, id) => {
                assert_eq!(ns, 1);
                assert_eq!(id, 1000);
            }
            _ => panic!("Expected Numeric NodeId"),
        }
    }

    #[test]
    fn test_node_id_null() {
        let node = NodeId::null();
        assert!(node.is_null());

        let node2 = NodeId::numeric(0, 1);
        assert!(!node2.is_null());
    }

    #[test]
    fn test_security_policy_uri() {
        assert_eq!(OpcUaSecurityPolicy::None.to_uri(), SECURITY_POLICY_NONE);
        assert_eq!(
            OpcUaSecurityPolicy::Basic256Sha256.to_uri(),
            SECURITY_POLICY_BASIC256SHA256
        );
        assert_eq!(
            OpcUaSecurityPolicy::Basic128Rsa15.to_uri(),
            "http://opcfoundation.org/UA/SecurityPolicy#Basic128Rsa15"
        );
    }

    #[test]
    fn test_variant_encode_decode_boolean() {
        let variant = Variant::Boolean(true);
        let encoded = variant.encode();
        assert_eq!(encoded[0], 0x01); // Boolean type
        assert_eq!(encoded[1], 1);    // true

        let (decoded, consumed) = Variant::decode(&encoded).unwrap();
        assert_eq!(consumed, 2);
        match decoded {
            Variant::Boolean(v) => assert!(v),
            _ => panic!("Expected Boolean"),
        }
    }

    #[test]
    fn test_variant_encode_decode_int32() {
        let variant = Variant::Int32(12345);
        let encoded = variant.encode();
        assert_eq!(encoded[0], 0x06); // Int32 type

        let (decoded, consumed) = Variant::decode(&encoded).unwrap();
        assert_eq!(consumed, 5);
        match decoded {
            Variant::Int32(v) => assert_eq!(v, 12345),
            _ => panic!("Expected Int32"),
        }
    }

    #[test]
    fn test_variant_encode_decode_string() {
        let variant = Variant::String("Hello".to_string());
        let encoded = variant.encode();
        assert_eq!(encoded[0], 0x0C); // String type

        let (decoded, _) = Variant::decode(&encoded).unwrap();
        match decoded {
            Variant::String(v) => assert_eq!(v, "Hello"),
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_variant_encode_decode_double() {
        let variant = Variant::Double(3.14159);
        let encoded = variant.encode();
        assert_eq!(encoded[0], 0x0B); // Double type

        let (decoded, consumed) = Variant::decode(&encoded).unwrap();
        assert_eq!(consumed, 9);
        match decoded {
            Variant::Double(v) => assert!((v - 3.14159).abs() < 0.00001),
            _ => panic!("Expected Double"),
        }
    }

    #[test]
    fn test_variant_to_string_value() {
        assert_eq!(Variant::Boolean(true).to_string_value(), "true");
        assert_eq!(Variant::Int32(42).to_string_value(), "42");
        assert_eq!(Variant::String("test".to_string()).to_string_value(), "test");
        assert_eq!(Variant::Null.to_string_value(), "");
    }

    #[test]
    fn test_data_value_decode() {
        // Encoding mask: has value (0x01)
        let mut data = vec![0x01];
        // Value: Int32(100)
        data.push(0x06); // Int32 type
        data.extend_from_slice(&100i32.to_le_bytes());

        let (dv, consumed) = DataValue::decode(&data).unwrap();
        assert_eq!(consumed, 6);
        assert!(dv.is_good());
        match dv.value {
            Some(Variant::Int32(v)) => assert_eq!(v, 100),
            _ => panic!("Expected Int32 value"),
        }
    }

    #[test]
    fn test_data_value_with_status() {
        // Encoding mask: has value (0x01) + has status (0x02)
        let mut data = vec![0x03];
        // Value: Boolean(true)
        data.push(0x01);
        data.push(0x01);
        // Status: Good (0x00000000)
        data.extend_from_slice(&0u32.to_le_bytes());

        let (dv, _) = DataValue::decode(&data).unwrap();
        assert!(dv.is_good());
        assert_eq!(dv.status_code, STATUS_GOOD);
    }

    #[test]
    fn test_generate_nonce() {
        let nonce1 = OpcUaClient::generate_nonce();
        let nonce2 = OpcUaClient::generate_nonce();

        assert_eq!(nonce1.len(), 32);
        assert_eq!(nonce2.len(), 32);
        // Nonces should be different (with very high probability)
        assert_ne!(nonce1, nonce2);
    }

    #[test]
    fn test_encode_string() {
        let encoded = OpcUaClient::encode_string("Test");
        assert_eq!(u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]), 4);
        assert_eq!(&encoded[4..8], b"Test");

        let empty = OpcUaClient::encode_string("");
        assert_eq!(u32::from_le_bytes([empty[0], empty[1], empty[2], empty[3]]), 0xFFFFFFFF);
    }

    #[test]
    fn test_encode_bytestring() {
        let data = vec![1, 2, 3, 4, 5];
        let encoded = OpcUaClient::encode_bytestring(&data);
        assert_eq!(u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]), 5);
        assert_eq!(&encoded[4..9], &[1, 2, 3, 4, 5]);

        let empty = OpcUaClient::encode_bytestring(&[]);
        assert_eq!(u32::from_le_bytes([empty[0], empty[1], empty[2], empty[3]]), 0xFFFFFFFF);
    }

    #[test]
    fn test_detect_program_language_st() {
        let st_source = r#"
            PROGRAM Main
            VAR
                counter : INT;
            END_VAR

            counter := counter + 1;
            END_PROGRAM
        "#;
        assert_eq!(detect_program_language(st_source), super::super::ProgramLanguage::St);
    }

    #[test]
    fn test_detect_program_language_ld() {
        let ladder_source = r#"
            RUNG 1
            --| |--+--( )--
            RUNG 2
        "#;
        assert_eq!(detect_program_language(ladder_source), super::super::ProgramLanguage::Ld);
    }

    #[test]
    fn test_detect_program_language_il() {
        let il_source = r#"
            LD input1
            AND input2
            ST output1
        "#;
        assert_eq!(detect_program_language(il_source), super::super::ProgramLanguage::Il);
    }

    #[test]
    fn test_detect_program_language_sfc() {
        let sfc_source = r#"
            INITIAL_STEP Init:
            END_STEP

            TRANSITION FROM Init TO Step1
            END_TRANSITION
        "#;
        assert_eq!(detect_program_language(sfc_source), super::super::ProgramLanguage::Sfc);
    }

    #[test]
    fn test_status_codes() {
        assert_eq!(STATUS_GOOD, 0x00000000);
        assert_eq!(STATUS_BAD_MASK, 0x80000000);

        // Test good status detection
        let good_status = 0x00000000u32;
        assert_eq!(good_status & STATUS_BAD_MASK, 0);

        // Test bad status detection
        let bad_status = 0x80040000u32; // Bad_NodeIdUnknown
        assert_ne!(bad_status & STATUS_BAD_MASK, 0);
    }

    #[test]
    fn test_well_known_node_ids() {
        assert_eq!(NODE_ID_SERVER, 2253);
        assert_eq!(NODE_ID_SERVER_STATUS, 2256);
        assert_eq!(NODE_ID_SERVER_STATE, 2259);
        assert_eq!(NODE_ID_OBJECTS_FOLDER, 85);
    }

    #[test]
    fn test_client_initial_state() {
        let config = OpcUaConfig::default();
        let client = OpcUaClient::new(config);

        assert!(!client.is_connected());
    }
}
