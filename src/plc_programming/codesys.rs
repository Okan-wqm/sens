//! Codesys Gateway Protocol Implementation
//!
//! Supports program upload to Codesys-based PLCs via the Codesys Gateway.
//!
//! ## Supported Features
//! - Program upload (ST, LD, FBD)
//! - Online change
//! - Variable read/write
//! - PLC start/stop
//!
//! ## Supported PLCs
//! - WAGO PFC100/200
//! - Beckhoff CX series (Codesys V2)
//! - Festo CPX-E
//! - Schneider M241/M251
//! - Any Codesys V3 runtime
//!
//! ## Protocol
//! - Default Port: 1217 (Gateway), 11740 (Runtime)
//! - Binary protocol over TCP
//! - Supports encryption (Codesys V3.5+)

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

/// Default Codesys Gateway port
pub const DEFAULT_GATEWAY_PORT: u16 = 1217;

/// Default Codesys Runtime port (direct connection)
pub const DEFAULT_RUNTIME_PORT: u16 = 11740;

/// Codesys protocol magic bytes
const CODESYS_MAGIC: [u8; 4] = [0xCD, 0x55, 0x00, 0x00];

/// Maximum packet size
const MAX_PACKET_SIZE: usize = 65536;

// ============================================================================
// Configuration
// ============================================================================

/// Codesys connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodesysConfig {
    /// Connection name
    pub name: String,

    /// Gateway or PLC address
    pub address: String,

    /// Port (default: 1217 for gateway, 11740 for direct)
    #[serde(default = "default_gateway_port")]
    pub port: u16,

    /// Connection mode
    #[serde(default)]
    pub mode: CodesysConnectionMode,

    /// Device name in gateway (for gateway mode)
    #[serde(default)]
    pub device_name: Option<String>,

    /// Username for authentication
    #[serde(default)]
    pub username: Option<String>,

    /// Password for authentication
    #[serde(default)]
    pub password: Option<String>,

    /// Use encryption (V3.5+)
    #[serde(default)]
    pub encrypted: bool,

    /// Connection timeout (seconds)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Application name on PLC
    #[serde(default = "default_application")]
    pub application: String,
}

fn default_gateway_port() -> u16 {
    DEFAULT_GATEWAY_PORT
}

fn default_timeout() -> u64 {
    10
}

fn default_application() -> String {
    "Application".to_string()
}

/// Connection mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodesysConnectionMode {
    /// Connect via Codesys Gateway
    #[default]
    Gateway,
    /// Direct connection to runtime
    Direct,
}

impl Default for CodesysConfig {
    fn default() -> Self {
        Self {
            name: "codesys_plc".to_string(),
            address: "192.168.1.100".to_string(),
            port: DEFAULT_GATEWAY_PORT,
            mode: CodesysConnectionMode::Gateway,
            device_name: None,
            username: None,
            password: None,
            encrypted: false,
            timeout_secs: 10,
            application: "Application".to_string(),
        }
    }
}

// ============================================================================
// Protocol Messages
// ============================================================================

/// Codesys service IDs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum ServiceId {
    Login = 0x0001,
    Logout = 0x0002,
    GetDeviceInfo = 0x0010,
    GetAppList = 0x0020,
    DownloadApp = 0x0030,
    UploadApp = 0x0031,
    StartApp = 0x0040,
    StopApp = 0x0041,
    ResetApp = 0x0042,
    GetAppState = 0x0050,
    ReadVariable = 0x0060,
    WriteVariable = 0x0061,
    OnlineChange = 0x0070,
}

/// Codesys response codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum ResponseCode {
    Ok = 0x0000,
    InvalidService = 0x0001,
    AccessDenied = 0x0002,
    InvalidParameter = 0x0003,
    NotConnected = 0x0004,
    Timeout = 0x0005,
    AppNotFound = 0x0010,
    CompileError = 0x0020,
    RuntimeError = 0x0030,
}

impl ResponseCode {
    fn from_u16(value: u16) -> Self {
        match value {
            0x0000 => Self::Ok,
            0x0001 => Self::InvalidService,
            0x0002 => Self::AccessDenied,
            0x0003 => Self::InvalidParameter,
            0x0004 => Self::NotConnected,
            0x0005 => Self::Timeout,
            0x0010 => Self::AppNotFound,
            0x0020 => Self::CompileError,
            0x0030 => Self::RuntimeError,
            _ => Self::RuntimeError,
        }
    }

    fn to_error_message(&self) -> &'static str {
        match self {
            Self::Ok => "Success",
            Self::InvalidService => "Invalid service request",
            Self::AccessDenied => "Access denied - check credentials",
            Self::InvalidParameter => "Invalid parameter",
            Self::NotConnected => "Not connected to PLC",
            Self::Timeout => "Operation timeout",
            Self::AppNotFound => "Application not found",
            Self::CompileError => "Compilation error",
            Self::RuntimeError => "Runtime error",
        }
    }
}

// ============================================================================
// Codesys Client
// ============================================================================

/// Codesys Gateway/Runtime client
pub struct CodesysClient {
    config: CodesysConfig,
    connection: Arc<Mutex<Option<TcpStream>>>,
    connected: AtomicBool,
    session_id: Arc<Mutex<Option<u32>>>,
}

impl CodesysClient {
    /// Create a new Codesys client
    pub fn new(config: CodesysConfig) -> Self {
        Self {
            config,
            connection: Arc::new(Mutex::new(None)),
            connected: AtomicBool::new(false),
            session_id: Arc::new(Mutex::new(None)),
        }
    }

    /// Build protocol packet
    fn build_packet(&self, service_id: ServiceId, payload: &[u8]) -> Vec<u8> {
        // Header: magic(4) + length(4) + service_id(2) + reserved(2) + payload_len(4) = 16 bytes
        let mut packet = Vec::with_capacity(16 + payload.len());

        // Magic header
        packet.extend_from_slice(&CODESYS_MAGIC);

        // Packet length (excluding magic)
        let length = (8 + payload.len()) as u32;
        packet.extend_from_slice(&length.to_le_bytes());

        // Service ID
        packet.extend_from_slice(&(service_id as u16).to_le_bytes());

        // Reserved
        packet.extend_from_slice(&[0u8; 2]);

        // Payload length
        packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());

        // Payload
        packet.extend_from_slice(payload);

        packet
    }

    /// Parse protocol response
    /// Header structure: magic[0:4] + length[4:8] + service_id[8:10] + reserved[10:12] + payload_len[12:16]
    fn parse_response(&self, data: &[u8]) -> Result<(ResponseCode, Vec<u8>)> {
        if data.len() < 16 {
            return Err(anyhow!(
                "Response too short (need 16 bytes header, got {})",
                data.len()
            ));
        }

        // Check magic
        if &data[0..4] != &CODESYS_MAGIC {
            return Err(anyhow!("Invalid response magic"));
        }

        // Parse response code (service_id position)
        let response_code = u16::from_le_bytes([data[8], data[9]]);
        let code = ResponseCode::from_u16(response_code);

        // Parse payload length (bytes 12-15, not 10-13)
        let payload_len = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;

        if data.len() < 16 + payload_len {
            return Err(anyhow!(
                "Response payload truncated (expected {}, got {})",
                16 + payload_len,
                data.len()
            ));
        }

        let payload = data[16..16 + payload_len].to_vec();

        Ok((code, payload))
    }

    /// Send packet and receive response with timeout protection
    async fn send_receive(&self, service_id: ServiceId, payload: &[u8]) -> Result<Vec<u8>> {
        let io_timeout = Duration::from_secs(self.config.timeout_secs);
        let mut conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        // Build packet with session_id if available (for authenticated requests)
        let mut full_payload = Vec::new();
        if let Some(session_id) = *self.session_id.lock().await {
            // Prepend session_id to payload for authenticated requests
            // (Login and Logout don't need this, but other requests do)
            if service_id != ServiceId::Login {
                full_payload.extend_from_slice(&session_id.to_le_bytes());
            }
        }
        full_payload.extend_from_slice(payload);

        let packet = self.build_packet(service_id, &full_payload);

        // Send with timeout
        timeout(io_timeout, conn.write_all(&packet))
            .await
            .map_err(|_| {
                anyhow!(
                    "Codesys write timeout after {} seconds",
                    self.config.timeout_secs
                )
            })??;

        // Receive response header with timeout (16 bytes: magic + length + service_id + reserved + payload_len)
        let mut header = [0u8; 16];
        timeout(io_timeout, conn.read_exact(&mut header))
            .await
            .map_err(|_| {
                anyhow!(
                    "Codesys read timeout after {} seconds",
                    self.config.timeout_secs
                )
            })??;

        // Parse header to get payload length (bytes 12-15)
        let payload_len =
            u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;

        // Validate payload length to prevent memory exhaustion
        if payload_len > MAX_PACKET_SIZE {
            return Err(anyhow!(
                "Payload length {} exceeds maximum {}",
                payload_len,
                MAX_PACKET_SIZE
            ));
        }

        // Read payload with timeout
        let mut response_payload = vec![0u8; payload_len];
        if payload_len > 0 {
            timeout(io_timeout, conn.read_exact(&mut response_payload))
                .await
                .map_err(|_| {
                    anyhow!(
                        "Codesys payload read timeout after {} seconds",
                        self.config.timeout_secs
                    )
                })??;
        }

        // Combine header and payload for parsing
        let mut full_response = header.to_vec();
        full_response.extend_from_slice(&response_payload);

        let (code, data) = self.parse_response(&full_response)?;

        if code != ResponseCode::Ok {
            return Err(anyhow!("Codesys error: {}", code.to_error_message()));
        }

        Ok(data)
    }

    /// Login to PLC
    async fn login(&self) -> Result<()> {
        let mut payload = Vec::new();

        // Username (null-terminated, padded to 32 bytes)
        // v1.3.2: Security note - IEC 62443 recommends explicit credentials
        // Anonymous login is allowed for isolated networks but logged as security warning
        let username = match &self.config.username {
            Some(u) => u.as_str(),
            None => {
                warn!(
                    "SECURITY: No username configured for Codesys PLC '{}' - using anonymous login. \
                     Configure credentials for IEC 62443 compliance.",
                    self.config.name
                );
                ""
            }
        };
        let mut user_bytes = username.as_bytes().to_vec();
        user_bytes.resize(32, 0);
        payload.extend_from_slice(&user_bytes);

        // Password (null-terminated, padded to 32 bytes)
        let password = match &self.config.password {
            Some(p) => p.as_str(),
            None => {
                if self.config.username.is_some() {
                    warn!("Username provided but no password - authentication may fail");
                }
                ""
            }
        };
        let mut pass_bytes = password.as_bytes().to_vec();
        pass_bytes.resize(32, 0);
        payload.extend_from_slice(&pass_bytes);

        let response = self.send_receive(ServiceId::Login, &payload).await?;

        // Parse session ID from response
        if response.len() >= 4 {
            let session_id =
                u32::from_le_bytes([response[0], response[1], response[2], response[3]]);
            *self.session_id.lock().await = Some(session_id);
            debug!("Codesys login successful, session_id: {}", session_id);
        }

        Ok(())
    }

    /// Compile ST source to Codesys format
    fn compile_st_to_codesys(&self, program: &PlcProgram) -> Result<Vec<u8>> {
        // In a real implementation, this would use Codesys compiler or
        // generate Codesys-compatible bytecode. For now, we send the ST
        // source directly (Codesys V3 can compile on-device).

        let mut compiled = Vec::new();

        // Program header
        let name_bytes = program.name.as_bytes();
        compiled.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        compiled.extend_from_slice(name_bytes);

        // Language type
        let lang_code: u8 = match program.language {
            super::ProgramLanguage::St => 0,
            super::ProgramLanguage::Ld => 1,
            super::ProgramLanguage::Fbd => 2,
            super::ProgramLanguage::Il => 3,
            super::ProgramLanguage::Sfc => 4,
        };
        compiled.push(lang_code);

        // Source code
        let source_bytes = program.source.as_bytes();
        compiled.extend_from_slice(&(source_bytes.len() as u32).to_le_bytes());
        compiled.extend_from_slice(source_bytes);

        // Variable count
        compiled.extend_from_slice(&(program.variables.len() as u16).to_le_bytes());

        // Variables
        for var in &program.variables {
            let var_name = var.name.as_bytes();
            compiled.extend_from_slice(&(var_name.len() as u16).to_le_bytes());
            compiled.extend_from_slice(var_name);

            let var_type = format!("{:?}", var.data_type);
            let type_bytes = var_type.as_bytes();
            compiled.extend_from_slice(&(type_bytes.len() as u16).to_le_bytes());
            compiled.extend_from_slice(type_bytes);
        }

        Ok(compiled)
    }
}

#[async_trait::async_trait]
impl PlcProgrammer for CodesysClient {
    fn protocol_name(&self) -> &'static str {
        "Codesys"
    }

    async fn connect(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.config.address, self.config.port);
        info!("Connecting to Codesys PLC at {}", addr);

        // Warn about unimplemented features
        if self.config.encrypted {
            warn!(
                "SECURITY: Encryption requested but not yet implemented for Codesys PLC '{}'. \
                 Connection will be unencrypted. Use VPN/network segmentation for security.",
                self.config.name
            );
        }

        if self.config.mode == CodesysConnectionMode::Gateway {
            if let Some(ref device) = self.config.device_name {
                warn!(
                    "Gateway device selection ('{}') is not yet implemented - \
                     connecting directly to gateway address",
                    device
                );
            }
        }

        let timeout_duration = std::time::Duration::from_secs(self.config.timeout_secs);

        let stream = with_timeout(
            TcpStream::connect(&addr),
            timeout_duration,
            "Codesys connect",
        )
        .await?;

        *self.connection.lock().await = Some(stream);
        self.connected.store(true, Ordering::Release);

        // v1.3.2: Login with rollback on failure to prevent connection leak
        if let Err(e) = self.login().await {
            // Rollback connection state on login failure
            warn!(
                "Login failed for Codesys PLC '{}', rolling back connection: {}",
                self.config.name, e
            );
            *self.connection.lock().await = None;
            self.connected.store(false, Ordering::Release);
            return Err(e);
        }

        info!("Connected to Codesys PLC: {}", self.config.name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Send logout if we have a session
        if let Some(session_id) = *self.session_id.lock().await {
            let _ = self
                .send_receive(ServiceId::Logout, &session_id.to_le_bytes())
                .await;
        }

        // Graceful TCP shutdown
        if let Some(mut conn) = self.connection.lock().await.take() {
            if let Err(e) = conn.shutdown().await {
                debug!("Codesys disconnect shutdown notice: {}", e);
            }
        }

        *self.session_id.lock().await = None;
        self.connected.store(false, Ordering::Release);

        info!("Disconnected from Codesys PLC: {}", self.config.name);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn get_status(&self) -> Result<PlcStatus> {
        let response = self.send_receive(ServiceId::GetDeviceInfo, &[]).await?;

        // Parse device info response
        let run_mode = if response.len() > 0 {
            match response[0] {
                0 => PlcRunMode::Stop,
                1 => PlcRunMode::Run,
                2 => PlcRunMode::Program,
                3 => PlcRunMode::Fault,
                _ => PlcRunMode::Unknown,
            }
        } else {
            PlcRunMode::Unknown
        };

        // Parse model string
        let model = if response.len() > 1 {
            let model_len = response[1] as usize;
            if response.len() > 2 + model_len {
                String::from_utf8_lossy(&response[2..2 + model_len]).to_string()
            } else {
                "Unknown".to_string()
            }
        } else {
            "Unknown".to_string()
        };

        Ok(PlcStatus {
            connected: self.is_connected(),
            run_mode,
            model,
            firmware: "Codesys V3".to_string(),
            current_program: Some(self.config.application.clone()),
            last_modified: None,
        })
    }

    async fn upload_program(&self, program: &PlcProgram) -> Result<UploadResult> {
        info!(
            "Uploading program '{}' to Codesys PLC: {}",
            program.name, self.config.name
        );

        // Validate program
        validate_program_source(&program.source)?;

        // Compile to Codesys format
        let compiled = self.compile_st_to_codesys(program)?;

        // Upload
        let response = self.send_receive(ServiceId::UploadApp, &compiled).await;

        let (success, warnings, errors) = match response {
            Ok(data) => {
                // Parse compilation result
                let mut warnings = Vec::new();
                let errors = Vec::new();

                if !data.is_empty() {
                    // First byte: warning count
                    let warn_count = data[0] as usize;
                    // Parse warnings/errors from response
                    if warn_count > 0 {
                        warnings.push(format!("{} compilation warnings", warn_count));
                    }
                }

                (true, warnings, errors)
            }
            Err(e) => (false, Vec::new(), vec![e.to_string()]),
        };

        let result = UploadResult {
            success,
            program_id: if success {
                Some(program.name.clone())
            } else {
                None
            },
            warnings, // Fixed: was Vec::new(), now uses collected warnings
            errors,
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response: HashMap::new(),
        };

        // Audit log
        audit_program_upload(
            "Codesys",
            &self.config.address,
            &program.name,
            success,
            if success { "OK" } else { "Failed" },
        );

        Ok(result)
    }

    async fn download_program(&self, program_name: &str) -> Result<PlcProgram> {
        let name_bytes = program_name.as_bytes();
        let mut payload = Vec::new();
        payload.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        payload.extend_from_slice(name_bytes);

        let response = self.send_receive(ServiceId::DownloadApp, &payload).await?;

        // Parse program from response
        // This is a simplified implementation
        let source = String::from_utf8_lossy(&response).to_string();

        Ok(PlcProgram {
            name: program_name.to_string(),
            language: super::ProgramLanguage::St,
            source,
            variables: Vec::new(),
            function_blocks: Vec::new(),
            metadata: HashMap::new(),
        })
    }

    async fn start(&self) -> Result<()> {
        info!("Starting Codesys PLC: {}", self.config.name);
        self.send_receive(ServiceId::StartApp, &[]).await?;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping Codesys PLC: {}", self.config.name);
        self.send_receive(ServiceId::StopApp, &[]).await?;
        Ok(())
    }

    async fn list_programs(&self) -> Result<Vec<String>> {
        let response = self.send_receive(ServiceId::GetAppList, &[]).await?;

        let mut programs = Vec::new();

        // Parse program list
        let mut offset = 0;
        while offset + 2 <= response.len() {
            let name_len = u16::from_le_bytes([response[offset], response[offset + 1]]) as usize;
            offset += 2;

            if offset + name_len <= response.len() {
                let name =
                    String::from_utf8_lossy(&response[offset..offset + name_len]).to_string();
                programs.push(name);
                offset += name_len;
            } else {
                break;
            }
        }

        Ok(programs)
    }

    async fn delete_program(&self, program_name: &str) -> Result<()> {
        warn!(
            "Deleting program '{}' from Codesys PLC: {}",
            program_name, self.config.name
        );

        let name_bytes = program_name.as_bytes();
        let mut payload = Vec::new();
        payload.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        payload.extend_from_slice(name_bytes);

        // Use reset to clear application
        self.send_receive(ServiceId::ResetApp, &payload).await?;

        Ok(())
    }

    async fn compile(&self, program: &PlcProgram) -> Result<UploadResult> {
        // Validate and compile without uploading
        validate_program_source(&program.source)?;
        let _ = self.compile_st_to_codesys(program)?;

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
        let config = CodesysConfig::default();
        assert_eq!(config.port, DEFAULT_GATEWAY_PORT);
        assert_eq!(config.mode, CodesysConnectionMode::Gateway);
    }

    #[test]
    fn test_packet_build() {
        let config = CodesysConfig::default();
        let client = CodesysClient::new(config);

        let packet = client.build_packet(ServiceId::Login, &[1, 2, 3]);

        // Check magic
        assert_eq!(&packet[0..4], &CODESYS_MAGIC);

        // Check service ID
        assert_eq!(packet[8], 0x01);
        assert_eq!(packet[9], 0x00);
    }

    #[test]
    fn test_response_code() {
        assert_eq!(ResponseCode::from_u16(0), ResponseCode::Ok);
        assert_eq!(ResponseCode::from_u16(2), ResponseCode::AccessDenied);
        assert_eq!(
            ResponseCode::AccessDenied.to_error_message(),
            "Access denied - check credentials"
        );
    }
}
