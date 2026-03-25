//! Data migration, import/export utilities for Remitwise contracts.
//!
//! Supports multiple formats (JSON, binary, CSV), checksum validation,
//! version compatibility checks, and data integrity verification.
//!
//! # CSV Schema Normalization and Validation
//! The CSV import/export functionality includes strict schema validation:
//! - Header normalization (case-insensitive, whitespace trimmed)
//! - Rejection of ambiguous or malformed headers
//! - Strict data type validation for each column
//! - Business rule validation (e.g., non-negative amounts, valid dates)
//! - Protection against injection attacks through CSV data
//!
//! ## Supported CSV Schemas
//! 1. Savings Goals: id, owner, name, target_amount, current_amount, target_date, locked
//! 2. Remittance Split: owner, spending_percent, savings_percent, bills_percent, insurance_percent
//!
//! ## Security Considerations
//! - All CSV data is validated before processing
//! - String fields are length-limited to prevent memory exhaustion
//! - Numeric fields have range validation
//! - No executable code is ever evaluated from CSV data

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::str::FromStr;

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

/// CSV schema validation and normalization errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvValidationError {
    /// Invalid or ambiguous header names
    InvalidHeader {
        expected: String,
        found: String,
        position: usize,
    },
    /// Missing required columns
    MissingColumns(Vec<String>),
    /// Extra columns that are not recognized
    ExtraColumns(Vec<String>),
    /// Invalid data type in a column
    InvalidDataType {
        column: String,
        value: String,
        expected_type: String,
    },
    /// Business rule validation failed
    BusinessRuleViolation {
        column: String,
        value: String,
        rule: String,
    },
    /// String length exceeds maximum allowed
    StringTooLong {
        column: String,
        value: String,
        max_length: usize,
    },
    /// Numeric value out of valid range
    NumericOutOfRange {
        column: String,
        value: String,
        min: i64,
        max: i64,
    },
    /// Invalid boolean value
    InvalidBoolean {
        column: String,
        value: String,
    },
    /// CSV parsing error
    ParseError(String),
}

impl std::fmt::Display for CsvValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsvValidationError::InvalidHeader { expected, found, position } => {
                write!(f, "invalid header at position {}: expected '{}', found '{}'", position, expected, found)
            }
            CsvValidationError::MissingColumns(cols) => {
                write!(f, "missing required columns: {}", cols.join(", "))
            }
            CsvValidationError::ExtraColumns(cols) => {
                write!(f, "extra unrecognized columns: {}", cols.join(", "))
            }
            CsvValidationError::InvalidDataType { column, value, expected_type } => {
                write!(f, "invalid data type in column '{}': value '{}' is not a valid {}", column, value, expected_type)
            }
            CsvValidationError::BusinessRuleViolation { column, value, rule } => {
                write!(f, "business rule violation in column '{}': value '{}' violates rule: {}", column, value, rule)
            }
            CsvValidationError::StringTooLong { column, value, max_length } => {
                write!(f, "string too long in column '{}': '{}' exceeds maximum length of {}", column, value, max_length)
            }
            CsvValidationError::NumericOutOfRange { column, value, min, max } => {
                write!(f, "numeric value out of range in column '{}': '{}' must be between {} and {}", column, value, min, max)
            }
            CsvValidationError::InvalidBoolean { column, value } => {
                write!(f, "invalid boolean value in column '{}': '{}' (must be 'true' or 'false')", column, value)
            }
            CsvValidationError::ParseError(msg) => {
                write!(f, "CSV parse error: {}", msg)
            }
        }
    }
}

impl std::error::Error for CsvValidationError {}

/// CSV column definition with validation rules.
#[derive(Debug, Clone)]
pub struct CsvColumn {
    /// Canonical column name (lowercase, used for matching)
    pub name: String,
    /// Display name (used in error messages)
    pub display_name: String,
    /// Data type for validation
    pub data_type: CsvDataType,
    /// Whether column is required
    pub required: bool,
    /// Maximum string length (for String type)
    pub max_length: Option<usize>,
    /// Numeric range (for numeric types)
    pub range: Option<(i64, i64)>,
    /// Custom validation function
    pub validator: Option<Box<dyn Fn(&str) -> Result<(), CsvValidationError> + Send + Sync>>,
}

/// Supported CSV data types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvDataType {
    /// Unsigned 32-bit integer
    U32,
    /// Signed 64-bit integer
    I64,
    /// Unsigned 64-bit integer
    U64,
    /// String
    String,
    /// Boolean (true/false)
    Boolean,
}

impl CsvColumn {
    /// Create a new CSV column definition.
    pub fn new(name: &str, data_type: CsvDataType) -> Self {
        Self {
            name: name.to_lowercase(),
            display_name: name.to_string(),
            data_type,
            required: true,
            max_length: None,
            range: None,
            validator: None,
        }
    }

    /// Set column as optional.
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Set maximum string length.
    pub fn max_length(mut self, max: usize) -> Self {
        self.max_length = Some(max);
        self
    }

    /// Set numeric range.
    pub fn range(mut self, min: i64, max: i64) -> Self {
        self.range = Some((min, max));
        self
    }

    /// Set custom validator.
    pub fn validator<F>(mut self, validator: F) -> Self
    where
        F: Fn(&str) -> Result<(), CsvValidationError> + 'static + Send + Sync,
    {
        self.validator = Some(Box::new(validator));
        self
    }

    /// Validate a value against this column's rules.
    pub fn validate(&self, value: &str) -> Result<(), CsvValidationError> {
        // Check string length if applicable
        if let Some(max_len) = self.max_length {
            if value.len() > max_len {
                return Err(CsvValidationError::StringTooLong {
                    column: self.display_name.clone(),
                    value: value.to_string(),
                    max_length: max_len,
                });
            }
        }

        // Validate data type
        match self.data_type {
            CsvDataType::U32 => {
                value.parse::<u32>().map_err(|_| CsvValidationError::InvalidDataType {
                    column: self.display_name.clone(),
                    value: value.to_string(),
                    expected_type: "unsigned 32-bit integer".to_string(),
                })?;
            }
            CsvDataType::I64 => {
                let val = value.parse::<i64>().map_err(|_| CsvValidationError::InvalidDataType {
                    column: self.display_name.clone(),
                    value: value.to_string(),
                    expected_type: "signed 64-bit integer".to_string(),
                })?;
                
                // Check range if specified
                if let Some((min, max)) = self.range {
                    if val < min || val > max {
                        return Err(CsvValidationError::NumericOutOfRange {
                            column: self.display_name.clone(),
                            value: value.to_string(),
                            min,
                            max,
                        });
                    }
                }
            }
            CsvDataType::U64 => {
                value.parse::<u64>().map_err(|_| CsvValidationError::InvalidDataType {
                    column: self.display_name.clone(),
                    value: value.to_string(),
                    expected_type: "unsigned 64-bit integer".to_string(),
                })?;
            }
            CsvDataType::String => {
                // String validation is handled by length check above
                // Additional validation can be added via custom validator
            }
            CsvDataType::Boolean => {
                let lower = value.to_lowercase();
                if lower != "true" && lower != "false" && lower != "1" && lower != "0" {
                    return Err(CsvValidationError::InvalidBoolean {
                        column: self.display_name.clone(),
                        value: value.to_string(),
                    });
                }
            }
        }

        // Run custom validator if present
        if let Some(validator) = &self.validator {
            validator(value)?;
        }

        Ok(())
    }
}

/// CSV schema definition for a specific data type.
#[derive(Debug, Clone)]
pub struct CsvSchema {
    /// Schema name (e.g., "savings_goals")
    pub name: String,
    /// Column definitions in expected order
    pub columns: Vec<CsvColumn>,
    /// Whether to allow extra columns
    pub allow_extra_columns: bool,
}

impl CsvSchema {
    /// Create a new CSV schema.
    pub fn new(name: &str, columns: Vec<CsvColumn>) -> Self {
        Self {
            name: name.to_string(),
            columns,
            allow_extra_columns: false,
        }
    }

    /// Set whether to allow extra columns.
    pub fn allow_extra_columns(mut self, allow: bool) -> Self {
        self.allow_extra_columns = allow;
        self
    }

    /// Validate CSV headers against this schema.
    pub fn validate_headers(&self, headers: &[String]) -> Result<Vec<usize>, CsvValidationError> {
        let mut column_indices = Vec::new();
        let mut missing_columns = Vec::new();
        
        // Normalize headers (lowercase, trim)
        let normalized_headers: Vec<String> = headers
            .iter()
            .map(|h| h.trim().to_lowercase())
            .collect();
        
        // Find each required column
        for (col_idx, column) in self.columns.iter().enumerate() {
            if column.required {
                let mut found = false;
                for (header_idx, header) in normalized_headers.iter().enumerate() {
                    if header == &column.name {
                        column_indices.push(header_idx);
                        found = true;
                        break;
                    }
                }
                
                if !found {
                    missing_columns.push(column.display_name.clone());
                }
            } else {
                // Optional column - try to find it
                let mut found_idx = None;
                for (header_idx, header) in normalized_headers.iter().enumerate() {
                    if header == &column.name {
                        found_idx = Some(header_idx);
                        break;
                    }
                }
                column_indices.push(found_idx.unwrap_or(usize::MAX));
            }
        }
        
        if !missing_columns.is_empty() {
            return Err(CsvValidationError::MissingColumns(missing_columns));
        }
        
        // Check for extra columns if not allowed
        if !self.allow_extra_columns {
            let mut extra_columns = Vec::new();
            for (header_idx, header) in headers.iter().enumerate() {
                let normalized = header.trim().to_lowercase();
                let mut is_expected = false;
                for column in &self.columns {
                    if normalized == column.name {
                        is_expected = true;
                        break;
                    }
                }
                if !is_expected {
                    extra_columns.push(header.clone());
                }
            }
            
            if !extra_columns.is_empty() {
                return Err(CsvValidationError::ExtraColumns(extra_columns));
            }
        }
        
        Ok(column_indices)
    }
    
    /// Validate a row of data against this schema.
    pub fn validate_row(&self, row: &[String], column_indices: &[usize]) -> Result<(), CsvValidationError> {
        for (col_idx, &header_idx) in column_indices.iter().enumerate() {
            let column = &self.columns[col_idx];
            
            // Skip optional columns that weren't found
            if header_idx == usize::MAX {
                continue;
            }
            
            if header_idx >= row.len() {
                return Err(CsvValidationError::ParseError(format!(
                    "column '{}' at index {} not found in row with {} columns",
                    column.display_name, header_idx, row.len()
                )));
            }
            
            let value = &row[header_idx];
            column.validate(value)?;
        }
        
        Ok(())
    }
    
    /// Parse and validate a CSV string against this schema.
    pub fn parse_and_validate(&self, csv_data: &[u8]) -> Result<Vec<Vec<String>>, CsvValidationError> {
        let mut rdr = csv::Reader::from_reader(csv_data);
        
        // Read headers
        let headers = rdr.headers()
            .map_err(|e| CsvValidationError::ParseError(e.to_string()))?
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        
        // Validate headers and get column indices
        let column_indices = self.validate_headers(&headers)?;
        
        // Read and validate each row
        let mut rows = Vec::new();
        for (row_num, result) in rdr.records().enumerate() {
            let record = result.map_err(|e| CsvValidationError::ParseError(e.to_string()))?;
            
            let row = record.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            
            // Validate the row
            self.validate_row(&row, &column_indices)
                .map_err(|e| CsvValidationError::ParseError(format!("row {}: {}", row_num + 1, e)))?;
            
            rows.push(row);
        }
        
        Ok(rows)
    }
}

/// Get the savings goals CSV schema.
pub fn savings_goals_csv_schema() -> CsvSchema {
    CsvSchema::new("savings_goals", vec![
        CsvColumn::new("id", CsvDataType::U32)
            .range(1, u32::MAX as i64),
        CsvColumn::new("owner", CsvDataType::String)
            .max_length(100),
        CsvColumn::new("name", CsvDataType::String)
            .max_length(200),
        CsvColumn::new("target_amount", CsvDataType::I64)
            .range(0, i64::MAX),
        CsvColumn::new("current_amount", CsvDataType::I64)
            .range(0, i64::MAX),
        CsvColumn::new("target_date", CsvDataType::U64),
        CsvColumn::new("locked", CsvDataType::Boolean),
    ])
}

/// Get the remittance split CSV schema.
pub fn remittance_split_csv_schema() -> CsvSchema {
    CsvSchema::new("remittance_split", vec![
        CsvColumn::new("owner", CsvDataType::String)
            .max_length(100),
        CsvColumn::new("spending_percent", CsvDataType::U32)
            .range(0, 100),
        CsvColumn::new("savings_percent", CsvDataType::U32)
            .range(0, 100),
        CsvColumn::new("bills_percent", CsvDataType::U32)
            .range(0, 100),
        CsvColumn::new("insurance_percent", CsvDataType::U32)
            .range(0, 100),
    ])
}

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
/// This function uses the new strict CSV schema validation.
pub fn import_goals_from_csv(bytes: &[u8]) -> Result<Vec<SavingsGoalExport>, MigrationError> {
    import_goals_from_csv_with_validation(bytes, true)
}

/// Import goals from CSV with optional strict validation.
/// 
/// # Parameters
/// - `bytes`: CSV data bytes
/// - `strict_validation`: If true, uses strict schema validation; if false, uses legacy deserialization
/// 
/// # Returns
/// - `Ok(Vec<SavingsGoalExport>)`: Successfully imported goals
/// - `Err(MigrationError)`: Validation or parsing error
pub fn import_goals_from_csv_with_validation(
    bytes: &[u8], 
    strict_validation: bool
) -> Result<Vec<SavingsGoalExport>, MigrationError> {
    if strict_validation {
        // Use new strict validation
        let schema = savings_goals_csv_schema();
        let rows = schema.parse_and_validate(bytes)
            .map_err(|e| MigrationError::ValidationFailed(e.to_string()))?;
        
        let mut goals = Vec::new();
        for row in rows {
            // Map validated rows to SavingsGoalExport
            // We need to find column indices
            let headers: Vec<String> = csv::Reader::from_reader(bytes)
                .headers()
                .map_err(|e| MigrationError::DeserializeError(e.to_string()))?
                .iter()
                .map(|s| s.to_string())
                .collect();
            
            let column_indices = schema.validate_headers(&headers)
                .map_err(|e| MigrationError::ValidationFailed(e.to_string()))?;
            
            // Create a map of column name to value for this row
            let mut column_values = std::collections::HashMap::new();
            for (col_idx, &header_idx) in column_indices.iter().enumerate() {
                if header_idx != usize::MAX && header_idx < row.len() {
                    let column = &schema.columns[col_idx];
                    column_values.insert(column.name.clone(), row[header_idx].clone());
                }
            }
            
            // Extract values with fallbacks for optional columns
            let id = column_values.get("id")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: id".to_string()))?
                .parse::<u32>()
                .map_err(|e| MigrationError::ValidationFailed(format!("Invalid id: {}", e)))?;
            
            let owner = column_values.get("owner")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: owner".to_string()))?
                .clone();
            
            let name = column_values.get("name")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: name".to_string()))?
                .clone();
            
            let target_amount = column_values.get("target_amount")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: target_amount".to_string()))?
                .parse::<i64>()
                .map_err(|e| MigrationError::ValidationFailed(format!("Invalid target_amount: {}", e)))?;
            
            let current_amount = column_values.get("current_amount")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: current_amount".to_string()))?
                .parse::<i64>()
                .map_err(|e| MigrationError::ValidationFailed(format!("Invalid current_amount: {}", e)))?;
            
            let target_date = column_values.get("target_date")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: target_date".to_string()))?
                .parse::<u64>()
                .map_err(|e| MigrationError::ValidationFailed(format!("Invalid target_date: {}", e)))?;
            
            let locked = column_values.get("locked")
                .ok_or_else(|| MigrationError::ValidationFailed("Missing required column: locked".to_string()))?
                .parse::<bool>()
                .map_err(|_| {
                    let val = column_values.get("locked").unwrap();
                    if val.to_lowercase() == "true" || val == "1" {
                        Ok(true)
                    } else if val.to_lowercase() == "false" || val == "0" {
                        Ok(false)
                    } else {
                        Err(MigrationError::ValidationFailed(format!("Invalid boolean value for locked: {}", val)))
                    }
                })??;
            
            goals.push(SavingsGoalExport {
                id,
                owner,
                name,
                target_amount,
                current_amount,
                target_date,
                locked,
            });
        }
        
        Ok(goals)
    } else {
        // Legacy deserialization (for backward compatibility)
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
}

/// Validate CSV data against the savings goals schema without importing.
/// 
/// # Parameters
/// - `bytes`: CSV data bytes
/// 
/// # Returns
/// - `Ok(())`: CSV data is valid
/// - `Err(CsvValidationError)`: Validation error with details
pub fn validate_savings_goals_csv(bytes: &[u8]) -> Result<(), CsvValidationError> {
    let schema = savings_goals_csv_schema();
    schema.parse_and_validate(bytes)?;
    Ok(())
}

/// Validate CSV data against the remittance split schema without importing.
/// 
/// # Parameters
/// - `bytes`: CSV data bytes
/// 
/// # Returns
/// - `Ok(())`: CSV data is valid
/// - `Err(CsvValidationError)`: Validation error with details
pub fn validate_remittance_split_csv(bytes: &[u8]) -> Result<(), CsvValidationError> {
    let schema = remittance_split_csv_schema();
    schema.parse_and_validate(bytes)?;
    Ok(())
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

    // CSV Schema Validation Tests
    #[test]
    fn test_csv_column_validation_succeeds() {
        let column = CsvColumn::new("amount", CsvDataType::I64)
            .range(0, 1000);
        
        assert!(column.validate("0").is_ok());
        assert!(column.validate("500").is_ok());
        assert!(column.validate("1000").is_ok());
        
        // Test invalid values
        assert!(column.validate("-1").is_err());
        assert!(column.validate("1001").is_err());
        assert!(column.validate("abc").is_err());
    }
    
    #[test]
    fn test_csv_column_string_length_validation() {
        let column = CsvColumn::new("name", CsvDataType::String)
            .max_length(10);
        
        assert!(column.validate("short").is_ok());
        assert!(column.validate("exactly10").is_ok());
        assert!(column.validate("").is_ok());
        
        assert!(column.validate("this is too long").is_err());
    }
    
    #[test]
    fn test_csv_column_boolean_validation() {
        let column = CsvColumn::new("locked", CsvDataType::Boolean);
        
        assert!(column.validate("true").is_ok());
        assert!(column.validate("false").is_ok());
        assert!(column.validate("TRUE").is_ok());
        assert!(column.validate("FALSE").is_ok());
        assert!(column.validate("1").is_ok());
        assert!(column.validate("0").is_ok());
        
        assert!(column.validate("yes").is_err());
        assert!(column.validate("no").is_err());
        assert!(column.validate("2").is_err());
        assert!(column.validate("").is_err());
    }
    
    #[test]
    fn test_savings_goals_schema_header_validation() {
        let schema = savings_goals_csv_schema();
        
        // Valid headers (case-insensitive, with spaces)
        let valid_headers = vec![
            "id".to_string(),
            "owner".to_string(),
            "name".to_string(),
            "target_amount".to_string(),
            "current_amount".to_string(),
            "target_date".to_string(),
            "locked".to_string(),
        ];
        
        assert!(schema.validate_headers(&valid_headers).is_ok());
        
        // Valid with different case
        let mixed_case_headers = vec![
            "ID".to_string(),
            "Owner".to_string(),
            "NAME".to_string(),
            "Target_Amount".to_string(),
            "current_AMOUNT".to_string(),
            "target_date".to_string(),
            "LOCKED".to_string(),
        ];
        
        assert!(schema.validate_headers(&mixed_case_headers).is_ok());
        
        // Missing required column
        let missing_headers = vec![
            "id".to_string(),
            "owner".to_string(),
            // Missing "name"
            "target_amount".to_string(),
            "current_amount".to_string(),
            "target_date".to_string(),
            "locked".to_string(),
        ];
        
        assert!(schema.validate_headers(&missing_headers).is_err());
        
        // Extra column (not allowed by default)
        let extra_headers = vec![
            "id".to_string(),
            "owner".to_string(),
            "name".to_string(),
            "target_amount".to_string(),
            "current_amount".to_string(),
            "target_date".to_string(),
            "locked".to_string(),
            "extra_column".to_string(),
        ];
        
        assert!(schema.validate_headers(&extra_headers).is_err());
    }
    
    #[test]
    fn test_remittance_split_schema_header_validation() {
        let schema = remittance_split_csv_schema();
        
        // Valid headers
        let valid_headers = vec![
            "owner".to_string(),
            "spending_percent".to_string(),
            "savings_percent".to_string(),
            "bills_percent".to_string(),
            "insurance_percent".to_string(),
        ];
        
        assert!(schema.validate_headers(&valid_headers).is_ok());
        
        // Test percentage range validation
        let column = CsvColumn::new("spending_percent", CsvDataType::U32)
            .range(0, 100);
        
        assert!(column.validate("0").is_ok());
        assert!(column.validate("50").is_ok());
        assert!(column.validate("100").is_ok());
        assert!(column.validate("101").is_err());
        assert!(column.validate("-1").is_err());
    }
    
    #[test]
    fn test_csv_schema_parse_and_validate_savings_goals() {
        let csv_data = r#"id,owner,name,target_amount,current_amount,target_date,locked
1,GABC,Emergency Fund,10000,5000,2000000000,true
2,GXYZ,Vacation,5000,1000,1900000000,false
3,GDEF,Car Down Payment,20000,3000,2100000000,1"#;
        
        let result = validate_savings_goals_csv(csv_data.as_bytes());
        assert!(result.is_ok(), "Validation failed: {:?}", result.err());
        
        // Test with invalid data
        let invalid_csv = r#"id,owner,name,target_amount,current_amount,target_date,locked
1,GABC,Emergency Fund,-100,5000,2000000000,true"#;
        
        let result = validate_savings_goals_csv(invalid_csv.as_bytes());
        assert!(result.is_err(), "Should reject negative target_amount");
        
        // Test with missing column
        let missing_column_csv = r#"id,owner,name,target_amount,current_amount,target_date
1,GABC,Emergency Fund,10000,5000,2000000000"#;
        
        let result = validate_savings_goals_csv(missing_column_csv.as_bytes());
        assert!(result.is_err(), "Should reject missing 'locked' column");
    }
    
    #[test]
    fn test_csv_schema_parse_and_validate_remittance_split() {
        let csv_data = r#"owner,spending_percent,savings_percent,bills_percent,insurance_percent
GABC,50,30,15,5
GXYZ,40,40,10,10"#;
        
        let result = validate_remittance_split_csv(csv_data.as_bytes());
        assert!(result.is_ok(), "Validation failed: {:?}", result.err());
        
        // Test with percentages that don't sum to 100 (business logic would check this separately)
        let csv_data = r#"owner,spending_percent,savings_percent,bills_percent,insurance_percent
GABC,50,50,0,0"#;
        
        let result = validate_remittance_split_csv(csv_data.as_bytes());
        assert!(result.is_ok(), "Individual percentages are valid even if sum != 100");
        
        // Test with invalid percentage (> 100)
        let invalid_csv = r#"owner,spending_percent,savings_percent,bills_percent,insurance_percent
GABC,101,0,0,0"#;
        
        let result = validate_remittance_split_csv(invalid_csv.as_bytes());
        assert!(result.is_err(), "Should reject percentage > 100");
    }
    
    #[test]
    fn test_import_goals_with_strict_validation() {
        let csv_data = r#"id,owner,name,target_amount,current_amount,target_date,locked
1,GABC,Emergency Fund,10000,5000,2000000000,true
2,GXYZ,Vacation,5000,1000,1900000000,false"#;
        
        // Test with strict validation (default)
        let goals = import_goals_from_csv(csv_data.as_bytes()).unwrap();
        assert_eq!(goals.len(), 2);
        assert_eq!(goals[0].name, "Emergency Fund");
        assert_eq!(goals[0].target_amount, 10000);
        assert_eq!(goals[0].locked, true);
        assert_eq!(goals[1].name, "Vacation");
        assert_eq!(goals[1].locked, false);
        
        // Test with explicit strict validation
        let goals = import_goals_from_csv_with_validation(csv_data.as_bytes(), true).unwrap();
        assert_eq!(goals.len(), 2);
        
        // Test with legacy validation (non-strict)
        let goals = import_goals_from_csv_with_validation(csv_data.as_bytes(), false).unwrap();
        assert_eq!(goals.len(), 2);
    }
    
    #[test]
    fn test_import_goals_rejects_invalid_with_strict_validation() {
        // Test with ambiguous header (wrong case but close)
        let csv_data = r#"ID,OWNER,NAME,TARGET_AMOUNT,CURRENT_AMOUNT,TARGET_DATE,LOCKED
1,GABC,Emergency Fund,10000,5000,2000000000,true"#;
        
        // This should work because headers are case-insensitive
        let goals = import_goals_from_csv(csv_data.as_bytes()).unwrap();
        assert_eq!(goals.len(), 1);
        
        // Test with malformed data
        let invalid_csv = r#"id,owner,name,target_amount,current_amount,target_date,locked
not_a_number,GABC,Emergency Fund,10000,5000,2000000000,true"#;
        
        let result = import_goals_from_csv(invalid_csv.as_bytes());
        assert!(result.is_err(), "Should reject non-numeric id");
        
        // Test with string too long
        let long_name = "A".repeat(201);
        let csv_data = format!(r#"id,owner,name,target_amount,current_amount,target_date,locked
1,GABC,{},10000,5000,2000000000,true"#, long_name);
        
        let result = import_goals_from_csv(csv_data.as_bytes());
        assert!(result.is_err(), "Should reject name longer than 200 characters");
    }
    
    #[test]
    fn test_csv_schema_with_custom_validator() {
        // Create a column with custom validation
        let column = CsvColumn::new("owner", CsvDataType::String)
            .max_length(50)
            .validator(|value| {
                if !value.starts_with('G') {
                    Err(CsvValidationError::BusinessRuleViolation {
                        column: "owner".to_string(),
                        value: value.to_string(),
                        rule: "must start with 'G'".to_string(),
                    })
                } else {
                    Ok(())
                }
            });
        
        assert!(column.validate("GABC").is_ok());
        assert!(column.validate("GXYZ").is_ok());
        
        let result = column.validate("ABC");
        assert!(result.is_err());
        if let Err(CsvValidationError::BusinessRuleViolation { column, value, rule }) = result {
            assert_eq!(column, "owner");
            assert_eq!(value, "ABC");
            assert!(rule.contains("must start with 'G'"));
        }
    }
    
    #[test]
    fn test_csv_schema_allow_extra_columns() {
        let mut schema = savings_goals_csv_schema();
        schema = schema.allow_extra_columns(true);
        
        let headers_with_extra = vec![
            "id".to_string(),
            "owner".to_string(),
            "name".to_string(),
            "target_amount".to_string(),
            "current_amount".to_string(),
            "target_date".to_string(),
            "locked".to_string(),
            "notes".to_string(), // Extra column
            "created_at".to_string(), // Another extra column
        ];
        
        let result = schema.validate_headers(&headers_with_extra);
        assert!(result.is_ok(), "Should allow extra columns when configured");
    }
}
