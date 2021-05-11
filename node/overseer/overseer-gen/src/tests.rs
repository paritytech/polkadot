
#[test]
fn ui_pass() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/ok-*.rs");
}
#[test]
fn ui_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/err-*.rs");
}
