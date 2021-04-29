# testing

Automated testing is an essential tool to assure correctness.

## Scopes

The testing strategy for polkadot is 4-fold:

### Unit testing

Boring, small scale correctness tests of individual functions.

### Integration tests

One particular subsystem (subsystem under test) interacts with a
mocked overseer that is made to assert incoming and outgoing messages
of the subsystem under test.

### System tests

Launching small scale networks, with multiple adversarial nodes without any further tooling required.
This should include tests around the thresholds in order to evaluate the error handling once certain
assumed invariants fail.

### Scale testing

Launching many nodes with configurable network speed and node features in a cluster of nodes.
At this scale the [`simnet`][simnet] comes into play which launches a full cluster of nodes.
Asserts are made based on metrics.

---

## Coverage

Coverage gives a _hint_ of the actually covered source lines by tests and test applications.

The state of the art is currently [tarpaulin][tarpaulin] which unfortunately yields a
lot false negatives (lines that are in fact covered, marked as uncovered, which leads to
lower coverage percentages).
Since late 2020 rust has gained [MIR based coverage tooling](
https://blog.rust-lang.org/inside-rust/2020/11/12/source-based-code-coverage.html).

```sh
# setup
rustup component add llvm-tools-preview
cargo install grcov miniserve

# wasm is not happy with the instrumentation
export SKIP_BUILD_WASM=true
# the actully collected coverage data
export LLVM_PROFILE_FILE="llvmcoveragedata-%p-%m.profraw"
# required rustc flags
export RUSTFLAGS="-Zinstrument-coverage"
# assure target dir is clean
cargo clean
# build
cargo +nightly build
# run tests to get coverage data
cargo +nightly test --all
# create the report out of all the test binaries
grcov . --binary-path ./target/debug -s . -t html --branch --ignore-not-existing -o ./coverage/
miniserve -r ./coverage
```

## Fuzzing

Fuzzing is an approach to verify correctness against arbitrary or partially structured inputs.

Currently implemented fuzzing targets:

* `erasure-coding`
* `bridges/storage-proof`

The tooling of choice here is `honggfuzz-rs` as it allows _fastest_ coverage according to "some paper" which is a positive feature when run as part of PRs.

Fuzzing is generally not applicable for data secured by cryptographic hashes or signatures. Either the input has to be specifically crafted, such that the discarded input
percentage stays in an acceptable range.
System level fuzzing is hence simply not feasible due to the amount of state that is required.

Other candidates to implement fuzzing are:

* `rpc`
* ...

## Writing small scope integration tests with preconfigured workers

Requirements:

* spawn nodes with preconfigured behaviours
* allow multiple types of configuration to be specified
* allow extensability via external crates
*...

---

[simnet]: https://github.com/paritytech/simnet_scripts
