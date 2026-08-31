use curve25519_dalek::{constants::X25519_BASEPOINT, montgomery::MontgomeryPoint};
use k256::schnorr::{
    signature::hazmat::{PrehashSigner, PrehashVerifier},
    Signature, SigningKey, VerifyingKey,
};
use omakure::enrollment::{EnrollmentRole, ManualEnrollmentRequest};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use sha2::{Digest, Sha256};
use snow::{params::NoiseParams, Builder, HandshakeState, TransportState};
use std::collections::BTreeSet;
use tempfile::TempDir;

const CERT_MAGIC: &[u8; 4] = b"OMTC";
const CERT_DOMAIN: &[u8] = b"omakure/transport-cert/v1\0";
const PROLOGUE: &[u8] = b"omakure/direct-transport/v1\0";

#[test]
fn owner_selected_contract_freezes_contract_and_public_vectors() {
    let fixture: toml::Value =
        toml::from_str(include_str!("fixtures/direct_transport_feasibility.toml"))
            .expect("direct transport fixture must parse");

    assert_eq!(fixture["format_version"].as_integer(), Some(2));
    assert_eq!(
        fixture["contract_id"].as_str(),
        Some("omakure/direct-transport/v1")
    );
    assert_eq!(
        fixture["status"].as_str(),
        Some("owner-approved-under-technical-review")
    );
    assert_eq!(fixture["wire_contract_status"].as_str(), Some("frozen"));
    assert_eq!(fixture["production_transport_claim"].as_bool(), Some(true));
    assert_eq!(
        fixture["identity_key_material"].as_str(),
        Some("one-normalized-k256-scalar")
    );
    assert_eq!(
        fixture["second_static_transport_keypair"].as_str(),
        Some("service-owned-x25519-only")
    );
    assert_eq!(
        fixture["noise_protocol_name"].as_str(),
        Some("Noise_XX_25519_ChaChaPoly_SHA256")
    );
    assert_eq!(
        fixture["noise_prologue_hex"].as_str(),
        Some(hex(PROLOGUE).as_str())
    );

    let candidates = fixture["candidates"]
        .as_array()
        .expect("candidate evidence must be an array");
    let names: BTreeSet<_> = candidates
        .iter()
        .map(|candidate| candidate["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "k256-ecdh-primitive",
            "libp2p-noise-xx",
            "noise-xx-25519-chachapoly-sha256",
            "rfc9180-hpke",
        ])
    );
    assert_eq!(
        candidates
            .iter()
            .find(|candidate| {
                candidate["name"].as_str() == Some("noise-xx-25519-chachapoly-sha256")
            })
            .unwrap()["status"]
            .as_str(),
        Some("selected")
    );
    assert!(candidates.iter().all(|candidate| {
        candidate["reason"]
            .as_str()
            .is_some_and(|reason| !reason.trim().is_empty())
    }));

    let vectors = fixture["vectors"].as_array().expect("two protocol vectors");
    assert_eq!(vectors.len(), 2);
    let initiator_vector = vectors
        .iter()
        .find(|vector| vector["role"].as_str() == Some("initiator"))
        .unwrap();
    let responder_vector = vectors
        .iter()
        .find(|vector| vector["role"].as_str() == Some("responder"))
        .unwrap();

    let initiator_cert = certificate(initiator_vector);
    let responder_cert = certificate(responder_vector);
    assert_certificate(initiator_vector, &initiator_cert);
    assert_certificate(responder_vector, &responder_cert);
    assert!(verify_certificate(initiator_vector, &initiator_cert).is_ok());
    assert_eq!(initiator_cert.len(), 245);
    assert_eq!(responder_cert.len(), 245);

    let params: NoiseParams = fixture["noise_protocol_name"]
        .as_str()
        .unwrap()
        .parse()
        .expect("Noise protocol name must be supported");
    let initiator_static = bytes(initiator_vector, "transport_private_key_hex");
    let responder_static = bytes(responder_vector, "transport_private_key_hex");
    let initiator_ephemeral = bytes(initiator_vector, "ephemeral_private_key_hex");
    let responder_ephemeral = bytes(responder_vector, "ephemeral_private_key_hex");
    assert!(local_x25519_key_check(&initiator_static).is_ok());
    assert!(local_x25519_key_check(&responder_static).is_ok());
    let mut initiator = Builder::new(params.clone())
        .prologue(PROLOGUE)
        .unwrap()
        .local_private_key(&initiator_static)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&initiator_ephemeral)
        .build_initiator()
        .unwrap();
    let mut responder = Builder::new(params)
        .prologue(PROLOGUE)
        .unwrap()
        .local_private_key(&responder_static)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&responder_ephemeral)
        .build_responder()
        .unwrap();

    let mut message = [0_u8; 4096];
    let mut payload = [0_u8; 4096];
    let mut actual_messages = Vec::new();

    let length = initiator.write_message(&[], &mut message).unwrap();
    actual_messages.push(message[..length].to_vec());
    adapter_read_message(
        &mut responder,
        1,
        &message[..length],
        &mut payload,
        &responder_static,
        None,
    )
    .unwrap();

    let responder_payload = payload_for_certificate(&responder_cert);
    let length = responder
        .write_message(&responder_payload, &mut message)
        .unwrap();
    actual_messages.push(message[..length].to_vec());
    let payload_length = adapter_read_message(
        &mut initiator,
        2,
        &message[..length],
        &mut payload,
        &initiator_static,
        Some(&responder_cert),
    )
    .unwrap();
    assert_eq!(&payload[..payload_length], responder_payload.as_slice());
    assert_payload_certificate(&payload[..payload_length], &responder_cert);
    assert!(
        x25519_adapter_check(&initiator_static, initiator.get_remote_static().unwrap()).is_ok()
    );

    let initiator_payload = payload_for_certificate(&initiator_cert);
    let length = initiator
        .write_message(&initiator_payload, &mut message)
        .unwrap();
    actual_messages.push(message[..length].to_vec());
    let payload_length = adapter_read_message(
        &mut responder,
        3,
        &message[..length],
        &mut payload,
        &responder_static,
        Some(&initiator_cert),
    )
    .unwrap();
    assert_eq!(&payload[..payload_length], initiator_payload.as_slice());
    assert_payload_certificate(&payload[..payload_length], &initiator_cert);
    assert!(
        x25519_adapter_check(&responder_static, responder.get_remote_static().unwrap()).is_ok()
    );
    assert!(initiator.is_handshake_finished());
    assert!(responder.is_handshake_finished());

    let expected_messages = [
        "payload_message_1_hex",
        "payload_message_2_hex",
        "payload_message_3_final_hex",
    ]
    .iter()
    .map(|field| fixture[field].as_str().unwrap())
    .collect::<Vec<_>>();
    for (actual, expected) in actual_messages.iter().zip(expected_messages) {
        assert_eq!(hex(actual), expected);
    }
    assert_eq!(
        hex(initiator.get_handshake_hash()),
        fixture["payload_handshake_hash_hex"].as_str().unwrap()
    );
    assert_eq!(
        initiator.get_remote_static().unwrap(),
        bytes(responder_vector, "transport_public_key_hex")
    );
    assert_eq!(
        responder.get_remote_static().unwrap(),
        bytes(initiator_vector, "transport_public_key_hex")
    );

    let mut initiator_transport = initiator.into_transport_mode().unwrap();
    let mut responder_transport = responder.into_transport_mode().unwrap();
    let mut ciphertext = [0_u8; 4096];
    let length = initiator_transport
        .write_message(b"public-vector-envelope", &mut ciphertext)
        .unwrap();
    assert_eq!(
        hex(&ciphertext[..length]),
        fixture["payload_data_ciphertext_hex"].as_str().unwrap()
    );
    let mut plaintext = [0_u8; 4096];
    let plaintext_length = responder_transport
        .read_message(&ciphertext[..length], &mut plaintext)
        .unwrap();
    assert_eq!(&plaintext[..plaintext_length], b"public-vector-envelope");

    let length = responder_transport
        .write_message(b"public-vector-reply", &mut ciphertext)
        .unwrap();
    assert_eq!(
        hex(&ciphertext[..length]),
        fixture["payload_reply_ciphertext_hex"].as_str().unwrap()
    );
}

#[test]
fn owner_selected_contract_rejects_certificate_mutations_and_unknown_frames() {
    let fixture: toml::Value =
        toml::from_str(include_str!("fixtures/direct_transport_feasibility.toml")).unwrap();
    let vector = &fixture["vectors"].as_array().unwrap()[0];
    let certificate = certificate(vector);

    for index in [0, 8, 40, 112, 141, 149, 157, 165, 180, 181, 244] {
        let mut mutation = certificate.clone();
        mutation[index] ^= 1;
        assert_ne!(
            mutation, certificate,
            "mutation must change certificate bytes"
        );
        assert!(verify_certificate(vector, &mutation).is_err());
    }

    let mut unknown_frame = frame(0x7f, b"unknown");
    assert_eq!(parse_frame(&unknown_frame), Err("unknown_frame_kind"));
    assert_eq!(parse_frame(&frame(3, &[0; 34])), Err("unknown_frame_kind"));
    unknown_frame[6] = 1;
    assert_eq!(parse_frame(&unknown_frame), Err("reserved_flags"));

    let mut oversized = vec![0xff, 0xff, 0xff, 0xff];
    oversized.extend_from_slice(b"bounded");
    assert_eq!(parse_frame(&oversized), Err("frame too large"));

    for payload in [Vec::new(), vec![2; 246], vec![1; 245]] {
        assert_eq!(
            validate_payload(&payload),
            Err("invalid_payload_kind_or_length")
        );
    }
}

#[test]
fn canonical_signed_direct_envelope_is_verified_inside_noise() {
    let fixture = fixture();
    let vector = vector_by_name(&fixture, "public-initiator-vector");
    let canonical = bytes(&fixture, "direct_envelope_canonical_hex");
    assert_eq!(
        std::str::from_utf8(&canonical).unwrap(),
        r#"{"created_at":1700000000,"kind":"direct","nonce":"0000000000000001","payload":{"command":"status","target":"performer-a"},"sender":"omk1_b14a904692024de6e1a48f0fb54116a92db125754e81a080f93f43f69fb545ad","version":1}"#
    );
    let signature = sign_bip340(
        &bytes(vector, "identity_private_key_hex"),
        b"omakure/direct-envelope/v1\0",
        &canonical,
    );
    assert_eq!(
        hex(&signature),
        fixture["direct_envelope_signature_hex"].as_str().unwrap()
    );
    verify_bip340(
        &bytes(vector, "identity_x_only_public_key_hex"),
        b"omakure/direct-envelope/v1\0",
        &canonical,
        &signature,
    )
    .unwrap();

    let (mut sender, mut receiver) = transport_pair(&fixture);
    let mut ciphertext = [0_u8; 4096];
    let mut plaintext = [0_u8; 4096];
    let mut inner = 0_u64.to_be_bytes().to_vec();
    inner.extend_from_slice(&[1, 1]);
    inner.extend_from_slice(&canonical);
    inner.extend_from_slice(&signature);
    let mut sequence_model = ReferenceContractModel::new();
    assert_eq!(
        sequence_model.accept_application_sequence(0),
        "not_authorized"
    );
    assert_eq!(
        sequence_model.authenticate_certificate(
            vector,
            &certificate(vector),
            1_700_000_000,
            "active",
        ),
        "active"
    );
    assert_eq!(
        sequence_model.accept_application_sequence(parse_inner_transport(&inner).unwrap().0),
        "ok"
    );
    let length = sender.write_message(&inner, &mut ciphertext).unwrap();
    let session_id = [0x42_u8; 32];
    let mut encrypted_frame_body = session_id.to_vec();
    encrypted_frame_body.extend_from_slice(&ciphertext[..length]);
    let encrypted_frame = frame(2, &encrypted_frame_body);
    let (plaintext_length, parsed_sequence, parsed_kind) =
        adapter_read_encrypted_frame(&mut receiver, &encrypted_frame, &mut plaintext, &session_id)
            .unwrap();
    assert_eq!(parsed_sequence, 0);
    assert_eq!(parsed_kind, 1);
    assert_eq!(&plaintext[..plaintext_length], inner.as_slice());
    assert_eq!(
        &plaintext[plaintext_length - 64..plaintext_length],
        signature.as_slice()
    );
    verify_bip340(
        &bytes(vector, "identity_x_only_public_key_hex"),
        b"omakure/direct-envelope/v1\0",
        &plaintext[10..plaintext_length - 64],
        &plaintext[plaintext_length - 64..plaintext_length],
    )
    .unwrap();

    for (sequence, kind, code) in [(1_u64, 2_u8, 7_u16), (2, 3, 1004)] {
        let mut control = sequence.to_be_bytes().to_vec();
        control.extend_from_slice(&[kind, 1]);
        control.extend_from_slice(&code.to_be_bytes());
        let length = sender.write_message(&control, &mut ciphertext).unwrap();
        let plaintext_length = receiver
            .read_message(&ciphertext[..length], &mut plaintext)
            .unwrap();
        assert_eq!(&plaintext[..plaintext_length], control.as_slice());
        let (parsed_sequence, parsed_kind, _) = parse_inner_transport(&control).unwrap();
        assert_eq!(parsed_kind, kind);
        assert_eq!(
            sequence_model.accept_control_sequence(parsed_sequence),
            "ok"
        );
    }
}

#[test]
fn adapter_and_reference_model_drive_crypto_time_replay_and_trust_boundaries() {
    let fixture = fixture();
    let probe = bytes(&fixture, "x25519_probe_scalar_hex");
    let prohibited_fixture = fixture["x25519_prohibited_public_hex"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| decode_hex(value.as_str().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(
        prohibited_fixture,
        prohibited_x25519_public_keys()
            .into_iter()
            .map(|key| key.to_vec())
            .collect::<Vec<_>>()
    );
    for public in prohibited_fixture {
        assert_eq!(public.len(), 32);
        assert!(local_x25519_public_check(&public).is_err());
        assert!(x25519_adapter_check(&probe, &public).is_err());
    }
    for vector in fixture["x25519_vectors"].as_array().unwrap() {
        let public = bytes(vector, "public_hex");
        match vector["expected"].as_str().unwrap() {
            "reject_all_zero" | "reject_invalid" => {
                assert!(x25519_adapter_check(&probe, &public).is_err())
            }
            "nonzero" => assert_eq!(
                hex(&x25519_adapter_check(&probe, &public).unwrap()),
                vector["expected_shared_hex"].as_str().unwrap()
            ),
            other => panic!("unknown X25519 vector result {other}"),
        }
    }

    for case in fixture["certificate_cases"].as_array().unwrap() {
        let vector = vector_by_name(&fixture, case["vector"].as_str().unwrap());
        let result = certificate_semantics(
            vector,
            &certificate(vector),
            case["now"].as_integer().unwrap(),
            case.get("expected_node_id")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| vector["node_id"].as_str().unwrap()),
        );
        assert_eq!(
            result,
            case["expected"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }

    let initiator_vector = vector_by_name(&fixture, "public-initiator-vector");
    let initiator_certificate = certificate(initiator_vector);
    let signed_manual = signed_manual_request(&fixture, initiator_vector);
    assert!(verify_signed_manual_request(&fixture, initiator_vector, &signed_manual).is_ok());
    let production_manual = production_manual_request(&fixture, initiator_vector);
    assert_eq!(production_manual, bytes(&fixture, "manual_request_hex"));
    assert_eq!(
        hex(&production_manual[production_manual.len() - 64..]),
        fixture["manual_signature_hex"].as_str().unwrap()
    );
    let parsed_manual = ManualEnrollmentRequest::decode(&production_manual).unwrap();
    parsed_manual
        .verify(u64_value(&fixture, "manual_created_at"))
        .unwrap();
    assert_eq!(parsed_manual.encode(), production_manual);
    for offset in 5..21 {
        let mut mutated = production_manual.clone();
        mutated[offset] ^= 1;
        let parsed = ManualEnrollmentRequest::decode(&mutated).unwrap();
        assert!(parsed
            .verify(u64_value(&fixture, "manual_created_at"))
            .is_err());

        let mut pairing = bytes(&fixture, "manual_pairing_id_hex");
        pairing[offset - 5] ^= 1;
        let resigned = production_manual_request_with_pairing(
            &fixture,
            initiator_vector,
            pairing.try_into().unwrap(),
        );
        let resigned = ManualEnrollmentRequest::decode(&resigned.unwrap()).unwrap();
        resigned
            .verify(u64_value(&fixture, "manual_created_at"))
            .unwrap();
        assert_ne!(resigned.signature, parsed_manual.signature);
    }
    let mut zero_pairing = bytes(&fixture, "manual_pairing_id_hex");
    zero_pairing.fill(0);
    assert!(production_manual_request_with_pairing(
        &fixture,
        initiator_vector,
        zero_pairing.try_into().unwrap(),
    )
    .is_err());
    let mut wrong_version = production_manual.clone();
    wrong_version[4] = 1;
    assert!(ManualEnrollmentRequest::decode(&wrong_version).is_err());
    assert!(
        ManualEnrollmentRequest::decode(&[production_manual.clone(), vec![0]].concat()).is_err()
    );
    for offset in [4, 21, 90, 122, 154, 171, 179, 187] {
        let mut mutated = signed_manual.clone();
        mutated[offset] ^= 1;
        assert!(verify_signed_manual_request(&fixture, initiator_vector, &mutated).is_err());
    }
    let signed_bundle_bytes = signed_bundle(initiator_vector, &initiator_certificate);
    assert_eq!(signed_bundle_bytes.len(), 604);
    assert_eq!(
        verify_signed_bundle(initiator_vector, &signed_bundle_bytes),
        Ok(())
    );
    assert!(verify_certificate(initiator_vector, &signed_bundle_bytes[250..495]).is_ok());
    let mut model = ReferenceContractModel::new();
    assert_eq!(model.interrupt_handshake(), "no_authorized_session");
    assert_eq!(model.accept_application_sequence(0), "not_authorized");
    let mut active_model = ReferenceContractModel::new();
    assert_eq!(
        active_model.authenticate_certificate(
            initiator_vector,
            &initiator_certificate,
            1_700_000_000,
            "active",
        ),
        "active"
    );
    assert_eq!(active_model.accept_application_sequence(0), "ok");
    assert_eq!(
        active_model.accept_control_sequence(0),
        "duplicate_sequence"
    );
    assert_eq!(active_model.accept_control_sequence(2), "sequence_gap");
    for case in fixture["model_cases"].as_array().unwrap() {
        let mut case_model = ReferenceContractModel::new();
        let state = case_model.authenticate_certificate(
            initiator_vector,
            &initiator_certificate,
            case["now"].as_integer().unwrap(),
            case["trust"].as_str().unwrap(),
        );
        assert_eq!(
            state,
            case["expected_state"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
    let mut enrollment_model = ReferenceContractModel::new();
    assert_eq!(
        enrollment_model.authenticate_certificate(
            initiator_vector,
            &initiator_certificate,
            1700000000,
            "none",
        ),
        "authenticated_untrusted"
    );
    assert_eq!(
        enrollment_model.stage_enrollment_message("bundle", &signed_bundle_bytes[7..23]),
        "staged"
    );
    assert_eq!(
        enrollment_model.stage_enrollment_message("bundle", &signed_bundle_bytes[7..23]),
        "replay"
    );
    assert_eq!(
        enrollment_model.stage_enrollment_message("application", &[1; 16]),
        "not_enrollment_message"
    );
    assert_eq!(enrollment_model.rotate(), "not_authorized");
    assert_eq!(enrollment_model.revoke(), "not_authorized");

    let authority = decode_hex("0000000000000000000000000000000000000000000000000000000000000002");
    let rotation_body = b"omakure/rotation/v1\0old=omk1_old\0new=omk1_new";
    let rotation_signature = sign_bip340(&authority, b"omakure/rotation/v1\0", rotation_body);
    let mut rotation_model = ReferenceContractModel::new();
    assert_eq!(
        rotation_model.authenticate_certificate(
            initiator_vector,
            &initiator_certificate,
            1_700_000_000,
            "active",
        ),
        "active"
    );
    assert_eq!(
        rotation_model.authorize_rotation(
            &SigningKey::from_slice(&authority)
                .unwrap()
                .verifying_key()
                .to_bytes(),
            rotation_body,
            &rotation_signature,
        ),
        "authorized"
    );
    assert_eq!(rotation_model.rotate(), "pending_replacement");

    for case in fixture["role_cases"].as_array().unwrap() {
        assert_eq!(
            role_result(case["value"].as_integer().unwrap() as u8),
            case["expected"].as_str().unwrap()
        );
    }
    for case in fixture["capability_cases"].as_array().unwrap() {
        let values = case["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            capability_result(&values),
            case["expected"].as_str().unwrap()
        );
    }

    let vector = vector_by_name(&fixture, "public-initiator-vector");
    let certificate = certificate(vector);
    let bundle = signed_bundle(vector, &certificate);
    assert_eq!(&bundle[250..495], certificate.as_slice());
    let mut mutated = bundle.clone();
    mutated[250] ^= 1;
    assert!(verify_signed_bundle(vector, &bundle).is_ok());
    assert!(verify_signed_bundle(vector, &mutated).is_err());
    for offset in [4, 5, 7, 23, 39, 48, 117, 186, 218, 495, 497, 524, 540] {
        let mut mutated = bundle.clone();
        mutated[offset] ^= 1;
        assert!(verify_signed_bundle(vector, &mutated).is_err());
    }
}

fn production_manual_request(fixture: &toml::Value, vector: &toml::Value) -> Vec<u8> {
    production_manual_request_with_pairing(
        fixture,
        vector,
        bytes(fixture, "manual_pairing_id_hex").try_into().unwrap(),
    )
    .unwrap()
}

fn production_manual_request_with_pairing(
    fixture: &toml::Value,
    vector: &toml::Value,
    pairing_id: [u8; 16],
) -> Result<Vec<u8>, omakure::enrollment::EnrollmentError> {
    let temp = TempDir::new().unwrap();
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(temp.path().join("state")),
            Some(temp.path().join("node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .unwrap();
    let identity =
        NodeIdentity::import(&context, &bytes(vector, "identity_private_key_hex")).unwrap();
    let offer = ManualEnrollmentRequest::create_with_material(
        &identity,
        bytes(vector, "transport_public_key_hex")
            .try_into()
            .unwrap(),
        EnrollmentRole::Conductor,
        vec!["baseline-push".to_string()],
        u64_value(fixture, "manual_created_at"),
        u64_value(fixture, "manual_expires_at") - u64_value(fixture, "manual_created_at"),
        pairing_id,
        bytes(fixture, "manual_request_id_hex").try_into().unwrap(),
        bytes(fixture, "manual_code_hex").try_into().unwrap(),
    )?;
    Ok(offer.request.encode())
}

#[test]
fn frame_fixture_vectors_cover_body_lengths_and_header_rejection() {
    let fixture = fixture();
    for case in fixture["frame_cases"].as_array().unwrap() {
        let encoded = if let Some(raw_hex) = case.get("raw_hex") {
            decode_hex(raw_hex.as_str().unwrap())
        } else {
            let mut encoded = Vec::new();
            let body = decode_hex(case["body_hex"].as_str().unwrap());
            let length = 4 + body.len();
            encoded.extend_from_slice(&(length as u32).to_be_bytes());
            encoded.extend_from_slice(&[
                case["version"].as_integer().unwrap() as u8,
                case["kind"].as_integer().unwrap() as u8,
                (case["flags"].as_integer().unwrap() as u16 >> 8) as u8,
                case["flags"].as_integer().unwrap() as u8,
            ]);
            encoded.extend_from_slice(&body);
            encoded
        };
        let actual = parse_frame(&encoded)
            .map(|_| "ok")
            .unwrap_or_else(|error| error);
        assert_eq!(
            actual,
            case["expected"].as_str().unwrap(),
            "{}",
            case["name"]
        );
    }
    for case in fixture["handshake_order_cases"].as_array().unwrap() {
        assert_eq!(
            handshake_order(
                case["previous"].as_integer().unwrap() as u8,
                case["incoming"].as_integer().unwrap() as u8,
            ),
            case["expected"].as_str().unwrap()
        );
    }
    let max_frame = frame(2, &vec![0; 1_048_576]);
    assert!(parse_frame(&max_frame).is_ok());
    let oversized_frame = frame(2, &vec![0; 1_048_577]);
    assert_eq!(parse_frame(&oversized_frame), Err("frame too large"));
}

#[test]
fn prologue_downgrade_and_interrupted_handshake_are_not_authorized() {
    let fixture = fixture();
    let initiator_vector = vector_by_name(&fixture, "public-initiator-vector");
    let responder_vector = vector_by_name(&fixture, "public-responder-vector");
    let params: NoiseParams = fixture["noise_protocol_name"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut initiator = Builder::new(params.clone())
        .prologue(b"omakure/direct-transport/v0\0")
        .unwrap()
        .local_private_key(&bytes(initiator_vector, "transport_private_key_hex"))
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&bytes(initiator_vector, "ephemeral_private_key_hex"))
        .build_initiator()
        .unwrap();
    let mut responder = Builder::new(params)
        .prologue(PROLOGUE)
        .unwrap()
        .local_private_key(&bytes(responder_vector, "transport_private_key_hex"))
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&bytes(responder_vector, "ephemeral_private_key_hex"))
        .build_responder()
        .unwrap();
    let mut message = [0_u8; 4096];
    let mut payload = [0_u8; 4096];
    let initiator_static = bytes(initiator_vector, "transport_private_key_hex");
    let responder_static = bytes(responder_vector, "transport_private_key_hex");
    let length = initiator.write_message(&[], &mut message).unwrap();
    adapter_read_message(
        &mut responder,
        1,
        &message[..length],
        &mut payload,
        &responder_static,
        None,
    )
    .unwrap();
    let length = responder.write_message(&[], &mut message).unwrap();
    assert!(adapter_read_message(
        &mut initiator,
        2,
        &message[..length],
        &mut payload,
        &initiator_static,
        None,
    )
    .is_err());
    assert!(!initiator.is_handshake_finished());

    assert_eq!(
        fixture["noise_protocol_name"].as_str(),
        Some("Noise_XX_25519_ChaChaPoly_SHA256")
    );
    assert_ne!(
        "Noise_XX_25519_ChaChaPoly_SHA256",
        "Noise_NN_25519_ChaChaPoly_SHA256"
    );
}

#[test]
fn snow_rekey_boundary_is_synchronized_in_both_directions() {
    let fixture = fixture();
    for case in fixture["rekey_cases"].as_array().unwrap() {
        assert_eq!(
            rekey_decision(
                case["messages"].as_integer().unwrap() as u64,
                case["bytes"].as_integer().unwrap() as u64,
                case["next_plaintext"].as_integer().unwrap() as u64,
            ),
            case["expected"].as_str().unwrap()
        );
    }
    let mut rekey_model = RekeyModel::at_boundary();
    assert_eq!(rekey_model.receive_boundary(false), "missing_rekey");
    assert_eq!(rekey_model.receive_boundary(true), "rekeyed");
    let mut early_rekey_model = RekeyModel::default();
    assert_eq!(early_rekey_model.receive_boundary(true), "early_rekey");

    let (mut missing_sender, mut missing_receiver) = transport_pair(&fixture);
    let mut ciphertext = [0_u8; 128];
    let mut plaintext = [0_u8; 128];
    missing_sender.rekey_outgoing();
    let length = missing_sender
        .write_message(b"missing incoming rekey", &mut ciphertext)
        .unwrap();
    assert!(missing_receiver
        .read_message(&ciphertext[..length], &mut plaintext)
        .is_err());
    let (mut early_sender, mut early_receiver) = transport_pair(&fixture);
    early_receiver.rekey_incoming();
    let length = early_sender
        .write_message(b"early incoming rekey", &mut ciphertext)
        .unwrap();
    assert!(early_receiver
        .read_message(&ciphertext[..length], &mut plaintext)
        .is_err());

    let (mut initiator, mut responder) = transport_pair(&fixture);
    for _ in 0..2 {
        let length = initiator.write_message(b"old", &mut ciphertext).unwrap();
        responder
            .read_message(&ciphertext[..length], &mut plaintext)
            .unwrap();
    }
    let initiator_nonce = initiator.sending_nonce();
    initiator.rekey_outgoing();
    responder.rekey_incoming();
    let length = initiator.write_message(b"new", &mut ciphertext).unwrap();
    responder
        .read_message(&ciphertext[..length], &mut plaintext)
        .unwrap();
    assert_eq!(initiator.sending_nonce(), initiator_nonce + 1);

    for _ in 0..2 {
        let length = responder.write_message(b"old", &mut ciphertext).unwrap();
        initiator
            .read_message(&ciphertext[..length], &mut plaintext)
            .unwrap();
    }
    let responder_nonce = responder.sending_nonce();
    responder.rekey_outgoing();
    initiator.rekey_incoming();
    let length = responder.write_message(b"new", &mut ciphertext).unwrap();
    initiator
        .read_message(&ciphertext[..length], &mut plaintext)
        .unwrap();
    assert_eq!(responder.sending_nonce(), responder_nonce + 1);

    let before_failure = initiator.sending_nonce();
    let mut write_model = WriteFailureModel::default();
    for _ in 0..3 {
        assert!(initiator.write_message(b"failure", &mut [0_u8; 1]).is_err());
        assert_eq!(write_model.failed_write(), write_model.failures);
    }
    assert_eq!(initiator.sending_nonce(), before_failure);
    assert!(write_model.closed);
}

fn certificate(vector: &toml::Value) -> Vec<u8> {
    let mut body = Vec::with_capacity(181);
    body.extend_from_slice(CERT_MAGIC);
    body.push(1);
    body.push(1);
    body.extend_from_slice(&0_u16.to_be_bytes());
    body.extend_from_slice(&bytes(vector, "identity_x_only_public_key_hex"));
    body.extend_from_slice(vector["node_id"].as_str().unwrap().as_bytes());
    body.extend_from_slice(&bytes(vector, "transport_public_key_hex"));
    body.extend_from_slice(&u64_value(vector, "key_epoch").to_be_bytes());
    body.extend_from_slice(&u64_value(vector, "not_before").to_be_bytes());
    body.extend_from_slice(&u64_value(vector, "not_after").to_be_bytes());
    body.extend_from_slice(&bytes(vector, "certificate_id_hex"));
    assert_eq!(body.len(), 181);

    body.extend_from_slice(&bytes(vector, "certificate_signature_hex"));
    body
}

fn payload_for_certificate(certificate: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(246);
    payload.push(1);
    payload.extend_from_slice(certificate);
    payload
}

fn assert_payload_certificate(payload: &[u8], certificate: &[u8]) {
    assert_eq!(payload.len(), 246);
    assert_eq!(payload[0], 1);
    assert_eq!(&payload[1..], certificate);
}

fn adapter_read_message(
    handshake: &mut HandshakeState,
    message_number: u8,
    message: &[u8],
    payload: &mut [u8],
    local_static: &[u8],
    expected_certificate: Option<&[u8]>,
) -> Result<usize, &'static str> {
    if handshake_order(message_number.saturating_sub(1), message_number) != "ok" {
        return Err("handshake_order");
    }
    if message_number <= 2 {
        x25519_adapter_check(local_static, message.get(..32).ok_or("invalid_x25519")?)?;
    }
    let payload_length = handshake
        .read_message(message, payload)
        .map_err(|_| "handshake_failed")?;
    if message_number >= 2 {
        let remote_static = handshake
            .get_remote_static()
            .ok_or("missing_remote_static")?;
        x25519_adapter_check(local_static, remote_static)?;
        let expected_certificate = expected_certificate.ok_or("missing_certificate")?;
        assert_payload_certificate(&payload[..payload_length], expected_certificate);
        if &expected_certificate[109..141] != remote_static {
            return Err("certificate_transport_mismatch");
        }
        verify_certificate(
            vector_by_name(
                &fixture(),
                if message_number == 2 {
                    "public-responder-vector"
                } else {
                    "public-initiator-vector"
                },
            ),
            expected_certificate,
        )?;
        x25519_adapter_check(local_static, &expected_certificate[109..141])?;
    } else if payload_length != 0 {
        return Err("unexpected_handshake_payload");
    }
    Ok(payload_length)
}

fn local_x25519_key_check(private: &[u8]) -> Result<[u8; 32], &'static str> {
    let scalar: [u8; 32] = private.try_into().map_err(|_| "scalar_length")?;
    let public = X25519_BASEPOINT.mul_clamped(scalar).to_bytes();
    validate_x25519_public(&public)?;
    Ok(public)
}

fn x25519_adapter_check(scalar: &[u8], public: &[u8]) -> Result<[u8; 32], &'static str> {
    let scalar: [u8; 32] = scalar.try_into().map_err(|_| "scalar_length")?;
    let public: [u8; 32] = public.try_into().map_err(|_| "public_length")?;
    validate_x25519_public(&public)?;
    let shared = MontgomeryPoint(public).mul_clamped(scalar).to_bytes();
    if shared.iter().fold(0_u8, |value, byte| value | byte) == 0 {
        return Err("low_order_x25519");
    }
    Ok(shared)
}

fn validate_x25519_public(public: &[u8]) -> Result<(), &'static str> {
    let public: [u8; 32] = public.try_into().map_err(|_| "public_length")?;
    if is_prohibited_x25519_public(&public) {
        return Err("low_order_x25519");
    }
    Ok(())
}

fn local_x25519_public_check(public: &[u8]) -> Result<(), &'static str> {
    validate_x25519_public(public)
}

fn is_prohibited_x25519_public(public: &[u8; 32]) -> bool {
    prohibited_x25519_public_keys()
        .iter()
        .fold(0_u8, |found, candidate| {
            let difference = candidate
                .iter()
                .zip(public)
                .fold(0_u8, |value, (left, right)| value | (left ^ right));
            found | u8::from(difference == 0)
        })
        != 0
}

fn prohibited_x25519_public_keys() -> [[u8; 32]; 7] {
    [
        [0; 32],
        [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
        decode_hex("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800")
            .try_into()
            .unwrap(),
        decode_hex("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157")
            .try_into()
            .unwrap(),
        decode_hex("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
            .try_into()
            .unwrap(),
        decode_hex("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
            .try_into()
            .unwrap(),
        decode_hex("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
            .try_into()
            .unwrap(),
    ]
}

fn fixture() -> toml::Value {
    toml::from_str(include_str!("fixtures/direct_transport_feasibility.toml"))
        .expect("direct transport fixture must parse")
}

fn vector_by_name<'a>(fixture: &'a toml::Value, name: &str) -> &'a toml::Value {
    fixture["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"].as_str() == Some(name))
        .expect("fixture vector exists")
}

fn certificate_semantics(
    vector: &toml::Value,
    certificate: &[u8],
    now: i64,
    expected_node_id: &str,
) -> &'static str {
    if verify_certificate(vector, certificate).is_err() {
        return "invalid_certificate";
    }
    let embedded_node_id = std::str::from_utf8(&certificate[40..109]).unwrap();
    if embedded_node_id != expected_node_id
        || node_id_from_public(&certificate[8..40]) != embedded_node_id
    {
        return "identity_mismatch";
    }
    let not_before = i64::from_be_bytes(certificate[149..157].try_into().unwrap());
    let not_after = i64::from_be_bytes(certificate[157..165].try_into().unwrap());
    if now + 300 < not_before {
        return "clock_skew";
    }
    if now >= not_after {
        return "expired";
    }
    "ok"
}

struct ReferenceContractModel {
    state: &'static str,
    next_sequence: u64,
    replay_keys: BTreeSet<(String, Vec<u8>)>,
    authorized_operation: Option<&'static str>,
}

impl ReferenceContractModel {
    fn new() -> Self {
        Self {
            state: "handshaking",
            next_sequence: 0,
            replay_keys: BTreeSet::new(),
            authorized_operation: None,
        }
    }

    fn authenticate_certificate(
        &mut self,
        vector: &toml::Value,
        certificate: &[u8],
        now: i64,
        trust: &str,
    ) -> &'static str {
        if verify_certificate(vector, certificate).is_err() {
            self.state = "closed";
            return self.state;
        }
        let not_before = i64::from_be_bytes(certificate[149..157].try_into().unwrap());
        let not_after = i64::from_be_bytes(certificate[157..165].try_into().unwrap());
        if now + 300 < not_before {
            self.state = "clock_skew";
        } else if now >= not_after {
            self.state = "expired";
        } else {
            self.state = match trust {
                "active" => "active",
                "revoked" => "revoked",
                "rotate" => "pending_replacement",
                "none" => "authenticated_untrusted",
                _ => "closed",
            };
        }
        self.state
    }

    fn stage_enrollment_message(&mut self, kind: &str, key: &[u8]) -> &'static str {
        if self.state != "authenticated_untrusted" {
            return "not_enrollment_channel";
        }
        if !matches!(kind, "bundle" | "manual_request") {
            return "not_enrollment_message";
        }
        if !self.replay_keys.insert((kind.to_string(), key.to_vec())) {
            return "replay";
        }
        "staged"
    }

    fn accept_application_sequence(&mut self, sequence: u64) -> &'static str {
        self.accept_authorized_sequence(sequence)
    }

    fn accept_control_sequence(&mut self, sequence: u64) -> &'static str {
        self.accept_authorized_sequence(sequence)
    }

    fn accept_authorized_sequence(&mut self, sequence: u64) -> &'static str {
        if self.state != "active" {
            return "not_authorized";
        }
        if sequence < self.next_sequence {
            return "duplicate_sequence";
        }
        if sequence > self.next_sequence {
            self.state = "closed";
            return "sequence_gap";
        }
        self.next_sequence += 1;
        "ok"
    }

    fn interrupt_handshake(&mut self) -> &'static str {
        if self.state == "handshaking" {
            self.state = "closed";
            "no_authorized_session"
        } else {
            "already_authorized"
        }
    }

    fn authorize_rotation(
        &mut self,
        authority_public: &[u8],
        body: &[u8],
        signature: &[u8],
    ) -> &'static str {
        if self.state != "active" {
            return "not_authorized";
        }
        if verify_bip340(authority_public, b"omakure/rotation/v1\0", body, signature).is_err() {
            return "unauthorized";
        }
        self.authorized_operation = Some("rotate");
        "authorized"
    }

    fn rotate(&mut self) -> &'static str {
        if self.authorized_operation != Some("rotate") {
            return "not_authorized";
        }
        self.state = "pending_replacement";
        self.authorized_operation = None;
        self.state
    }

    fn revoke(&mut self) -> &'static str {
        if self.state != "active" {
            return "not_authorized";
        }
        self.state = "revoked";
        self.state
    }
}

const REKEY_MESSAGES: u64 = 1_048_576;
const REKEY_BYTES: u64 = 1_073_741_824;

fn rekey_decision(messages: u64, bytes: u64, next_plaintext: u64) -> &'static str {
    if messages.saturating_add(1) >= REKEY_MESSAGES
        || bytes.saturating_add(next_plaintext) >= REKEY_BYTES
    {
        "rekey"
    } else {
        "no_rekey"
    }
}

#[derive(Default)]
struct RekeyModel {
    required: bool,
    rekeyed: bool,
}

impl RekeyModel {
    fn at_boundary() -> Self {
        Self {
            required: true,
            rekeyed: false,
        }
    }

    fn receive_boundary(&mut self, rekey_received: bool) -> &'static str {
        if self.rekeyed {
            return "early_rekey";
        }
        if !self.required && rekey_received {
            return "early_rekey";
        }
        if !rekey_received {
            return "missing_rekey";
        }
        self.rekeyed = true;
        "rekeyed"
    }
}

#[derive(Default)]
struct WriteFailureModel {
    failures: u8,
    closed: bool,
}

impl WriteFailureModel {
    fn failed_write(&mut self) -> u8 {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 3 {
            self.closed = true;
        }
        self.failures
    }
}

fn signed_manual_request(fixture: &toml::Value, vector: &toml::Value) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"OMMA");
    body.push(2);
    body.extend_from_slice(&bytes(fixture, "manual_pairing_id_hex"));
    body.extend_from_slice(&bytes(fixture, "manual_request_id_hex"));
    body.extend_from_slice(vector["node_id"].as_str().unwrap().as_bytes());
    body.extend_from_slice(&bytes(vector, "identity_x_only_public_key_hex"));
    body.extend_from_slice(&bytes(vector, "transport_public_key_hex"));
    body.push(1);
    body.push(1);
    body.extend_from_slice(&(13_u16).to_be_bytes());
    body.extend_from_slice(b"baseline-push");
    body.extend_from_slice(&u64_value(fixture, "manual_created_at").to_be_bytes());
    body.extend_from_slice(&u64_value(fixture, "manual_expires_at").to_be_bytes());
    body.extend_from_slice(&bytes(fixture, "manual_code_hash_hex"));
    let signature = sign_bip340(
        &bytes(vector, "identity_private_key_hex"),
        b"omakure/manual-enrollment/v1\0",
        &body,
    );
    body.extend_from_slice(&signature);
    body
}

fn verify_signed_manual_request(
    fixture: &toml::Value,
    vector: &toml::Value,
    request: &[u8],
) -> Result<(), &'static str> {
    if request.len() > 2_048 || request.len() < 64 || &request[..4] != b"OMMA" {
        return Err("manual_shape");
    }
    if request[4] != 2 || request.len() != 299 {
        return Err("manual_version_or_length");
    }
    if request[5..21] != bytes(fixture, "manual_pairing_id_hex")[..] {
        return Err("manual_pairing_id");
    }
    if request[21..37] != decode_hex("00000000000000000000000000000001")[..] {
        return Err("manual_request_id");
    }
    let proposer_node_id = std::str::from_utf8(&request[37..106]).map_err(|_| "manual_node_id")?;
    if proposer_node_id != vector["node_id"].as_str().unwrap() {
        return Err("manual_node_id");
    }
    let proposer_key = &request[106..138];
    if node_id_from_public(proposer_key) != proposer_node_id {
        return Err("manual_identity_mismatch");
    }
    if proposer_key != bytes(vector, "identity_x_only_public_key_hex")
        || request[138..170] != bytes(vector, "transport_public_key_hex")[..]
        || role_result(request[170]) == "invalid_role"
        || request[171] != 1
        || u16::from_be_bytes(request[172..174].try_into().unwrap()) != 13
        || &request[174..187] != b"baseline-push"
    {
        return Err("manual_fields");
    }
    if u64::from_be_bytes(request[187..195].try_into().unwrap()) != 1_700_000_000
        || u64::from_be_bytes(request[195..203].try_into().unwrap()) != 1_702_592_000
        || u64::from_be_bytes(request[195..203].try_into().unwrap())
            <= u64::from_be_bytes(request[187..195].try_into().unwrap())
        || u64::from_be_bytes(request[195..203].try_into().unwrap())
            - u64::from_be_bytes(request[187..195].try_into().unwrap())
            > 30 * 24 * 60 * 60
    {
        return Err("manual_validity");
    }
    let code_hash = Sha256::digest(
        [
            b"omakure/manual-enrollment/v1\0".as_slice(),
            &decode_hex("000102030405060708090a0b0c0d0e0f"),
        ]
        .concat(),
    );
    if request[203..235] != code_hash[..] {
        return Err("manual_code_hash");
    }
    verify_bip340(
        proposer_key,
        b"omakure/manual-enrollment/v1\0",
        &request[..request.len() - 64],
        &request[request.len() - 64..],
    )
}

fn signed_bundle(vector: &toml::Value, certificate: &[u8]) -> Vec<u8> {
    let mut body = bundle_body(vector, certificate);
    let signature = sign_bip340(
        &decode_hex("0000000000000000000000000000000000000000000000000000000000000002"),
        b"omakure/enrollment-bundle/v1\0",
        &body,
    );
    body.extend_from_slice(&signature);
    body
}

fn verify_signed_bundle(vector: &toml::Value, bundle: &[u8]) -> Result<(), &'static str> {
    if bundle.len() != 604 || &bundle[..4] != b"OMEB" {
        return Err("bundle_shape");
    }
    if bundle[4] != 1 || bundle[5..7] != [0, 0] || bundle[7..23] != [7; 16] {
        return Err("bundle_header");
    }
    if bundle[23..39] != [8; 16] {
        return Err("bundle_authority");
    }
    if u16::from_be_bytes(bundle[39..41].try_into().unwrap()) != 7
        || &bundle[41..48] != b"omakure"
        || &bundle[48..117] != vector["node_id"].as_str().unwrap().as_bytes()
        || &bundle[117..186] != vector["node_id"].as_str().unwrap().as_bytes()
        || &bundle[186..218] != bytes(vector, "identity_x_only_public_key_hex").as_slice()
        || &bundle[218..250] != bytes(vector, "transport_public_key_hex").as_slice()
    {
        return Err("bundle_subject");
    }
    verify_certificate(vector, &bundle[250..495])?;
    if bundle[250 + 8..250 + 40] != bundle[186..218]
        || bundle[250 + 109..250 + 141] != bundle[218..250]
    {
        return Err("bundle_certificate_binding");
    }
    if role_result(bundle[495]) == "invalid_role" || bundle[496] != 2 {
        return Err("bundle_role_or_capability_count");
    }
    if bundle[497..499] != [0, 13][..]
        || &bundle[499..512] != b"baseline-push"
        || bundle[512..514] != [0, 10]
        || &bundle[514..524] != b"remote-run"
    {
        return Err("bundle_capabilities");
    }
    let issued_at = u64::from_be_bytes(bundle[524..532].try_into().unwrap());
    let expires_at = u64::from_be_bytes(bundle[532..540].try_into().unwrap());
    if expires_at <= issued_at || expires_at - issued_at > 30 * 24 * 60 * 60 {
        return Err("bundle_validity");
    }
    let authority = SigningKey::from_slice(&decode_hex(
        "0000000000000000000000000000000000000000000000000000000000000002",
    ))
    .map_err(|_| "authority_key")?;
    verify_bip340(
        &authority.verifying_key().to_bytes(),
        b"omakure/enrollment-bundle/v1\0",
        &bundle[..540],
        &bundle[540..],
    )
}

fn sign_bip340(scalar: &[u8], domain: &[u8], body: &[u8]) -> Vec<u8> {
    let digest = Sha256::digest([domain, body].concat());
    SigningKey::from_slice(scalar)
        .unwrap()
        .sign_prehash(&digest)
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn verify_bip340(
    public: &[u8],
    domain: &[u8],
    body: &[u8],
    signature: &[u8],
) -> Result<(), &'static str> {
    let public: [u8; 32] = public.try_into().map_err(|_| "public_key")?;
    let key = VerifyingKey::from_bytes((&public).into()).map_err(|_| "public_key")?;
    let signature = Signature::try_from(signature).map_err(|_| "signature")?;
    let digest = Sha256::digest([domain, body].concat());
    key.verify_prehash(&digest, &signature)
        .map_err(|_| "signature_verification")
}

fn node_id_from_public(public_key: &[u8]) -> String {
    let mut input = b"omakure/node-id/v1\0".to_vec();
    input.extend_from_slice(public_key);
    format!("omk1_{}", hex(Sha256::digest(input).as_slice()))
}

fn role_result(value: u8) -> &'static str {
    match value {
        1 => "conductor",
        2 => "performer",
        _ => "invalid_role",
    }
}

fn capability_result(values: &[String]) -> &'static str {
    let allowlist = [
        "backup-orchestration",
        "baseline-push",
        "inventory-health",
        "lost-device-revocation",
        "notifications",
        "remote-run",
        "ssh-credential-rotation",
    ];
    if values.iter().any(|value| {
        value.is_empty()
            || value.len() > 64
            || value.bytes().any(|byte| {
                !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
            })
    }) {
        return "invalid_grammar";
    }
    if values
        .iter()
        .any(|value| !allowlist.contains(&value.as_str()))
    {
        return "unsupported";
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        if values.windows(2).any(|pair| pair[0] == pair[1]) {
            return "duplicate";
        }
        return "unsorted";
    }
    if values.len() > 32 || values.iter().map(String::len).sum::<usize>() > 4096 {
        return "too_large";
    }
    "ok"
}

fn bundle_body(vector: &toml::Value, certificate: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"OMEB");
    body.push(1);
    body.extend_from_slice(&0_u16.to_be_bytes());
    body.extend_from_slice(&[7; 16]);
    body.extend_from_slice(&[8; 16]);
    body.extend_from_slice(&(7_u16).to_be_bytes());
    body.extend_from_slice(b"omakure");
    body.extend_from_slice(vector["node_id"].as_str().unwrap().as_bytes());
    body.extend_from_slice(vector["node_id"].as_str().unwrap().as_bytes());
    body.extend_from_slice(&bytes(vector, "identity_x_only_public_key_hex"));
    body.extend_from_slice(&bytes(vector, "transport_public_key_hex"));
    body.extend_from_slice(certificate);
    body.push(2);
    body.push(2);
    for capability in ["baseline-push", "remote-run"] {
        body.extend_from_slice(&(capability.len() as u16).to_be_bytes());
        body.extend_from_slice(capability.as_bytes());
    }
    body.extend_from_slice(&1_700_000_000_u64.to_be_bytes());
    body.extend_from_slice(&1_702_592_000_u64.to_be_bytes());
    body
}

fn validate_payload(payload: &[u8]) -> Result<&[u8], &'static str> {
    if payload.len() != 246 || payload[0] != 1 {
        return Err("invalid_payload_kind_or_length");
    }
    Ok(&payload[1..])
}

fn transport_pair(fixture: &toml::Value) -> (TransportState, TransportState) {
    let initiator_vector = vector_by_name(fixture, "public-initiator-vector");
    let responder_vector = vector_by_name(fixture, "public-responder-vector");
    let params: NoiseParams = fixture["noise_protocol_name"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let initiator_static = bytes(initiator_vector, "transport_private_key_hex");
    let responder_static = bytes(responder_vector, "transport_private_key_hex");
    let initiator_ephemeral = bytes(initiator_vector, "ephemeral_private_key_hex");
    let responder_ephemeral = bytes(responder_vector, "ephemeral_private_key_hex");
    let initiator_cert = certificate(initiator_vector);
    let responder_cert = certificate(responder_vector);
    let mut initiator = Builder::new(params.clone())
        .prologue(PROLOGUE)
        .unwrap()
        .local_private_key(&initiator_static)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&initiator_ephemeral)
        .build_initiator()
        .unwrap();
    let mut responder = Builder::new(params)
        .prologue(PROLOGUE)
        .unwrap()
        .local_private_key(&responder_static)
        .unwrap()
        .fixed_ephemeral_key_for_testing_only(&responder_ephemeral)
        .build_responder()
        .unwrap();
    let mut message = [0_u8; 4096];
    let mut payload = [0_u8; 4096];
    let length = initiator.write_message(&[], &mut message).unwrap();
    adapter_read_message(
        &mut responder,
        1,
        &message[..length],
        &mut payload,
        &responder_static,
        None,
    )
    .unwrap();
    let length = responder
        .write_message(&payload_for_certificate(&responder_cert), &mut message)
        .unwrap();
    adapter_read_message(
        &mut initiator,
        2,
        &message[..length],
        &mut payload,
        &initiator_static,
        Some(&responder_cert),
    )
    .unwrap();
    let length = initiator
        .write_message(&payload_for_certificate(&initiator_cert), &mut message)
        .unwrap();
    adapter_read_message(
        &mut responder,
        3,
        &message[..length],
        &mut payload,
        &responder_static,
        Some(&initiator_cert),
    )
    .unwrap();
    (
        initiator.into_transport_mode().unwrap(),
        responder.into_transport_mode().unwrap(),
    )
}

fn assert_certificate(vector: &toml::Value, certificate: &[u8]) {
    assert_eq!(certificate[..4], *CERT_MAGIC);
    assert_eq!(certificate[4], 1);
    assert_eq!(certificate[5], 1);
    assert_eq!(&certificate[6..8], &[0, 0]);
    assert!(verify_certificate(vector, certificate).is_ok());
    assert_eq!(
        vector["node_id"].as_str().unwrap(),
        String::from_utf8(certificate[40..109].to_vec()).unwrap()
    );
}

fn verify_certificate(vector: &toml::Value, certificate: &[u8]) -> Result<(), &'static str> {
    if certificate.len() != 245 || &certificate[..4] != CERT_MAGIC {
        return Err("certificate shape");
    }
    if certificate[4] != 1 || certificate[5] != 1 || certificate[6..8] != [0, 0] {
        return Err("certificate header");
    }
    let key_bytes: [u8; 32] = certificate[8..40]
        .try_into()
        .map_err(|_| "identity key length")?;
    let node_id = std::str::from_utf8(&certificate[40..109]).map_err(|_| "node id")?;
    if node_id_from_public(&key_bytes) != node_id {
        return Err("node id derivation");
    }
    if key_bytes != bytes(vector, "identity_x_only_public_key_hex").as_slice()
        || &certificate[109..141] != bytes(vector, "transport_public_key_hex").as_slice()
    {
        return Err("certificate_identity_binding");
    }
    validate_x25519_public(&certificate[109..141])?;
    let epoch = u64::from_be_bytes(certificate[141..149].try_into().unwrap());
    if epoch == 0 || epoch != u64_value(vector, "key_epoch") {
        return Err("certificate_epoch");
    }
    let not_before = u64::from_be_bytes(certificate[149..157].try_into().unwrap());
    let not_after = u64::from_be_bytes(certificate[157..165].try_into().unwrap());
    if not_after <= not_before || not_after - not_before > 2 * 365 * 24 * 60 * 60 {
        return Err("certificate_lifetime");
    }
    if certificate[165..181] != bytes(vector, "certificate_id_hex")[..] {
        return Err("certificate_id");
    }
    let key = VerifyingKey::from_bytes((&key_bytes).into()).map_err(|_| "identity key")?;
    let signature = Signature::try_from(&certificate[181..245]).map_err(|_| "signature")?;
    let digest = Sha256::digest([CERT_DOMAIN, &certificate[..181]].concat());
    key.verify_prehash(&digest, &signature)
        .map_err(|_| "certificate signature")
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let length = 4 + body.len();
    let mut frame = Vec::with_capacity(4 + length);
    frame.extend_from_slice(&(length as u32).to_be_bytes());
    frame.extend_from_slice(&[1, kind, 0, 0]);
    frame.extend_from_slice(body);
    frame
}

fn parse_frame(frame: &[u8]) -> Result<&[u8], &'static str> {
    if frame.len() < 4 {
        return Err("truncated_length_prefix");
    }
    let length = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    if length < 4 {
        return Err("invalid_length");
    }
    if length > 1_048_580 {
        return Err("frame too large");
    }
    if frame.len() < length + 4 {
        return Err("truncated_frame");
    }
    if frame.len() != length + 4 {
        return Err("length_mismatch");
    }
    if frame[4] != 1 {
        return Err("unsupported_version");
    }
    if frame[6..8] != [0, 0] {
        return Err("reserved_flags");
    }
    if !matches!(frame[5], 1..=2) {
        return Err("unknown_frame_kind");
    }
    let body = &frame[8..];
    let valid_body = match frame[5] {
        1 => (1..=4096).contains(&body.len()) && matches!(body[0], 1..=3),
        2 => body.len() >= 60,
        _ => false,
    };
    if !valid_body {
        return Err("invalid_body_length");
    }
    Ok(body)
}

fn handshake_order(previous: u8, incoming: u8) -> &'static str {
    if incoming != previous + 1 || !matches!(incoming, 1..=3) {
        "handshake_order"
    } else {
        "ok"
    }
}

fn parse_inner_transport(inner: &[u8]) -> Result<(u64, u8, &[u8]), &'static str> {
    if inner.len() < 12 {
        return Err("inner_body_length");
    }
    let sequence = u64::from_be_bytes(inner[..8].try_into().unwrap());
    if inner[9] != 1 {
        return Err("inner_version");
    }
    let body = &inner[10..];
    match inner[8] {
        1 if body.len() >= 64 => Ok((sequence, 1, body)),
        2 | 3 if body.len() == 2 => Ok((sequence, inner[8], body)),
        _ => Err("inner_kind_or_length"),
    }
}

fn adapter_read_encrypted_frame(
    transport: &mut TransportState,
    frame_bytes: &[u8],
    plaintext: &mut [u8],
    expected_session_id: &[u8],
) -> Result<(usize, u64, u8), &'static str> {
    if frame_bytes.get(5) != Some(&2) {
        return Err("encrypted_frame_kind");
    }
    let body = parse_frame(frame_bytes)?;
    if body.len() < 32 || &body[..32] != expected_session_id {
        return Err("session_id");
    }
    let plaintext_length = transport
        .read_message(&body[32..], plaintext)
        .map_err(|_| "transport_decrypt")?;
    let (sequence, kind, _) = parse_inner_transport(&plaintext[..plaintext_length])?;
    Ok((plaintext_length, sequence, kind))
}

fn bytes(value: &toml::Value, field: &str) -> Vec<u8> {
    decode_hex(value[field].as_str().unwrap())
}

fn u64_value(value: &toml::Value, field: &str) -> u64 {
    value[field].as_integer().unwrap().try_into().unwrap()
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2));
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
