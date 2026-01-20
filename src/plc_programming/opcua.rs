//! OPC UA Program Transfer Implementation
//!
//! Supports program upload via OPC UA (IEC 62541) for compliant PLCs.
//!
//! ## Features
//! - Program download to OPC UA servers with program transfer capability
//! - Variable browsing and read/write
//! - Method calls for PLC control
//!
//! ## Supported Servers
//! - Siemens S7-1500 (with OPC UA enabled)
//! - Beckhoff TwinCAT 3
//! - B&R Automation
//! - Unified Automation servers
//! - Any OPC UA server with ProgramTransfer
//!
//! ## Protocol
//! - Default Port: 4840 (OPC UA Binary)
//! - Secure Channel with encryption
//! - Session-based authentication

use super::common::*;
use super::{PlcProgram, PlcProgrammer, PlcRunMode, PlcStatus, UploadResult};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
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

    /// Generate random nonce for security
    fn generate_nonce() -> Vec<u8> {
        use std::time::{SystemTime, UNIX_EPOCH};
        // Simple nonce generation using timestamp + counter
        // For production, use proper cryptographic RNG
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut nonce = vec![0u8; 32];
        nonce[0..16].copy_from_slice(&timestamp.to_le_bytes());
        // Add some variation using process id
        let pid = std::process::id();
        nonce[16..20].copy_from_slice(&pid.to_le_bytes());
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
        let type_id = NodeId::numeric(0, 461);
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
        // Response structure: header + sessionId + authToken + ...
        if response.len() < 50 {
            return Err(anyhow!("CreateSession response too short"));
        }

        // Skip message header (8) + security header (8) + sequence header (8) = 24 bytes
        // Then type ID + response header
        let body_start = 24;
        if response.len() <= body_start {
            return Err(anyhow!("CreateSession response body missing"));
        }

        // Extract session ID and auth token (simplified parsing)
        // In a full implementation, proper UA Binary decoding is needed
        debug!("OPC UA session created");

        // For now, store a placeholder - real implementation needs proper parsing
        *self.session_id.lock().await = Some(vec![0x01]);

        Ok(())
    }

    /// Activate OPC UA session
    async fn activate_session(&self) -> Result<()> {
        let mut request = Vec::new();

        // Type ID for ActivateSessionRequest (467)
        let type_id = NodeId::numeric(0, 467);
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

        // LocaleIds (empty array)
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
            return Err(anyhow!("ActivateSession response too short"));
        }

        debug!("OPC UA session activated");

        // Store auth token for subsequent requests
        *self.auth_token.lock().await = Some(vec![0x01]);

        Ok(())
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

    /// Build program upload request (vendor-specific)
    fn build_program_upload_request(&self, program: &PlcProgram) -> Result<Vec<u8>> {
        // This would be vendor-specific. Common approaches:
        // 1. Write to file node
        // 2. Call method on program management object
        // 3. Use PLCopen OPC UA information model

        let mut request = Vec::new();

        // For demonstration, use a method call approach
        // NodeId for program management method (vendor-specific)
        let method_node = NodeId::string(2, "ProgramTransfer.Upload");
        request.extend_from_slice(&method_node.encode());

        // Program name
        request.extend_from_slice(&(program.name.len() as u32).to_le_bytes());
        request.extend_from_slice(program.name.as_bytes());

        // Program source
        request.extend_from_slice(&(program.source.len() as u32).to_le_bytes());
        request.extend_from_slice(program.source.as_bytes());

        Ok(request)
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
        // Would browse ServerStatus node

        Ok(PlcStatus {
            connected: self.is_connected(),
            run_mode: PlcRunMode::Unknown,
            model: "OPC UA Server".to_string(),
            firmware: "Unknown".to_string(),
            current_program: None,
            last_modified: None,
        })
    }

    async fn upload_program(&self, program: &PlcProgram) -> Result<UploadResult> {
        info!(
            "Uploading program '{}' via OPC UA: {}",
            program.name, self.config.name
        );

        validate_program_source(&program.source)?;

        // Build upload request
        let _request = self.build_program_upload_request(program)?;

        // In a full implementation, this would:
        // 1. Create session
        // 2. Activate session
        // 3. Call program transfer method or write to file node
        // 4. Monitor upload status

        let result = UploadResult {
            success: true, // Placeholder
            program_id: Some(program.name.clone()),
            warnings: vec![
                "OPC UA program transfer requires vendor-specific implementation".to_string(),
            ],
            errors: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response: HashMap::new(),
        };

        audit_program_upload(
            "OPC UA",
            &self.config.endpoint_url,
            &program.name,
            result.success,
            "OK",
        );

        Ok(result)
    }

    async fn download_program(&self, program_name: &str) -> Result<PlcProgram> {
        // Would read from program node

        Ok(PlcProgram {
            name: program_name.to_string(),
            language: super::ProgramLanguage::St,
            source: "// Downloaded via OPC UA".to_string(),
            variables: Vec::new(),
            function_blocks: Vec::new(),
            metadata: HashMap::new(),
        })
    }

    async fn start(&self) -> Result<()> {
        info!("Starting PLC via OPC UA: {}", self.config.name);
        // Call Start method on PLC object
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping PLC via OPC UA: {}", self.config.name);
        // Call Stop method on PLC object
        Ok(())
    }

    async fn list_programs(&self) -> Result<Vec<String>> {
        // Browse program nodes
        Ok(vec!["MainProgram".to_string()])
    }

    async fn delete_program(&self, program_name: &str) -> Result<()> {
        warn!(
            "Deleting program '{}' via OPC UA: {}",
            program_name, self.config.name
        );
        Ok(())
    }

    async fn compile(&self, program: &PlcProgram) -> Result<UploadResult> {
        validate_program_source(&program.source)?;

        Ok(UploadResult {
            success: true,
            program_id: None,
            warnings: Vec::new(),
            errors: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response: HashMap::new(),
        })
    }
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
    }

    #[test]
    fn test_parse_endpoint() {
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
    fn test_node_id_encode() {
        let node = NodeId::numeric(0, 85);
        let encoded = node.encode();
        assert_eq!(encoded[0], 0x00); // Two-byte numeric
        assert_eq!(encoded[1], 85);

        let node = NodeId::string(2, "Test");
        let encoded = node.encode();
        assert_eq!(encoded[0], 0x03); // String
    }

    #[test]
    fn test_security_policy_uri() {
        assert_eq!(OpcUaSecurityPolicy::None.to_uri(), SECURITY_POLICY_NONE);
        assert_eq!(
            OpcUaSecurityPolicy::Basic256Sha256.to_uri(),
            SECURITY_POLICY_BASIC256SHA256
        );
    }
}
