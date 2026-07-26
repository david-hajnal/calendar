#[test]
fn backend_binary_is_available() {
    let binary = env!("CARGO_BIN_EXE_commoncal-backend");

    assert!(!binary.is_empty());
}
