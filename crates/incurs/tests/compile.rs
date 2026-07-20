#[test]
fn typed_api_rejects_contract_mismatches() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}
