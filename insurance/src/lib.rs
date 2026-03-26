#![no_std]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use remitwise_common::CoverageType;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String, Vec,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const THIRTY_DAYS: u64 = 30 * 24 * 60 * 60;
const MAX_NAME_LEN: u32 = 64;
const MAX_EXT_REF_LEN: u32 = 128;
const MAX_POLICIES: u32 = 1_000;
/// coverage_amount <= monthly_premium * 12 * RATIO_MULTIPLIER
const RATIO_MULTIPLIER: i128 = 500;

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Owner,
    PolicyCount,
    Policy(u32),
    ActivePolicies,
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single insurance policy record.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Policy {
    pub id: u32,
    pub owner: Address,
    pub name: String,
    pub coverage_type: CoverageType,
    pub monthly_premium: i128,
    pub coverage_amount: i128,
    pub active: bool,
    pub next_payment_due: u64,
    pub last_payment_at: u64,
    pub created_at: u64,
    pub external_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
pub struct PolicyCreatedEvent {
    pub policy_id: u32,
    pub name: String,
    pub coverage_type: CoverageType,
    pub monthly_premium: i128,
    pub coverage_amount: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct PremiumPaidEvent {
    pub policy_id: u32,
    pub name: String,
    pub amount: i128,
    pub next_payment_date: u64,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct PolicyDeactivatedEvent {
    pub policy_id: u32,
    pub name: String,
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum InsuranceError {
    Unauthorized = 1,
    AlreadyInitialized = 2,
    NotInitialized = 3,
    PolicyNotFound = 4,
    PolicyInactive = 5,
    InvalidName = 6,
    InvalidPremium = 7,
    InvalidCoverageAmount = 8,
    UnsupportedCombination = 9,
    InvalidExternalRef = 10,
    MaxPoliciesReached = 11,
}

// ---------------------------------------------------------------------------
// Per-coverage-type constraints
// ---------------------------------------------------------------------------

struct CoverageConstraints {
    min_premium: i128,
    max_premium: i128,
    min_coverage: i128,
    max_coverage: i128,
}

fn constraints_for(ct: CoverageType) -> CoverageConstraints {
    match ct {
        CoverageType::Health => CoverageConstraints {
            min_premium: 1_000_000,
            max_premium: 500_000_000,
            min_coverage: 10_000_000,
            max_coverage: 100_000_000_000,
        },
        CoverageType::Life => CoverageConstraints {
            min_premium: 500_000,
            max_premium: 1_000_000_000,
            min_coverage: 50_000_000,
            max_coverage: 500_000_000_000,
        },
        CoverageType::Property => CoverageConstraints {
            min_premium: 2_000_000,
            max_premium: 2_000_000_000,
            min_coverage: 100_000_000,
            max_coverage: 1_000_000_000_000,
        },
        CoverageType::Auto => CoverageConstraints {
            min_premium: 1_500_000,
            max_premium: 750_000_000,
            min_coverage: 20_000_000,
            max_coverage: 200_000_000_000,
        },
        CoverageType::Liability => CoverageConstraints {
            min_premium: 800_000,
            max_premium: 400_000_000,
            min_coverage: 5_000_000,
            max_coverage: 50_000_000_000,
        },
    }
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct InsuranceContract;

#[contractimpl]
impl InsuranceContract {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Initialize the contract. Must be called exactly once.
    ///
    /// # Authorization
    /// `owner` must sign.
    pub fn init(env: Env, owner: Address) {
        owner.require_auth();
        if env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::Owner)
            .is_some()
        {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Owner, &owner);
        env.storage().instance().set(&DataKey::PolicyCount, &0u32);
        let empty: Vec<u32> = Vec::new(&env);
        env.storage()
            .instance()
            .set(&DataKey::ActivePolicies, &empty);
    }

    // -----------------------------------------------------------------------
    // create_policy
    // -----------------------------------------------------------------------

    /// Create a new insurance policy.
    ///
    /// # Authorization
    /// `caller` must sign. Any authenticated address may create a policy.
    ///
    /// # Errors (panics)
    /// - `"not initialized"` — `init` was never called
    /// - `"name cannot be empty"` — empty name
    /// - `"name too long"` — name exceeds 64 bytes
    /// - `"monthly_premium must be positive"` — premium ≤ 0
    /// - `"coverage_amount must be positive"` — coverage ≤ 0
    /// - `"monthly_premium out of range for coverage type"` — outside per-type bounds
    /// - `"coverage_amount out of range for coverage type"` — outside per-type bounds
    /// - `"unsupported combination: coverage_amount too high relative to premium"` — ratio guard
    /// - `"external_ref length out of range"` — ext_ref empty or > 128 bytes
    /// - `"max policies reached"` — active policy count ≥ 1,000
    pub fn create_policy(
        env: Env,
        caller: Address,
        name: String,
        coverage_type: CoverageType,
        monthly_premium: i128,
        coverage_amount: i128,
        external_ref: Option<String>,
    ) -> u32 {
        caller.require_auth();
        Self::require_initialized(&env);

        // Name validation
        if name.len() == 0 {
            panic!("name cannot be empty");
        }
        if name.len() > MAX_NAME_LEN {
            panic!("name too long");
        }

        // Numeric sign checks
        if monthly_premium <= 0 {
            panic!("monthly_premium must be positive");
        }
        if coverage_amount <= 0 {
            panic!("coverage_amount must be positive");
        }

        // Per-type range checks
        let c = constraints_for(coverage_type);
        if monthly_premium < c.min_premium || monthly_premium > c.max_premium {
            panic!("monthly_premium out of range for coverage type");
        }
        if coverage_amount < c.min_coverage || coverage_amount > c.max_coverage {
            panic!("coverage_amount out of range for coverage type");
        }

        // Ratio guard: coverage_amount <= monthly_premium * 12 * 500
        let annual = monthly_premium
            .checked_mul(12)
            .expect("overflow in annual premium");
        let max_coverage = annual
            .checked_mul(RATIO_MULTIPLIER)
            .expect("overflow in ratio guard");
        if coverage_amount > max_coverage {
            panic!("unsupported combination: coverage_amount too high relative to premium");
        }

        // External ref validation
        if let Some(ref r) = external_ref {
            if r.len() == 0 || r.len() > MAX_EXT_REF_LEN {
                panic!("external_ref length out of range");
            }
        }

        // Capacity check
        let mut active: Vec<u32> = env
            .storage()
            .instance()
            .get(&DataKey::ActivePolicies)
            .unwrap_or_else(|| Vec::new(&env));
        if active.len() >= MAX_POLICIES {
            panic!("max policies reached");
        }

        // Assign ID
        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PolicyCount)
            .unwrap_or(0);
        let policy_id = count.checked_add(1).expect("policy id overflow");
        env.storage()
            .instance()
            .set(&DataKey::PolicyCount, &policy_id);

        let now = env.ledger().timestamp();
        let policy = Policy {
            id: policy_id,
            owner: caller,
            name: name.clone(),
            coverage_type,
            monthly_premium,
            coverage_amount,
            active: true,
            next_payment_due: now.saturating_add(THIRTY_DAYS),
            last_payment_at: 0,
            created_at: now,
            external_ref,
        };

        env.storage()
            .instance()
            .set(&DataKey::Policy(policy_id), &policy);

        active.push_back(policy_id);
        env.storage()
            .instance()
            .set(&DataKey::ActivePolicies, &active);

        env.events().publish(
            (symbol_short!("created"), symbol_short!("policy")),
            PolicyCreatedEvent {
                policy_id,
                name,
                coverage_type,
                monthly_premium,
                coverage_amount,
                timestamp: now,
            },
        );

        policy_id
    }

    // -----------------------------------------------------------------------
    // pay_premium
    // -----------------------------------------------------------------------

    /// Record a premium payment. `amount` must equal the policy's `monthly_premium`.
    ///
    /// # Authorization
    /// `caller` must sign. Any authenticated address may pay (not restricted to owner).
    ///
    /// # Errors (panics)
    /// - `"not initialized"`
    /// - `"policy not found"`
    /// - `"policy inactive"`
    /// - `"amount must equal monthly_premium"`
    pub fn pay_premium(env: Env, caller: Address, policy_id: u32, amount: i128) -> bool {
        caller.require_auth();
        Self::require_initialized(&env);

        let mut policy: Policy = env
            .storage()
            .instance()
            .get(&DataKey::Policy(policy_id))
            .unwrap_or_else(|| panic!("policy not found"));

        if !policy.active {
            panic!("policy inactive");
        }
        if amount != policy.monthly_premium {
            panic!("amount must equal monthly_premium");
        }

        let now = env.ledger().timestamp();
        policy.last_payment_at = now;
        policy.next_payment_due = now.saturating_add(THIRTY_DAYS);

        let name = policy.name.clone();
        let next = policy.next_payment_due;

        env.storage()
            .instance()
            .set(&DataKey::Policy(policy_id), &policy);

        env.events().publish(
            (symbol_short!("paid"), symbol_short!("premium")),
            PremiumPaidEvent {
                policy_id,
                name,
                amount,
                next_payment_date: next,
                timestamp: now,
            },
        );

        true
    }

    // -----------------------------------------------------------------------
    // deactivate_policy
    // -----------------------------------------------------------------------

    /// Deactivate a policy. Owner-only.
    ///
    /// # Authorization
    /// `owner` must sign and must be the contract owner set during `init`.
    ///
    /// # Errors (panics)
    /// - `"not initialized"`
    /// - `"unauthorized"` — caller is not the contract owner
    /// - `"policy not found"`
    /// - `"policy already inactive"`
    pub fn deactivate_policy(env: Env, owner: Address, policy_id: u32) -> bool {
        owner.require_auth();
        Self::require_initialized(&env);
        Self::require_owner(&env, &owner);

        let mut policy: Policy = env
            .storage()
            .instance()
            .get(&DataKey::Policy(policy_id))
            .unwrap_or_else(|| panic!("policy not found"));

        if !policy.active {
            panic!("policy already inactive");
        }

        policy.active = false;
        let name = policy.name.clone();
        env.storage()
            .instance()
            .set(&DataKey::Policy(policy_id), &policy);

        // Remove from active list
        let mut active: Vec<u32> = env
            .storage()
            .instance()
            .get(&DataKey::ActivePolicies)
            .unwrap_or_else(|| Vec::new(&env));
        let mut new_active: Vec<u32> = Vec::new(&env);
        for id in active.iter() {
            if id != policy_id {
                new_active.push_back(id);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::ActivePolicies, &new_active);

        let now = env.ledger().timestamp();
        env.events().publish(
            (symbol_short!("deactive"), symbol_short!("policy")),
            PolicyDeactivatedEvent {
                policy_id,
                name,
                timestamp: now,
            },
        );

        true
    }

    // -----------------------------------------------------------------------
    // set_external_ref
    // -----------------------------------------------------------------------

    /// Update or clear the external reference on a policy. Owner-only.
    ///
    /// # Authorization
    /// `owner` must sign and must be the contract owner.
    ///
    /// # Errors (panics)
    /// - `"not initialized"`
    /// - `"unauthorized"`
    /// - `"policy not found"`
    /// - `"external_ref length out of range"` — if provided, must be 1–128 bytes
    pub fn set_external_ref(
        env: Env,
        owner: Address,
        policy_id: u32,
        ext_ref: Option<String>,
    ) -> bool {
        owner.require_auth();
        Self::require_initialized(&env);
        Self::require_owner(&env, &owner);

        let mut policy: Policy = env
            .storage()
            .instance()
            .get(&DataKey::Policy(policy_id))
            .unwrap_or_else(|| panic!("policy not found"));

        if let Some(ref r) = ext_ref {
            if r.len() == 0 || r.len() > MAX_EXT_REF_LEN {
                panic!("external_ref length out of range");
            }
        }

        policy.external_ref = ext_ref;
        env.storage()
            .instance()
            .set(&DataKey::Policy(policy_id), &policy);

        true
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Returns all active policy IDs.
    pub fn get_active_policies(env: Env) -> Vec<u32> {
        Self::require_initialized(&env);
        env.storage()
            .instance()
            .get(&DataKey::ActivePolicies)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Returns the full `Policy` record. Panics if not found.
    pub fn get_policy(env: Env, policy_id: u32) -> Policy {
        Self::require_initialized(&env);
        env.storage()
            .instance()
            .get(&DataKey::Policy(policy_id))
            .unwrap_or_else(|| panic!("policy not found"))
    }

    /// Returns the sum of `monthly_premium` across all active policies.
    pub fn get_total_monthly_premium(env: Env) -> i128 {
        Self::require_initialized(&env);
        let active: Vec<u32> = env
            .storage()
            .instance()
            .get(&DataKey::ActivePolicies)
            .unwrap_or_else(|| Vec::new(&env));
        let mut total: i128 = 0;
        for id in active.iter() {
            if let Some(policy) = env
                .storage()
                .instance()
                .get::<_, Policy>(&DataKey::Policy(id))
            {
                if policy.active {
                    total = total.saturating_add(policy.monthly_premium);
                }
            }
        }
        total
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn require_initialized(env: &Env) {
        if env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::Owner)
            .is_none()
        {
            panic!("not initialized");
        }
    }

    fn require_owner(env: &Env, caller: &Address) {
        let owner: Address = env
            .storage()
            .instance()
            .get(&DataKey::Owner)
            .unwrap_or_else(|| panic!("not initialized"));
        if *caller != owner {
            panic!("unauthorized");
        }
    }
}

// Re-export as `Insurance` for backward-compat with existing tests that use
// `Insurance` / `InsuranceClient` names.
pub use InsuranceContract as Insurance;

#[cfg(test)]
mod test;
