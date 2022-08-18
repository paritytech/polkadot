# Testing

Automated testing is an essential tool to assure correctness.

## Scopes

The testing strategy for polkadot is 4-fold:

### Unit testing (1)

Boring, small scale correctness tests of individual functions.

### Integration tests

There are two variants of integration tests:

#### Subsystem tests (2)

One particular subsystem (subsystem under test) interacts with a
mocked overseer that is made to assert incoming and outgoing messages
of the subsystem under test.
This is largely present today, but has some fragmentation in the evolved
integration test implementation. A `proc-macro`/`macro_rules` would allow
for more consistent implementation and structure.

#### Behavior tests (3)

Launching small scale networks, with multiple adversarial nodes without any further tooling required.
This should include tests around the thresholds in order to evaluate the error handling once certain
assumed invariants fail.

For this purpose based on `AllSubsystems` and `proc-macro` `AllSubsystemsGen`.

This assumes a simplistic test runtime.

#### Testing at scale (4)

Launching many nodes with configurable network speed and node features in a cluster of nodes.
At this scale the [Simnet][simnet] comes into play which launches a full cluster of nodes.
The scale is handled by spawning a kubernetes cluster and the meta description
is covered by [Gurke][Gurke].
Asserts are made using Grafana rules, based on the existing prometheus metrics. This can
be extended by adding an additional service translating `jaeger` spans into addition
prometheus avoiding additional polkadot source changes.

_Behavior tests_ and _testing at scale_ have naturally soft boundary.
The most significant difference is the presence of a real network and
the number of nodes, since a single host often not capable to run
multiple nodes at once.

---

## Coverage

Coverage gives a _hint_ of the actually covered source lines by tests and test applications.

The state of the art is currently [tarpaulin][tarpaulin] which unfortunately yields a
lot of false negatives. Lines that are in fact covered, marked as uncovered due to a mere linebreak in a statement can cause these artifacts. This leads to
lower coverage percentages than there actually is.

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
rm -r target/{debug,tests}
# run tests to get coverage data
cargo +nightly test --all

# create the *html* report out of all the test binaries
# mostly useful for local inspection
grcov . --binary-path ./target/debug -s . -t html --branch --ignore-not-existing -o ./coverage/
miniserve -r ./coverage

# create a *codecov* compatible report
grcov . --binary-path ./target/debug/ -s . -t lcov --branch --ignore-not-existing --ignore "/*" -o lcov.info
```

The test coverage in `lcov` can the be published to <https://codecov.io>.

```sh
bash <(curl -s https://codecov.io/bash) -f lcov.info
```

or just printed as part of the PR using a github action i.e. [`jest-lcov-reporter`](https://github.com/marketplace/actions/jest-lcov-reporter).

For full examples on how to use [`grcov` /w polkadot specifics see the github repo](https://github.com/mozilla/grcov#coverallscodecov-output).

## Fuzzing

Fuzzing is an approach to verify correctness against arbitrary or partially structured inputs.

Currently implemented fuzzing targets:

* `erasure-coding`

The tooling of choice here is `honggfuzz-rs` as it allows _fastest_ coverage according to "some paper" which is a positive feature when run as part of PRs.

Fuzzing is generally not applicable for data secured by cryptographic hashes or signatures. Either the input has to be specifically crafted, such that the discarded input
percentage stays in an acceptable range.
System level fuzzing is hence simply not feasible due to the amount of state that is required.

Other candidates to implement fuzzing are:

* `rpc`
* ...

## Performance metrics

There are various ways of performance metrics.

* timing with `criterion`
* cache hits/misses w/ `iai` harness or `criterion-perf`
* `coz` a performance based compiler

Most of them are standard tools to aid in the creation of statistical tests regarding change in time of certain unit tests.

`coz` is meant for runtime. In our case, the system is far too large to yield a sufficient number of measurements in finite time.
An alternative approach could be to record incoming package streams per subsystem and store dumps of them, which in return could be replayed repeatedly at an
accelerated speed, with which enough metrics could be obtained to yield
information on which areas would improve the metrics.
This unfortunately will not yield much information, since most if not all of the subsystem code is linear based on the input to generate one or multiple output messages, it is unlikely to get any useful metrics without mocking a sufficiently large part of the other subsystem which overlaps with [#Integration tests] which is unfortunately not repeatable as of now.
As such the effort gain seems low and this is not pursued at the current time.

## Writing small scope integration tests with preconfigured workers

Requirements:

* spawn nodes with preconfigured behaviors
* allow multiple types of configuration to be specified
* allow extendability via external crates
* ...

---

## Implementation of different behavior strain nodes

### Goals

The main goals are is to allow creating a test node which
exhibits a certain behavior by utilizing a subset of _wrapped_ or _replaced_ subsystems easily.
The runtime must not matter at all for these tests and should be simplistic.
The execution must be fast, this mostly means to assure a close to zero network latency as
well as shorting the block time and epoch times down to a few `100ms` and a few dozend blocks per epoch.

### Approach

#### MVP

A simple small scale builder pattern would suffice for stage one implementation of allowing to
replace individual subsystems.
An alternative would be to harness the existing `AllSubsystems` type
and replace the subsystems as needed.

#### Full `proc-macro` implementation

`Overseer` is a common pattern.
It could be extracted as `proc` macro and generative `proc-macro`.
This would replace the `AllSubsystems` type as well as implicitly create
the `AllMessages` enum as  `AllSubsystemsGen` does today.

The implementation is yet to be completed, see the [implementation PR](https://github.com/paritytech/polkadot/pull/2962) for details.

##### Declare an overseer implementation

```rust
struct BehaveMaleficient;

impl OverseerGen for BehaveMaleficient {
 fn generate<'a, Spawner, RuntimeClient>(
  &self,
  args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
 ) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandler), Error>
 where
  RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
  RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
  Spawner: 'static + overseer::gen::Spawner + Clone + Unpin,
 {
  let spawner = args.spawner.clone();
  let leaves = args.leaves.clone();
  let runtime_client = args.runtime_client.clone();
  let registry = args.registry.clone();
  let candidate_validation_config = args.candidate_validation_config.clone();
  // modify the subsystem(s) as needed:
  let all_subsystems = create_default_subsystems(args)?.
        // or spawn an entirely new set

        replace_candidate_validation(
   // create the filtered subsystem
   FilteredSubsystem::new(
    CandidateValidationSubsystem::with_config(
     candidate_validation_config,
     Metrics::register(registry)?,
    ),
                // an implementation of
    Skippy::default(),
   ),
  );

  Overseer::new(leaves, all_subsystems, registry, runtime_client, spawner)
   .map_err(|e| e.into())

        // A builder pattern will simplify this further
        // WIP https://github.com/paritytech/polkadot/pull/2962
 }
}

fn main() -> eyre::Result<()> {
 color_eyre::install()?;
 let cli = Cli::from_args();
 assert_matches::assert_matches!(cli.subcommand, None);
 polkadot_cli::run_node(cli, BehaveMaleficient)?;
 Ok(())
}
```

[`variant-a`](../node/malus/src/variant-a.rs) is a fully working example.

#### Simnet

Spawn a kubernetes cluster based on a meta description using [Gurke] with the
[Simnet] scripts.

Coordinated attacks of multiple nodes or subsystems must be made possible via
a side-channel, that is out of scope for this document.

The individual node configurations are done as targets with a particular
builder configuration.

#### Behavior tests w/o Simnet

Commonly this will require multiple nodes, and most machines are limited to
running two or three nodes concurrently.
Hence, this is not the common case and is just an implementation _idea_.

```rust
behavior_testcase!{
"TestRuntime" =>
"Alice": <AvailabilityDistribution=DropAll, .. >,
"Bob": <AvailabilityDistribution=DuplicateSend, .. >,
"Charles": Default,
"David": "Charles",
"Eve": "Bob",
}
```

[Gurke]: https://github.com/paritytech/gurke
[simnet]: https://github.com/paritytech/simnet_scripts
