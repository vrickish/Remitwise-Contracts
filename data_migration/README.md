# Data Migration Module

## Overview
The Data Migration module provides import/export utilities for Remitwise contracts, supporting multiple formats (JSON, binary, CSV) with checksum validation, version compatibility checks, and data integrity verification.

## Features

### Core Functionality
- **Multi-format support**: JSON, binary, CSV, and encrypted formats
- **Version compatibility**: Schema versioning with backward/forward compatibility checks
- **Data integrity**: SHA256 checksum validation for all exports
- **Migration tracking**: Event emission for indexing and audit trails

### CSV Schema Normalization and Validation (New Feature)
The module now includes strict CSV schema validation with the following capabilities:

1. **Header Normalization**
   - Case-insensitive header matching
   - Whitespace trimming
   - Rejection of ambiguous or malformed headers

2. **Data Type Validation**
   - Strict type checking for all columns
   - Range validation for numeric fields
   - Length limits for string fields
   - Boolean value validation (true/false/1/0)

3. **Business Rule Validation**
   - Custom validators for domain-specific rules
   - Protection against injection attacks
   - Memory exhaustion prevention via length limits

4. **Schema Definitions**
   - Pre-defined schemas for savings goals and remittance split data
   - Extensible schema system for new data types
   - Optional column support

## Usage Examples

### Importing Savings Goals from CSV
```rust
use data_migration::{import_goals_from_csv, validate_savings_goals_csv};

// Validate CSV before import
let csv_data = b"id,owner,name,target_amount,current_amount,target_date,locked\n1,GABC,Emergency Fund,10000,5000,2000000000,true";
validate_savings_goals_csv(csv_data)?;

// Import with strict validation (default)
let goals = import_goals_from_csv(csv_data)?;

// Import with legacy validation (backward compatibility)
let goals = import_goals_from_csv_with_validation(csv_data, false)?;
```

### Exporting Data
```rust
use data_migration::{ExportSnapshot, SnapshotPayload, SavingsGoalsExport, export_to_csv};

let export = SavingsGoalsExport {
    next_id: 2,
    goals: vec![/* goals data */],
};

let csv_bytes = export_to_csv(&export)?;
```

### Custom CSV Schema Validation
```rust
use data_migration::{CsvSchema, CsvColumn, CsvDataType};

let custom_schema = CsvSchema::new("custom_data", vec![
    CsvColumn::new("id", CsvDataType::U32)
        .range(1, 1000),
    CsvColumn::new("name", CsvDataType::String)
        .max_length(100)
        .validator(|value| {
            if value.is_empty() {
                Err(CsvValidationError::BusinessRuleViolation {
                    column: "name".to_string(),
                    value: value.to_string(),
                    rule: "cannot be empty".to_string(),
                })
            } else {
                Ok(())
            }
        }),
]);

let validation_result = custom_schema.parse_and_validate(csv_data)?;
```

## Security Considerations

### CSV Security
1. **Input Validation**: All CSV data is validated before processing
2. **Length Limits**: String fields have maximum length limits to prevent memory exhaustion
3. **Type Safety**: Strict type validation prevents type confusion attacks
4. **No Code Execution**: CSV data is never evaluated as code
5. **Injection Protection**: Proper escaping and validation prevents injection attacks

### Data Integrity
1. **Checksum Verification**: All exports include SHA256 checksums
2. **Version Validation**: Only compatible schema versions are accepted
3. **Tamper Detection**: Checksum mismatches are detected and rejected

### Privacy Considerations
1. **Encrypted Format**: Support for encrypted payloads (caller handles encryption/decryption)
2. **Data Minimization**: Only necessary data is included in exports
3. **Access Control**: Caller must implement appropriate access controls

## Error Handling

### Migration Errors
- `IncompatibleVersion`: Schema version mismatch
- `ChecksumMismatch`: Data integrity violation
- `InvalidFormat`: Malformed data format
- `ValidationFailed`: Business rule violation
- `DeserializeError`: Parsing failure

### CSV Validation Errors
- `InvalidHeader`: Ambiguous or malformed header
- `MissingColumns`: Required columns not found
- `ExtraColumns`: Unrecognized columns (when not allowed)
- `InvalidDataType`: Type mismatch
- `BusinessRuleViolation`: Domain-specific rule violation
- `StringTooLong`: String exceeds maximum length
- `NumericOutOfRange`: Number outside valid range
- `InvalidBoolean`: Invalid boolean value
- `ParseError`: CSV parsing error

## Testing

### Test Coverage Requirements
- Minimum 95% test coverage
- Edge case testing for all validation rules
- Security vulnerability testing
- Backward compatibility testing

### Running Tests
```bash
cargo test -p data_migration
```

### Test Categories
1. **Unit Tests**: Individual function testing
2. **Integration Tests**: End-to-end import/export flows
3. **Security Tests**: Malformed input handling
4. **Performance Tests**: Large dataset handling

## Schema Definitions

### Savings Goals Schema
| Column | Type | Required | Validation Rules |
|--------|------|----------|------------------|
| id | u32 | Yes | Range: 1 to u32::MAX |
| owner | String | Yes | Max length: 100 chars |
| name | String | Yes | Max length: 200 chars |
| target_amount | i64 | Yes | Range: 0 to i64::MAX |
| current_amount | i64 | Yes | Range: 0 to i64::MAX |
| target_date | u64 | Yes | None |
| locked | Boolean | Yes | Values: true/false/1/0 |

### Remittance Split Schema
| Column | Type | Required | Validation Rules |
|--------|------|----------|------------------|
| owner | String | Yes | Max length: 100 chars |
| spending_percent | u32 | Yes | Range: 0 to 100 |
| savings_percent | u32 | Yes | Range: 0 to 100 |
| bills_percent | u32 | Yes | Range: 0 to 100 |
| insurance_percent | u32 | Yes | Range: 0 to 100 |

## Migration Strategy

### Version Compatibility
- Current schema version: 1
- Minimum supported version: 1
- Version validation: `MIN_SUPPORTED_VERSION <= version <= SCHEMA_VERSION`

### Backward Compatibility
- Legacy CSV import supported via `strict_validation: false`
- Schema versioning for forward compatibility
- Graceful degradation for unknown variants

### Rollback Support
- Rollback metadata tracking
- Previous version and checksum storage
- Timestamped rollback points

## Performance Considerations

### Memory Usage
- Streaming CSV parsing for large files
- Length limits prevent memory exhaustion
- Efficient data structures for validation

### Processing Speed
- Optimized validation algorithms
- Early exit on validation failures
- Batch processing support

## Integration Guidelines

### Indexer Integration
- Migration events emitted for indexing
- Versioned event payloads
- Graceful handling of unknown variants

### Contract Integration
- Use via Cargo dependency
- Re-export common types
- Shared schema version constants

## Contributing

### Adding New Schemas
1. Define new `CsvSchema` with appropriate columns
2. Add validation rules for each column
3. Create test coverage for the new schema
4. Update documentation

### Schema Changes
1. Bump `SCHEMA_VERSION` for backward-incompatible changes
2. Update `MIN_SUPPORTED_VERSION` if old formats can't be imported
3. Add migration path for existing data
4. Update indexer event definitions if needed

## License
Part of the Remitwise Contracts project. See main project LICENSE for details.