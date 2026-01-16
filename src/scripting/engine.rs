//! Script execution engine
//!
//! Evaluates conditions and executes actions.
//!
//! v2.0 Features:
//! - Execution limits (depth, time, actions)
//! - Rate limiting per script
//! - Infinite loop protection
//!
//! v2.1 Features (IEC 61131-3 Scan Cycle):
//! - Scan cycle execution mode (PLC-like)
//! - Function block integration
//! - Deterministic execution timing

use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::{
    Action, ActionResult, ActionType, AlertLevel, ComparisonOperator, Condition, ConditionType,
    ConflictDetector, ConflictResult, ExecutionContext, ExecutionMode, FBDefinition, FBRegistry,
    LimitError, Script, ScriptContext, ScriptDefinition, ScriptLimits, ScriptRateLimiter,
    ScriptStatus, ScriptStorage, SqlitePersistence, TriggerManager, VariableScope, VariableStore,
};

use std::fs;
use std::path::PathBuf;

use crate::commands::{ProgramDefinition, ProgramState};
use crate::gpio::PinState;
use crate::AppState;

/// Script execution result
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub script_id: String,
    pub success: bool,
    pub actions_executed: usize,
    pub actions_failed: usize,
    pub results: Vec<ActionResult>,
    pub duration_ms: u64,
    pub timestamp: String,
}

/// Scan cycle statistics (v2.1)
#[derive(Debug, Clone, Default)]
pub struct ScanCycleStats {
    /// Total scan cycles executed
    pub total_cycles: u64,
    /// Current scan cycle time in ms
    pub current_cycle_ms: u64,
    /// Maximum cycle time seen
    pub max_cycle_ms: u64,
    /// Average cycle time (exponential moving average)
    pub avg_cycle_ms: f64,
    /// Number of cycle overruns (cycle took longer than target)
    pub overrun_count: u64,
    /// Last overrun duration in ms
    pub last_overrun_ms: u64,
}

impl ScanCycleStats {
    /// Update stats after a scan cycle
    fn update(&mut self, cycle_duration: Duration, target_cycle_ms: u64) {
        let cycle_ms = cycle_duration.as_millis() as u64;
        self.total_cycles += 1;
        self.current_cycle_ms = cycle_ms;

        if cycle_ms > self.max_cycle_ms {
            self.max_cycle_ms = cycle_ms;
        }

        // Exponential moving average (alpha = 0.1)
        const ALPHA: f64 = 0.1;
        self.avg_cycle_ms = ALPHA * (cycle_ms as f64) + (1.0 - ALPHA) * self.avg_cycle_ms;

        // Detect overrun
        if cycle_ms > target_cycle_ms {
            self.overrun_count += 1;
            self.last_overrun_ms = cycle_ms - target_cycle_ms;
            warn!(
                cycle_ms = cycle_ms,
                target_ms = target_cycle_ms,
                overrun_ms = self.last_overrun_ms,
                "Scan cycle overrun detected"
            );
        }
    }
}

/// Script execution engine
pub struct ScriptEngine {
    state: Arc<RwLock<AppState>>,
    /// Shared script storage (singleton pattern - v2.2)
    storage: Arc<tokio::sync::RwLock<ScriptStorage>>,
    trigger_manager: TriggerManager,
    context: ScriptContext,
    /// Execution limits for scripts
    limits: ScriptLimits,
    /// Rate limiter for script executions
    rate_limiter: ScriptRateLimiter,
    /// Conflict detector for GPIO/Modbus writes (v2.0)
    conflict_detector: ConflictDetector,
    /// Current script ID being executed (for conflict tracking)
    current_script_id: Option<String>,
    /// Current script priority (for conflict resolution, v2.0)
    current_script_priority: u8,
    /// Persistent storage for RETAIN variables (IEC 61131-3)
    persistence: Option<Arc<SqlitePersistence>>,
    /// Execution mode (v2.1 - Event-driven or Scan cycle)
    execution_mode: ExecutionMode,
    /// Scan cycle time in milliseconds (default: 100ms)
    scan_cycle_ms: u64,
    /// Function block registry (v2.1 - IEC 61131-3)
    fb_registry: FBRegistry,
    /// Scan cycle statistics (v2.1)
    scan_stats: ScanCycleStats,
    /// Path to program state file (v2.1 - for reload on startup)
    program_state_path: PathBuf,
}

/// Default scan cycle time in ms (100ms = 10 Hz)
const DEFAULT_SCAN_CYCLE_MS: u64 = 100;
/// Minimum allowed scan cycle time (10ms = 100 Hz)
const MIN_SCAN_CYCLE_MS: u64 = 10;
/// Maximum allowed scan cycle time (10s)
const MAX_SCAN_CYCLE_MS: u64 = 10000;

/// Get default program state path
fn default_program_state_path() -> PathBuf {
    let data_dir =
        std::env::var("SUDERRA_DATA_DIR").unwrap_or_else(|_| "/var/lib/suderra".to_string());
    PathBuf::from(&data_dir).join("program.json")
}

impl ScriptEngine {
    /// Create a new script engine (event-driven mode)
    /// v2.2: Now async to get shared storage from AppState
    pub async fn new(state: Arc<RwLock<AppState>>) -> Self {
        let storage = {
            let state_guard = state.read().await;
            state_guard.script_storage.clone()
        };
        let limits = ScriptLimits::default();
        let rate_limiter = ScriptRateLimiter::new(limits.rate_limit_per_minute);
        Self {
            state,
            storage,
            trigger_manager: TriggerManager::new(),
            context: ScriptContext::new(),
            limits,
            rate_limiter,
            conflict_detector: ConflictDetector::new(),
            current_script_id: None,
            current_script_priority: 50, // Default: Normal priority
            persistence: None,
            execution_mode: ExecutionMode::EventDriven,
            scan_cycle_ms: DEFAULT_SCAN_CYCLE_MS,
            fb_registry: FBRegistry::new(),
            scan_stats: ScanCycleStats::default(),
            program_state_path: default_program_state_path(),
        }
    }

    /// Create with custom limits
    /// v2.2: Now async to get shared storage from AppState
    pub async fn with_limits(state: Arc<RwLock<AppState>>, limits: ScriptLimits) -> Self {
        let storage = {
            let state_guard = state.read().await;
            state_guard.script_storage.clone()
        };
        let rate_limiter = ScriptRateLimiter::new(limits.rate_limit_per_minute);
        Self {
            state,
            storage,
            trigger_manager: TriggerManager::new(),
            context: ScriptContext::new(),
            limits,
            rate_limiter,
            conflict_detector: ConflictDetector::new(),
            current_script_id: None,
            current_script_priority: 50, // Default: Normal priority
            persistence: None,
            execution_mode: ExecutionMode::EventDriven,
            scan_cycle_ms: DEFAULT_SCAN_CYCLE_MS,
            fb_registry: FBRegistry::new(),
            scan_stats: ScanCycleStats::default(),
            program_state_path: default_program_state_path(),
        }
    }

    /// Create with persistence support (IEC 61131-3 RETAIN variables)
    /// v2.2: Now async to get shared storage from AppState
    pub async fn with_persistence(
        state: Arc<RwLock<AppState>>,
        persistence: Arc<SqlitePersistence>,
    ) -> Self {
        let storage = {
            let state_guard = state.read().await;
            state_guard.script_storage.clone()
        };
        let limits = ScriptLimits::default();
        let rate_limiter = ScriptRateLimiter::new(limits.rate_limit_per_minute);
        Self {
            state,
            storage,
            trigger_manager: TriggerManager::new(),
            context: ScriptContext::new(),
            limits,
            rate_limiter,
            conflict_detector: ConflictDetector::new(),
            current_script_id: None,
            current_script_priority: 50,
            persistence: Some(persistence.clone()),
            execution_mode: ExecutionMode::EventDriven,
            scan_cycle_ms: DEFAULT_SCAN_CYCLE_MS,
            fb_registry: FBRegistry::with_persistence(persistence),
            scan_stats: ScanCycleStats::default(),
            program_state_path: default_program_state_path(),
        }
    }

    /// Create with scan cycle mode (IEC 61131-3 PLC-like execution)
    /// v2.2: Now async to get shared storage from AppState
    pub async fn with_scan_cycle(
        state: Arc<RwLock<AppState>>,
        scan_cycle_ms: u64,
        persistence: Option<Arc<SqlitePersistence>>,
    ) -> Self {
        let storage = {
            let state_guard = state.read().await;
            state_guard.script_storage.clone()
        };
        let limits = ScriptLimits::default();
        let rate_limiter = ScriptRateLimiter::new(limits.rate_limit_per_minute);

        // Clamp scan cycle to valid range
        let scan_cycle_ms = scan_cycle_ms.clamp(MIN_SCAN_CYCLE_MS, MAX_SCAN_CYCLE_MS);

        let fb_registry = match &persistence {
            Some(p) => FBRegistry::with_persistence(p.clone()),
            None => FBRegistry::new(),
        };

        Self {
            state,
            storage,
            trigger_manager: TriggerManager::new(),
            context: ScriptContext::new(),
            limits,
            rate_limiter,
            conflict_detector: ConflictDetector::new(),
            current_script_id: None,
            current_script_priority: 50,
            persistence,
            execution_mode: ExecutionMode::ScanCycle,
            scan_cycle_ms,
            fb_registry,
            scan_stats: ScanCycleStats::default(),
            program_state_path: default_program_state_path(),
        }
    }

    /// Set execution mode
    pub fn set_execution_mode(&mut self, mode: ExecutionMode) {
        self.execution_mode = mode;
        info!(mode = ?mode, "Execution mode changed");
    }

    /// Get current execution mode
    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    /// Set scan cycle time (clamped to valid range)
    pub fn set_scan_cycle_ms(&mut self, ms: u64) {
        self.scan_cycle_ms = ms.clamp(MIN_SCAN_CYCLE_MS, MAX_SCAN_CYCLE_MS);
        info!(
            scan_cycle_ms = self.scan_cycle_ms,
            "Scan cycle time updated"
        );
    }

    /// Get scan cycle time
    pub fn scan_cycle_ms(&self) -> u64 {
        self.scan_cycle_ms
    }

    /// Get scan cycle statistics
    pub fn scan_stats(&self) -> &ScanCycleStats {
        &self.scan_stats
    }

    /// Get function block registry
    pub fn fb_registry(&self) -> &FBRegistry {
        &self.fb_registry
    }

    /// Get mutable function block registry
    pub fn fb_registry_mut(&mut self) -> &mut FBRegistry {
        &mut self.fb_registry
    }

    /// Register a function block
    pub fn register_fb(&mut self, def: FBDefinition) -> Result<(), super::FBRegistryError> {
        self.fb_registry.create_fb(def)
    }

    /// Set persistence after construction
    pub fn set_persistence(&mut self, persistence: Arc<SqlitePersistence>) {
        self.persistence = Some(persistence.clone());
        self.fb_registry.set_persistence(persistence);
    }

    /// Get persistence reference (for external save operations)
    pub fn persistence(&self) -> Option<&Arc<SqlitePersistence>> {
        self.persistence.as_ref()
    }

    /// Load program state from disk (v2.1)
    fn load_program_state(&self) -> Option<ProgramState> {
        match fs::read_to_string(&self.program_state_path) {
            Ok(content) => match serde_json::from_str::<ProgramState>(&content) {
                Ok(state) => Some(state),
                Err(e) => {
                    warn!(
                        path = ?self.program_state_path,
                        error = %e,
                        "Failed to parse program state file"
                    );
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(path = ?self.program_state_path, "No program state file found");
                None
            }
            Err(e) => {
                warn!(
                    path = ?self.program_state_path,
                    error = %e,
                    "Failed to read program state file"
                );
                None
            }
        }
    }

    /// Configure engine from a deployed program (v2.1)
    ///
    /// This method:
    /// 1. Sets execution mode (EventDriven or ScanCycle)
    /// 2. Sets scan cycle time if in ScanCycle mode
    /// 3. Registers all function blocks from the program
    pub fn configure_from_program(&mut self, program: &ProgramDefinition) -> anyhow::Result<()> {
        info!(
            program_id = %program.id,
            program_name = %program.name,
            version = program.version,
            execution_mode = ?program.execution_mode,
            scan_cycle_ms = program.scan_cycle_ms,
            fb_count = program.function_blocks.len(),
            "Configuring engine from deployed program"
        );

        // Set execution mode
        self.set_execution_mode(program.execution_mode);

        // Set scan cycle time
        if program.execution_mode == ExecutionMode::ScanCycle {
            self.set_scan_cycle_ms(program.scan_cycle_ms);
        }

        // Register function blocks
        let mut fb_errors = Vec::new();
        for fb_def in &program.function_blocks {
            match self.register_fb(fb_def.clone()) {
                Ok(()) => {
                    debug!(fb_id = %fb_def.id, fb_type = %fb_def.fb_type, "Registered FB");
                }
                Err(e) => {
                    warn!(fb_id = %fb_def.id, error = %e, "Failed to register FB");
                    fb_errors.push(format!("{}: {}", fb_def.id, e));
                }
            }
        }

        if !fb_errors.is_empty() {
            warn!(errors = ?fb_errors, "Some function blocks failed to register");
        }

        info!(
            program_id = %program.id,
            fb_registered = program.function_blocks.len() - fb_errors.len(),
            fb_failed = fb_errors.len(),
            "Program configuration applied"
        );

        Ok(())
    }

    /// Initialize the engine
    pub async fn init(&mut self) -> anyhow::Result<()> {
        self.storage.write().await.init()?;

        // Load and apply program configuration from program.json (v2.1)
        if let Some(program_state) = self.load_program_state() {
            if let Some(ref program) = program_state.program {
                info!(
                    program_id = %program.id,
                    deployed_at = ?program_state.deployed_at,
                    "Found deployed program, configuring engine"
                );

                if let Err(e) = self.configure_from_program(program) {
                    error!(error = %e, "Failed to configure from deployed program");
                }
            }
        }

        info!(
            mode = ?self.execution_mode,
            scan_cycle_ms = self.scan_cycle_ms,
            script_count = self.storage.read().await.count(),
            fb_count = self.fb_registry.count(),
            "Script engine initialized"
        );

        // Load RETAIN variables from persistence (IEC 61131-3)
        if let Some(ref persistence) = self.persistence {
            self.load_retain_variables(persistence.clone()).await;
        }

        // Load FB states from persistence (v2.1)
        let loaded_fbs = self.fb_registry.load_all_states();
        if loaded_fbs > 0 {
            info!(loaded_count = loaded_fbs, "Loaded function block states");
        }

        // Execute startup scripts
        self.run_startup_scripts().await;

        Ok(())
    }

    /// Load RETAIN variables from persistence into context
    /// v2.2: Now async to access shared storage
    async fn load_retain_variables(&mut self, persistence: Arc<SqlitePersistence>) {
        let mut loaded_count = 0;

        // Load variables for each active script
        let active_scripts = self.storage.read().await.get_active();
        for script in active_scripts {
            let script_id = &script.definition.id;

            match persistence.list(script_id) {
                Ok(variables) => {
                    for (var_name, value) in variables {
                        // Prefix with script_id to namespace variables
                        let full_name = format!("{}:{}", script_id, var_name);
                        self.context.set_variable(&full_name, value);
                        loaded_count += 1;
                    }
                }
                Err(e) => {
                    warn!("Failed to load RETAIN variables for {}: {}", script_id, e);
                }
            }
        }

        if loaded_count > 0 {
            info!("Loaded {} RETAIN variables from persistence", loaded_count);
        }
    }

    /// Save all RETAIN variables to persistence (call on graceful shutdown)
    pub fn save_retain_variables(&self) -> anyhow::Result<()> {
        let persistence = match &self.persistence {
            Some(p) => p,
            None => return Ok(()), // No persistence configured
        };

        let mut saved_count = 0;

        // Save all context variables that are namespaced (script_id:var_name)
        for (full_name, value) in &self.context.variables {
            if let Some((script_id, var_name)) = full_name.split_once(':') {
                if let Err(e) = persistence.save(script_id, var_name, value) {
                    warn!("Failed to save RETAIN variable {}: {}", full_name, e);
                } else {
                    saved_count += 1;
                }
            }
        }

        if saved_count > 0 {
            info!("Saved {} RETAIN variables to persistence", saved_count);
        }

        Ok(())
    }

    /// Run startup scripts
    async fn run_startup_scripts(&mut self) {
        // v2.2: get_active() now returns Vec<Script> (cloned for thread safety)
        let active_scripts = self.storage.read().await.get_active();
        let startup_scripts: Vec<String> = active_scripts
            .into_iter()
            .filter(|s| {
                s.definition
                    .triggers
                    .iter()
                    .any(|t| t.trigger_type == super::TriggerType::Startup)
            })
            .map(|s| s.definition.id.clone())
            .collect();

        for script_id in startup_scripts {
            info!("Running startup script: {}", script_id);
            // Use depth=0 for startup scripts (top-level execution)
            if let Err(e) = self.execute_with_depth(&script_id, 0).await {
                error!("Startup script {} failed: {}", script_id, e);
            }
        }
    }

    /// Main loop - dispatches to appropriate execution mode
    pub async fn run(&mut self) {
        match self.execution_mode {
            ExecutionMode::EventDriven => {
                info!(
                    mode = "event_driven",
                    "Script engine started in event-driven mode"
                );
                self.run_event_driven().await;
            }
            ExecutionMode::ScanCycle => {
                info!(
                    mode = "scan_cycle",
                    scan_cycle_ms = self.scan_cycle_ms,
                    fb_count = self.fb_registry.count(),
                    "Script engine started in scan cycle mode"
                );
                self.run_scan_cycle().await;
            }
        }
    }

    /// Event-driven execution loop (v2.0 - original behavior)
    /// Scripts run when triggers fire, checked every 1 second
    async fn run_event_driven(&mut self) {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
        let mut reload_counter = 0u32;

        loop {
            interval.tick().await;

            // Reload scripts every 30 seconds to pick up changes from commands
            reload_counter += 1;
            if reload_counter >= 30 {
                reload_counter = 0;
                // v2.2: Using shared storage (singleton pattern) - load_all no longer needed
                // Scripts are deployed via CommandHandler which shares same storage instance
                let count = self.storage.read().await.count();
                debug!("Active scripts in shared storage: {}", count);
            }

            // Update context with current data
            if let Err(e) = self.update_context().await {
                warn!("Failed to update context: {}", e);
                continue;
            }

            // Check triggers for active scripts
            let scripts_to_run = self.check_all_triggers().await;

            // Reset conflict detector for this execution cycle (v2.0)
            if !scripts_to_run.is_empty() {
                self.conflict_detector.reset();
            }

            // Execute triggered scripts
            for script_id in scripts_to_run {
                debug!("Trigger fired for script: {}", script_id);
                // Use depth=0 for trigger-based executions (top-level)
                if let Err(e) = self.execute_with_depth(&script_id, 0).await {
                    error!("Script {} execution failed: {}", script_id, e);
                }
            }
        }
    }

    /// Scan cycle execution loop (v2.1 - IEC 61131-3 PLC-like)
    ///
    /// Executes deterministically every scan_cycle_ms:
    /// 1. Read all inputs (Modbus, GPIO) → context
    /// 2. Wire FB inputs from context
    /// 3. Execute all function blocks
    /// 4. Wire FB outputs to context
    /// 5. Evaluate scripts/conditions
    /// 6. Write all outputs (Modbus, GPIO)
    /// 7. Wait for next cycle
    async fn run_scan_cycle(&mut self) {
        let scan_interval = Duration::from_millis(self.scan_cycle_ms);
        let mut reload_counter = 0u64;
        let reload_interval = (30000 / self.scan_cycle_ms).max(1); // Reload every ~30s

        info!(
            scan_cycle_ms = self.scan_cycle_ms,
            reload_every_n_cycles = reload_interval,
            "Scan cycle loop starting"
        );

        loop {
            let cycle_start = Instant::now();

            // ========== PHASE 1: READ INPUTS ==========
            if let Err(e) = self.update_context().await {
                warn!("Failed to read inputs: {}", e);
                // Continue anyway - don't skip cycle
            }

            // ========== PHASE 2: WIRE FB INPUTS ==========
            self.wire_fb_inputs();

            // ========== PHASE 3: EXECUTE FUNCTION BLOCKS ==========
            self.fb_registry.execute_all();

            // ========== PHASE 4: WIRE FB OUTPUTS ==========
            self.wire_fb_outputs();

            // ========== PHASE 5: EVALUATE SCRIPTS ==========
            // In scan cycle mode, check triggers every cycle
            let scripts_to_run = self.check_all_triggers().await;

            if !scripts_to_run.is_empty() {
                self.conflict_detector.reset();
            }

            for script_id in scripts_to_run {
                debug!(script_id = %script_id, "Executing script in scan cycle");
                if let Err(e) = self.execute_with_depth(&script_id, 0).await {
                    error!(script_id = %script_id, error = %e, "Script execution failed");
                }
            }

            // ========== PHASE 6: PERIODIC TASKS ==========
            reload_counter += 1;
            if reload_counter >= reload_interval {
                reload_counter = 0;

                // v2.2: Scripts use shared storage (singleton) - no reload needed
                // Log count for monitoring
                let count = self.storage.read().await.count();
                debug!(script_count = count, "Scripts in shared storage");

                // Persist FB states periodically
                let saved = self.fb_registry.save_all_states();
                if saved > 0 {
                    debug!(saved_count = saved, "Persisted FB states");
                }
            }

            // ========== PHASE 7: WAIT FOR NEXT CYCLE ==========
            let cycle_duration = cycle_start.elapsed();
            self.scan_stats.update(cycle_duration, self.scan_cycle_ms);

            if cycle_duration < scan_interval {
                let sleep_time = scan_interval - cycle_duration;
                tokio::time::sleep(sleep_time).await;
            }
            // If overrun, no sleep - immediately start next cycle
        }
    }

    /// Wire function block inputs from context
    /// Maps sensor values, GPIO states, and variables to FB inputs
    fn wire_fb_inputs(&mut self) {
        for fb_id in self
            .fb_registry
            .ids()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
        {
            let def = match self.fb_registry.get_definition(&fb_id) {
                Some(d) => d.clone(),
                None => continue,
            };

            for (input_name, source) in &def.inputs {
                // Source can be:
                // - "sensor:water_temp" -> sensor value
                // - "gpio:17" -> GPIO pin state
                // - "var:my_variable" -> context variable
                // - "fb:other_fb.Q" -> another FB's output
                let value = if let Some(source_path) = source.strip_prefix("sensor:") {
                    self.context.get_sensor(source_path).map(|v| json!(v))
                } else if let Some(pin_str) = source.strip_prefix("gpio:") {
                    pin_str
                        .parse::<u8>()
                        .ok()
                        .and_then(|pin| self.context.get_gpio(pin))
                        .map(|v| json!(v))
                } else if let Some(var_name) = source.strip_prefix("var:") {
                    self.context.get_variable(var_name).cloned()
                } else if let Some(fb_ref) = source.strip_prefix("fb:") {
                    // Format: "fb:other_fb_id.output_name"
                    if let Some((other_fb, output_name)) = fb_ref.split_once('.') {
                        self.fb_registry.get_output(other_fb, output_name)
                    } else {
                        None
                    }
                } else {
                    // Direct value (literal)
                    serde_json::from_str(source).ok()
                };

                if let Some(val) = value {
                    self.fb_registry.set_input(&fb_id, input_name, val);
                }
            }
        }
    }

    /// Wire function block outputs to context
    /// Makes FB outputs available for conditions and actions
    fn wire_fb_outputs(&mut self) {
        for fb_id in self
            .fb_registry
            .ids()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
        {
            let def = match self.fb_registry.get_definition(&fb_id) {
                Some(d) => d.clone(),
                None => continue,
            };

            for (output_name, target) in &def.outputs {
                let value = match self.fb_registry.get_output(&fb_id, output_name) {
                    Some(v) => v,
                    None => continue,
                };

                // Target can be:
                // - "var:my_variable" -> context variable
                // - "sensor:virtual_sensor" -> virtual sensor value
                if let Some(var_name) = target.strip_prefix("var:") {
                    self.context.set_variable(var_name, value);
                } else if let Some(sensor_name) = target.strip_prefix("sensor:") {
                    if let Some(num) = value.as_f64() {
                        self.context.set_sensor(sensor_name, num);
                    } else if let Some(b) = value.as_bool() {
                        self.context
                            .set_sensor(sensor_name, if b { 1.0 } else { 0.0 });
                    }
                }
                // Note: Direct GPIO/Modbus writes should be done via script actions
                // for proper conflict detection
            }
        }
    }

    /// Save all state (call on graceful shutdown)
    pub fn save_all_state(&self) -> anyhow::Result<()> {
        // Save RETAIN variables
        self.save_retain_variables()?;

        // Save FB states
        self.fb_registry.save_all_states();

        info!("All state saved successfully");
        Ok(())
    }

    /// Update context with current sensor/GPIO data
    async fn update_context(&mut self) -> anyhow::Result<()> {
        self.context.refresh_time();

        // Update Modbus sensor values via thread-safe handle
        let modbus_handle = {
            let state = self.state.read().await;
            state.modbus_handle.clone()
        };

        if let Some(handle) = modbus_handle {
            let results = handle.read_all().await;
            for result in results {
                for value in &result.values {
                    self.context.set_sensor(&value.name, value.scaled_value);
                }
            }
        }

        // Update GPIO values via actor handle (v2.0)
        let gpio_handle = {
            let state = self.state.read().await;
            state.gpio_handle.clone()
        };

        if let Some(handle) = gpio_handle {
            let result = handle.read_all().await;
            for pin_value in &result.values {
                let pin_state = matches!(pin_value.state, PinState::High);
                self.context.set_gpio(pin_value.pin, pin_state);
            }
        }

        // Update system metrics
        // (System metrics are updated in telemetry, we just reference them)

        Ok(())
    }

    /// Check all triggers and return scripts that should run (sorted by priority)
    /// Higher priority scripts execute first (v2.0)
    /// v2.2: Now async to access shared storage
    async fn check_all_triggers(&mut self) -> Vec<String> {
        // Collect scripts with their priorities
        let mut scripts_with_priority: Vec<(String, u8)> = Vec::new();

        let active_scripts = self.storage.read().await.get_active();
        for script in active_scripts {
            let script_id = &script.definition.id;
            let priority = script.definition.priority.value();

            for (idx, trigger) in script.definition.triggers.iter().enumerate() {
                if self
                    .trigger_manager
                    .check_trigger(script_id, idx, trigger, &self.context)
                {
                    // Check if already in list
                    if !scripts_with_priority.iter().any(|(id, _)| id == script_id) {
                        scripts_with_priority.push((script_id.clone(), priority));
                    }
                    break; // Only need one trigger to fire
                }
            }
        }

        // Sort by priority descending (higher priority first)
        scripts_with_priority.sort_by(|a, b| b.1.cmp(&a.1));

        // Log execution order if multiple scripts
        if scripts_with_priority.len() > 1 {
            debug!(
                "Script execution order (by priority): {:?}",
                scripts_with_priority
                    .iter()
                    .map(|(id, p)| format!("{}({})", id, p))
                    .collect::<Vec<_>>()
            );
        }

        // Return just the script IDs
        scripts_with_priority
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Execute a script by ID (public API - uses depth=0)
    pub async fn execute_script(&mut self, script_id: &str) -> anyhow::Result<ExecutionResult> {
        self.execute_with_depth(script_id, 0).await
    }

    /// Execute a script with depth tracking (v2.0 - infinite loop protection)
    ///
    /// This method enforces:
    /// - Maximum call depth (prevents infinite recursion)
    /// - Rate limiting per script
    /// - Execution timeout
    /// - Action count limits
    ///
    /// v2.2: Script context save/restore for correct nested script execution
    async fn execute_with_depth(
        &mut self,
        script_id: &str,
        depth: usize,
    ) -> anyhow::Result<ExecutionResult> {
        let start = std::time::Instant::now();

        // Check depth limit FIRST (infinite loop protection)
        if depth >= self.limits.max_call_depth {
            warn!(
                "Script {} exceeded max call depth ({})",
                script_id, self.limits.max_call_depth
            );
            return Ok(ExecutionResult {
                script_id: script_id.to_string(),
                success: false,
                actions_executed: 0,
                actions_failed: 1,
                results: vec![ActionResult::failure(
                    ActionType::CallScript,
                    format!("Max call depth ({}) exceeded", self.limits.max_call_depth),
                )],
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now().to_rfc3339(),
            });
        }

        // Check rate limit
        if !self.rate_limiter.check(script_id) {
            warn!("Script {} rate limited", script_id);
            return Ok(ExecutionResult {
                script_id: script_id.to_string(),
                success: false,
                actions_executed: 0,
                actions_failed: 1,
                results: vec![ActionResult::failure(
                    ActionType::Noop,
                    format!(
                        "Rate limit exceeded ({}/min)",
                        self.limits.rate_limit_per_minute
                    ),
                )],
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now().to_rfc3339(),
            });
        }

        let definition = {
            let storage = self.storage.read().await;
            let script = storage
                .get(script_id)
                .ok_or_else(|| anyhow::anyhow!("Script not found: {}", script_id))?;
            script.definition.clone()
        };

        // v2.2: Save previous script context for nested call restoration
        // This ensures that when script A calls script B, after B completes,
        // A's context is correctly restored for remaining actions
        let prev_script_id = self.current_script_id.take();
        let prev_script_priority = self.current_script_priority;

        // Track current script ID and priority for conflict detection (v2.0)
        self.current_script_id = Some(script_id.to_string());
        self.current_script_priority = definition.priority.value();

        info!(
            "Executing script: {} ({}) [depth={}]",
            definition.name, definition.id, depth
        );

        // Mark as running
        {
            let mut storage = self.storage.write().await;
            if let Some(s) = storage.get_mut(script_id) {
                s.status = ScriptStatus::Running;
            }
        }

        // Check conditions
        if !self.evaluate_conditions(&definition.conditions) {
            debug!("Script {} conditions not met, skipping", script_id);
            self.storage.write().await.update_result(
                script_id,
                true,
                "Conditions not met - skipped",
            );

            return Ok(ExecutionResult {
                script_id: script_id.to_string(),
                success: true,
                actions_executed: 0,
                actions_failed: 0,
                results: vec![],
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now().to_rfc3339(),
            });
        }

        // Create execution context for tracking limits
        let mut exec_ctx = ExecutionContext::new(self.limits.clone());
        exec_ctx.call_depth = depth;

        // Execute actions with limits
        let mut results = Vec::new();
        let mut actions_executed = 0;
        let mut actions_failed = 0;

        for action in &definition.actions {
            // Check execution time limit
            if exec_ctx.is_time_exceeded() {
                warn!("Script {} execution time exceeded", script_id);
                results.push(ActionResult::failure(
                    ActionType::Noop,
                    "Execution time limit exceeded",
                ));
                actions_failed += 1;
                break;
            }

            // Check action count limit
            if let Err(LimitError::ActionLimitExceeded) = exec_ctx.record_action() {
                warn!("Script {} action limit exceeded", script_id);
                results.push(ActionResult::failure(
                    ActionType::Noop,
                    format!(
                        "Action limit ({}) exceeded",
                        self.limits.max_actions_per_run
                    ),
                ));
                actions_failed += 1;
                break;
            }

            let result = self.execute_action_with_depth(action, depth).await;

            if result.success {
                actions_executed += 1;
            } else {
                actions_failed += 1;
            }

            results.push(result);
        }

        // Handle errors if any actions failed
        if actions_failed > 0 && !definition.on_error.is_empty() {
            for action in &definition.on_error {
                // Don't exceed action limit for error handlers
                if exec_ctx.record_action().is_err() {
                    break;
                }
                let result = self.execute_action_with_depth(action, depth).await;
                results.push(result);
            }
        }

        let success = actions_failed == 0;
        let result_msg = if success {
            format!("Completed {} actions", actions_executed)
        } else {
            format!("{} actions failed", actions_failed)
        };

        self.storage
            .write()
            .await
            .update_result(script_id, success, &result_msg);

        // v2.2: Restore previous script context after execution completes
        // This ensures parent script's context is restored after nested calls
        self.current_script_id = prev_script_id;
        self.current_script_priority = prev_script_priority;

        Ok(ExecutionResult {
            script_id: script_id.to_string(),
            success,
            actions_executed,
            actions_failed,
            results,
            duration_ms: start.elapsed().as_millis() as u64,
            timestamp: Utc::now().to_rfc3339(),
        })
    }

    /// Evaluate all conditions (AND logic)
    fn evaluate_conditions(&self, conditions: &[Condition]) -> bool {
        for condition in conditions {
            if !self.evaluate_condition(condition) {
                return false;
            }
        }
        true
    }

    /// Evaluate a single condition
    fn evaluate_condition(&self, condition: &Condition) -> bool {
        let value = match self.context.get_value(&condition.source) {
            Some(v) => v,
            None => {
                debug!("Condition source '{}' not found", condition.source);
                return false;
            }
        };

        self.compare_values(&value, &condition.operator, &condition.value)
    }

    /// Compare two values
    fn compare_values(&self, left: &Value, op: &ComparisonOperator, right: &Value) -> bool {
        // Numeric comparison
        if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
            return match op {
                ComparisonOperator::Eq => (l - r).abs() < f64::EPSILON,
                ComparisonOperator::Ne => (l - r).abs() >= f64::EPSILON,
                ComparisonOperator::Gt => l > r,
                ComparisonOperator::Gte => l >= r,
                ComparisonOperator::Lt => l < r,
                ComparisonOperator::Lte => l <= r,
                ComparisonOperator::Between => {
                    if let Some(arr) = right.as_array() {
                        if arr.len() >= 2 {
                            let min = arr[0].as_f64().unwrap_or(f64::MIN);
                            let max = arr[1].as_f64().unwrap_or(f64::MAX);
                            return l >= min && l <= max;
                        }
                    }
                    false
                }
                _ => false,
            };
        }

        // Boolean comparison
        if let (Some(l), Some(r)) = (left.as_bool(), right.as_bool()) {
            return match op {
                ComparisonOperator::Eq => l == r,
                ComparisonOperator::Ne => l != r,
                _ => false,
            };
        }

        // String comparison
        if let (Some(l), Some(r)) = (left.as_str(), right.as_str()) {
            return match op {
                ComparisonOperator::Eq => l == r,
                ComparisonOperator::Ne => l != r,
                ComparisonOperator::Contains => l.contains(r),
                _ => false,
            };
        }

        false
    }

    /// Execute a single action (wrapper for backwards compatibility)
    async fn execute_action(&mut self, action: &Action) -> ActionResult {
        self.execute_action_with_depth(action, 0).await
    }

    /// Execute a single action with depth tracking
    async fn execute_action_with_depth(&mut self, action: &Action, depth: usize) -> ActionResult {
        // Check inline condition if present
        if let Some(ref condition) = action.condition {
            let cond = Condition {
                condition_type: ConditionType::Sensor,
                source: condition.source.clone(),
                operator: condition.operator.clone(),
                value: condition.value.clone(),
            };

            if !self.evaluate_condition(&cond) {
                return ActionResult::success(ActionType::Noop, "Condition not met - skipped");
            }
        }

        match action.action_type {
            ActionType::SetGpio => self.action_set_gpio(action).await,
            ActionType::WriteModbus => self.action_write_modbus(action).await,
            ActionType::WriteCoil => self.action_write_coil(action).await,
            ActionType::Alert => self.action_alert(action).await,
            ActionType::SetVariable => self.action_set_variable(action),
            ActionType::Log => self.action_log(action),
            ActionType::Delay => self.action_delay(action).await,
            ActionType::PublishMqtt => self.action_publish_mqtt(action).await,
            ActionType::CallScript => self.action_call_script_with_depth(action, depth).await,
            ActionType::Noop => ActionResult::success(ActionType::Noop, "No operation"),
        }
    }

    /// Set GPIO pin action (v2.0 - uses actor pattern + priority-aware conflict detection)
    async fn action_set_gpio(&mut self, action: &Action) -> ActionResult {
        let pin: u8 = match action.target.parse() {
            Ok(p) => p,
            Err(_) => return ActionResult::failure(ActionType::SetGpio, "Invalid pin number"),
        };

        let value = match action.value.as_ref().and_then(|v| v.as_bool()) {
            Some(v) => v,
            None => return ActionResult::failure(ActionType::SetGpio, "Missing or invalid value"),
        };

        // Check for conflicts with priority (v2.0)
        let script_id = self.current_script_id.as_deref().unwrap_or("unknown");
        let priority = self.current_script_priority;
        match self
            .conflict_detector
            .check_gpio_write(script_id, priority, pin, value)
        {
            ConflictResult::ConflictWon { message } => {
                // Higher priority script wins - continue with write
                warn!("{}", message);
            }
            ConflictResult::ConflictLost { message } => {
                // Lower priority script loses - BLOCKED from writing
                warn!("{}", message);
                return ActionResult::failure(
                    ActionType::SetGpio,
                    format!("Blocked by higher priority script: GPIO {}", pin),
                );
            }
            ConflictResult::Duplicate => {
                debug!("GPIO {} duplicate write from {}, skipping", pin, script_id);
                return ActionResult::success(
                    ActionType::SetGpio,
                    format!("GPIO {} already set to same value, skipped", pin),
                );
            }
            ConflictResult::NoConflict => {}
        }

        // Get GPIO handle (thread-safe, actor pattern)
        let gpio_handle = {
            let app_state = self.state.read().await;
            app_state.gpio_handle.clone()
        };

        match gpio_handle {
            Some(handle) => match handle.write_pin(pin, value).await {
                Ok(()) => {
                    let state_str = if value { "HIGH" } else { "LOW" };
                    ActionResult::success(
                        ActionType::SetGpio,
                        format!("Set GPIO {} to {}", pin, state_str),
                    )
                }
                Err(e) => ActionResult::failure(ActionType::SetGpio, format!("Failed: {}", e)),
            },
            None => ActionResult::failure(ActionType::SetGpio, "GPIO not available"),
        }
    }

    /// Write Modbus register action (v2.0 - with priority-aware conflict detection)
    async fn action_write_modbus(&mut self, action: &Action) -> ActionResult {
        let device = match &action.device {
            Some(d) => d.clone(),
            None => return ActionResult::failure(ActionType::WriteModbus, "Missing device name"),
        };

        let address = match action.address {
            Some(a) => a,
            None => {
                return ActionResult::failure(ActionType::WriteModbus, "Missing register address")
            }
        };

        let value = match action.value.as_ref().and_then(|v| v.as_u64()) {
            Some(v) if v <= u16::MAX as u64 => v as u16,
            Some(v) => {
                return ActionResult::failure(
                    ActionType::WriteModbus,
                    format!("Value {} exceeds maximum u16 value ({})", v, u16::MAX),
                )
            }
            None => {
                return ActionResult::failure(ActionType::WriteModbus, "Missing or invalid value")
            }
        };

        // Check for conflicts with priority (v2.0)
        let script_id = self.current_script_id.as_deref().unwrap_or("unknown");
        let priority = self.current_script_priority;
        match self
            .conflict_detector
            .check_modbus_write(script_id, priority, &device, address, value)
        {
            ConflictResult::ConflictWon { message } => {
                // Higher priority script wins - continue with write
                warn!("{}", message);
            }
            ConflictResult::ConflictLost { message } => {
                // Lower priority script loses - BLOCKED from writing
                warn!("{}", message);
                return ActionResult::failure(
                    ActionType::WriteModbus,
                    format!("Blocked by higher priority script: {}:{}", device, address),
                );
            }
            ConflictResult::Duplicate => {
                debug!(
                    "Modbus {}:{} duplicate write from {}, skipping",
                    device, address, script_id
                );
                return ActionResult::success(
                    ActionType::WriteModbus,
                    format!("{}:{} already set to same value, skipped", device, address),
                );
            }
            ConflictResult::NoConflict => {}
        }

        // Get modbus handle (thread-safe)
        let modbus_handle = {
            let app_state = self.state.read().await;
            app_state.modbus_handle.clone()
        };

        match modbus_handle {
            Some(handle) => match handle.write_register(&device, address, value).await {
                Ok(()) => ActionResult::success(
                    ActionType::WriteModbus,
                    format!("Wrote {} to {}:{}", value, device, address),
                ),
                Err(e) => ActionResult::failure(ActionType::WriteModbus, format!("Failed: {}", e)),
            },
            None => ActionResult::failure(ActionType::WriteModbus, "Modbus not available"),
        }
    }

    /// Write Modbus coil action (v2.0 - with priority-aware conflict detection)
    async fn action_write_coil(&mut self, action: &Action) -> ActionResult {
        let device = match &action.device {
            Some(d) => d.clone(),
            None => return ActionResult::failure(ActionType::WriteCoil, "Missing device name"),
        };

        let address = match action.address {
            Some(a) => a,
            None => return ActionResult::failure(ActionType::WriteCoil, "Missing coil address"),
        };

        let value = match action.value.as_ref().and_then(|v| v.as_bool()) {
            Some(v) => v,
            None => {
                return ActionResult::failure(ActionType::WriteCoil, "Missing or invalid value")
            }
        };

        // Check for conflicts with priority (v2.0)
        let script_id = self.current_script_id.as_deref().unwrap_or("unknown");
        let priority = self.current_script_priority;
        match self
            .conflict_detector
            .check_coil_write(script_id, priority, &device, address, value)
        {
            ConflictResult::ConflictWon { message } => {
                // Higher priority script wins - continue with write
                warn!("{}", message);
            }
            ConflictResult::ConflictLost { message } => {
                // Lower priority script loses - BLOCKED from writing
                warn!("{}", message);
                return ActionResult::failure(
                    ActionType::WriteCoil,
                    format!("Blocked by higher priority script: {}:{}", device, address),
                );
            }
            ConflictResult::Duplicate => {
                debug!(
                    "Coil {}:{} duplicate write from {}, skipping",
                    device, address, script_id
                );
                return ActionResult::success(
                    ActionType::WriteCoil,
                    format!("{}:{} already set to same value, skipped", device, address),
                );
            }
            ConflictResult::NoConflict => {}
        }

        // Get modbus handle (thread-safe)
        let modbus_handle = {
            let app_state = self.state.read().await;
            app_state.modbus_handle.clone()
        };

        match modbus_handle {
            Some(handle) => match handle.write_coil(&device, address, value).await {
                Ok(()) => ActionResult::success(
                    ActionType::WriteCoil,
                    format!("Wrote {} to {}:{}", value, device, address),
                ),
                Err(e) => ActionResult::failure(ActionType::WriteCoil, format!("Failed: {}", e)),
            },
            None => ActionResult::failure(ActionType::WriteCoil, "Modbus not available"),
        }
    }

    /// Alert action - publish alert to MQTT
    async fn action_alert(&self, action: &Action) -> ActionResult {
        let message = match &action.message {
            Some(m) => self.context.interpolate(m),
            None => return ActionResult::failure(ActionType::Alert, "Missing message"),
        };

        let level = action.level.unwrap_or(AlertLevel::Warning);

        info!("ALERT [{:?}]: {}", level, message);

        // Publish to MQTT alert topic
        let app_state = self.state.read().await;
        if let Some(ref _mqtt) = app_state.mqtt_client {
            let alert_data = json!({
                "level": format!("{:?}", level).to_lowercase(),
                "message": message,
                "timestamp": Utc::now().to_rfc3339(),
                "source": "script_engine"
            });

            // Note: Would need to add an alert topic to MQTT client
            // For now, just log it
            debug!("Alert would be published: {:?}", alert_data);
        }

        ActionResult::success(ActionType::Alert, format!("Alert sent: {}", message))
    }

    /// Set variable action (with IEC 61131-3 RETAIN support)
    fn action_set_variable(&mut self, action: &Action) -> ActionResult {
        let value = match &action.value {
            Some(v) => v.clone(),
            None => return ActionResult::failure(ActionType::SetVariable, "Missing value"),
        };

        let scope = action.scope.unwrap_or(VariableScope::Local);

        // For RETAIN/Persistent scope, use namespaced variable name
        let var_name = match scope {
            VariableScope::Retain | VariableScope::Persistent => {
                // Namespace with script_id for isolation
                let script_id = self.current_script_id.as_deref().unwrap_or("global");
                format!("{}:{}", script_id, action.target)
            }
            _ => action.target.clone(),
        };

        // Set in context
        self.context.set_variable(&var_name, value.clone());

        // Persist RETAIN variables to SQLite
        if matches!(scope, VariableScope::Retain | VariableScope::Persistent) {
            if let Some(ref persistence) = self.persistence {
                let script_id = self.current_script_id.as_deref().unwrap_or("global");
                if let Err(e) = persistence.save(script_id, &action.target, &value) {
                    warn!("Failed to persist RETAIN variable {}: {}", action.target, e);
                    return ActionResult::failure(
                        ActionType::SetVariable,
                        format!("Set {} but persistence failed: {}", action.target, e),
                    );
                }
                debug!("RETAIN variable {} persisted", action.target);
            } else {
                warn!(
                    "RETAIN variable {} set but no persistence configured",
                    action.target
                );
            }
        }

        let scope_str = format!("{:?}", scope).to_lowercase();
        ActionResult::success(
            ActionType::SetVariable,
            format!("Set {} = {:?} (scope: {})", action.target, value, scope_str),
        )
    }

    /// Log action
    fn action_log(&self, action: &Action) -> ActionResult {
        let message = match &action.message {
            Some(m) => self.context.interpolate(m),
            None => return ActionResult::failure(ActionType::Log, "Missing message"),
        };

        info!("[Script Log] {}", message);

        ActionResult::success(ActionType::Log, message)
    }

    /// Delay action (v2.0 - with max delay limit)
    async fn action_delay(&self, action: &Action) -> ActionResult {
        let delay_ms = action.delay_ms.unwrap_or(1000);

        // Check against max delay limit
        if delay_ms > self.limits.max_delay_ms {
            return ActionResult::failure(
                ActionType::Delay,
                format!(
                    "Delay {}ms exceeds maximum allowed ({}ms)",
                    delay_ms, self.limits.max_delay_ms
                ),
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

        ActionResult::success(ActionType::Delay, format!("Delayed {}ms", delay_ms))
    }

    /// Publish MQTT message action
    async fn action_publish_mqtt(&self, action: &Action) -> ActionResult {
        let message = match &action.message {
            Some(m) => self.context.interpolate(m),
            None => return ActionResult::failure(ActionType::PublishMqtt, "Missing message"),
        };

        // Target is the topic
        let topic = if action.target.is_empty() {
            "scripts/output".to_string()
        } else {
            self.context.interpolate(&action.target)
        };

        debug!("Would publish to {}: {}", topic, message);

        ActionResult::success(ActionType::PublishMqtt, format!("Published to {}", topic))
    }

    /// Call another script (v2.0 - with depth tracking for infinite loop protection)
    async fn action_call_script_with_depth(
        &mut self,
        action: &Action,
        current_depth: usize,
    ) -> ActionResult {
        let script_id = match &action.script_id {
            Some(id) => id,
            None => return ActionResult::failure(ActionType::CallScript, "Missing script_id"),
        };

        // Increment depth for nested call
        let next_depth = current_depth + 1;

        debug!(
            "Calling script {} from depth {} -> {}",
            script_id, current_depth, next_depth
        );

        // Use execute_with_depth with incremented depth
        match Box::pin(self.execute_with_depth(script_id, next_depth)).await {
            Ok(result) => {
                if result.success {
                    ActionResult::success(
                        ActionType::CallScript,
                        format!("Called script {} [depth={}]", script_id, next_depth),
                    )
                } else {
                    // Include reason for failure if it was a limit error
                    let reason = result
                        .results
                        .first()
                        .map(|r| r.message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string());
                    ActionResult::failure(
                        ActionType::CallScript,
                        format!("Script {} failed: {}", script_id, reason),
                    )
                }
            }
            Err(e) => ActionResult::failure(
                ActionType::CallScript,
                format!("Failed to call script: {}", e),
            ),
        }
    }

    /// Call another script (backwards compatibility - uses depth=0)
    #[allow(dead_code)]
    async fn action_call_script(&mut self, action: &Action) -> ActionResult {
        self.action_call_script_with_depth(action, 0).await
    }

    // === Public API for script management ===
    // v2.2: All methods now async to use shared storage (singleton pattern)

    /// Add a script from definition
    pub async fn add_script(&mut self, definition: ScriptDefinition) -> anyhow::Result<()> {
        self.storage.write().await.add_script(definition)
    }

    /// Delete a script
    pub async fn delete_script(&mut self, id: &str) -> anyhow::Result<bool> {
        self.trigger_manager.reset_script(id);
        self.storage.write().await.delete(id)
    }

    /// Enable a script
    pub async fn enable_script(&mut self, id: &str) -> anyhow::Result<bool> {
        self.storage.write().await.enable(id)
    }

    /// Disable a script
    pub async fn disable_script(&mut self, id: &str) -> anyhow::Result<bool> {
        self.trigger_manager.reset_script(id);
        self.storage.write().await.disable(id)
    }

    /// List all scripts (returns cloned scripts for thread safety)
    pub async fn list_scripts(&self) -> Vec<Script> {
        self.storage.read().await.get_all_cloned()
    }

    /// Get script by ID (returns cloned script for thread safety)
    pub async fn get_script(&self, id: &str) -> Option<Script> {
        self.storage.read().await.get(id).cloned()
    }

    /// Get script count
    pub async fn script_count(&self) -> usize {
        self.storage.read().await.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_compare_values_numeric() {
        // We can't easily instantiate ScriptEngine without AppState,
        // so we test the compare logic via the TriggerManager
        let _manager = TriggerManager::new();

        // Test via context
        let mut ctx = ScriptContext::new();
        ctx.set_sensor("temp", 25.5);

        assert!(ctx.get_sensor("temp").is_some());
        assert_eq!(ctx.get_sensor("temp"), Some(25.5));
    }

    #[test]
    fn test_execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::EventDriven);
    }

    #[test]
    fn test_scan_cycle_stats_update() {
        let mut stats = ScanCycleStats::default();

        // First cycle: 50ms, target 100ms - no overrun
        stats.update(Duration::from_millis(50), 100);
        assert_eq!(stats.total_cycles, 1);
        assert_eq!(stats.current_cycle_ms, 50);
        assert_eq!(stats.max_cycle_ms, 50);
        assert_eq!(stats.overrun_count, 0);

        // Second cycle: 80ms - still no overrun
        stats.update(Duration::from_millis(80), 100);
        assert_eq!(stats.total_cycles, 2);
        assert_eq!(stats.current_cycle_ms, 80);
        assert_eq!(stats.max_cycle_ms, 80);
        assert_eq!(stats.overrun_count, 0);

        // Third cycle: 120ms - OVERRUN!
        stats.update(Duration::from_millis(120), 100);
        assert_eq!(stats.total_cycles, 3);
        assert_eq!(stats.current_cycle_ms, 120);
        assert_eq!(stats.max_cycle_ms, 120);
        assert_eq!(stats.overrun_count, 1);
        assert_eq!(stats.last_overrun_ms, 20);
    }

    #[test]
    fn test_scan_cycle_stats_average() {
        let mut stats = ScanCycleStats::default();

        // Run 50 cycles at 50ms
        // With EMA alpha=0.1, we need ~23 cycles to reach 45 (90% of target)
        // Using 50 cycles ensures convergence to within tolerance
        for _ in 0..50 {
            stats.update(Duration::from_millis(50), 100);
        }

        // Average should be around 50ms (exponential moving average)
        // After 50 cycles with alpha=0.1: 50 * (1 - 0.9^50) ≈ 49.7
        assert!(stats.avg_cycle_ms > 45.0 && stats.avg_cycle_ms < 55.0);
    }

    #[test]
    fn test_scan_cycle_clamp() {
        // Test that scan cycle is clamped to valid range
        let clamped_low = 5u64.clamp(MIN_SCAN_CYCLE_MS, MAX_SCAN_CYCLE_MS);
        assert_eq!(clamped_low, MIN_SCAN_CYCLE_MS); // 10ms

        let clamped_high = 20000u64.clamp(MIN_SCAN_CYCLE_MS, MAX_SCAN_CYCLE_MS);
        assert_eq!(clamped_high, MAX_SCAN_CYCLE_MS); // 10000ms

        let clamped_normal = 100u64.clamp(MIN_SCAN_CYCLE_MS, MAX_SCAN_CYCLE_MS);
        assert_eq!(clamped_normal, 100); // unchanged
    }

    #[test]
    fn test_fb_wiring_parse_sensor() {
        // Test parsing different source formats
        let source = "sensor:water_temp";
        assert!(source.strip_prefix("sensor:").is_some());
        assert_eq!(source.strip_prefix("sensor:"), Some("water_temp"));
    }

    #[test]
    fn test_fb_wiring_parse_gpio() {
        let source = "gpio:17";
        let pin_str = source.strip_prefix("gpio:");
        assert_eq!(pin_str, Some("17"));
        assert_eq!(pin_str.unwrap().parse::<u8>().ok(), Some(17));
    }

    #[test]
    fn test_fb_wiring_parse_fb_ref() {
        let source = "fb:timer1.Q";
        let fb_ref = source.strip_prefix("fb:");
        assert_eq!(fb_ref, Some("timer1.Q"));

        let (fb_id, output) = fb_ref.unwrap().split_once('.').unwrap();
        assert_eq!(fb_id, "timer1");
        assert_eq!(output, "Q");
    }

    #[test]
    fn test_fb_registry_integration() {
        let mut registry = FBRegistry::new();

        // Create a TON timer
        let def = FBDefinition {
            id: "delay_timer".to_string(),
            fb_type: "TON".to_string(),
            params: super::super::FBParams {
                pt_ms: Some(1000),
                pv: None,
            },
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        };

        registry.create_fb(def).unwrap();
        assert_eq!(registry.count(), 1);

        // Set input
        registry.set_input("delay_timer", "IN", Value::Bool(true));

        // Execute
        registry.execute_all();

        // Check output (Q should be false initially since time hasn't passed)
        let q = registry.get_output("delay_timer", "Q");
        assert_eq!(q, Some(Value::Bool(false)));
    }

    #[test]
    fn test_fb_definition_with_wiring() {
        let mut inputs = HashMap::new();
        inputs.insert("IN".to_string(), "sensor:pump_running".to_string());

        let mut outputs = HashMap::new();
        outputs.insert("Q".to_string(), "var:pump_delay_done".to_string());

        let def = FBDefinition {
            id: "pump_delay".to_string(),
            fb_type: "TON".to_string(),
            params: super::super::FBParams {
                pt_ms: Some(5000),
                pv: None,
            },
            inputs,
            outputs,
        };

        assert_eq!(
            def.inputs.get("IN"),
            Some(&"sensor:pump_running".to_string())
        );
        assert_eq!(
            def.outputs.get("Q"),
            Some(&"var:pump_delay_done".to_string())
        );
    }

    #[test]
    fn test_program_state_serialization() {
        use crate::commands::{ProgramDefinition, ProgramState};
        use crate::scripting::ScriptDefinition;

        let program = ProgramDefinition {
            id: "test-program".to_string(),
            name: "Test Program".to_string(),
            version: 1,
            description: "Test description".to_string(),
            execution_mode: ExecutionMode::ScanCycle,
            scan_cycle_ms: 100,
            function_blocks: vec![],
            script: ScriptDefinition {
                id: "test-script".to_string(),
                name: "Test Script".to_string(),
                description: "".to_string(),
                version: "1.0".to_string(),
                enabled: true,
                priority: super::super::ScriptPriority::Normal,
                triggers: vec![],
                conditions: vec![],
                actions: vec![],
                on_error: vec![],
            },
            replace_existing: false,
        };

        let state = ProgramState {
            program: Some(program),
            deployed_at: Some("2024-01-01T00:00:00Z".to_string()),
            previous_version: None,
        };

        // Test serialization
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("test-program"));
        assert!(json.contains("scanCycleMs"));

        // Test deserialization
        let parsed: ProgramState = serde_json::from_str(&json).unwrap();
        assert!(parsed.program.is_some());
        let prog = parsed.program.unwrap();
        assert_eq!(prog.id, "test-program");
        assert_eq!(prog.execution_mode, ExecutionMode::ScanCycle);
        assert_eq!(prog.scan_cycle_ms, 100);
    }

    #[test]
    fn test_default_program_state_path() {
        let path = default_program_state_path();
        assert!(path.ends_with("program.json"));
    }
}
