# XCM Simulator Fuzzer

This project will fuzz-test the XCM simulator. It can catch reachable panics, timeouts as well as integer overflows and underflows.

## Install dependencies

```
cargo install honggfuzz
```

## Run the fuzzer

In this directory, run this command:

```
cargo hfuzz run xcm-fuzzer
```

## Run a single input

In this directory, run this command:

```
cargo hfuzz run-debug xcm-fuzzer hfuzz_workspace/xcm-fuzzer/fuzzer_input_file
```

## Generate coverage

In this directory, run these four commands:

```
RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Copt-level=0 -Clink-dead-code -Coverflow-checks=off -Zpanic_abort_tests -Cpanic=abort" CARGO_INCREMENTAL=0 SKIP_WASM_BUILD=1 CARGO_HOME=./cargo cargo build
../../../target/debug/xcm-fuzzer hfuzz_workspace/xcm-fuzzer/input/
zip -0 ccov.zip `find ../../../target/ \( -name "*.gc*" -o -name "test-*.gc*" \) -print`
grcov ccov.zip -s ../../../ -t html --llvm --branch --ignore-not-existing -o ./coverage
```

The code coverage will be in `./coverage/index.html`.
