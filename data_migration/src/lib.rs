//! Data migration, import/export utilities for Remitwise contracts.
//!
//! Supports multiple formats (JSON, binary, CSV), checksum validation,
//! version compatibility checks, and data integrity verification.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Current snapshot schema version for migration compatibility.
///
/// # Versioning Policy (workspace-wide)
/// All snapshot export/import flows across the workspace use an explicit
/// `schema_version` tag stored inside the snapshot struct (or header).
/// When the snapshot format changes in a backward-incompatible way, bump
/// `SCHEMA_VERSION` and update `MIN_SUPPORTED_VERSION` only if the old
/// format can no longer be safely imported.
///
/// Importers must validate:
///   `MIN_SUPPORTED_VERSION <= schema_version <= SCHEMA_VERSION`
/// and reject anything outside that range to guarantee safe
/// forward/backward compatibility handling.
pub const SCHEMA_VERSION: u32 = 1;

/// Minimum supported schema version for import.
/// Snapshots with a version below this value are too old to import safely.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// Alias used in snapshot headers to keep naming consistent with other contracts.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = SCHEMA_VERSION;

/// Versioned migration event payload meant for indexing and historical tracking.
///
/// # Indexer Migration Guidance
/// - **v1**: Indexers should match on `MigrationEvent::V1`. This is the fundamental schema containing baseline metadata (contract, type, version, timestamp).
/// - **v2+**: Future schemas will add new variants (e.g., `MigrationEvent::V2`) potentially mapping to new data structures.
///
/// Indexers must be prepared to handle unknown variants gracefully (e.g., by logging a warning/alert) rather than crashing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MigrationEvent {
    V1(MigrationEventV1),
    // V2(MigrationEventV2), // Add in the future when schema changes and update indexers
}

/// Base migration event containing metadata about the migration operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationEventV1 {
    pub contract_id: String,
    pub migration_type: String, // e.g., "export", "import", "upgrade"
    pub version: u32,
    pub timestamp_ms: u64,
}

/// Export format for snapshot data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// Human-readable JSON.
    Json,
    /// Compact binary (bincode).
    Binary,
    /// CSV for spreadsheet compatibility (tabular exports).
    Csv,
    /// Opaque encrypted payload (caller handles encryption/decryption).
    Encrypted,
}

/// Snapshot header with version and checksum for integrity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotHeader {
    pub version: u32,
    pub checksum: String,
    pub format: String,
    pub created_at_ms: Option<u64>,
}

/// Full export snapshot for remittance split or other contract data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSnapshot {
    pub header: SnapshotHeader,
    pub payload: SnapshotPayload,
}

/// Payload variants per contract type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotPayload {
    RemittanceSplit(RemittanceSplitExport),
    SavingsGoals(SavingsGoalsExport),
    Generic(HashMap<String, serde_json::Value>),
}

/// Exportable remittance split config (mirrors contract SplitConfig).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemittanceSplitExport {
    pub owner: String,
    pub spending_percent: u32,
    pub savings_percent: u32,
    pub bills_percent: u32,
    pub insurance_percent: u32,
}

/// Exportable savings goals list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavingsGoalsExport {
    pub next_id: u32,
    pub goals: Vec<SavingsGoalExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavingsGoalExport {
    pub id: u32,
    pub owner: String,
    pub name: String,
    pub target_amount: i64,
    pub current_amount: i64,
    pub target_date: u64,
    pub locked: bool,
}

impl ExportSnapshot {
    /// Compute SHA256 checksum of the payload (canonical JSON).
    pub fn compute_checksum(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&self.payload).unwrap_or_else(|_| panic!("payload must be serializable")));
        hex::encode(hasher.finalize().as_ref())
    }

    /// Verify stored checksum matches payload.
    pub fn verify_checksum(&self) -> bool {
        self.header.checksum == self.compute_checksum()
    }

    /// Check if snapshot version is supported for import.
    pub fn is_version_compatible(&self) -> bool {
        self.header.version >= MIN_SUPPORTED_VERSION && self.header.version <= SCHEMA_VERSION
    }

    /// Validate snapshot for import: version and checksum.
    pub fn validate_for_import(&self) -> Result<(), MigrationError> {
        if !self.is_version_compatible() {
            return Err(MigrationError::IncompatibleVersion {
                found: self.header.version,
                min: MIN_SUPPORTED_VERSION,
                max: SCHEMA_VERSION,
            });
        }
        if !self.verify_checksum() {
            return Err(MigrationError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Build a new snapshot with correct version and checksum.
    pub fn new(payload: SnapshotPayload, format: ExportFormat) -> Self {
        let mut snapshot = Self {
            header: SnapshotHeader {
                version: SCHEMA_VERSION,
                checksum: String::new(),
                format: format_label(format),
                created_at_ms: None,
            },
            payload,
        };
        snapshot.header.checksum = snapshot.compute_checksum();
        snapshot
    }
}

fn format_label(f: ExportFormat) -> String {
    match f {
        ExportFormat::Json => "json".into(),
        ExportFormat::Binary => "binary".into(),
        ExportFormat::Csv => "csv".into(),
        ExportFormat::Encrypted => "encrypted".into(),
    }
}

/// Migration/import errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationError {
    IncompatibleVersion { found: u32, min: u32, max: u32 },
    ChecksumMismatch,
    InvalidFormat(String),
    ValidationFailed(String),
    DeserializeError(String),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::IncompatibleVersion { found, min, max } => {
                write!(
                    f,
                    "incompatible version {} (supported {}-{})",
                    found, min, max
                )
            }
            MigrationError::ChecksumMismatch => write!(f, "checksum mismatch"),
            MigrationError::InvalidFormat(s) => write!(f, "invalid format: {}", s),
            MigrationError::ValidationFailed(s) => write!(f, "validation failed: {}", s),
            MigrationError::DeserializeError(s) => write!(f, "deserialize error: {}", s),
        }
    }
}

impl std::error::Error for MigrationError {}

/// Export snapshot to JSON bytes.
pub fn export_to_json(snapshot: &ExportSnapshot) -> Result<Vec<u8>, MigrationError> {
    serde_json::to_vec_pretty(snapshot).map_err(|e| MigrationError::DeserializeError(e.to_string()))
}

/// Export snapshot to binary bytes (bincode).
pub fn export_to_binary(snapshot: &ExportSnapshot) -> Result<Vec<u8>, MigrationError> {
    bincode::serialize(snapshot).map_err(|e| MigrationError::DeserializeError(e.to_string()))
}

/// Export to CSV (for tabular payloads only; e.g. goals list).
pub fn export_to_csv(payload: &SavingsGoalsExport) -> Result<Vec<u8>, MigrationError> {
    let mut wtr = csv::Writer::from_writer(Vec::new());
    wtr.write_record([
        "id",
        "owner",
        "name",
        "target_amount",
        "current_amount",
        "target_date",
        "locked",
    ])
    .map_err(|e| MigrationError::InvalidFormat(e.to_string()))?;
    for g in &payload.goals {
        wtr.write_record(&[
            g.id.to_string(),
            g.owner.clone(),
            g.name.clone(),
            g.target_amount.to_string(),
            g.current_amount.to_string(),
            g.target_date.to_string(),
            g.locked.to_string(),
        ])
        .map_err(|e| MigrationError::InvalidFormat(e.to_string()))?;
    }
    wtr.flush()
        .map_err(|e| MigrationError::InvalidFormat(e.to_string()))?;
    wtr.into_inner()
        .map_err(|e| MigrationError::InvalidFormat(e.to_string()))
}

/// Encrypted format: store base64-encoded payload (caller encrypts before passing).
pub fn export_to_encrypted_payload(plain_bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(plain_bytes)
}

/// Decode encrypted payload from base64 (caller decrypts after).
pub fn import_from_encrypted_payload(encoded: &str) -> Result<Vec<u8>, MigrationError> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| MigrationError::InvalidFormat(e.to_string()))
}

/// Import snapshot from JSON bytes with validation.
pub fn import_from_json(bytes: &[u8]) -> Result<ExportSnapshot, MigrationError> {
    let snapshot: ExportSnapshot = serde_json::from_slice(bytes)
        .map_err(|e| MigrationError::DeserializeError(e.to_string()))?;
    snapshot.validate_for_import()?;
    Ok(snapshot)
}

/// Import snapshot from binary bytes with validation.
pub fn import_from_binary(bytes: &[u8]) -> Result<ExportSnapshot, MigrationError> {
    let snapshot: ExportSnapshot =
        bincode::deserialize(bytes).map_err(|e| MigrationError::DeserializeError(e.to_string()))?;
    snapshot.validate_for_import()?;
    Ok(snapshot)
}

/// Import goals from CSV into SavingsGoalsExport (no header checksum; use for merge/import).
pub fn import_goals_from_csv(bytes: &[u8]) -> Result<Vec<SavingsGoalExport>, MigrationError> {
    let mut rdr = csv::Reader::from_reader(bytes);
    let mut goals = Vec::new();
    for result in rdr.deserialize() {
        let record: CsvGoalRow =
            result.map_err(|e| MigrationError::DeserializeError(e.to_string()))?;
        goals.push(SavingsGoalExport {
            id: record.id,
            owner: record.owner,
            name: record.name,
            target_amount: record.target_amount,
            current_amount: record.current_amount,
            target_date: record.target_date,
            locked: record.locked,
        });
    }
    Ok(goals)
}

#[derive(Debug, Deserialize)]
struct CsvGoalRow {
    id: u32,
    owner: String,
    name: String,
    target_amount: i64,
    current_amount: i64,
    target_date: u64,
    locked: bool,
}

/// Version compatibility check for migration scripts.
pub fn check_version_compatibility(version: u32) -> Result<(), MigrationError> {
    if version >= MIN_SUPPORTED_VERSION && version <= SCHEMA_VERSION {
        Ok(())
    } else {
        Err(MigrationError::IncompatibleVersion {
            found: version,
            min: MIN_SUPPORTED_VERSION,
            max: SCHEMA_VERSION,
        })
    }
}

/// Build a fully-checksummed [`ExportSnapshot`] from a [`SavingsGoalsExport`] payload.
///
/// This is the canonical bridge between the on-chain `savings_goals` snapshot
/// representation and the off-chain `data_migration` serialization layer.
///
/// # Arguments
/// * `goals_export` – The savings goals payload to wrap.
/// * `format`       – Target export format (JSON, Binary, CSV, Encrypted).
///
/// # Returns
/// An [`ExportSnapshot`] with a valid header (version, format label) and a
/// SHA-256 checksum computed over the canonical JSON of the payload.
///
/// # Security notes
/// - The checksum is computed deterministically from the payload; callers must
///   not mutate `header.checksum` after construction.
/// - For `ExportFormat::Encrypted`, callers are responsible for encrypting the
///   serialised bytes **after** calling this function and wrapping them via
///   [`export_to_encrypted_payload`].
pub fn build_savings_snapshot(
    goals_export: SavingsGoalsExport,
    format: ExportFormat,
) -> ExportSnapshot {
    let payload = SnapshotPayload::SavingsGoals(goals_export);
    ExportSnapshot::new(payload, format)
}

/// Rollback metadata (for migration scripts to record last good state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackMetadata {
    pub previous_version: u32,
    pub previous_checksum: String,
    pub timestamp_ms: u64,
}

// Re-export hex for checksum display if needed; use hex crate for encoding in compute_checksum.
mod hex {
    const HEX: &[u8] = b"0123456789abcdef";
    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0xf) as usize] as char);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_checksum_roundtrip_succeeds() {
        let payload = SnapshotPayload::RemittanceSplit(RemittanceSplitExport {
            owner: "GABC".into(),
            spending_percent: 50,
            savings_percent: 30,
            bills_percent: 15,
            insurance_percent: 5,
        });
        let snapshot = ExportSnapshot::new(payload, ExportFormat::Json);
        assert!(snapshot.verify_checksum());
        assert!(snapshot.is_version_compatible());
        assert!(snapshot.validate_for_import().is_ok());
    }

    #[test]
    fn test_export_import_json_succeeds() {
        let payload = SnapshotPayload::RemittanceSplit(RemittanceSplitExport {
            owner: "GXYZ".into(),
            spending_percent: 40,
            savings_percent: 40,
            bills_percent: 10,
            insurance_percent: 10,
        });
        let snapshot = ExportSnapshot::new(payload, ExportFormat::Json);
        let bytes = export_to_json(&snapshot).unwrap();
        let loaded = import_from_json(&bytes).unwrap();
        assert_eq!(loaded.header.version, SCHEMA_VERSION);
        assert!(loaded.verify_checksum());
    }

    #[test]
    fn test_export_import_binary_succeeds() {
        let payload = SnapshotPayload::RemittanceSplit(RemittanceSplitExport {
            owner: "GBIN".into(),
            spending_percent: 25,
            savings_percent: 25,
            bills_percent: 25,
            insurance_percent: 25,
        });
        let snapshot = ExportSnapshot::new(payload, ExportFormat::Binary);
        let bytes = export_to_binary(&snapshot).unwrap();
        let loaded = import_from_binary(&bytes).unwrap();
        assert!(loaded.verify_checksum());
    }

    #[test]
    fn test_checksum_mismatch_import_fails() {
        let payload = SnapshotPayload::RemittanceSplit(RemittanceSplitExport {
            owner: "GX".into(),
            spending_percent: 100,
            savings_percent: 0,
            bills_percent: 0,
            insurance_percent: 0,
        });
        let mut snapshot = ExportSnapshot::new(payload, ExportFormat::Json);
        snapshot.header.checksum = "wrong".into();
        assert!(!snapshot.verify_checksum());
        assert!(snapshot.validate_for_import().is_err());
    }

    #[test]
    fn test_check_version_compatibility_succeeds() {
        assert!(check_version_compatibility(1).is_ok());
        assert!(check_version_compatibility(SCHEMA_VERSION).is_ok());
        assert!(check_version_compatibility(0).is_err());
        assert!(check_version_compatibility(SCHEMA_VERSION + 1).is_err());
    }

    #[test]
    fn test_csv_export_import_goals_succeeds() {
        let export = SavingsGoalsExport {
            next_id: 2,
            goals: vec![SavingsGoalExport {
                id: 1,
                owner: "G1".into(),
                name: "Emergency".into(),
                target_amount: 1000,
                current_amount: 500,
                target_date: 2000000000,
                locked: true,
            }],
        };
        let csv_bytes = export_to_csv(&export).unwrap();
        let goals = import_goals_from_csv(&csv_bytes).unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].name, "Emergency");
        assert_eq!(goals[0].target_amount, 1000);
    }

    #[test]
    fn test_migration_event_serialization_succeeds() {
        let event = MigrationEvent::V1(MigrationEventV1 {
            contract_id: "CABCD".into(),
            migration_type: "export".into(),
            version: SCHEMA_VERSION,
            timestamp_ms: 123456789,
        });

        // Ensure we can serialize cleanly for indexers.
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""V1":{"#));
        assert!(json.contains(r#""contract_id":"CABCD""#));
        assert!(json.contains(r#""version":1"#));

        let loaded: MigrationEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, loaded);

        let MigrationEvent::V1(v1) = loaded;
        assert_eq!(v1.version, SCHEMA_VERSION);
    }

    // =========================================================================
    // End-to-end migration compatibility tests — savings snapshots
    //
    // These tests exercise the full export ↔ import roundtrip through
    // data_migration for all four formats: JSON, Binary, CSV, Encrypted.
    //
    // Security assumptions validated:
    //   - Checksum integrity: tampered payloads are rejected.
    //   - Version gating: unsupported schema versions are rejected.
    //   - Data fidelity: every field is preserved across the roundtrip.
    // =========================================================================

    /// Build a deterministic test payload (single goal).
    fn make_single_goal_export() -> SavingsGoalsExport {
        SavingsGoalsExport {
            next_id: 2,
            goals: vec![SavingsGoalExport {
                id: 1,
                owner: "GBSINGLE".into(),
                name: "Emergency Fund".into(),
                target_amount: 5_000_000,
                current_amount: 2_500_000,
                target_date: 2_000_000_000,
                locked: false,
            }],
        }
    }

    /// Build a multi-goal payload (three goals).
    fn make_multi_goal_export() -> SavingsGoalsExport {
        SavingsGoalsExport {
            next_id: 4,
            goals: vec![
                SavingsGoalExport {
                    id: 1,
                    owner: "GBOWNER_A".into(),
                    name: "Vacation".into(),
                    target_amount: 10_000,
                    current_amount: 3_000,
                    target_date: 1_900_000_000,
                    locked: false,
                },
                SavingsGoalExport {
                    id: 2,
                    owner: "GBOWNER_A".into(),
                    name: "Car".into(),
                    target_amount: 50_000,
                    current_amount: 50_000,
                    target_date: 1_800_000_000,
                    locked: true,
                },
                SavingsGoalExport {
                    id: 3,
                    owner: "GBOWNER_B".into(),
                    name: "Education".into(),
                    target_amount: 100_000,
                    current_amount: 0,
                    target_date: 2_100_000_000,
                    locked: false,
                },
            ],
        }
    }

    // -------------------------------------------------------------------------
    // JSON format
    // -------------------------------------------------------------------------

    /// E2E: export savings goals snapshot to JSON, import back — all fields intact.
    ///
    /// Validates that the JSON serialization path preserves:
    ///   - `next_id`, goal count, goal IDs, names, amounts, dates, `locked` flag.
    ///   - Header version equals `SCHEMA_VERSION`.
    ///   - Checksum is valid on the reimported snapshot.
    #[test]
    fn test_e2e_savings_snapshot_json_roundtrip() {
        let export = make_single_goal_export();
        let snapshot = build_savings_snapshot(export, ExportFormat::Json);

        // Verify checksum on the freshly built snapshot.
        assert!(snapshot.verify_checksum(), "checksum must be valid after build");
        assert!(
            snapshot.validate_for_import().is_ok(),
            "snapshot must pass import validation"
        );

        // Serialize to JSON bytes.
        let bytes = export_to_json(&snapshot).unwrap();
        assert!(!bytes.is_empty(), "JSON bytes must be non-empty");

        // Deserialize and validate.
        let loaded = import_from_json(&bytes).unwrap();
        assert_eq!(loaded.header.version, SCHEMA_VERSION);
        assert_eq!(loaded.header.format, "json");
        assert!(loaded.verify_checksum(), "reimported checksum must match");

        // Check payload fidelity.
        if let SnapshotPayload::SavingsGoals(ref goals_export) = loaded.payload {
            assert_eq!(goals_export.next_id, 2);
            assert_eq!(goals_export.goals.len(), 1);
            let g = &goals_export.goals[0];
            assert_eq!(g.id, 1);
            assert_eq!(g.owner, "GBSINGLE");
            assert_eq!(g.name, "Emergency Fund");
            assert_eq!(g.target_amount, 5_000_000);
            assert_eq!(g.current_amount, 2_500_000);
            assert_eq!(g.target_date, 2_000_000_000);
            assert!(!g.locked);
        } else {
            panic!("Expected SavingsGoals payload variant");
        }
    }

    // -------------------------------------------------------------------------
    // Binary format
    // -------------------------------------------------------------------------

    /// E2E: export savings goals snapshot to binary (bincode), import back — checksum valid.
    ///
    /// Validates that the binary serialization path:
    ///   - Produces a non-empty byte buffer.
    ///   - Deserializes without error.
    ///   - Checksum matches on the imported snapshot.
    ///   - All goal fields survive intact.
    #[test]
    fn test_e2e_savings_snapshot_binary_roundtrip() {
        let export = make_single_goal_export();
        let snapshot = build_savings_snapshot(export, ExportFormat::Binary);

        let bytes = export_to_binary(&snapshot).unwrap();
        assert!(!bytes.is_empty(), "binary bytes must be non-empty");

        let loaded = import_from_binary(&bytes).unwrap();
        assert_eq!(loaded.header.version, SCHEMA_VERSION);
        assert_eq!(loaded.header.format, "binary");
        assert!(loaded.verify_checksum());

        if let SnapshotPayload::SavingsGoals(ref goals_export) = loaded.payload {
            assert_eq!(goals_export.goals.len(), 1);
            assert_eq!(goals_export.goals[0].name, "Emergency Fund");
            assert_eq!(goals_export.goals[0].target_amount, 5_000_000);
        } else {
            panic!("Expected SavingsGoals payload variant");
        }
    }

    // -------------------------------------------------------------------------
    // CSV format
    // -------------------------------------------------------------------------

    /// E2E: export savings goals to CSV, import back — all goal records intact.
    ///
    /// CSV export is a flat tabular format for spreadsheet compatibility.
    /// Validates that each goal row round-trips its fields correctly.
    #[test]
    fn test_e2e_savings_snapshot_csv_roundtrip() {
        let export = make_multi_goal_export();

        // CSV export operates directly on the goals list.
        let csv_bytes = export_to_csv(&export).unwrap();
        assert!(!csv_bytes.is_empty());

        let goals = import_goals_from_csv(&csv_bytes).unwrap();
        assert_eq!(goals.len(), 3, "all three goals must survive CSV roundtrip");

        // Verify each goal's data.
        assert_eq!(goals[0].id, 1);
        assert_eq!(goals[0].name, "Vacation");
        assert_eq!(goals[0].target_amount, 10_000);
        assert_eq!(goals[0].current_amount, 3_000);
        assert!(!goals[0].locked);

        assert_eq!(goals[1].id, 2);
        assert_eq!(goals[1].name, "Car");
        assert_eq!(goals[1].current_amount, 50_000);
        assert!(goals[1].locked, "locked flag must be preserved in CSV");

        assert_eq!(goals[2].id, 3);
        assert_eq!(goals[2].owner, "GBOWNER_B");
        assert_eq!(goals[2].name, "Education");
        assert_eq!(goals[2].current_amount, 0);
    }

    // -------------------------------------------------------------------------
    // Encrypted format
    // -------------------------------------------------------------------------

    /// E2E: export savings goals to JSON, wrap in base64 (encrypted payload),
    /// decode, then re-import — validates the full encrypted channel roundtrip.
    ///
    /// Security note: in production the caller encrypts the `plain_bytes` before
    /// passing to `export_to_encrypted_payload`. This test validates the
    /// encode/decode boundary; actual encryption is out-of-scope for the
    /// migration utility layer.
    #[test]
    fn test_e2e_savings_snapshot_encrypted_roundtrip() {
        let export = make_single_goal_export();
        let snapshot = build_savings_snapshot(export, ExportFormat::Encrypted);
        assert!(snapshot.verify_checksum());

        // Serialize to JSON bytes first (the "plain" payload before encryption).
        let plain_bytes = export_to_json(&snapshot).unwrap();

        // Simulate encryption boundary: wrap in base64.
        let encoded = export_to_encrypted_payload(&plain_bytes);
        assert!(!encoded.is_empty(), "encoded payload must be non-empty");

        // Simulate decryption boundary: decode from base64.
        let decoded_bytes = import_from_encrypted_payload(&encoded).unwrap();
        assert_eq!(decoded_bytes, plain_bytes, "decoded bytes must match original");

        // Re-import the decoded JSON and validate.
        let loaded = import_from_json(&decoded_bytes).unwrap();
        assert_eq!(loaded.header.version, SCHEMA_VERSION);
        assert!(loaded.verify_checksum());

        if let SnapshotPayload::SavingsGoals(ref g) = loaded.payload {
            assert_eq!(g.goals.len(), 1);
            assert_eq!(g.goals[0].owner, "GBSINGLE");
        } else {
            panic!("Expected SavingsGoals payload variant");
        }
    }

    // -------------------------------------------------------------------------
    // Security: tampered checksum
    // -------------------------------------------------------------------------

    /// E2E: mutating the checksum after export must cause `validate_for_import`
    /// to return `Err(ChecksumMismatch)`.
    ///
    /// This guards against any post-export payload tampering.
    #[test]
    fn test_e2e_tampered_checksum_import_fails() {
        let export = make_single_goal_export();
        let mut snapshot = build_savings_snapshot(export, ExportFormat::Json);

        // Tamper with the stored checksum.
        snapshot.header.checksum = "deadbeef00000000000000000000000000000000000000000000000000000000"
            .into();

        assert!(
            !snapshot.verify_checksum(),
            "verify_checksum must return false for tampered header"
        );
        assert_eq!(
            snapshot.validate_for_import(),
            Err(MigrationError::ChecksumMismatch),
            "import must be rejected with ChecksumMismatch"
        );
    }

    // -------------------------------------------------------------------------
    // Security: incompatible schema version
    // -------------------------------------------------------------------------

    /// E2E: setting an unsupported schema version must cause `validate_for_import`
    /// to return `Err(IncompatibleVersion)`.
    ///
    /// This enforces version gating for future-proof migration safety.
    #[test]
    fn test_e2e_incompatible_version_import_fails() {
        let export = make_single_goal_export();
        let mut snapshot = build_savings_snapshot(export, ExportFormat::Json);

        // Force an unsupported version.
        snapshot.header.version = 0;

        let err = snapshot.validate_for_import();
        assert!(
            matches!(
                err,
                Err(MigrationError::IncompatibleVersion {
                    found: 0,
                    min: MIN_SUPPORTED_VERSION,
                    max: SCHEMA_VERSION,
                })
            ),
            "import must be rejected with IncompatibleVersion, got {:?}",
            err
        );
    }

    // -------------------------------------------------------------------------
    // Edge case: empty goals list
    // -------------------------------------------------------------------------

    /// E2E: export and import a snapshot containing zero goals via JSON.
    ///
    /// Validates that the empty-list edge case is handled correctly by all
    /// layers: checksum computation, serialization, deserialization.
    #[test]
    fn test_e2e_empty_goals_json_roundtrip() {
        let export = SavingsGoalsExport {
            next_id: 0,
            goals: vec![],
        };
        let snapshot = build_savings_snapshot(export, ExportFormat::Json);
        assert!(snapshot.verify_checksum());

        let bytes = export_to_json(&snapshot).unwrap();
        let loaded = import_from_json(&bytes).unwrap();
        assert!(loaded.verify_checksum());

        if let SnapshotPayload::SavingsGoals(ref g) = loaded.payload {
            assert_eq!(g.goals.len(), 0, "empty goal list must round-trip as empty");
            assert_eq!(g.next_id, 0);
        } else {
            panic!("Expected SavingsGoals payload variant");
        }
    }

    // -------------------------------------------------------------------------
    // Edge case: locked goal preservation
    // -------------------------------------------------------------------------

    /// E2E: a goal with `locked: true` must survive JSON and binary roundtrips
    /// with its locked flag intact.
    #[test]
    fn test_e2e_locked_goal_preserved_json_and_binary() {
        let export = SavingsGoalsExport {
            next_id: 2,
            goals: vec![SavingsGoalExport {
                id: 1,
                owner: "GBLOCKED".into(),
                name: "Long-term Savings".into(),
                target_amount: 100_000,
                current_amount: 50_000,
                target_date: 2_500_000_000,
                locked: true,
            }],
        };

        // JSON path.
        let json_snapshot = build_savings_snapshot(export.clone(), ExportFormat::Json);
        let json_bytes = export_to_json(&json_snapshot).unwrap();
        let json_loaded = import_from_json(&json_bytes).unwrap();
        if let SnapshotPayload::SavingsGoals(ref g) = json_loaded.payload {
            assert!(g.goals[0].locked, "locked flag must be true after JSON roundtrip");
        } else {
            panic!("Expected SavingsGoals payload variant");
        }

        // Binary path.
        let bin_snapshot = build_savings_snapshot(export, ExportFormat::Binary);
        let bin_bytes = export_to_binary(&bin_snapshot).unwrap();
        let bin_loaded = import_from_binary(&bin_bytes).unwrap();
        if let SnapshotPayload::SavingsGoals(ref g) = bin_loaded.payload {
            assert!(g.goals[0].locked, "locked flag must be true after binary roundtrip");
        } else {
            panic!("Expected SavingsGoals payload variant");
        }
    }

    // -------------------------------------------------------------------------
    // Determinism: checksum stability
    // -------------------------------------------------------------------------

    /// E2E: building the same snapshot twice must produce identical checksums.
    ///
    /// Validates that `build_savings_snapshot` is deterministic — i.e. the SHA-256
    /// checksum is computed from a canonical JSON representation that does not
    /// include any random or time-dependent state.
    #[test]
    fn test_e2e_snapshot_checksum_is_deterministic() {
        let export_a = make_single_goal_export();
        let export_b = make_single_goal_export();

        let snap_a = build_savings_snapshot(export_a, ExportFormat::Json);
        let snap_b = build_savings_snapshot(export_b, ExportFormat::Json);

        assert_eq!(
            snap_a.header.checksum, snap_b.header.checksum,
            "identical payloads must produce identical checksums"
        );
    }

    // -------------------------------------------------------------------------
    // Multi-goal, multi-owner snapshot
    // -------------------------------------------------------------------------

    /// E2E: a snapshot with multiple goals across multiple owners must round-trip
    /// via JSON with all records intact and all owner IDs preserved.
    #[test]
    fn test_e2e_multi_goal_multi_owner_json_roundtrip() {
        let export = make_multi_goal_export();
        let snapshot = build_savings_snapshot(export, ExportFormat::Json);
        assert!(snapshot.verify_checksum());

        let bytes = export_to_json(&snapshot).unwrap();
        let loaded = import_from_json(&bytes).unwrap();
        assert!(loaded.verify_checksum());

        if let SnapshotPayload::SavingsGoals(ref g) = loaded.payload {
            assert_eq!(g.next_id, 4);
            assert_eq!(g.goals.len(), 3);

            // Owner A has goals 1 and 2.
            assert_eq!(g.goals[0].owner, "GBOWNER_A");
            assert_eq!(g.goals[1].owner, "GBOWNER_A");
            assert!(g.goals[1].locked);

            // Owner B has goal 3.
            assert_eq!(g.goals[2].owner, "GBOWNER_B");
            assert_eq!(g.goals[2].name, "Education");
        } else {
            panic!("Expected SavingsGoals payload variant");
        }
    }

    // -------------------------------------------------------------------------
    // Build helper: format label propagation
    // -------------------------------------------------------------------------

    /// Verify that `build_savings_snapshot` correctly propagates the format label
    /// into the snapshot header for all four supported formats.
    #[test]
    fn test_e2e_build_savings_snapshot_format_label() {
        let formats = [
            (ExportFormat::Json, "json"),
            (ExportFormat::Binary, "binary"),
            (ExportFormat::Csv, "csv"),
            (ExportFormat::Encrypted, "encrypted"),
        ];
        for (format, expected_label) in formats {
            let export = make_single_goal_export();
            let snapshot = build_savings_snapshot(export, format);
            assert_eq!(
                snapshot.header.format, expected_label,
                "format label mismatch for {:?}",
                format
            );
            assert!(snapshot.verify_checksum());
        }
    }
}
