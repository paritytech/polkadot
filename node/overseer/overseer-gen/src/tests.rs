// The generated code requires quite a bit of surrounding code to work.
// Please refer to [the examples](examples/dummy.rs) and
// [the minimal usage example](../examples/minimal-example.rs).

#[test]
fn ui_compile_fail() {
	let t = trybuild::TestCases::new();
	t.compile_fail("tests/ui/err-*.rs");
}
