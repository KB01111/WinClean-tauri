//! Shared optimizer runtime state managed by Tauri.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::backend::Backends;
use super::helper_client::HelperManager;
use super::orchestrator::Origin;
use super::service_plan::ServiceChange;

/// How long a reviewed plan stays approvable.
pub const PLAN_TTL_MS: i64 = 10 * 60 * 1000;
const MAX_PENDING_PLANS: usize = 16;

/// A plan the backend validated and handed to the user (or the AI layer) for
/// review. The token is single-use and bound to the plan's fingerprint, so
/// "apply" can only ever run the exact plan that was reviewed.
#[derive(Clone, Debug)]
pub struct PreparedPlan {
    pub changes: Vec<ServiceChange>,
    pub fingerprint: String,
    pub origin: Origin,
    pub expires_at_ms: i64,
}

#[derive(Default)]
pub struct PlanStore {
    inner: Mutex<HashMap<String, PreparedPlan>>,
}

impl PlanStore {
    pub fn insert(&self, plan: PreparedPlan, now_ms: i64) -> Result<String, String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| format!("random source failed: {e}"))?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, p| p.expires_at_ms > now_ms);
        if map.len() >= MAX_PENDING_PLANS {
            // Drop the oldest-expiring entry rather than refusing a new review.
            if let Some(oldest) = map.iter().min_by_key(|(_, p)| p.expires_at_ms).map(|(k, _)| k.clone()) {
                map.remove(&oldest);
            }
        }
        map.insert(token.clone(), plan);
        Ok(token)
    }

    /// Consumes the token. A token works once, and only before it expires.
    pub fn take(&self, token: &str, now_ms: i64) -> Result<PreparedPlan, String> {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let plan = map.remove(token).ok_or_else(|| "This plan was already applied or is no longer available. Review it again.".to_string())?;
        if plan.expires_at_ms <= now_ms {
            return Err("This plan expired. Review it again before applying.".to_string());
        }
        Ok(plan)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct OptimizerState {
    pub backends: Backends,
    pub helper: Arc<HelperManager>,
    pub plans: PlanStore,
}

impl OptimizerState {
    pub fn system() -> Self {
        let helper = Arc::new(HelperManager::new(super::helper_windows::system_launcher()));
        Self { backends: Backends::system(helper.clone()), helper, plans: PlanStore::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(expires_at_ms: i64) -> PreparedPlan {
        PreparedPlan { changes: vec![], fingerprint: "fp".into(), origin: Origin::User, expires_at_ms }
    }

    #[test]
    fn a_token_applies_once() {
        let store = PlanStore::default();
        let t = store.insert(plan(1_000), 0).unwrap();
        assert_eq!(t.len(), 32);
        assert!(store.take(&t, 10).is_ok());
        assert!(store.take(&t, 10).unwrap_err().contains("already applied"));
    }

    #[test]
    fn expired_and_unknown_tokens_are_refused() {
        let store = PlanStore::default();
        let t = store.insert(plan(1_000), 0).unwrap();
        assert!(store.take(&t, 1_000).unwrap_err().contains("expired"));
        assert!(store.take("deadbeef", 0).is_err());
    }

    #[test]
    fn tokens_are_unique_and_the_store_is_bounded() {
        let store = PlanStore::default();
        let tokens: std::collections::HashSet<String> = (0..40).map(|_| store.insert(plan(10_000), 0).unwrap()).collect();
        assert_eq!(tokens.len(), 40);
        assert!(store.len() <= MAX_PENDING_PLANS);
    }

    #[test]
    fn expired_plans_are_pruned_on_insert() {
        let store = PlanStore::default();
        store.insert(plan(100), 0).unwrap();
        store.insert(plan(10_000), 5_000).unwrap();
        assert_eq!(store.len(), 1);
    }
}
