# Remittance Split Contract

A Soroban smart contract for configuring and executing percentage-based USDC distributions
across spending, savings, bills, and insurance categories.

## Security Model

`distribute_usdc` is the only function that moves funds. It enforces the following invariants
in strict order before any token interaction occurs:

1. **Auth first** — `from.require_auth()` is the very first operation; no state is read before
   the caller proves authority.
2. **Pause guard** — the contract must not be globally paused.
3. **Owner-only** — `from` must equal the address stored as `config.owner` at initialization.
   Any other address is rejected with `Unauthorized`, even if it can self-authorize.
4. **Trusted token** — `usdc_contract` must match the address pinned in `config.usdc_contract`
   at initialization time. Passing a different address returns `UntrustedTokenContract`,
   preventing token-substitution attacks.
5. **Amount validation** — `total_amount` must be > 0.
6. **Self-transfer guard** — none of the four destination accounts may equal `from`.
   Returns `SelfTransferNotAllowed` if any match.
7. **Replay protection** — nonce must equal `get_nonce(from)` and is incremented after success.
8. **Audit + event** — a `DistributionCompleted` event is emitted on success for off-chain indexing.

## Features

- Percentage-based allocation (spending / savings / bills / insurance, must sum to 100)
- Hardened `distribute_usdc` with 7-layer auth checks
- Nonce-based replay protection on all state-changing operations
- Pause / unpause with admin controls
- Remittance schedules (create / modify / cancel)
- Snapshot export/import with checksum verification
- Audit log (last 100 entries, ring-buffer)
- TTL extension on every state-changing call
- **FNV-1a checksum** over all config fields and metadata to detect tampering
- **Version-gated** snapshot format with min/max boundary enforcement

## Quickstart

```rust
// 1. Initialize — pin the trusted USDC contract address at setup time
client.initialize_split(
    &owner,
    &0,           // nonce
    &usdc_addr,   // trusted token contract — immutable after init
    &50,          // spending %
    &30,          // savings %
    &15,          // bills %
    &5,           // insurance %
);

// 2. Distribute
client.distribute_usdc(
    &usdc_addr,   // must match the address stored at init
    &owner,       // must be config.owner and must authorize
    &1,           // nonce (increments after each call)
    &AccountGroup { spending, savings, bills, insurance },
    &1_000_0000000, // stroops
);
```

## API Reference

### Data Structures

#### `SplitConfig`

```rust
pub struct SplitConfig {
    pub owner: Address,
    pub spending_percent: u32,
    pub savings_percent: u32,
    pub bills_percent: u32,
    pub insurance_percent: u32,
    pub timestamp: u64,
    pub initialized: bool,
    /// Trusted USDC contract address — pinned at initialization, validated on every distribute_usdc call.
    pub usdc_contract: Address,
}
```

#### `AccountGroup`

```rust
pub struct AccountGroup {
    pub spending: Address,
    pub savings: Address,
    pub bills: Address,
    pub insurance: Address,
}
```

### Functions

#### `initialize_split(env, owner, nonce, usdc_contract, spending_percent, savings_percent, bills_percent, insurance_percent) -> bool`

Initializes the split configuration and pins the trusted USDC token contract address.

- `owner` must authorize.
- `usdc_contract` is stored immutably and validated on every `distribute_usdc` call.
- Percentages must sum to exactly 100.
- Can only be called once (`AlreadyInitialized` on repeat).

#### `distribute_usdc(env, usdc_contract, from, nonce, accounts, total_amount) -> bool`

Distributes USDC from `from` to the four split destination accounts.

**Security checks (in order):**
1. `from.require_auth()`
2. Contract not paused
3. `from == config.owner`
4. `usdc_contract == config.usdc_contract`
5. `total_amount > 0`
6. No destination account equals `from`
7. Nonce matches

**Errors:**
| Error | Condition |
|---|---|
| `Unauthorized` | Caller is not the config owner, or contract is paused |
| `UntrustedTokenContract` | `usdc_contract` ≠ stored trusted address |
| `SelfTransferNotAllowed` | Any destination account equals `from` |
| `InvalidAmount` | `total_amount` ≤ 0 |
| `NotInitialized` | Contract not yet initialized |
| `InvalidNonce` | Replay attempt |

#### `update_split(env, caller, nonce, spending_percent, savings_percent, bills_percent, insurance_percent) -> bool`

Updates split percentages. Owner-only, nonce-protected.

---

### Snapshot Export / Import

#### `export_snapshot(env, caller) -> Option<ExportSnapshot>`

Exports the current split configuration as a portable, integrity-verified snapshot.

The snapshot includes a **FNV-1a checksum** computed over:
- snapshot `version`
- all four percentage fields
- `config.timestamp`
- `config.initialized` flag
- `exported_at` (ledger timestamp at export time)

**Parameters:**
- `caller`: Address of the owner (must authorize)

**Returns:** `Some(ExportSnapshot)` on success, `None` if not initialized

**Events:** emits `SplitEvent::SnapshotExported`

**ExportSnapshot structure:**
```rust
pub struct ExportSnapshot {
    pub version: u32,      // snapshot format version (currently 2)
    pub checksum: u64,     // FNV-1a integrity hash
    pub config: SplitConfig,
    pub exported_at: u64,  // ledger timestamp at export
}
```

---

#### `import_snapshot(env, caller, nonce, snapshot) -> bool`

Restores a split configuration from a previously exported snapshot.

**Integrity checks performed (in order):**

| # | Check | Error |
|---|-------|-------|
| 1 | `snapshot.version` within `[MIN_SNAPSHOT_VERSION, SNAPSHOT_VERSION]` | `UnsupportedVersion` |
| 2 | FNV-1a checksum matches recomputed value | `ChecksumMismatch` |
| 3 | `snapshot.config.initialized == true` | `SnapshotNotInitialized` |
| 4 | Each percentage field `<= 100` | `InvalidPercentageRange` |
| 5 | Sum of percentages `== 100` | `InvalidPercentages` |
| 6 | `config.timestamp` and `exported_at` not in the future | `FutureTimestamp` |
| 7 | Caller is the current contract owner | `Unauthorized` |
| 8 | `snapshot.config.owner == caller` | `OwnerMismatch` |

**Parameters:**
- `caller`: Address of the caller (must be current owner and snapshot owner)
- `nonce`: Replay-protection nonce (must equal current stored nonce)
- `snapshot`: `ExportSnapshot` returned by `export_snapshot`

**Returns:** `true` on success

**Events:** emits `SplitEvent::SnapshotImported`

**Note:** `nonce` is only incremented by `initialize_split` and `import_snapshot`. `update_split` checks the nonce but does **not** increment it.

---

#### `verify_snapshot(env, snapshot) -> bool`

Read-only integrity check for a snapshot payload — performs all structural checks (version, checksum, initialized flag, percentage ranges and sum, timestamp bounds) without requiring authorization or modifying state.

**Parameters:**
- `snapshot`: `ExportSnapshot` to verify

**Returns:** `true` if all integrity checks pass, `false` otherwise

**Use case:** pre-flight validation before calling `import_snapshot`, or off-chain verification of exported payloads.

#### `calculate_split(env, total_amount) -> Vec<i128>`

Pure calculation — returns `[spending, savings, bills, insurance]` amounts.
Insurance receives the integer-division remainder to guarantee `sum == total_amount`.

#### `get_config(env) -> Option<SplitConfig>`

Returns the current configuration, or `None` if not initialized.

#### `get_nonce(env, address) -> u64`

Returns the current nonce for `address`. Pass this value as the `nonce` argument on the next call.

## Error Reference

```rust
pub enum RemittanceSplitError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    PercentagesDoNotSumTo100 = 3,
    InvalidAmount = 4,
    Overflow = 5,
    Unauthorized = 6,
    InvalidNonce = 7,
    UnsupportedVersion = 8,
    ChecksumMismatch = 9,
    InvalidDueDate = 10,
    ScheduleNotFound = 11,
    UntrustedTokenContract = 12,   // NEW: token substitution attack prevention
    SelfTransferNotAllowed = 13,   // NEW: self-transfer guard
}
```

## Events

| Topic | Data | When |
|---|---|---|
| `("split", Initialized)` | `owner: Address` | `initialize_split` succeeds |
| `("split", Updated)` | `caller: Address` | `update_split` succeeds |
| `("split", Calculated)` | `total_amount: i128` | `calculate_split` called |
| `("split", DistributionCompleted)` | `(from: Address, total_amount: i128)` | `distribute_usdc` succeeds |
| `("split", SnapshotExported)` | `caller: Address` | `export_snapshot` succeeds |
| `("split", SnapshotImported)` | `caller: Address` | `import_snapshot` succeeds |

## Security Assumptions

- The `usdc_contract` address passed to `initialize_split` must be a legitimate SEP-41 token.
  The contract does not verify the token's bytecode — it trusts the address provided at init.
- The owner is responsible for keeping their signing key secure. There is no key rotation
  mechanism; deploy a new contract instance if ownership must change.
- Nonces are per-address and stored in instance storage. They are not shared across contract
  instances.
- The pause mechanism is a defense-in-depth control. It does not protect against a compromised
  owner key.

## Running Tests

```bash
cargo test -p remittance_split
```

Test coverage includes:
- Happy-path distribution with real SAC token balances verified
- All 7 auth checks individually (owner, token, self-transfer, pause, nonce, amount, init)
- Replay attack prevention
- Rounding correctness (sum always equals total)
- Overflow detection for large i128 values
- Boundary percentages (100/0/0/0, 0/0/0/100, 25/25/25/25)
- Multiple sequential distributions with nonce advancement
- Event emission verification
- TTL extension
- Snapshot export/import integrity (checksum, version, ownership, timestamp, nonce)

## Snapshot Security

- **Tamper detection**: the FNV-1a checksum covers every config field and the `exported_at` timestamp; any bit flip in the payload produces a different checksum and is rejected with `ChecksumMismatch`
- **Version gating**: snapshots from unknown future versions or deprecated past versions are rejected with `UnsupportedVersion`; current supported range is `[2, 2]`
- **Ownership binding**: a snapshot can only be imported by the address recorded in `snapshot.config.owner`; cross-owner replay is rejected with `OwnerMismatch`
- **Time sanity**: both `config.timestamp` and `exported_at` must not exceed the current ledger timestamp; future-dated snapshots are rejected with `FutureTimestamp`
- **Replay protection**: `import_snapshot` requires the caller's current nonce, then increments it, preventing the same snapshot from being replayed
- **Pre-flight verify**: use `verify_snapshot` to validate a snapshot payload off-chain or before submitting `import_snapshot`
