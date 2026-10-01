use omakure::direct_transport::TransportCertificate;
use omakure::node::NodeContext;
use omakure::node_identity::NodeIdentity;

pub fn load(context: &NodeContext) -> (NodeIdentity, [u8; 32], TransportCertificate) {
    let identity = NodeIdentity::load_existing(context).expect("load node identity");
    let raw_private = std::fs::read(context.transport_key_path()).expect("read transport key");
    let private: [u8; 32] = <[u8; 32]>::try_from(raw_private.as_slice())
        .unwrap_or_else(|_| panic!("transport key length: {} bytes", raw_private.len()));
    let certificate = TransportCertificate::from_bytes(
        &std::fs::read(context.transport_certificate_path()).expect("read transport certificate"),
    )
    .expect("parse transport certificate");
    (identity, private, certificate)
}
