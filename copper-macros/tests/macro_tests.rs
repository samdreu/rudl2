#[test]
fn ui_tests() {
    let t = trybuild::TestCases::new();

    t.pass("tests/ui/pass/*");
    t.compile_fail("tests/ui/fail/*");
}
