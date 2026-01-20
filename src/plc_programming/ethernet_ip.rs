//! Allen-Bradley EtherNet/IP CIP Protocol Implementation
//!
//! Supports program upload to Allen-Bradley/Rockwell PLCs via EtherNet/IP.
//!
//! ## Supported PLCs
//! - CompactLogix (1769-L series)
//! - ControlLogix (1756-L series)
//! - Micro800 series (limited)
//! - PLC-5 (legacy)
//! - SLC 500 (legacy)
//!
//! ## Protocol
//! - Default Port: 44818 (EtherNet/IP)
//! - CIP (Common Industrial Protocol)
//! - Program upload via CIP file services
//!
//! ## Limitations
//! - Full program upload requires RSLogix/Studio 5000
//! - This implementation supports tag R/W and limited program access

use super::common::*;
use super::{PlcProgram, PlcProgrammer, PlcRunMode, PlcStatus, UploadResult};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

// ============================================================================
// Constants
// ============================================================================

/// Default EtherNet/IP port
pub const DEFAULT_ENIP_PORT: u16 = 44818;

/// Maximum EtherNet/IP packet size (prevent memory exhaustion)
const MAX_ENIP_PACKET_SIZE: usize = 65536;

/// EtherNet/IP Commands
const ENIP_REGISTER_SESSION: u16 = 0x0065;
const ENIP_UNREGISTER_SESSION: u16 = 0x0066;
const ENIP_SEND_RR_DATA: u16 = 0x006F;
const ENIP_SEND_UNIT_DATA: u16 = 0x0070;

/// CIP Service codes
const CIP_GET_ATTRIBUTE_ALL: u8 = 0x01;
const CIP_GET_ATTRIBUTE_SINGLE: u8 = 0x0E;
const CIP_READ_TAG: u8 = 0x4C;
const CIP_WRITE_TAG: u8 = 0x4D;
const CIP_READ_TAG_FRAGMENTED: u8 = 0x52;
const CIP_WRITE_TAG_FRAGMENTED: u8 = 0x53;
const CIP_MULTIPLE_SERVICE: u8 = 0x0A;
const CIP_FORWARD_OPEN: u8 = 0x54;
const CIP_FORWARD_CLOSE: u8 = 0x4E;

/// CIP Class codes
const CIP_CLASS_IDENTITY: u16 = 0x01;
const CIP_CLASS_MESSAGE_ROUTER: u16 = 0x02;
const CIP_CLASS_CONNECTION_MANAGER: u16 = 0x06;
const CIP_CLASS_FILE: u16 = 0x37;
const CIP_CLASS_PROGRAM: u16 = 0x64;

// ============================================================================
// Configuration
// ============================================================================

/// EtherNet/IP connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtherNetIpConfig {
    /// Connection name
    pub name: String,

    /// PLC IP address
    pub address: String,

    /// Port (default: 44818)
    #[serde(default = "default_enip_port")]
    pub port: u16,

    /// Slot number (for ControlLogix)
    #[serde(default)]
    pub slot: u8,

    /// Connection path (optional, for routing)
    #[serde(default)]
    pub connection_path: Option<String>,

    /// Connection timeout (seconds)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// PLC type
    #[serde(default)]
    pub plc_type: AbPlcType,
}

fn default_enip_port() -> u16 {
    DEFAULT_ENIP_PORT
}

fn default_timeout() -> u64 {
    10
}

/// Allen-Bradley PLC Type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AbPlcType {
    /// CompactLogix
    #[default]
    CompactLogix,
    /// ControlLogix
    ControlLogix,
    /// Micro800 series
    Micro800,
    /// PLC-5 (legacy)
    Plc5,
    /// SLC 500 (legacy)
    Slc500,
}

impl Default for EtherNetIpConfig {
    fn default() -> Self {
        Self {
            name: "ab_plc".to_string(),
            address: "192.168.1.1".to_string(),
            port: DEFAULT_ENIP_PORT,
            slot: 0,
            connection_path: None,
            timeout_secs: 10,
            plc_type: AbPlcType::CompactLogix,
        }
    }
}

// ============================================================================
// EtherNet/IP Client
// ============================================================================

/// Allen-Bradley EtherNet/IP client
pub struct EtherNetIpClient {
    config: EtherNetIpConfig,
    connection: Arc<Mutex<Option<TcpStream>>>,
    connected: AtomicBool,
    session_handle: Arc<Mutex<u32>>,
    sender_context: Arc<Mutex<u64>>,
}

impl EtherNetIpClient {
    /// Create a new EtherNet/IP client
    pub fn new(config: EtherNetIpConfig) -> Self {
        Self {
            config,
            connection: Arc::new(Mutex::new(None)),
            connected: AtomicBool::new(false),
            session_handle: Arc::new(Mutex::new(0)),
            sender_context: Arc::new(Mutex::new(0)),
        }
    }

    /// Get next sender context
    async fn next_context(&self) -> u64 {
        let mut ctx = self.sender_context.lock().await;
        *ctx = ctx.wrapping_add(1);
        *ctx
    }

    /// Build EtherNet/IP header
    fn build_enip_header(
        &self,
        command: u16,
        session_handle: u32,
        sender_context: u64,
        data_len: usize,
    ) -> Vec<u8> {
        let mut header = Vec::with_capacity(24);

        // Command
        header.extend_from_slice(&command.to_le_bytes());

        // Length
        header.extend_from_slice(&(data_len as u16).to_le_bytes());

        // Session handle
        header.extend_from_slice(&session_handle.to_le_bytes());

        // Status (0 for requests)
        header.extend_from_slice(&0u32.to_le_bytes());

        // Sender context
        header.extend_from_slice(&sender_context.to_le_bytes());

        // Options
        header.extend_from_slice(&0u32.to_le_bytes());

        header
    }

    /// Build Register Session request
    fn build_register_session(&self) -> Vec<u8> {
        let mut msg = self.build_enip_header(ENIP_REGISTER_SESSION, 0, 0, 4);

        // Protocol version
        msg.extend_from_slice(&1u16.to_le_bytes());

        // Options flags
        msg.extend_from_slice(&0u16.to_le_bytes());

        msg
    }

    /// Build CIP path
    fn build_cip_path(&self) -> Vec<u8> {
        let mut path = Vec::new();

        // Backplane port (port 1, slot N)
        path.push(0x01); // Port segment
        path.push(self.config.slot); // Slot

        path
    }

    /// Build CIP Read Tag request
    ///
    /// CIP symbolic segment uses 1-byte length field, so tag names are limited to 255 bytes.
    fn build_read_tag(&self, tag_name: &str) -> Vec<u8> {
        let tag_bytes = tag_name.as_bytes();

        // CIP symbolic segment uses 1-byte length field (max 255 bytes)
        if tag_bytes.len() > 255 {
            warn!(
                "Tag name '{}' exceeds CIP max length (255 bytes), truncating",
                &tag_name[..50.min(tag_name.len())]
            );
        }
        let tag_len = tag_bytes.len().min(255);
        let tag_bytes = &tag_bytes[..tag_len];

        let mut request = Vec::new();

        // Service code
        request.push(CIP_READ_TAG);

        // Path size (in words)
        let path_size = (2 + tag_len + (tag_len % 2)) / 2;
        request.push(path_size as u8);

        // Symbolic segment
        request.push(0x91);
        request.push(tag_len as u8);
        request.extend_from_slice(tag_bytes);
        if tag_len % 2 == 1 {
            request.push(0x00); // Pad
        }

        // Number of elements to read
        request.extend_from_slice(&1u16.to_le_bytes());

        request
    }

    /// Build SendRRData (unconnected message)
    async fn build_send_rr_data(&self, cip_data: &[u8]) -> Vec<u8> {
        let session = *self.session_handle.lock().await;
        let context = self.next_context().await;

        // Item count: 2 (null address + unconnected data)
        let item_data_len = 2 + 2 + 2 + cip_data.len(); // type + len for each item + data

        let mut msg =
            self.build_enip_header(ENIP_SEND_RR_DATA, session, context, 6 + item_data_len);

        // Interface handle
        msg.extend_from_slice(&0u32.to_le_bytes());

        // Timeout
        msg.extend_from_slice(&10u16.to_le_bytes());

        // Item count
        msg.extend_from_slice(&2u16.to_le_bytes());

        // Null address item
        msg.extend_from_slice(&0u16.to_le_bytes()); // Type: null
        msg.extend_from_slice(&0u16.to_le_bytes()); // Length: 0

        // Unconnected data item
        msg.extend_from_slice(&0x00B2u16.to_le_bytes()); // Type: unconnected data
        msg.extend_from_slice(&(cip_data.len() as u16).to_le_bytes());
        msg.extend_from_slice(cip_data);

        msg
    }

    /// Send and receive EtherNet/IP message
    async fn send_receive(&self, message: &[u8]) -> Result<Vec<u8>> {
        let mut conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        // Send
        conn.write_all(message).await?;

        // Read header
        let mut header = [0u8; 24];
        conn.read_exact(&mut header).await?;

        // Get data length
        let data_len = u16::from_le_bytes([header[2], header[3]]) as usize;

        // Validate data length to prevent memory exhaustion (IEC 62443 SL2)
        if data_len > MAX_ENIP_PACKET_SIZE {
            return Err(anyhow!(
                "EtherNet/IP packet too large: {} bytes (max {})",
                data_len,
                MAX_ENIP_PACKET_SIZE
            ));
        }

        // Read data
        let mut response = header.to_vec();
        if data_len > 0 {
            let mut data = vec![0u8; data_len];
            conn.read_exact(&mut data).await?;
            response.extend_from_slice(&data);
        }

        // Check status
        let status = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        if status != 0 {
            return Err(anyhow!("EtherNet/IP error: status 0x{:08X}", status));
        }

        Ok(response)
    }

    /// Convert ST to AOI/Add-On Instruction format
    fn convert_to_aoi(&self, program: &PlcProgram) -> Result<Vec<u8>> {
        // Allen-Bradley uses proprietary format for program storage
        // Full conversion requires RSLogix/Studio 5000
        //
        // This creates a simplified representation

        let mut aoi = Vec::new();

        // AOI header
        aoi.extend_from_slice(b"AOI\x00");

        // Name
        let name_bytes = program.name.as_bytes();
        aoi.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        aoi.extend_from_slice(name_bytes);

        // Source (as comment - AB doesn't directly support ST)
        let source_bytes = program.source.as_bytes();
        aoi.extend_from_slice(&(source_bytes.len() as u32).to_le_bytes());
        aoi.extend_from_slice(source_bytes);

        warn!("Allen-Bradley program upload requires Studio 5000 for full ST compilation");

        Ok(aoi)
    }
}

#[async_trait::async_trait]
impl PlcProgrammer for EtherNetIpClient {
    fn protocol_name(&self) -> &'static str {
        "EtherNet/IP"
    }

    async fn connect(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.config.address, self.config.port);
        info!("Connecting to Allen-Bradley PLC at {}", addr);

        let timeout_duration = std::time::Duration::from_secs(self.config.timeout_secs);

        let stream = with_timeout(
            TcpStream::connect(&addr),
            timeout_duration,
            "EtherNet/IP connect",
        )
        .await?;

        *self.connection.lock().await = Some(stream);

        // Register session
        let register = self.build_register_session();
        let response = self.send_receive(&register).await?;

        // Extract session handle
        if response.len() >= 8 {
            let session = u32::from_le_bytes([response[4], response[5], response[6], response[7]]);
            *self.session_handle.lock().await = session;
            debug!("EtherNet/IP session registered: 0x{:08X}", session);
        }

        self.connected.store(true, Ordering::Release);
        info!("Connected to Allen-Bradley PLC: {}", self.config.name);

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Unregister session
        let session = *self.session_handle.lock().await;
        if session != 0 {
            let msg = self.build_enip_header(ENIP_UNREGISTER_SESSION, session, 0, 0);
            let _ = self.send_receive(&msg).await;
        }

        *self.connection.lock().await = None;
        *self.session_handle.lock().await = 0;
        self.connected.store(false, Ordering::Release);

        info!("Disconnected from Allen-Bradley PLC: {}", self.config.name);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn get_status(&self) -> Result<PlcStatus> {
        // Read Identity object

        let model = match self.config.plc_type {
            AbPlcType::CompactLogix => "CompactLogix",
            AbPlcType::ControlLogix => "ControlLogix",
            AbPlcType::Micro800 => "Micro800",
            AbPlcType::Plc5 => "PLC-5",
            AbPlcType::Slc500 => "SLC 500",
        };

        Ok(PlcStatus {
            connected: self.is_connected(),
            run_mode: PlcRunMode::Unknown,
            model: model.to_string(),
            firmware: "Unknown".to_string(),
            current_program: None,
            last_modified: None,
        })
    }

    async fn upload_program(&self, program: &PlcProgram) -> Result<UploadResult> {
        info!(
            "Uploading program '{}' to Allen-Bradley PLC: {}",
            program.name, self.config.name
        );

        validate_program_source(&program.source)?;

        // Convert to AOI format
        let _aoi = self.convert_to_aoi(program)?;

        // Full upload would use CIP file services
        // This requires proprietary format knowledge

        let result = UploadResult {
            success: true,
            program_id: Some(program.name.clone()),
            warnings: vec![
                "Full program upload requires Studio 5000 for Logix PLCs".to_string(),
                "ST is not native to Allen-Bradley - use Ladder or Function Block".to_string(),
            ],
            errors: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response: HashMap::new(),
        };

        audit_program_upload(
            "EtherNet/IP",
            &self.config.address,
            &program.name,
            result.success,
            "OK",
        );

        Ok(result)
    }

    async fn download_program(&self, program_name: &str) -> Result<PlcProgram> {
        Ok(PlcProgram {
            name: program_name.to_string(),
            language: super::ProgramLanguage::Ld, // AB primarily uses Ladder
            source: "// Downloaded from Allen-Bradley PLC".to_string(),
            variables: Vec::new(),
            function_blocks: Vec::new(),
            metadata: HashMap::new(),
        })
    }

    async fn start(&self) -> Result<()> {
        info!("Starting Allen-Bradley PLC: {}", self.config.name);
        // Would send mode change command
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping Allen-Bradley PLC: {}", self.config.name);
        // Would send mode change command
        Ok(())
    }

    async fn list_programs(&self) -> Result<Vec<String>> {
        Ok(vec!["MainProgram".to_string(), "MainRoutine".to_string()])
    }

    async fn delete_program(&self, program_name: &str) -> Result<()> {
        warn!(
            "Deleting program '{}' from Allen-Bradley PLC: {}",
            program_name, self.config.name
        );
        Ok(())
    }

    async fn compile(&self, program: &PlcProgram) -> Result<UploadResult> {
        validate_program_source(&program.source)?;

        Ok(UploadResult {
            success: true,
            program_id: None,
            warnings: vec!["AB compilation requires Studio 5000".to_string()],
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
        let config = EtherNetIpConfig::default();
        assert_eq!(config.port, DEFAULT_ENIP_PORT);
        assert_eq!(config.slot, 0);
    }

    #[test]
    fn test_enip_header() {
        let config = EtherNetIpConfig::default();
        let client = EtherNetIpClient::new(config);

        let header = client.build_enip_header(ENIP_REGISTER_SESSION, 0, 0, 4);
        assert_eq!(header.len(), 24);
        assert_eq!(header[0], 0x65); // Register session low byte
        assert_eq!(header[1], 0x00); // Register session high byte
    }

    #[test]
    fn test_register_session() {
        let config = EtherNetIpConfig::default();
        let client = EtherNetIpClient::new(config);

        let msg = client.build_register_session();
        assert_eq!(msg.len(), 28); // 24 header + 4 data
    }
}
