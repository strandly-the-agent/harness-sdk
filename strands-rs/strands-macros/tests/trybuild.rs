//! Compile tests for `#[tool]` and `#[tools]`.

#[test]
fn pass() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/pass/*.rs");
}

#[test]
fn compile_fail() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/fail/*.rs");
}
