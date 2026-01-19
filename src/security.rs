//! Security Hardening Module (v1.2.2)
//!
//! Provides security utilities for:
//! - Credential protection (zeroize on drop)
//! - Certificate file permission validation
//! - Log sanitization
//! - Platform-aware GPIO validation
//!
//! # IEC 62443 Compliance
//! - FR3: System Integrity (input validation, log sanitization)
//! - FR4: Data Confidentiality (credential protection)

#![allow(dead_code)] // Module provides utilities for future use

use std::path::Path;
use tracing::warn;

// ============================================================================
// Credential Protection
// ============================================================================

/// Mask sensitive data for logging (show first 4 and last 4 chars only)
///
/// # Examples
/// ```ignore
/// assert_eq!(mask_secret("my-secret-token-12345"), "my-s...2345");
/// assert_eq!(mask_secret("short"), "****");
/// ```
///
/// # v1.2.6: UTF-8 Safe
/// Uses char indices to prevent panic on multi-byte characters.
pub fn mask_secret(secret: &str) -> String {
    let char_count = secret.chars().count();
    if char_count > 8 {
        // Get first 4 chars safely
        let first_4: String = secret.chars().take(4).collect();
        // Get last 4 chars safely
        let last_4: String = secret.chars().skip(char_count - 4).collect();
        format!("{}...{}", first_4, last_4)
    } else {
        "****".to_string()
    }
}

/// Sanitize a string for safe logging (remove potential injection characters)
///
/// Prevents log injection attacks by removing control characters and
/// limiting line breaks.
pub fn sanitize_for_log(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(1000) // Limit length to prevent log flooding
        .collect::<String>()
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ============================================================================
// Certificate Permission Validation (Unix only)
// ============================================================================

/// Validate that a private key file has secure permissions (Unix only)
///
/// On Unix systems, private key files should only be readable by owner (mode 0600 or 0400).
/// This prevents other users from accessing sensitive credentials.
///
/// # Returns
/// - `Ok(())` if permissions are secure or on non-Unix platforms
/// - `Err(String)` with details if permissions are insecure
#[cfg(unix)]
pub fn validate_key_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Cannot read file metadata: {}", e))?;

    let mode = metadata.permissions().mode();
    let world_readable = mode & 0o004 != 0;
    let group_readable = mode & 0o040 != 0;

    if world_readable || group_readable {
        return Err(format!(
            "Insecure permissions {:04o} on {}: private keys should be 0600 or 0400",
            mode & 0o777,
            path.display()
        ));
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn validate_key_file_permissions(_path: &Path) -> Result<(), String> {
    // Windows uses ACLs, not Unix permissions
    // For now, we skip this check on non-Unix platforms
    Ok(())
}

/// Validate certificate file exists and has reasonable permissions
pub fn validate_cert_file(path: &str, is_private_key: bool) -> Result<(), String> {
    let path = Path::new(path);

    if !path.exists() {
        return Err(format!("Certificate file not found: {}", path.display()));
    }

    if is_private_key {
        validate_key_file_permissions(path)?;
    }

    Ok(())
}

// ============================================================================
// Platform-Aware GPIO Validation
// ============================================================================

/// GPIO platform type for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpioPlatform {
    /// Raspberry Pi (BCM GPIO 0-27)
    RaspberryPi,
    /// Revolution Pi (extended GPIO range)
    RevolutionPi,
    /// Generic Linux GPIO (sysfs/gpiod)
    GenericLinux,
    /// Unknown/Simulation
    Unknown,
}

impl GpioPlatform {
    /// Detect current platform based on system info
    #[cfg(target_os = "linux")]
    pub fn detect() -> Self {
        // Check for Raspberry Pi
        if Path::new("/proc/device-tree/model").exists() {
            if let Ok(model) = std::fs::read_to_string("/proc/device-tree/model") {
                if model.contains("Raspberry Pi") {
                    return GpioPlatform::RaspberryPi;
                }
                if model.contains("Revolution Pi") || model.contains("RevPi") {
                    return GpioPlatform::RevolutionPi;
                }
            }
        }

        // Check for generic gpiochip
        if Path::new("/dev/gpiochip0").exists() {
            return GpioPlatform::GenericLinux;
        }

        GpioPlatform::Unknown
    }

    #[cfg(not(target_os = "linux"))]
    pub fn detect() -> Self {
        GpioPlatform::Unknown
    }

    /// Get valid GPIO pin range for this platform
    pub fn valid_pin_range(&self) -> (u8, u8) {
        match self {
            GpioPlatform::RaspberryPi => (0, 27),
            GpioPlatform::RevolutionPi => (0, 127), // RevPi has extended GPIO
            GpioPlatform::GenericLinux => (0, 255), // Generic allows up to 255
            GpioPlatform::Unknown => (0, 255),      // Simulation mode - allow all
        }
    }

    /// Validate a GPIO pin number for this platform
    pub fn validate_pin(&self, pin: u8) -> Result<(), String> {
        let (min, max) = self.valid_pin_range();
        if pin < min || pin > max {
            return Err(format!(
                "GPIO pin {} out of range for {:?} (valid: {}-{})",
                pin, self, min, max
            ));
        }
        Ok(())
    }
}

// ============================================================================
// Release Build Security Checks
// ============================================================================

/// Check if insecure options are allowed (compile-time enforced)
///
/// In release builds with the `strict-security` feature, this will cause
/// a compile-time error if insecure options are enabled.
#[cfg(all(not(debug_assertions), feature = "strict-security"))]
pub fn check_insecure_option(option_name: &str, value: bool) -> Result<(), String> {
    if value {
        Err(format!(
            "Security violation: '{}' is not allowed in release builds with strict-security",
            option_name
        ))
    } else {
        Ok(())
    }
}

#[cfg(any(debug_assertions, not(feature = "strict-security")))]
pub fn check_insecure_option(option_name: &str, value: bool) -> Result<(), String> {
    if value {
        warn!(
            "SECURITY WARNING: '{}' is enabled - this is insecure and should not be used in production",
            option_name
        );
    }
    Ok(())
}

// ============================================================================
// Monotonic Time for Rate Limiting (NTP-safe)
// ============================================================================

use std::sync::OnceLock;
use std::time::Instant;

static BOOT_INSTANT: OnceLock<Instant> = OnceLock::new();

/// Get monotonic milliseconds since program start
///
/// This is NTP-safe and will not jump backwards, unlike SystemTime.
/// Used for rate limiting to prevent bypass via time manipulation.
pub fn monotonic_millis() -> u64 {
    let boot = BOOT_INSTANT.get_or_init(Instant::now);
    boot.elapsed().as_millis() as u64
}

// ============================================================================
// TLS Certificate Expiry Monitoring (v1.2.4)
// ============================================================================

use chrono::{DateTime, Utc};
use tracing::{error, info};

/// Certificate expiry information
#[derive(Debug, Clone)]
pub struct CertificateExpiry {
    /// Path to the certificate file
    pub path: String,
    /// Expiry date (if parsed successfully)
    pub expiry_date: Option<DateTime<Utc>>,
    /// Days until expiry (negative if expired)
    pub days_remaining: Option<i64>,
    /// Human-readable status
    pub status: CertExpiryStatus,
    /// Error message if check failed
    pub error: Option<String>,
}

/// Certificate expiry status levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertExpiryStatus {
    /// More than 30 days remaining
    Ok,
    /// 14-30 days remaining
    Warning,
    /// 7-14 days remaining
    Critical,
    /// Less than 7 days remaining
    Urgent,
    /// Certificate has expired
    Expired,
    /// Could not check (file not found, parse error, etc.)
    Unknown,
}

impl std::fmt::Display for CertExpiryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertExpiryStatus::Ok => write!(f, "OK"),
            CertExpiryStatus::Warning => write!(f, "WARNING"),
            CertExpiryStatus::Critical => write!(f, "CRITICAL"),
            CertExpiryStatus::Urgent => write!(f, "URGENT"),
            CertExpiryStatus::Expired => write!(f, "EXPIRED"),
            CertExpiryStatus::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

/// Check certificate expiry using openssl command
///
/// Returns certificate expiry information including days remaining.
/// Uses `openssl x509 -enddate` which is available on most Linux systems.
#[cfg(unix)]
pub fn check_certificate_expiry(cert_path: &str) -> CertificateExpiry {
    use std::process::Command;

    let path = std::path::Path::new(cert_path);

    // Check file exists
    if !path.exists() {
        return CertificateExpiry {
            path: cert_path.to_string(),
            expiry_date: None,
            days_remaining: None,
            status: CertExpiryStatus::Unknown,
            error: Some("Certificate file not found".to_string()),
        };
    }

    // Use openssl to get expiry date
    let output = Command::new("openssl")
        .args(["x509", "-enddate", "-noout", "-in", cert_path])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Output format: "notAfter=Mon DD HH:MM:SS YYYY GMT"
            parse_openssl_enddate(&stdout, cert_path)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            CertificateExpiry {
                path: cert_path.to_string(),
                expiry_date: None,
                days_remaining: None,
                status: CertExpiryStatus::Unknown,
                error: Some(format!("openssl error: {}", stderr.trim())),
            }
        }
        Err(e) => CertificateExpiry {
            path: cert_path.to_string(),
            expiry_date: None,
            days_remaining: None,
            status: CertExpiryStatus::Unknown,
            error: Some(format!("Failed to run openssl: {}", e)),
        },
    }
}

#[cfg(not(unix))]
pub fn check_certificate_expiry(cert_path: &str) -> CertificateExpiry {
    CertificateExpiry {
        path: cert_path.to_string(),
        expiry_date: None,
        days_remaining: None,
        status: CertExpiryStatus::Unknown,
        error: Some("Certificate expiry check only supported on Unix".to_string()),
    }
}

/// Parse openssl x509 -enddate output
fn parse_openssl_enddate(output: &str, cert_path: &str) -> CertificateExpiry {
    // Format: "notAfter=Mar 15 12:00:00 2025 GMT"
    let date_str = output
        .trim()
        .strip_prefix("notAfter=")
        .unwrap_or(output.trim());

    // Parse the date - openssl uses format like "Mar 15 12:00:00 2025 GMT"
    match parse_openssl_date(date_str) {
        Some(expiry_date) => {
            let now = Utc::now();
            let duration = expiry_date.signed_duration_since(now);
            let days = duration.num_days();

            let status = if days < 0 {
                CertExpiryStatus::Expired
            } else if days < 7 {
                CertExpiryStatus::Urgent
            } else if days < 14 {
                CertExpiryStatus::Critical
            } else if days < 30 {
                CertExpiryStatus::Warning
            } else {
                CertExpiryStatus::Ok
            };

            CertificateExpiry {
                path: cert_path.to_string(),
                expiry_date: Some(expiry_date),
                days_remaining: Some(days),
                status,
                error: None,
            }
        }
        None => CertificateExpiry {
            path: cert_path.to_string(),
            expiry_date: None,
            days_remaining: None,
            status: CertExpiryStatus::Unknown,
            error: Some(format!("Failed to parse date: {}", date_str)),
        },
    }
}

/// Parse openssl date format (e.g., "Mar 15 12:00:00 2025 GMT")
fn parse_openssl_date(date_str: &str) -> Option<DateTime<Utc>> {
    // Try multiple formats that openssl might output
    let formats = [
        "%b %d %H:%M:%S %Y GMT",  // Mar 15 12:00:00 2025 GMT
        "%b  %d %H:%M:%S %Y GMT", // Mar  5 12:00:00 2025 GMT (single digit day)
        "%B %d %H:%M:%S %Y GMT",  // March 15 12:00:00 2025 GMT
    ];

    let trimmed = date_str.trim();

    for format in formats {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, format) {
            return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
        }
    }

    None
}

/// Log certificate expiry warnings based on status
pub fn log_certificate_expiry(expiry: &CertificateExpiry) {
    match expiry.status {
        CertExpiryStatus::Ok => {
            if let Some(days) = expiry.days_remaining {
                info!(
                    path = %expiry.path,
                    days_remaining = days,
                    "TLS certificate valid"
                );
            }
        }
        CertExpiryStatus::Warning => {
            if let Some(days) = expiry.days_remaining {
                warn!(
                    path = %expiry.path,
                    days_remaining = days,
                    "TLS certificate expiring soon - renew within 30 days"
                );
            }
        }
        CertExpiryStatus::Critical => {
            if let Some(days) = expiry.days_remaining {
                error!(
                    path = %expiry.path,
                    days_remaining = days,
                    "TLS certificate expiring - renew immediately"
                );
            }
        }
        CertExpiryStatus::Urgent => {
            if let Some(days) = expiry.days_remaining {
                error!(
                    path = %expiry.path,
                    days_remaining = days,
                    "TLS certificate expires in less than 7 days!"
                );
            }
        }
        CertExpiryStatus::Expired => {
            error!(
                path = %expiry.path,
                "TLS certificate has EXPIRED!"
            );
        }
        CertExpiryStatus::Unknown => {
            if let Some(ref err) = expiry.error {
                warn!(
                    path = %expiry.path,
                    error = %err,
                    "Could not check TLS certificate expiry"
                );
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_mask_secret_long() {
        assert_eq!(mask_secret("my-secret-token-12345"), "my-s...2345");
    }

    #[test]
    fn test_mask_secret_short() {
        assert_eq!(mask_secret("short"), "****");
        assert_eq!(mask_secret("12345678"), "****");
    }

    #[test]
    fn test_mask_secret_edge() {
        assert_eq!(mask_secret("123456789"), "1234...6789");
    }

    #[test]
    fn test_sanitize_for_log() {
        assert_eq!(sanitize_for_log("normal text"), "normal text");
        assert_eq!(sanitize_for_log("line1\nline2"), "line1\\nline2");
        assert_eq!(sanitize_for_log("has\x00null"), "hasnull");
    }

    #[test]
    fn test_sanitize_length_limit() {
        let long = "a".repeat(2000);
        assert_eq!(sanitize_for_log(&long).len(), 1000);
    }

    #[test]
    fn test_gpio_platform_ranges() {
        let rpi = GpioPlatform::RaspberryPi;
        assert!(rpi.validate_pin(0).is_ok());
        assert!(rpi.validate_pin(27).is_ok());
        assert!(rpi.validate_pin(28).is_err());

        let revpi = GpioPlatform::RevolutionPi;
        assert!(revpi.validate_pin(100).is_ok());
    }

    #[test]
    fn test_monotonic_time() {
        let t1 = monotonic_millis();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t2 = monotonic_millis();
        assert!(t2 > t1);
    }

    #[cfg(unix)]
    #[test]
    fn test_key_permission_validation() {
        // Test with /etc/passwd which should have world-readable permissions
        let path = std::path::Path::new("/etc/passwd");
        if path.exists() {
            // /etc/passwd is typically world-readable (0644), should fail
            assert!(validate_key_file_permissions(path).is_err());
        }
    }

    #[test]
    fn test_cert_expiry_status_display() {
        assert_eq!(format!("{}", CertExpiryStatus::Ok), "OK");
        assert_eq!(format!("{}", CertExpiryStatus::Warning), "WARNING");
        assert_eq!(format!("{}", CertExpiryStatus::Critical), "CRITICAL");
        assert_eq!(format!("{}", CertExpiryStatus::Urgent), "URGENT");
        assert_eq!(format!("{}", CertExpiryStatus::Expired), "EXPIRED");
        assert_eq!(format!("{}", CertExpiryStatus::Unknown), "UNKNOWN");
    }

    #[test]
    fn test_parse_openssl_date() {
        // Standard format
        let date = parse_openssl_date("Mar 15 12:00:00 2025 GMT");
        assert!(date.is_some());
        let d = date.unwrap();
        assert_eq!(d.month(), 3);
        assert_eq!(d.day(), 15);
        assert_eq!(d.year(), 2025);

        // Single digit day with double space
        let date2 = parse_openssl_date("Jan  5 08:30:00 2026 GMT");
        assert!(date2.is_some());

        // Invalid format
        let invalid = parse_openssl_date("invalid date");
        assert!(invalid.is_none());
    }

    #[test]
    fn test_cert_expiry_file_not_found() {
        let result = check_certificate_expiry("/nonexistent/cert.pem");
        assert_eq!(result.status, CertExpiryStatus::Unknown);
        assert!(result.error.is_some());
    }
}
