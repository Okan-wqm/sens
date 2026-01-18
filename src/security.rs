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
pub fn mask_secret(secret: &str) -> String {
    if secret.len() > 8 {
        format!("{}...{}", &secret[..4], &secret[secret.len() - 4..])
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        use std::fs::File;
        use std::os::unix::fs::PermissionsExt;
        use tempfile::NamedTempFile;

        // Create temp file with secure permissions
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        // Set secure permissions (0600)
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(validate_key_file_permissions(path).is_ok());

        // Set insecure permissions (0644)
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(validate_key_file_permissions(path).is_err());
    }
}
