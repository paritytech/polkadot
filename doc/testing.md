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

### Testing at scale

Launching many nodes with configurable network speed and node features in a cluster of nodes.
At this scale the [`simnet`][simnet] comes into play which launches a full cluster of nodes.
Asserts are(?) made based on metrics.

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

export CARGO_INCREMENTAL=0
# wasm is not happy with the instrumentation
export SKIP_BUILD_WASM=true
export BUILD_DUMMY_WASM_BINARY=true
# the actully collected coverage data
export LLVM_PROFILE_FILE="llvmcoveragedata-%p-%m.profraw"
# build wasm without instrumentation
export WASM_TARGET_DIRECTORY=/tmp/wasm
cargo +nightly build
# required rust flags
export RUSTFLAGS="-Zinstrument-coverage"
# assure target dir is clean
cargo clean
# run tests to get coverage data
cargo +nightly test --all

# create the *html* report out of all the test binaries
# mostly useful for local inspection
grcov . --binary-path ./target/debug -s . -t html --branch --ignore-not-existing -o ./coverage/
miniserve -r ./coverage

# create a *codecov* compatible report
grcov . --binary-path ./target/debug/ -s . -t lcov --branch --ignore-not-existing --ignore "/*" -o lcov.info
```

The test coverage in `lcov` can the be published to <codecov.io>.

```sh
bash <(curl -s https://codecov.io/bash) -f lcov.info
```

or just printed as part of the PR using a gh action i.e. [jest-lcov-reporter](https://github.com/marketplace/actions/jest-lcov-reporter).

For full examples on how to use [grcov /w polkadot specifics see the gh repo](https://github.com/mozilla/grcov#coverallscodecov-output).

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
* ...

---


## Implementation of different behaviour strain nodes.

### Goals

The main goals are is to allow creating a test node which
exhibits a certain behaviour by utilizing a subset of _wrapped_ or _replaced_ subsystems easily.
The runtime must not matter at all for these tests and should be simplistic.
The execution must be fast, this mostly means to assure a close to zero network latency as
well as shorting the block time and epoch times down to a few `100ms` and a few dozend blocks per epoch.

### Approach

`AllSubsystems` is an intermediate mocking type. As such it is a prime target for modification.
`AllSubsystemsGen` is a proc-macro that should be extended as needed for per subsystem specific
logic that would otherwise cause significant boilerplate additions.

There are common patterns, where i.e. a subsystem produces garbage, or does not produce any output.
These most be provided by default for re-usability. Another option would be a `CopyCat` node that
picks one other node and just repeats whatever the initial node does. These are just _ideas_
and might not prove viable or yield signifcant outcomes.

### Impl

```rust
launch_integration_testcase!{
"TestRuntime" =>
"Alice": SubsystemsAll<GenericParam=DropAll,..>,
"Bob": SubsystemsAll<GenericParam=DropSome,..>,
"Charles": Default,
"David": "Bob",
"Eve": "Bob",
}
```

> TODO format sucks, revise, how does cli interact with it? Do we want configurable profiles?

The coordination of multiple subsystems across nodes must be made possible via
a side-channel. That means those nodes most be able to prepare attacks that
are collaborative based on each others actions.

> TODO


[simnet]: https://github.com/paritytech/simnet_scripts
