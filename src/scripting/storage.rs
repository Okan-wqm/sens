//! Script storage and persistence
//!
//! Handles saving, loading, and managing scripts on the edge device.
//!
//! ## Memory Safety
//! This module enforces a maximum script count to prevent unbounded memory growth.
//! Scripts exceeding the limit will be rejected with an error.
//!
//! ## Thread Safety (v1.2.0)
//! Uses `tokio::sync::RwLock` for concurrent access from multiple async tasks.
//! All public methods are now async to support lock acquisition.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::ScriptDefinition;

/// Default scripts directory
const DEFAULT_SCRIPTS_DIR: &str = "/etc/suderra/scripts";

/// Maximum allowed script ID length
const MAX_SCRIPT_ID_LENGTH: usize = 64;

/// Default maximum number of scripts (memory protection)
const DEFAULT_MAX_SCRIPTS: usize = 100;

/// Validate script ID to prevent path traversal attacks
///
/// # Security
/// This function prevents:
/// - Path traversal attacks (../, /, \)
/// - Overly long IDs that could cause filesystem issues
/// - Special characters that could be problematic
fn validate_script_id(id: &str) -> Result<()> {
    // Check for path traversal attempts
    if id.contains("..") {
        return Err(anyhow!("Invalid script ID: path traversal detected (..)"));
    }
    if id.contains('/') || id.contains('\\') {
        return Err(anyhow!("Invalid script ID: path separators not allowed"));
    }

    // Check length
    if id.is_empty() {
        return Err(anyhow!("Invalid script ID: cannot be empty"));
    }
    if id.len() > MAX_SCRIPT_ID_LENGTH {
        return Err(anyhow!(
            "Invalid script ID: exceeds maximum length of {} characters",
            MAX_SCRIPT_ID_LENGTH
        ));
    }

    // Only allow alphanumeric, hyphen, and underscore
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow!(
            "Invalid script ID: only alphanumeric, hyphen, and underscore allowed"
        ));
    }

    // Prevent hidden files
    if id.starts_with('.') {
        return Err(anyhow!("Invalid script ID: cannot start with dot"));
    }

    Ok(())
}

/// Script metadata and state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    /// Script definition
    pub definition: ScriptDefinition,

    /// Current status
    pub status: ScriptStatus,

    /// Last execution time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<DateTime<Utc>>,

    /// Last execution result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,

    /// Error count (resets on successful run)
    #[serde(default)]
    pub error_count: u32,

    /// Created timestamp
    pub created_at: DateTime<Utc>,

    /// Updated timestamp
    pub updated_at: DateTime<Utc>,
}

/// Script status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptStatus {
    /// Script is active and will be triggered
    Active,
    /// Script is paused (won't trigger)
    Paused,
    /// Script has errors and is disabled
    Error,
    /// Script is currently executing
    Running,
}

/// Script storage manager with bounded capacity and thread-safe access
///
/// # Thread Safety (v1.2.0)
/// Uses `tokio::sync::RwLock` to allow multiple readers or one writer.
/// All mutating methods acquire a write lock; read-only methods acquire a read lock.
pub struct ScriptStorage {
    scripts_dir: PathBuf,
    /// Thread-safe script map protected by RwLock
    scripts: RwLock<HashMap<String, Script>>,
    /// Maximum number of scripts allowed (memory protection)
    max_scripts: usize,
}

impl ScriptStorage {
    /// Create a new script storage with default capacity
    pub fn new(scripts_dir: Option<&str>) -> Self {
        Self::with_capacity(scripts_dir, DEFAULT_MAX_SCRIPTS)
    }

    /// Create a new script storage with specified maximum capacity
    pub fn with_capacity(scripts_dir: Option<&str>, max_scripts: usize) -> Self {
        let dir = scripts_dir.unwrap_or(DEFAULT_SCRIPTS_DIR);
        Self {
            scripts_dir: PathBuf::from(dir),
            scripts: RwLock::new(HashMap::with_capacity(max_scripts.min(100))),
            max_scripts,
        }
    }

    /// Get the maximum script capacity
    pub fn capacity(&self) -> usize {
        self.max_scripts
    }

    /// Check if storage is at capacity (async for lock acquisition)
    pub async fn is_full(&self) -> bool {
        let scripts = self.scripts.read().await;
        scripts.len() >= self.max_scripts
    }

    /// Initialize storage and load existing scripts
    pub async fn init(&self) -> Result<()> {
        // Create scripts directory if it doesn't exist
        if !self.scripts_dir.exists() {
            fs::create_dir_all(&self.scripts_dir).with_context(|| {
                format!("Failed to create scripts directory: {:?}", self.scripts_dir)
            })?;
            info!("Created scripts directory: {:?}", self.scripts_dir);
        }

        // Load existing scripts
        self.load_all().await?;

        let count = {
            let scripts = self.scripts.read().await;
            scripts.len()
        };

        info!("Script storage initialized with {} scripts", count);
        Ok(())
    }

    /// Load all scripts from disk
    pub async fn load_all(&self) -> Result<()> {
        let mut scripts = self.scripts.write().await;
        scripts.clear();

        let entries = fs::read_dir(&self.scripts_dir)
            .with_context(|| format!("Failed to read scripts directory: {:?}", self.scripts_dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path
                .extension()
                .map(|e| e == "json" || e == "yaml")
                .unwrap_or(false)
            {
                match self.load_script_file(&path) {
                    Ok(script) => {
                        info!(
                            "Loaded script: {} ({})",
                            script.definition.name, script.definition.id
                        );
                        scripts.insert(script.definition.id.clone(), script);
                    }
                    Err(e) => {
                        warn!("Failed to load script from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Load a single script file
    fn load_script_file(&self, path: &PathBuf) -> Result<Script> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read script file: {:?}", path))?;

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let definition: ScriptDefinition = match extension {
            "yaml" | "yml" => serde_yaml::from_str(&content)
                .with_context(|| format!("Failed to parse YAML script: {:?}", path))?,
            _ => serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON script: {:?}", path))?,
        };

        Ok(Script {
            status: if definition.enabled {
                ScriptStatus::Active
            } else {
                ScriptStatus::Paused
            },
            last_run: None,
            last_result: None,
            error_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            definition,
        })
    }

    /// Save a script
    ///
    /// # Memory Safety
    /// Returns an error if the storage is at capacity and the script is new.
    /// Updates to existing scripts are always allowed.
    ///
    /// # Thread Safety
    /// Acquires a write lock on the scripts map.
    pub async fn save(&self, script: Script) -> Result<()> {
        // Validate script ID to prevent path traversal
        validate_script_id(&script.definition.id)?;

        let mut scripts = self.scripts.write().await;

        // Check capacity for new scripts (updates always allowed)
        let is_new = !scripts.contains_key(&script.definition.id);
        if is_new && scripts.len() >= self.max_scripts {
            return Err(anyhow!(
                "Script storage at capacity ({}/{}). Delete existing scripts first.",
                scripts.len(),
                self.max_scripts
            ));
        }

        let filename = format!("{}.json", script.definition.id);
        let path = self.scripts_dir.join(&filename);

        let content = serde_json::to_string_pretty(&script.definition)
            .context("Failed to serialize script")?;

        fs::write(&path, content)
            .with_context(|| format!("Failed to write script file: {:?}", path))?;

        info!("Saved script: {} to {:?}", script.definition.id, path);

        scripts.insert(script.definition.id.clone(), script);
        Ok(())
    }

    /// Add or update a script from definition
    pub async fn add_script(&self, definition: ScriptDefinition) -> Result<()> {
        let script = Script {
            status: if definition.enabled {
                ScriptStatus::Active
            } else {
                ScriptStatus::Paused
            },
            last_run: None,
            last_result: None,
            error_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            definition,
        };

        self.save(script).await
    }

    /// Get a script by ID (returns cloned copy for thread safety)
    pub async fn get(&self, id: &str) -> Option<Script> {
        let scripts = self.scripts.read().await;
        scripts.get(id).cloned()
    }

    /// Get all scripts (returns cloned copies for thread safety)
    pub async fn get_all(&self) -> Vec<Script> {
        let scripts = self.scripts.read().await;
        scripts.values().cloned().collect()
    }

    /// Get active scripts (returns cloned copies for thread safety)
    pub async fn get_active(&self) -> Vec<Script> {
        let scripts = self.scripts.read().await;
        scripts
            .values()
            .filter(|s| s.status == ScriptStatus::Active && s.definition.enabled)
            .cloned()
            .collect()
    }

    /// Delete a script
    pub async fn delete(&self, id: &str) -> Result<bool> {
        // Validate script ID to prevent path traversal
        validate_script_id(id)?;

        let mut scripts = self.scripts.write().await;

        if scripts.remove(id).is_some() {
            // Delete file
            let filename = format!("{}.json", id);
            let path = self.scripts_dir.join(&filename);

            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to delete script file: {:?}", path))?;
            }

            info!("Deleted script: {}", id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Enable a script
    pub async fn enable(&self, id: &str) -> Result<bool> {
        // Validate script ID to prevent path traversal
        validate_script_id(id)?;

        let mut scripts = self.scripts.write().await;

        if let Some(script) = scripts.get_mut(id) {
            script.definition.enabled = true;
            script.status = ScriptStatus::Active;
            script.updated_at = Utc::now();

            // Save to disk
            let definition = script.definition.clone();
            let filename = format!("{}.json", id);
            let path = self.scripts_dir.join(&filename);

            let content = serde_json::to_string_pretty(&definition)?;
            fs::write(&path, content)?;

            info!("Enabled script: {}", id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Disable a script
    pub async fn disable(&self, id: &str) -> Result<bool> {
        // Validate script ID to prevent path traversal
        validate_script_id(id)?;

        let mut scripts = self.scripts.write().await;

        if let Some(script) = scripts.get_mut(id) {
            script.definition.enabled = false;
            script.status = ScriptStatus::Paused;
            script.updated_at = Utc::now();

            // Save to disk
            let definition = script.definition.clone();
            let filename = format!("{}.json", id);
            let path = self.scripts_dir.join(&filename);

            let content = serde_json::to_string_pretty(&definition)?;
            fs::write(&path, content)?;

            info!("Disabled script: {}", id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Update script execution result
    pub async fn update_result(&self, id: &str, success: bool, result: &str) {
        let mut scripts = self.scripts.write().await;

        if let Some(script) = scripts.get_mut(id) {
            script.last_run = Some(Utc::now());
            script.last_result = Some(result.to_string());

            if success {
                script.error_count = 0;
                if script.status == ScriptStatus::Running {
                    script.status = ScriptStatus::Active;
                }
            } else {
                script.error_count += 1;

                // Disable after 5 consecutive errors
                if script.error_count >= 5 {
                    script.status = ScriptStatus::Error;
                    warn!(
                        "Script {} disabled after {} consecutive errors",
                        id, script.error_count
                    );
                }
            }
        }
    }

    /// Set script status to Running
    pub async fn set_running(&self, id: &str) -> bool {
        let mut scripts = self.scripts.write().await;
        if let Some(script) = scripts.get_mut(id) {
            script.status = ScriptStatus::Running;
            true
        } else {
            false
        }
    }

    /// Get script count
    pub async fn count(&self) -> usize {
        let scripts = self.scripts.read().await;
        scripts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_status_serialization() {
        assert_eq!(
            serde_json::to_string(&ScriptStatus::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&ScriptStatus::Paused).unwrap(),
            "\"paused\""
        );
    }

    #[test]
    fn test_validate_script_id_valid() {
        // Valid IDs should pass
        assert!(validate_script_id("my-script").is_ok());
        assert!(validate_script_id("my_script_123").is_ok());
        assert!(validate_script_id("Script01").is_ok());
        assert!(validate_script_id("a").is_ok());
    }

    #[test]
    fn test_validate_script_id_path_traversal() {
        // Path traversal attempts should fail
        assert!(validate_script_id("../etc/passwd").is_err());
        assert!(validate_script_id("..").is_err());
        assert!(validate_script_id("foo/../bar").is_err());
        assert!(validate_script_id("/etc/passwd").is_err());
        assert!(validate_script_id("foo/bar").is_err());
        assert!(validate_script_id("C:\\Windows\\System32").is_err());
        assert!(validate_script_id("foo\\bar").is_err());
    }

    #[test]
    fn test_validate_script_id_special_chars() {
        // Special characters should fail
        assert!(validate_script_id("script;rm -rf /").is_err());
        assert!(validate_script_id("script$HOME").is_err());
        assert!(validate_script_id("script`id`").is_err());
        assert!(validate_script_id("script|cat").is_err());
        assert!(validate_script_id("script name").is_err()); // space
    }

    #[test]
    fn test_validate_script_id_edge_cases() {
        // Empty ID should fail
        assert!(validate_script_id("").is_err());

        // Hidden file should fail
        assert!(validate_script_id(".hidden").is_err());

        // Too long ID should fail
        let long_id = "a".repeat(MAX_SCRIPT_ID_LENGTH + 1);
        assert!(validate_script_id(&long_id).is_err());

        // Max length should pass
        let max_id = "a".repeat(MAX_SCRIPT_ID_LENGTH);
        assert!(validate_script_id(&max_id).is_ok());
    }
}
