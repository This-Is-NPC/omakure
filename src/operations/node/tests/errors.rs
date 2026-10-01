use super::*;
use crate::node_registry::RegistryError;

#[test]
fn registry_error_mapping_preserves_public_codes_and_messages() {
    let cases = [
        (
            RegistryError::SelfTrust,
            OperationErrorCode::Conflict,
            "peer cannot trust itself",
        ),
        (
            RegistryError::BundleConflict,
            OperationErrorCode::Conflict,
            "signed enrollment bundle conflicts with existing trust state",
        ),
        (
            RegistryError::ConductorConflict,
            OperationErrorCode::Conflict,
            "an active conductor already exists",
        ),
        (
            RegistryError::PublisherConductorConflict,
            OperationErrorCode::Conflict,
            "a baseline publisher cannot also be a conductor",
        ),
        (
            RegistryError::EnrollmentReplay,
            OperationErrorCode::EnrollmentReplay,
            "manual enrollment request was replayed",
        ),
        (
            RegistryError::BundleReplay,
            OperationErrorCode::EnrollmentReplay,
            "signed enrollment bundle was replayed",
        ),
        (
            RegistryError::BootstrapProofConsumed,
            OperationErrorCode::EnrollmentReplay,
            "signed enrollment bootstrap proof was already consumed",
        ),
        (
            RegistryError::EnrollmentCapacity,
            OperationErrorCode::RegistryInvalid,
            "manual enrollment replay capacity is exhausted",
        ),
        (
            RegistryError::BundleCapacity,
            OperationErrorCode::RegistryInvalid,
            "signed enrollment replay capacity is exhausted",
        ),
        (
            RegistryError::InvalidSchema("details".into()),
            OperationErrorCode::RegistryInvalid,
            "node trust registry is invalid or corrupt",
        ),
        (
            RegistryError::Corrupt("details".into()),
            OperationErrorCode::RegistryInvalid,
            "node trust registry is invalid or corrupt",
        ),
        (
            RegistryError::Sqlite(rusqlite::Error::InvalidQuery),
            OperationErrorCode::RegistryInvalid,
            "node trust registry is invalid or corrupt",
        ),
    ];

    for (error, code, message) in cases {
        let mapped = map_registry_error(error);
        assert_eq!(mapped.code, code);
        assert_eq!(mapped.message, message);
    }
}
