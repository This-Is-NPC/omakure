use serde_json::Value;

pub fn canonical(value: &Value) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("canonical JSON")
}
