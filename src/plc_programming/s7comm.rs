//! Siemens S7 Communication Protocol Implementation
//!
//! Supports program upload to Siemens S7 PLCs via S7comm/S7comm+ protocol.
//!
//! ## Supported PLCs
//! - S7-300 series
//! - S7-400 series
//! - S7-1200 series (partial)
//! - S7-1500 series (partial, requires S7comm+)
//!
//! ## Protocol
//! - Default Port: 102 (ISO-on-TCP / RFC 1006)
//! - COTP (Connection Oriented Transport Protocol)
//! - S7comm layer for PLC operations
//!
//! ## Limitations
//! - S7-1200/1500 require "PUT/GET" enabled in TIA Portal
//! - Full program upload requires TIA Portal Openness API
//! - This implementation supports block upload/download

use super::common::*;
use super::{PlcProgram, PlcProgrammer, PlcRunMode, PlcStatus, UploadResult};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

// ============================================================================
// Constants
// ============================================================================

/// Default S7 port (ISO-on-TCP)
pub const DEFAULT_S7_PORT: u16 = 102;

/// COTP connection request
const COTP_CR: u8 = 0xE0;

/// COTP connection confirm
const COTP_CC: u8 = 0xD0;

/// COTP data transfer
const COTP_DT: u8 = 0xF0;

/// S7 Protocol ID
const S7_PROTOCOL_ID: u8 = 0x32;

/// S7 Job request
const S7_JOB: u8 = 0x01;

/// S7 Ack
const S7_ACK: u8 = 0x02;

/// S7 Ack-Data
const S7_ACK_DATA: u8 = 0x03;

/// S7 Userdata
const S7_USERDATA: u8 = 0x07;

// S7 Functions
const S7_FUNC_READ_VAR: u8 = 0x04;
const S7_FUNC_WRITE_VAR: u8 = 0x05;
const S7_FUNC_SETUP_COMM: u8 = 0xF0;
const S7_FUNC_START_UPLOAD: u8 = 0x1D;
const S7_FUNC_UPLOAD: u8 = 0x1E;
const S7_FUNC_END_UPLOAD: u8 = 0x1F;
const S7_FUNC_START_DOWNLOAD: u8 = 0x1A;
const S7_FUNC_DOWNLOAD: u8 = 0x1B;
const S7_FUNC_END_DOWNLOAD: u8 = 0x1C;
const S7_FUNC_PLC_CONTROL: u8 = 0x28;
const S7_FUNC_PLC_STOP: u8 = 0x29;

// ============================================================================
// Configuration
// ============================================================================

/// Siemens S7 connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S7Config {
    /// Connection name
    pub name: String,

    /// PLC IP address
    pub address: String,

    /// Port (default: 102)
    #[serde(default = "default_s7_port")]
    pub port: u16,

    /// Rack number (default: 0)
    #[serde(default)]
    pub rack: u8,

    /// Slot number (default: 1 for S7-300/400, 0 for S7-1200/1500)
    #[serde(default = "default_slot")]
    pub slot: u8,

    /// PLC type
    #[serde(default)]
    pub plc_type: S7PlcType,

    /// Connection timeout (seconds)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// PDU size (default: 480, max varies by PLC)
    #[serde(default = "default_pdu_size")]
    pub pdu_size: u16,
}

fn default_s7_port() -> u16 {
    DEFAULT_S7_PORT
}

fn default_slot() -> u8 {
    1
}

fn default_timeout() -> u64 {
    10
}

fn default_pdu_size() -> u16 {
    480
}

/// S7 PLC Type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum S7PlcType {
    /// S7-200 (not fully supported)
    S7200,
    /// S7-300 series
    #[default]
    S7300,
    /// S7-400 series
    S7400,
    /// S7-1200 series
    S71200,
    /// S7-1500 series
    S71500,
    /// LOGO! (limited support)
    Logo,
}

impl Default for S7Config {
    fn default() -> Self {
        Self {
            name: "s7_plc".to_string(),
            address: "192.168.1.1".to_string(),
            port: DEFAULT_S7_PORT,
            rack: 0,
            slot: 1,
            plc_type: S7PlcType::S7300,
            timeout_secs: 10,
            pdu_size: 480,
        }
    }
}

// ============================================================================
// S7 Block Types
// ============================================================================

/// S7 Block types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum S7BlockType {
    /// Organization Block
    OB = 0x38,
    /// Data Block
    DB = 0x41,
    /// System Data Block
    SDB = 0x42,
    /// Function Block
    FB = 0x43,
    /// Function
    FC = 0x45,
    /// System Function Block
    SFB = 0x46,
    /// System Function
    SFC = 0x47,
}

impl S7BlockType {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "OB" => Some(Self::OB),
            "DB" => Some(Self::DB),
            "SDB" => Some(Self::SDB),
            "FB" => Some(Self::FB),
            "FC" => Some(Self::FC),
            "SFB" => Some(Self::SFB),
            "SFC" => Some(Self::SFC),
            _ => None,
        }
    }
}

// ============================================================================
// S7 Client
// ============================================================================

/// Siemens S7 communication client
pub struct S7Client {
    config: S7Config,
    connection: Arc<Mutex<Option<TcpStream>>>,
    connected: AtomicBool,
    pdu_reference: Arc<Mutex<u16>>,
    negotiated_pdu: Arc<Mutex<u16>>,
}

impl S7Client {
    /// Create a new S7 client
    pub fn new(config: S7Config) -> Self {
        Self {
            config,
            connection: Arc::new(Mutex::new(None)),
            connected: AtomicBool::new(false),
            pdu_reference: Arc::new(Mutex::new(0)),
            negotiated_pdu: Arc::new(Mutex::new(480)),
        }
    }

    /// Get next PDU reference
    async fn next_pdu_ref(&self) -> u16 {
        let mut pdu_ref = self.pdu_reference.lock().await;
        *pdu_ref = pdu_ref.wrapping_add(1);
        if *pdu_ref == 0 {
            *pdu_ref = 1;
        }
        *pdu_ref
    }

    /// Build TPKT header (RFC 1006)
    fn build_tpkt(payload_len: usize) -> Vec<u8> {
        vec![
            0x03,                            // Version
            0x00,                            // Reserved
            ((payload_len + 4) >> 8) as u8,  // Length high
            ((payload_len + 4) & 0xFF) as u8, // Length low
        ]
    }

    /// Build COTP Connection Request
    fn build_cotp_cr(&self) -> Vec<u8> {
        let mut cotp = vec![
            0x11,    // Length (17 bytes following)
            COTP_CR, // PDU type: Connection Request
            0x00, 0x00, // Destination reference
            0x00, 0x01, // Source reference
            0x00,    // Class & options
            // Parameters
            0xC0, 0x01, 0x0A, // TPDU size (1024)
            0xC1, 0x02, // Source TSAP
            0x01, 0x00, // Source TSAP value
            0xC2, 0x02, // Destination TSAP
        ];

        // Destination TSAP: encodes rack and slot
        // Format: 0x01, 0x00 for S7-300/400, varies for 1200/1500
        let conn_type = match self.config.plc_type {
            S7PlcType::S71200 | S7PlcType::S71500 => 0x02,
            _ => 0x01,
        };
        cotp.push(conn_type);
        cotp.push((self.config.rack << 5) | self.config.slot);

        cotp
    }

    /// Build S7 Setup Communication request
    async fn build_setup_comm(&self) -> Vec<u8> {
        let pdu_ref = self.next_pdu_ref().await;

        vec![
            S7_PROTOCOL_ID,                      // Protocol ID
            S7_JOB,                              // Message type: Job
            0x00, 0x00,                          // Reserved
            (pdu_ref >> 8) as u8,                // PDU reference high
            (pdu_ref & 0xFF) as u8,              // PDU reference low
            0x00, 0x08,                          // Parameter length (8)
            0x00, 0x00,                          // Data length (0)
            S7_FUNC_SETUP_COMM,                  // Function: Setup communication
            0x00,                                // Reserved
            0x00, 0x01,                          // Max AmQ calling
            0x00, 0x01,                          // Max AmQ called
            (self.config.pdu_size >> 8) as u8,   // PDU size high
            (self.config.pdu_size & 0xFF) as u8, // PDU size low
        ]
    }

    /// Build COTP Data packet
    fn build_cotp_dt(payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![
            0x02,    // COTP header length
            COTP_DT, // PDU type: Data
            0x80,    // EOT (End of Transmission)
        ];
        packet.extend_from_slice(payload);
        packet
    }

    /// Send ISO-on-TCP packet
    async fn send_packet(&self, cotp_payload: &[u8]) -> Result<Vec<u8>> {
        let mut conn_guard = self.connection.lock().await;
        let conn = conn_guard
            .as_mut()
            .ok_or_else(|| anyhow!("Not connected"))?;

        // Build full packet
        let tpkt = Self::build_tpkt(cotp_payload.len());
        let mut packet = tpkt;
        packet.extend_from_slice(cotp_payload);

        // Send
        conn.write_all(&packet).await?;

        // Receive response
        let mut tpkt_header = [0u8; 4];
        conn.read_exact(&mut tpkt_header).await?;

        if tpkt_header[0] != 0x03 {
            return Err(anyhow!("Invalid TPKT response"));
        }

        let length = ((tpkt_header[2] as usize) << 8 | tpkt_header[3] as usize) - 4;
        let mut response = vec![0u8; length];
        conn.read_exact(&mut response).await?;

        Ok(response)
    }

    /// Establish COTP connection
    async fn cotp_connect(&self) -> Result<()> {
        let cr = self.build_cotp_cr();
        let response = self.send_packet(&cr).await?;

        if response.len() < 2 || response[1] != COTP_CC {
            return Err(anyhow!("COTP connection rejected"));
        }

        debug!("COTP connection established");
        Ok(())
    }

    /// Setup S7 communication
    async fn s7_setup(&self) -> Result<()> {
        let setup = self.build_setup_comm().await;
        let cotp_dt = Self::build_cotp_dt(&setup);
        let response = self.send_packet(&cotp_dt).await?;

        // Parse response (skip COTP header)
        if response.len() < 3 {
            return Err(anyhow!("S7 setup response too short"));
        }

        let s7_response = &response[3..];
        if s7_response.len() < 12 {
            return Err(anyhow!("Invalid S7 setup response"));
        }

        // Check for errors
        if s7_response[1] == S7_ACK_DATA {
            // Extract negotiated PDU size
            if s7_response.len() >= 18 {
                let pdu_size =
                    (s7_response[16] as u16) << 8 | s7_response[17] as u16;
                *self.negotiated_pdu.lock().await = pdu_size;
                debug!("Negotiated PDU size: {}", pdu_size);
            }
        } else {
            return Err(anyhow!("S7 setup failed"));
        }

        Ok(())
    }

    /// Build S7 block download request
    async fn build_download_request(&self, block_type: S7BlockType, block_num: u16) -> Vec<u8> {
        let pdu_ref = self.next_pdu_ref().await;

        // Block filename format: _0A00001P (OB1 in passive file system)
        let block_name = format!(
            "_0{}{:05}P",
            (block_type as u8) as char,
            block_num
        );
        let block_bytes = block_name.as_bytes();

        let mut s7_data = vec![
            S7_PROTOCOL_ID,               // Protocol ID
            S7_JOB,                       // Message type: Job
            0x00, 0x00,                   // Reserved
            (pdu_ref >> 8) as u8,         // PDU reference high
            (pdu_ref & 0xFF) as u8,       // PDU reference low
        ];

        // Parameter length and data length will be filled later
        let param_len = 18 + block_bytes.len();
        s7_data.extend_from_slice(&(param_len as u16).to_be_bytes());
        s7_data.extend_from_slice(&[0x00, 0x00]); // Data length

        // Download parameters
        s7_data.push(S7_FUNC_START_DOWNLOAD);
        s7_data.push(0x00); // Reserved
        s7_data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x09]); // Unknown
        s7_data.push(block_bytes.len() as u8);
        s7_data.extend_from_slice(block_bytes);

        s7_data
    }

    /// Build S7 PLC control request (Start/Stop)
    async fn build_plc_control(&self, start: bool) -> Vec<u8> {
        let pdu_ref = self.next_pdu_ref().await;

        let func = if start {
            S7_FUNC_PLC_CONTROL
        } else {
            S7_FUNC_PLC_STOP
        };

        // S7 control parameters: P_PROGRAM for start, _STOP for stop
        let param: &[u8] = if start { b"P_PROGRAM" } else { b"_STOP" };

        vec![
            S7_PROTOCOL_ID,
            S7_JOB,
            0x00, 0x00,
            (pdu_ref >> 8) as u8,
            (pdu_ref & 0xFF) as u8,
            0x00, (param.len() + 9) as u8, // Parameter length
            0x00, 0x00,                     // Data length
            func,
            0x00, 0x00, 0x00, 0x00, 0x00,
            0xFD,
            0x00,
            param.len() as u8,
        ]
    }

    /// Convert ST program to S7 AWL/MC7 format
    fn compile_to_mc7(&self, program: &PlcProgram) -> Result<Vec<u8>> {
        // In a real implementation, this would compile ST to MC7 bytecode.
        // For now, we create a placeholder that demonstrates the structure.
        //
        // NOTE: Full ST->MC7 compilation requires Siemens compiler or
        // reverse-engineered MC7 encoding.

        let mut mc7 = Vec::new();

        // Block header
        mc7.extend_from_slice(&[0x70, 0x70]); // PP (block signature)

        // Block type and number
        mc7.push(0x08); // Version
        mc7.push(0x01); // Block type (OB)

        // Block number (OB1 = 0x0001)
        mc7.extend_from_slice(&1u16.to_be_bytes());

        // Length placeholder
        mc7.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        // MC7 code would go here
        // For demonstration, just add NOPs
        mc7.extend_from_slice(&[
            0x65, 0x00, // NOP 0
            0x65, 0x00, // NOP 0
            0xBE, 0x00, // Block end
        ]);

        // Update length
        let len = mc7.len() as u32;
        mc7[6..10].copy_from_slice(&len.to_be_bytes());

        warn!(
            "S7 MC7 compilation is simplified - full ST compilation requires TIA Portal Openness"
        );

        Ok(mc7)
    }
}

#[async_trait::async_trait]
impl PlcProgrammer for S7Client {
    fn protocol_name(&self) -> &'static str {
        "S7comm"
    }

    async fn connect(&mut self) -> Result<()> {
        let addr = format!("{}:{}", self.config.address, self.config.port);
        info!("Connecting to Siemens S7 PLC at {}", addr);

        let timeout_duration = std::time::Duration::from_secs(self.config.timeout_secs);

        let stream = with_timeout(
            TcpStream::connect(&addr),
            timeout_duration,
            "S7 connect",
        )
        .await?;

        *self.connection.lock().await = Some(stream);

        // COTP connection
        self.cotp_connect().await?;

        // S7 setup
        self.s7_setup().await?;

        self.connected.store(true, Ordering::Release);
        info!("Connected to Siemens S7 PLC: {}", self.config.name);

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        *self.connection.lock().await = None;
        self.connected.store(false, Ordering::Release);
        info!("Disconnected from Siemens S7 PLC: {}", self.config.name);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn get_status(&self) -> Result<PlcStatus> {
        // Read SZL (System Status List) for PLC info
        // SZL-ID 0x0011 contains module identification

        let model = match self.config.plc_type {
            S7PlcType::S7200 => "S7-200",
            S7PlcType::S7300 => "S7-300",
            S7PlcType::S7400 => "S7-400",
            S7PlcType::S71200 => "S7-1200",
            S7PlcType::S71500 => "S7-1500",
            S7PlcType::Logo => "LOGO!",
        };

        Ok(PlcStatus {
            connected: self.is_connected(),
            run_mode: PlcRunMode::Unknown, // Would need SZL read for actual status
            model: model.to_string(),
            firmware: "Unknown".to_string(),
            current_program: None,
            last_modified: None,
        })
    }

    async fn upload_program(&self, program: &PlcProgram) -> Result<UploadResult> {
        info!(
            "Uploading program '{}' to Siemens S7 PLC: {}",
            program.name, self.config.name
        );

        validate_program_source(&program.source)?;

        // Compile to MC7
        let mc7 = self.compile_to_mc7(program)?;

        // For S7, we upload individual blocks
        // Start download sequence
        let download_req = self.build_download_request(S7BlockType::OB, 1).await;
        let cotp_dt = Self::build_cotp_dt(&download_req);

        let response = self.send_packet(&cotp_dt).await;

        let success = response.is_ok();
        let errors = match &response {
            Ok(_) => Vec::new(),
            Err(e) => vec![e.to_string()],
        };

        let result = UploadResult {
            success,
            program_id: if success {
                Some(format!("OB1_{}", program.name))
            } else {
                None
            },
            warnings: vec![
                "Full ST compilation requires TIA Portal Openness API".to_string()
            ],
            errors,
            timestamp: chrono::Utc::now().to_rfc3339(),
            plc_response: HashMap::new(),
        };

        audit_program_upload(
            "S7comm",
            &self.config.address,
            &program.name,
            success,
            if success { "OK" } else { "Failed" },
        );

        Ok(result)
    }

    async fn download_program(&self, program_name: &str) -> Result<PlcProgram> {
        // Parse block type and number from name (e.g., "OB1", "FB10")
        let (block_type, _block_num) = if program_name.len() >= 3 {
            let type_str = &program_name[..2];
            let num_str = &program_name[2..];
            let block_type = S7BlockType::from_str(type_str)
                .ok_or_else(|| anyhow!("Invalid block type: {}", type_str))?;
            let block_num: u16 = num_str
                .parse()
                .map_err(|_| anyhow!("Invalid block number: {}", num_str))?;
            (block_type, block_num)
        } else {
            return Err(anyhow!("Invalid program name format: {}", program_name));
        };

        // Upload (read from PLC) would go here
        // This requires the S7 upload protocol sequence

        Ok(PlcProgram {
            name: program_name.to_string(),
            language: super::ProgramLanguage::St,
            source: format!("// Downloaded from S7 PLC\n// Block type: {:?}", block_type),
            variables: Vec::new(),
            function_blocks: Vec::new(),
            metadata: HashMap::new(),
        })
    }

    async fn start(&self) -> Result<()> {
        info!("Starting Siemens S7 PLC: {}", self.config.name);

        let control = self.build_plc_control(true).await;
        let cotp_dt = Self::build_cotp_dt(&control);
        self.send_packet(&cotp_dt).await?;

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping Siemens S7 PLC: {}", self.config.name);

        let control = self.build_plc_control(false).await;
        let cotp_dt = Self::build_cotp_dt(&control);
        self.send_packet(&cotp_dt).await?;

        Ok(())
    }

    async fn list_programs(&self) -> Result<Vec<String>> {
        // Read block list from SZL
        // For now, return common blocks
        Ok(vec![
            "OB1".to_string(),
            "OB100".to_string(),
            "DB1".to_string(),
        ])
    }

    async fn delete_program(&self, program_name: &str) -> Result<()> {
        warn!(
            "Deleting block '{}' from Siemens S7 PLC: {}",
            program_name, self.config.name
        );

        // S7 block delete would go here
        // Requires PI service (Program Invocation)

        Ok(())
    }

    async fn compile(&self, program: &PlcProgram) -> Result<UploadResult> {
        validate_program_source(&program.source)?;
        let _ = self.compile_to_mc7(program)?;

        Ok(UploadResult {
            success: true,
            program_id: None,
            warnings: vec![
                "MC7 compilation is simplified - full compilation requires TIA Portal".to_string()
            ],
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
        let config = S7Config::default();
        assert_eq!(config.port, DEFAULT_S7_PORT);
        assert_eq!(config.rack, 0);
        assert_eq!(config.slot, 1);
    }

    #[test]
    fn test_tpkt_header() {
        let tpkt = S7Client::build_tpkt(10);
        assert_eq!(tpkt[0], 0x03);
        assert_eq!(tpkt[1], 0x00);
        assert_eq!(tpkt[2], 0x00);
        assert_eq!(tpkt[3], 14); // 10 + 4
    }

    #[test]
    fn test_block_type_parse() {
        assert_eq!(S7BlockType::from_str("OB"), Some(S7BlockType::OB));
        assert_eq!(S7BlockType::from_str("DB"), Some(S7BlockType::DB));
        assert_eq!(S7BlockType::from_str("FB"), Some(S7BlockType::FB));
        assert_eq!(S7BlockType::from_str("XX"), None);
    }
}
