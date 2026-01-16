//! Parallel Branch Execution for SFC (IEC 61131-3)
//!
//! Supports parallel divergence and convergence patterns from
//! Sequential Function Charts (SFC):
//!
//! ```text
//!     ┌─────┐
//!     │ S1  │
//!     └──┬──┘
//!        │
//!    ┌───┴───┐          <- Parallel Divergence
//!    ↓       ↓
//! ┌─────┐ ┌─────┐
//! │ S2  │ │ S3  │       <- Parallel Branches
//! └──┬──┘ └──┬──┘
//!    │       │
//!    └───┬───┘          <- Parallel Convergence (AND/OR)
//!        ↓
//!     ┌─────┐
//!     │ S4  │
//!     └─────┘
//! ```
//!
//! v2.1 Features:
//! - AND synchronization (all branches must complete)
//! - OR synchronization (first branch to complete)
//! - Timeout per branch
//! - Cancellation support

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::{Action, ActionResult, ActionType};

// ============================================================================
// Types
// ============================================================================

/// Synchronization type for parallel convergence
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncType {
    /// All branches must complete before continuing (default)
    And,
    /// First branch to complete allows continuation (others cancelled)
    Or,
    /// All branches must complete, but one timeout doesn't fail the whole
    AllSettled,
}

impl Default for SyncType {
    fn default() -> Self {
        SyncType::And
    }
}

/// Parallel branch definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelBranch {
    /// Unique branch ID
    pub id: String,
    /// Actions to execute in this branch
    pub actions: Vec<Action>,
    /// Optional timeout for this branch (ms)
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Branch priority (higher = more important in OR mode)
    #[serde(default)]
    pub priority: u8,
}

/// Parallel divergence point (split into multiple branches)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelDivergence {
    /// Unique ID for this parallel block
    pub id: String,
    /// Branches to execute in parallel
    pub branches: Vec<ParallelBranch>,
    /// How to synchronize at convergence
    #[serde(default)]
    pub sync_type: SyncType,
    /// Overall timeout for all branches (ms)
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Result of a single branch execution
#[derive(Debug, Clone)]
pub struct BranchResult {
    pub branch_id: String,
    pub success: bool,
    pub results: Vec<ActionResult>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
}

impl BranchResult {
    pub fn success(branch_id: String, results: Vec<ActionResult>, duration_ms: u64) -> Self {
        Self {
            branch_id,
            success: true,
            results,
            duration_ms,
            timed_out: false,
            cancelled: false,
        }
    }

    pub fn failure(branch_id: String, results: Vec<ActionResult>, duration_ms: u64) -> Self {
        Self {
            branch_id,
            success: false,
            results,
            duration_ms,
            timed_out: false,
            cancelled: false,
        }
    }

    pub fn timeout(branch_id: String, results: Vec<ActionResult>, duration_ms: u64) -> Self {
        Self {
            branch_id,
            success: false,
            results,
            duration_ms,
            timed_out: true,
            cancelled: false,
        }
    }

    pub fn cancelled(branch_id: String) -> Self {
        Self {
            branch_id,
            success: false,
            results: vec![],
            duration_ms: 0,
            timed_out: false,
            cancelled: true,
        }
    }
}

/// Result of parallel execution
#[derive(Debug, Clone)]
pub struct ParallelResult {
    pub divergence_id: String,
    pub sync_type: SyncType,
    pub branch_results: Vec<BranchResult>,
    pub total_duration_ms: u64,
    pub all_success: bool,
    pub any_success: bool,
}

impl ParallelResult {
    pub fn new(
        divergence_id: String,
        sync_type: SyncType,
        branch_results: Vec<BranchResult>,
        duration_ms: u64,
    ) -> Self {
        let all_success = branch_results.iter().all(|r| r.success);
        let any_success = branch_results.iter().any(|r| r.success);

        Self {
            divergence_id,
            sync_type,
            branch_results,
            total_duration_ms: duration_ms,
            all_success,
            any_success,
        }
    }

    /// Check if parallel execution succeeded based on sync type
    pub fn is_success(&self) -> bool {
        match self.sync_type {
            SyncType::And => self.all_success,
            SyncType::Or => self.any_success,
            SyncType::AllSettled => true, // Always "succeeds" - caller checks individual results
        }
    }
}

// ============================================================================
// Parallel Executor
// ============================================================================

/// Executor for parallel branches
pub struct ParallelExecutor {
    /// Maximum number of concurrent branches
    max_concurrent: usize,
    /// Default timeout per branch (ms)
    default_timeout_ms: u64,
}

impl ParallelExecutor {
    /// Create new parallel executor
    pub fn new() -> Self {
        Self {
            max_concurrent: 10,
            default_timeout_ms: 30000, // 30 seconds
        }
    }

    /// Create with custom settings
    pub fn with_settings(max_concurrent: usize, default_timeout_ms: u64) -> Self {
        Self {
            max_concurrent: max_concurrent.max(1),
            default_timeout_ms,
        }
    }

    /// Execute parallel divergence with action executor function
    ///
    /// The `action_executor` function is called for each action and should
    /// return the ActionResult. This allows the engine to provide its own
    /// action execution logic.
    pub async fn execute<F, Fut>(
        &self,
        divergence: &ParallelDivergence,
        action_executor: F,
    ) -> ParallelResult
    where
        F: Fn(Action) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = ActionResult> + Send,
    {
        let start = Instant::now();
        let branch_count = divergence.branches.len().min(self.max_concurrent);

        info!(
            divergence_id = %divergence.id,
            branch_count = branch_count,
            sync_type = ?divergence.sync_type,
            "Starting parallel execution"
        );

        if divergence.branches.is_empty() {
            return ParallelResult::new(divergence.id.clone(), divergence.sync_type, vec![], 0);
        }

        // Create channels for branch completion
        let (tx, mut rx) = mpsc::channel::<BranchResult>(branch_count);

        // Spawn branch tasks
        let mut handles = Vec::new();
        for branch in divergence.branches.iter().take(self.max_concurrent) {
            let branch = branch.clone();
            let tx = tx.clone();
            let executor = action_executor.clone();
            let timeout = branch
                .timeout_ms
                .or(divergence.timeout_ms)
                .unwrap_or(self.default_timeout_ms);

            let handle = tokio::spawn(async move {
                let result = execute_branch(&branch, executor, timeout).await;
                let _ = tx.send(result).await;
            });
            handles.push(handle);
        }

        // Drop our copy of tx so rx completes when all branches done
        drop(tx);

        // Collect results based on sync type
        let mut results = Vec::new();
        let overall_timeout = divergence.timeout_ms.unwrap_or(self.default_timeout_ms);
        let deadline = Instant::now() + Duration::from_millis(overall_timeout);

        match divergence.sync_type {
            SyncType::And | SyncType::AllSettled => {
                // Wait for ALL branches
                while let Ok(result) =
                    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), rx.recv())
                        .await
                {
                    match result {
                        Some(branch_result) => {
                            debug!(
                                branch_id = %branch_result.branch_id,
                                success = branch_result.success,
                                "Branch completed"
                            );
                            results.push(branch_result);
                        }
                        None => break, // All branches done
                    }
                }

                // If we didn't get all results, some timed out
                if results.len() < branch_count {
                    warn!(
                        got = results.len(),
                        expected = branch_count,
                        "Some branches timed out"
                    );
                    // Cancel remaining handles
                    for handle in &handles {
                        handle.abort();
                    }
                }
            }
            SyncType::Or => {
                // Wait for FIRST successful branch
                while let Ok(result) =
                    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), rx.recv())
                        .await
                {
                    match result {
                        Some(branch_result) => {
                            let success = branch_result.success;
                            results.push(branch_result);

                            if success {
                                debug!("First successful branch completed, cancelling others");
                                // Cancel remaining branches
                                for handle in &handles {
                                    handle.abort();
                                }
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        info!(
            divergence_id = %divergence.id,
            total_branches = results.len(),
            duration_ms = duration_ms,
            "Parallel execution completed"
        );

        ParallelResult::new(
            divergence.id.clone(),
            divergence.sync_type,
            results,
            duration_ms,
        )
    }
}

impl Default for ParallelExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a single branch
async fn execute_branch<F, Fut>(
    branch: &ParallelBranch,
    action_executor: F,
    timeout_ms: u64,
) -> BranchResult
where
    F: Fn(Action) -> Fut,
    Fut: std::future::Future<Output = ActionResult>,
{
    let start = Instant::now();
    let deadline = start + Duration::from_millis(timeout_ms);
    let mut results = Vec::new();
    let mut all_success = true;

    for action in &branch.actions {
        // Check timeout before each action
        if Instant::now() >= deadline {
            warn!(branch_id = %branch.id, "Branch timed out");
            return BranchResult::timeout(
                branch.id.clone(),
                results,
                start.elapsed().as_millis() as u64,
            );
        }

        let result = action_executor(action.clone()).await;
        if !result.success {
            all_success = false;
        }
        results.push(result);
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    if all_success {
        BranchResult::success(branch.id.clone(), results, duration_ms)
    } else {
        BranchResult::failure(branch.id.clone(), results, duration_ms)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn create_test_action(id: &str) -> Action {
        Action {
            action_type: ActionType::Log,
            target: String::new(),
            value: None,
            message: Some(format!("Test action {}", id)),
            level: None,
            device: None,
            address: None,
            delay_ms: None,
            script_id: None,
            condition: None,
            scope: None,
        }
    }

    #[tokio::test]
    async fn test_parallel_and_sync() {
        let executor = ParallelExecutor::new();

        let divergence = ParallelDivergence {
            id: "test-parallel".to_string(),
            branches: vec![
                ParallelBranch {
                    id: "branch-1".to_string(),
                    actions: vec![create_test_action("1")],
                    timeout_ms: None,
                    priority: 0,
                },
                ParallelBranch {
                    id: "branch-2".to_string(),
                    actions: vec![create_test_action("2")],
                    timeout_ms: None,
                    priority: 0,
                },
            ],
            sync_type: SyncType::And,
            timeout_ms: Some(5000),
        };

        let result = executor
            .execute(&divergence, |_action| async {
                ActionResult::success(ActionType::Log, "Done")
            })
            .await;

        assert!(result.is_success());
        assert_eq!(result.branch_results.len(), 2);
        assert!(result.all_success);
    }

    #[tokio::test]
    async fn test_parallel_or_sync() {
        let executor = ParallelExecutor::new();
        let execution_count = Arc::new(AtomicUsize::new(0));

        let divergence = ParallelDivergence {
            id: "test-or".to_string(),
            branches: vec![
                ParallelBranch {
                    id: "slow-branch".to_string(),
                    actions: vec![create_test_action("slow")],
                    timeout_ms: None,
                    priority: 0,
                },
                ParallelBranch {
                    id: "fast-branch".to_string(),
                    actions: vec![create_test_action("fast")],
                    timeout_ms: None,
                    priority: 0,
                },
            ],
            sync_type: SyncType::Or,
            timeout_ms: Some(5000),
        };

        let count_clone = execution_count.clone();
        let result = executor
            .execute(&divergence, move |_action| {
                let count = count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    ActionResult::success(ActionType::Log, "Done")
                }
            })
            .await;

        assert!(result.is_success());
        assert!(result.any_success);
        // At least one branch should have completed
        assert!(!result.branch_results.is_empty());
    }

    #[tokio::test]
    async fn test_parallel_and_with_failure() {
        let executor = ParallelExecutor::new();

        let divergence = ParallelDivergence {
            id: "test-fail".to_string(),
            branches: vec![
                ParallelBranch {
                    id: "success-branch".to_string(),
                    actions: vec![create_test_action("ok")],
                    timeout_ms: None,
                    priority: 0,
                },
                ParallelBranch {
                    id: "fail-branch".to_string(),
                    actions: vec![create_test_action("fail")],
                    timeout_ms: None,
                    priority: 0,
                },
            ],
            sync_type: SyncType::And,
            timeout_ms: Some(5000),
        };

        let result = executor
            .execute(&divergence, |action| async move {
                if action
                    .message
                    .as_ref()
                    .map(|m| m.contains("fail"))
                    .unwrap_or(false)
                {
                    ActionResult::failure(ActionType::Log, "Intentional failure")
                } else {
                    ActionResult::success(ActionType::Log, "Done")
                }
            })
            .await;

        assert!(!result.is_success()); // AND sync fails if any branch fails
        assert!(!result.all_success);
        assert!(result.any_success);
    }

    #[tokio::test]
    async fn test_parallel_all_settled() {
        let executor = ParallelExecutor::new();

        let divergence = ParallelDivergence {
            id: "test-settled".to_string(),
            branches: vec![
                ParallelBranch {
                    id: "branch-1".to_string(),
                    actions: vec![create_test_action("1")],
                    timeout_ms: None,
                    priority: 0,
                },
                ParallelBranch {
                    id: "branch-2".to_string(),
                    actions: vec![create_test_action("fail")],
                    timeout_ms: None,
                    priority: 0,
                },
            ],
            sync_type: SyncType::AllSettled,
            timeout_ms: Some(5000),
        };

        let result = executor
            .execute(&divergence, |action| async move {
                if action
                    .message
                    .as_ref()
                    .map(|m| m.contains("fail"))
                    .unwrap_or(false)
                {
                    ActionResult::failure(ActionType::Log, "Intentional failure")
                } else {
                    ActionResult::success(ActionType::Log, "Done")
                }
            })
            .await;

        // AllSettled always "succeeds" - caller checks individual results
        assert!(result.is_success());
        assert_eq!(result.branch_results.len(), 2);
    }

    #[tokio::test]
    async fn test_empty_branches() {
        let executor = ParallelExecutor::new();

        let divergence = ParallelDivergence {
            id: "empty".to_string(),
            branches: vec![],
            sync_type: SyncType::And,
            timeout_ms: None,
        };

        let result = executor
            .execute(&divergence, |_| async {
                ActionResult::success(ActionType::Log, "Done")
            })
            .await;

        assert!(result.is_success());
        assert_eq!(result.branch_results.len(), 0);
    }

    #[test]
    fn test_branch_result_constructors() {
        let success = BranchResult::success("b1".to_string(), vec![], 100);
        assert!(success.success);
        assert!(!success.timed_out);
        assert!(!success.cancelled);

        let timeout = BranchResult::timeout("b2".to_string(), vec![], 5000);
        assert!(!timeout.success);
        assert!(timeout.timed_out);

        let cancelled = BranchResult::cancelled("b3".to_string());
        assert!(!cancelled.success);
        assert!(cancelled.cancelled);
    }

    #[test]
    fn test_sync_type_default() {
        assert_eq!(SyncType::default(), SyncType::And);
    }

    #[test]
    fn test_parallel_result_is_success() {
        // AND: all must succeed
        let all_success = ParallelResult::new(
            "test".to_string(),
            SyncType::And,
            vec![
                BranchResult::success("b1".to_string(), vec![], 100),
                BranchResult::success("b2".to_string(), vec![], 100),
            ],
            200,
        );
        assert!(all_success.is_success());

        let one_failed = ParallelResult::new(
            "test".to_string(),
            SyncType::And,
            vec![
                BranchResult::success("b1".to_string(), vec![], 100),
                BranchResult::failure("b2".to_string(), vec![], 100),
            ],
            200,
        );
        assert!(!one_failed.is_success());

        // OR: any can succeed
        let or_one_success = ParallelResult::new(
            "test".to_string(),
            SyncType::Or,
            vec![
                BranchResult::success("b1".to_string(), vec![], 100),
                BranchResult::failure("b2".to_string(), vec![], 100),
            ],
            200,
        );
        assert!(or_one_success.is_success());
    }
}
