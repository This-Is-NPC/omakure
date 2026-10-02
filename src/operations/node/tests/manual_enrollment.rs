use super::*;

#[test]
fn manual_enrollment_stages_requires_code_and_promotes_atomically() {
    let target_temp = TempDir::new().unwrap();
    let candidate_temp = TempDir::new().unwrap();
    let target = node_context(target_temp.path());
    let candidate = node_context(candidate_temp.path());
    let mut target_config = NodeConfig::default();
    target_config.trust.enrollment = "manual".into();
    initialize_node(&target, &target_config).unwrap();
    initialize_node(&candidate, &NodeConfig::default()).unwrap();

    let candidate_identity = NodeIdentity::load_existing(&candidate).unwrap();
    let candidate_transport =
        crate::node_transport::LocalTransport::load_existing(&candidate, &candidate_identity)
            .unwrap();
    let offer = ManualEnrollmentRequest::create(
        &candidate_identity,
        *candidate_transport.certificate().transport_public(),
        EnrollmentRole::Performer,
        vec!["remote-run".into()],
        crate::util::time::unix_seconds(),
        300,
    )
    .unwrap();
    let certificate_hex = hex::encode(candidate_transport.certificate().as_bytes());

    fail_enrollment_audits(&target, true);
    let stage_error = stage_manual_enrollment(
        &target,
        &offer.request,
        candidate_transport.certificate().as_bytes(),
    )
    .unwrap_err();
    assert_eq!(stage_error.code, OperationErrorCode::RegistryInvalid);
    assert!(list_pending_enrollments(&target).unwrap().is_empty());
    fail_enrollment_audits(&target, false);

    let pending = stage_manual_enrollment(
        &target,
        &offer.request,
        candidate_transport.certificate().as_bytes(),
    )
    .unwrap();
    assert_eq!(pending.state, "pending");
    assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);
    let replay = stage_manual_enrollment(
        &target,
        &offer.request,
        candidate_transport.certificate().as_bytes(),
    )
    .unwrap_err();
    assert_eq!(replay.code, OperationErrorCode::EnrollmentReplay);
    assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);
    let fresh_offer = ManualEnrollmentRequest::create(
        &candidate_identity,
        *candidate_transport.certificate().transport_public(),
        EnrollmentRole::Performer,
        vec!["remote-run".into()],
        crate::util::time::unix_seconds(),
        300,
    )
    .unwrap();
    let pending_conflict = stage_manual_enrollment(
        &target,
        &fresh_offer.request,
        candidate_transport.certificate().as_bytes(),
    )
    .unwrap_err();
    assert_eq!(pending_conflict.code, OperationErrorCode::EnrollmentReplay);
    assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);

    fail_enrollment_audits(&target, true);
    let denied = approve_manual_enrollment(
        &target,
        ManualEnrollmentApprovalRequest {
            request_hex: offer.request_hex(),
            transport_certificate: certificate_hex.clone(),
            code: hex::encode(&[0u8; enrollment::CODE_BYTES]),
            actor: "operator".into(),
            reason: "wrong code".into(),
            confirmed: true,
            expected_node_id: None,
        },
    )
    .unwrap_err();
    assert_eq!(denied.code, OperationErrorCode::RegistryInvalid);
    assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);
    fail_enrollment_audits(&target, false);

    let approved = approve_manual_enrollment(
        &target,
        ManualEnrollmentApprovalRequest {
            request_hex: offer.request_hex(),
            transport_certificate: certificate_hex,
            code: hex::encode(&offer.code),
            actor: "operator".into(),
            reason: "approved manually".into(),
            confirmed: true,
            expected_node_id: Some(offer.request.proposer_node_id.clone()),
        },
    )
    .unwrap();
    assert_eq!(approved.state, "active");
    assert!(list_pending_enrollments(&target).unwrap().is_empty());
    assert_eq!(list_trusted_peers(&target).unwrap()[0].state, "active");
    let active_conflict = stage_manual_enrollment(
        &target,
        &fresh_offer.request,
        candidate_transport.certificate().as_bytes(),
    )
    .unwrap_err();
    assert_eq!(active_conflict.code, OperationErrorCode::Conflict);
    assert!(list_pending_enrollments(&target).unwrap().is_empty());
}

#[test]
fn manual_enrollment_rejection_audit_failure_is_atomic() {
    let target_temp = TempDir::new().unwrap();
    let candidate_temp = TempDir::new().unwrap();
    let target = node_context(target_temp.path());
    let candidate = node_context(candidate_temp.path());
    let mut target_config = NodeConfig::default();
    target_config.trust.enrollment = "manual".into();
    initialize_node(&target, &target_config).unwrap();
    initialize_node(&candidate, &NodeConfig::default()).unwrap();

    let candidate_identity = NodeIdentity::load_existing(&candidate).unwrap();
    let candidate_transport =
        crate::node_transport::LocalTransport::load_existing(&candidate, &candidate_identity)
            .unwrap();
    let offer = ManualEnrollmentRequest::create(
        &candidate_identity,
        *candidate_transport.certificate().transport_public(),
        EnrollmentRole::Performer,
        vec!["remote-run".into()],
        crate::util::time::unix_seconds(),
        300,
    )
    .unwrap();
    let pending = stage_manual_enrollment(
        &target,
        &offer.request,
        candidate_transport.certificate().as_bytes(),
    )
    .unwrap();
    assert_eq!(pending.state, "pending");

    fail_enrollment_audits(&target, true);
    let error = reject_manual_enrollment(
        &target,
        ManualEnrollmentRejectionRequest {
            node_id: offer.request.proposer_node_id.clone(),
            actor: "operator".into(),
            reason: "reject test".into(),
            confirmed: true,
        },
    )
    .unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);
    fail_enrollment_audits(&target, false);
}
