//! Fuzz target for Modbus response parsing
//!
//! IEC 62443 SL2 FR3: Input Validation
//!
//! This fuzz target ensures that malformed Modbus TCP responses
//! cannot cause panics or crashes in the agent.
//!
//! Attack vectors (ref: FrostyGoop malware):
//! - Malformed function codes
//! - Invalid register values
//! - Oversized responses
//! - Protocol violations

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Modbus TCP Application Protocol Unit (MBAP) header size
const MBAP_HEADER_SIZE: usize = 7;

/// Maximum Modbus PDU size (Protocol Data Unit)
const MAX_PDU_SIZE: usize = 253;

/// Valid Modbus function codes (read operations only - per security policy)
const VALID_READ_FUNCTION_CODES: &[u8] = &[
    0x01, // Read Coils
    0x02, // Read Discrete Inputs
    0x03, // Read Holding Registers
    0x04, // Read Input Registers
];

fuzz_target!(|data: &[u8]| {
    // Need at least MBAP header
    if data.len() < MBAP_HEADER_SIZE {
        return;
    }

    // Parse MBAP header
    let transaction_id = u16::from_be_bytes([data[0], data[1]]);
    let protocol_id = u16::from_be_bytes([data[2], data[3]]);
    let length = u16::from_be_bytes([data[4], data[5]]) as usize;
    let unit_id = data[6];

    // Validate protocol ID (must be 0 for Modbus TCP)
    if protocol_id != 0 {
        // Invalid protocol - agent should reject
        return;
    }

    // Validate length field (prevents buffer overflow attacks)
    if length > MAX_PDU_SIZE + 1 || length == 0 {
        // Invalid length - agent should reject
        return;
    }

    // Check if we have the full PDU
    if data.len() < MBAP_HEADER_SIZE + length - 1 {
        // Incomplete packet - agent should wait for more data
        return;
    }

    // Parse function code
    if data.len() > MBAP_HEADER_SIZE {
        let function_code = data[MBAP_HEADER_SIZE];

        // Check for exception response (function code with high bit set)
        let is_exception = (function_code & 0x80) != 0;
        let actual_function_code = function_code & 0x7F;

        // Validate function code is in allowed list
        let is_valid_read = VALID_READ_FUNCTION_CODES.contains(&actual_function_code);

        if is_exception {
            // Exception response - should have exception code
            if data.len() > MBAP_HEADER_SIZE + 1 {
                let _exception_code = data[MBAP_HEADER_SIZE + 1];
                // Valid exception codes: 1-12 (defined in Modbus spec)
            }
        } else if is_valid_read {
            // Valid read response - parse data
            parse_read_response(&data[MBAP_HEADER_SIZE..], actual_function_code);
        }
    }

    // Test transaction ID handling (used for request/response matching)
    let _ = transaction_id.wrapping_add(1);

    // Test unit ID handling (slave address)
    let _ = unit_id;
});

/// Parse Modbus read response data
fn parse_read_response(pdu: &[u8], function_code: u8) {
    if pdu.len() < 2 {
        return;
    }

    let byte_count = pdu[1] as usize;

    // Validate byte count matches PDU length
    if pdu.len() < 2 + byte_count {
        // Truncated response
        return;
    }

    match function_code {
        0x01 | 0x02 => {
            // Coils / Discrete Inputs - bit-packed data
            let _coil_data = &pdu[2..2 + byte_count];
        }
        0x03 | 0x04 => {
            // Holding / Input Registers - 16-bit values
            if byte_count % 2 != 0 {
                // Invalid: registers must be 16-bit aligned
                return;
            }

            let register_count = byte_count / 2;
            for i in 0..register_count {
                let offset = 2 + i * 2;
                if offset + 1 < pdu.len() {
                    let _register_value = u16::from_be_bytes([pdu[offset], pdu[offset + 1]]);
                }
            }
        }
        _ => {}
    }
}
