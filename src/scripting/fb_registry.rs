//! Function Block Registry
//!
//! Manages function block instances for IEC 61131-3 compliance:
//! - Instance lifecycle (create, destroy)
//! - State persistence via SQLite
//! - Unified execution in scan cycle
//! - Input/Output wiring

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use super::function_blocks::{FunctionBlock, CTD, CTU, CTUD, F_TRIG, R_TRIG, TOF, TON, TP};
use super::persistence::{FBState, FunctionBlockStore, SqlitePersistence};

// ============================================================================
// FB Definition (from program deployment)
// ============================================================================

/// Function block instance definition (received from cloud)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FBDefinition {
    /// Unique instance ID
    pub id: String,
    /// FB type (TON, CTU, etc.)
    pub fb_type: String,
    /// Initial parameters
    #[serde(default)]
    pub params: FBParams,
    /// Input wiring (input_name -> source)
    #[serde(default)]
    pub inputs: HashMap<String, String>,
    /// Output wiring (output_name -> target)
    #[serde(default)]
    pub outputs: HashMap<String, String>,
}

/// Function block parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FBParams {
    /// Preset time in ms (for timers)
    #[serde(default)]
    pub pt_ms: Option<u64>,
    /// Preset value (for counters)
    #[serde(default)]
    pub pv: Option<i32>,
}

// ============================================================================
// FB Registry
// ============================================================================

/// Registry for managing function block instances
pub struct FBRegistry {
    /// Active FB instances
    instances: HashMap<String, Box<dyn FunctionBlock>>,
    /// Instance definitions (for recreation/persistence)
    definitions: HashMap<String, FBDefinition>,
    /// Persistence backend (optional)
    persistence: Option<Arc<SqlitePersistence>>,
}

impl FBRegistry {
    /// Create new empty registry
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
            definitions: HashMap::new(),
            persistence: None,
        }
    }

    /// Create registry with persistence support
    pub fn with_persistence(persistence: Arc<SqlitePersistence>) -> Self {
        Self {
            instances: HashMap::new(),
            definitions: HashMap::new(),
            persistence: Some(persistence),
        }
    }

    /// Set persistence after construction
    pub fn set_persistence(&mut self, persistence: Arc<SqlitePersistence>) {
        self.persistence = Some(persistence);
    }

    /// Create and register a function block from definition
    pub fn create_fb(&mut self, def: FBDefinition) -> Result<(), FBRegistryError> {
        if self.instances.contains_key(&def.id) {
            return Err(FBRegistryError::AlreadyExists(def.id.clone()));
        }

        let fb = self.instantiate_fb(&def)?;

        // Try to restore state from persistence
        if let Some(ref persistence) = self.persistence {
            if let Ok(Some(state)) = persistence.load_fb_state(&def.id) {
                let mut fb = fb;
                if fb.deserialize_state(&state.state_data) {
                    debug!(fb_id = %def.id, "Restored FB state from persistence");
                    self.instances.insert(def.id.clone(), fb);
                    self.definitions.insert(def.id.clone(), def);
                    return Ok(());
                }
            }
        }

        self.instances.insert(def.id.clone(), fb);
        self.definitions.insert(def.id.clone(), def);
        Ok(())
    }

    /// Instantiate a function block from definition
    fn instantiate_fb(
        &self,
        def: &FBDefinition,
    ) -> Result<Box<dyn FunctionBlock>, FBRegistryError> {
        let fb: Box<dyn FunctionBlock> = match def.fb_type.to_uppercase().as_str() {
            "TON" => {
                let pt = def.params.pt_ms.unwrap_or(1000);
                Box::new(TON::new(pt))
            }
            "TOF" => {
                let pt = def.params.pt_ms.unwrap_or(1000);
                Box::new(TOF::new(pt))
            }
            "TP" => {
                let pt = def.params.pt_ms.unwrap_or(1000);
                Box::new(TP::new(pt))
            }
            "CTU" => {
                let pv = def.params.pv.unwrap_or(10);
                Box::new(CTU::new(pv))
            }
            "CTD" => {
                let pv = def.params.pv.unwrap_or(10);
                Box::new(CTD::new(pv))
            }
            "CTUD" => {
                let pv = def.params.pv.unwrap_or(10);
                Box::new(CTUD::new(pv))
            }
            "R_TRIG" => Box::new(R_TRIG::new()),
            "F_TRIG" => Box::new(F_TRIG::new()),
            _ => {
                return Err(FBRegistryError::UnknownType(def.fb_type.clone()));
            }
        };

        Ok(fb)
    }

    /// Remove a function block
    pub fn remove_fb(&mut self, id: &str) -> bool {
        // Clear persistence
        if let Some(ref persistence) = self.persistence {
            let _ = persistence.clear_fb_state(id);
        }

        self.definitions.remove(id);
        self.instances.remove(id).is_some()
    }

    /// Get a function block by ID
    pub fn get(&self, id: &str) -> Option<&Box<dyn FunctionBlock>> {
        self.instances.get(id)
    }

    /// Get mutable reference to a function block
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Box<dyn FunctionBlock>> {
        self.instances.get_mut(id)
    }

    /// Get definition by ID
    pub fn get_definition(&self, id: &str) -> Option<&FBDefinition> {
        self.definitions.get(id)
    }

    /// Check if FB exists
    pub fn contains(&self, id: &str) -> bool {
        self.instances.contains_key(id)
    }

    /// Get number of registered FBs
    pub fn count(&self) -> usize {
        self.instances.len()
    }

    /// Get all FB IDs
    pub fn ids(&self) -> Vec<&String> {
        self.instances.keys().collect()
    }

    /// Execute all function blocks (one scan cycle)
    pub fn execute_all(&mut self) {
        for (id, fb) in &mut self.instances {
            fb.execute();
            debug!(fb_id = %id, fb_type = fb.fb_type(), "FB executed");
        }
    }

    /// Set input on a function block
    pub fn set_input(&mut self, fb_id: &str, input_name: &str, value: Value) -> bool {
        if let Some(fb) = self.instances.get_mut(fb_id) {
            return fb.set_input(input_name, value);
        }
        false
    }

    /// Get output from a function block
    pub fn get_output(&self, fb_id: &str, output_name: &str) -> Option<Value> {
        if let Some(fb) = self.instances.get(fb_id) {
            return fb.get_output(output_name);
        }
        None
    }

    /// Save all FB states to persistence
    pub fn save_all_states(&self) -> usize {
        let persistence = match &self.persistence {
            Some(p) => p,
            None => return 0,
        };

        let mut saved = 0;
        for (id, fb) in &self.instances {
            let state = FBState {
                fb_id: id.clone(),
                fb_type: fb.fb_type().to_string(),
                state_data: fb.serialize_state(),
                last_executed: chrono::Utc::now().timestamp(),
            };

            if let Err(e) = persistence.save_fb_state(id, &state) {
                warn!(fb_id = %id, error = %e, "Failed to save FB state");
            } else {
                saved += 1;
            }
        }

        if saved > 0 {
            info!(count = saved, "Saved FB states to persistence");
        }

        saved
    }

    /// Load all FB states from persistence
    pub fn load_all_states(&mut self) -> usize {
        let persistence = match &self.persistence {
            Some(p) => p,
            None => return 0,
        };

        let mut loaded = 0;
        for (id, fb) in &mut self.instances {
            match persistence.load_fb_state(id) {
                Ok(Some(state)) => {
                    if fb.deserialize_state(&state.state_data) {
                        loaded += 1;
                        debug!(fb_id = %id, "Loaded FB state from persistence");
                    }
                }
                Ok(None) => {
                    debug!(fb_id = %id, "No persisted state found");
                }
                Err(e) => {
                    warn!(fb_id = %id, error = %e, "Failed to load FB state");
                }
            }
        }

        if loaded > 0 {
            info!(count = loaded, "Loaded FB states from persistence");
        }

        loaded
    }

    /// Reset all function blocks
    pub fn reset_all(&mut self) {
        for fb in self.instances.values_mut() {
            fb.reset();
        }
    }

    /// Clear all function blocks
    pub fn clear(&mut self) {
        // Clear persistence for all
        if let Some(ref persistence) = self.persistence {
            for id in self.instances.keys() {
                let _ = persistence.clear_fb_state(id);
            }
        }

        self.instances.clear();
        self.definitions.clear();
    }

    /// Get registry statistics
    pub fn stats(&self) -> FBRegistryStats {
        let mut type_counts: HashMap<String, usize> = HashMap::new();

        for fb in self.instances.values() {
            *type_counts.entry(fb.fb_type().to_string()).or_insert(0) += 1;
        }

        FBRegistryStats {
            total_count: self.instances.len(),
            type_counts,
        }
    }
}

impl Default for FBRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// FB Registry errors
#[derive(Debug, Clone)]
pub enum FBRegistryError {
    /// FB with this ID already exists
    AlreadyExists(String),
    /// Unknown FB type
    UnknownType(String),
    /// FB not found
    NotFound(String),
    /// Invalid parameters
    InvalidParams(String),
}

impl std::fmt::Display for FBRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FBRegistryError::AlreadyExists(id) => write!(f, "FB already exists: {}", id),
            FBRegistryError::UnknownType(t) => write!(f, "Unknown FB type: {}", t),
            FBRegistryError::NotFound(id) => write!(f, "FB not found: {}", id),
            FBRegistryError::InvalidParams(msg) => write!(f, "Invalid parameters: {}", msg),
        }
    }
}

impl std::error::Error for FBRegistryError {}

// ============================================================================
// Statistics
// ============================================================================

/// Registry statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FBRegistryStats {
    pub total_count: usize,
    pub type_counts: HashMap<String, usize>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_ton() {
        let mut registry = FBRegistry::new();

        let def = FBDefinition {
            id: "delay_timer".to_string(),
            fb_type: "TON".to_string(),
            params: FBParams {
                pt_ms: Some(5000),
                pv: None,
            },
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        };

        registry.create_fb(def).unwrap();

        assert!(registry.contains("delay_timer"));
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_create_counter() {
        let mut registry = FBRegistry::new();

        let def = FBDefinition {
            id: "cycle_counter".to_string(),
            fb_type: "CTU".to_string(),
            params: FBParams {
                pt_ms: None,
                pv: Some(100),
            },
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        };

        registry.create_fb(def).unwrap();

        assert!(registry.contains("cycle_counter"));
    }

    #[test]
    fn test_create_all_types() {
        let mut registry = FBRegistry::new();

        let types = ["TON", "TOF", "TP", "CTU", "CTD", "CTUD", "R_TRIG", "F_TRIG"];

        for (i, fb_type) in types.iter().enumerate() {
            let def = FBDefinition {
                id: format!("fb_{}", i),
                fb_type: fb_type.to_string(),
                params: FBParams::default(),
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            };

            registry.create_fb(def).unwrap();
        }

        assert_eq!(registry.count(), 8);
    }

    #[test]
    fn test_unknown_type() {
        let mut registry = FBRegistry::new();

        let def = FBDefinition {
            id: "unknown".to_string(),
            fb_type: "INVALID_TYPE".to_string(),
            params: FBParams::default(),
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        };

        let result = registry.create_fb(def);
        assert!(matches!(result, Err(FBRegistryError::UnknownType(_))));
    }

    #[test]
    fn test_duplicate_id() {
        let mut registry = FBRegistry::new();

        let def = FBDefinition {
            id: "timer1".to_string(),
            fb_type: "TON".to_string(),
            params: FBParams::default(),
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        };

        registry.create_fb(def.clone()).unwrap();

        let result = registry.create_fb(def);
        assert!(matches!(result, Err(FBRegistryError::AlreadyExists(_))));
    }

    #[test]
    fn test_set_input_get_output() {
        let mut registry = FBRegistry::new();

        let def = FBDefinition {
            id: "counter".to_string(),
            fb_type: "CTU".to_string(),
            params: FBParams {
                pv: Some(5),
                pt_ms: None,
            },
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        };

        registry.create_fb(def).unwrap();

        // Set count input
        registry.set_input("counter", "CU", Value::Bool(true));
        registry.execute_all();
        registry.set_input("counter", "CU", Value::Bool(false));
        registry.execute_all();

        // Check count
        let cv = registry.get_output("counter", "CV");
        assert_eq!(cv, Some(Value::Number(1.into())));
    }

    #[test]
    fn test_execute_all() {
        let mut registry = FBRegistry::new();

        // Create R_TRIG
        registry
            .create_fb(FBDefinition {
                id: "edge".to_string(),
                fb_type: "R_TRIG".to_string(),
                params: FBParams::default(),
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            })
            .unwrap();

        // Set input
        registry.set_input("edge", "CLK", Value::Bool(true));

        // Execute
        registry.execute_all();

        // Check output
        let q = registry.get_output("edge", "Q");
        assert_eq!(q, Some(Value::Bool(true))); // Rising edge detected
    }

    #[test]
    fn test_remove_fb() {
        let mut registry = FBRegistry::new();

        registry
            .create_fb(FBDefinition {
                id: "temp".to_string(),
                fb_type: "TON".to_string(),
                params: FBParams::default(),
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            })
            .unwrap();

        assert!(registry.contains("temp"));
        assert!(registry.remove_fb("temp"));
        assert!(!registry.contains("temp"));
    }

    #[test]
    fn test_stats() {
        let mut registry = FBRegistry::new();

        registry
            .create_fb(FBDefinition {
                id: "t1".to_string(),
                fb_type: "TON".to_string(),
                params: FBParams::default(),
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            })
            .unwrap();

        registry
            .create_fb(FBDefinition {
                id: "t2".to_string(),
                fb_type: "TON".to_string(),
                params: FBParams::default(),
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            })
            .unwrap();

        registry
            .create_fb(FBDefinition {
                id: "c1".to_string(),
                fb_type: "CTU".to_string(),
                params: FBParams::default(),
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            })
            .unwrap();

        let stats = registry.stats();
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.type_counts.get("TON"), Some(&2));
        assert_eq!(stats.type_counts.get("CTU"), Some(&1));
    }

    #[test]
    fn test_reset_all() {
        let mut registry = FBRegistry::new();

        registry
            .create_fb(FBDefinition {
                id: "counter".to_string(),
                fb_type: "CTU".to_string(),
                params: FBParams {
                    pv: Some(10),
                    pt_ms: None,
                },
                inputs: HashMap::new(),
                outputs: HashMap::new(),
            })
            .unwrap();

        // Count up
        for _ in 0..5 {
            registry.set_input("counter", "CU", Value::Bool(true));
            registry.execute_all();
            registry.set_input("counter", "CU", Value::Bool(false));
            registry.execute_all();
        }

        assert_eq!(
            registry.get_output("counter", "CV"),
            Some(Value::Number(5.into()))
        );

        // Reset all
        registry.reset_all();

        assert_eq!(
            registry.get_output("counter", "CV"),
            Some(Value::Number(0.into()))
        );
    }
}
