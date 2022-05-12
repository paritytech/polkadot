# overseer pattern

The overseer pattern is a partial actor pattern

## proc-macro

The proc macro provides a convenience generator with a builder pattern,
where at it's core it creates and spawns a set of subsystems, which are purely
declarative.

```rust
    #[overlord(signal=SigSigSig, event=Event, gen=AllMessages, error=OverseerError)]
    pub struct Overseer {
        #[subsystem(MsgA)]
        sub_a: AwesomeSubSysA,

        #[subsystem(MsgB)]
        sub_b: AwesomeSubSysB,
    }
```

* Each subsystem is annotated with `#[subsystem(_)]` where `MsgA` respectively `MsgB` are the messages
being consumed by that particular subsystem. Each of those subsystems is required to implement the subsystem
trait.
* `error=` tells the overseer to use the user provided
error type, if not provided a builtin one is used. Note that this is the one error type used throughout all calls, so make sure it does impl `From<E>` for all other error types `E` that are relevant to your application.
* `event=` declares an external event type, that injects certain events
into the overseer, without participating in the subsystem pattern.
* `signal=` defines a signal type to be used for the overseer. This is a shared "clock" for all subsystems.
* `gen=` defines a wrapping `enum` type that is used to wrap all messages that can be consumed by _any_ subsystem.

```rust
    /// Execution context, always requred.
    pub struct DummyCtx;

    /// Task spawner, always required.
    pub struct DummySpawner;

    fn main() {
        let _overseer = Overseer::builder()
            .sub_a(AwesomeSubSysA::default())
            .sub_b(AwesomeSubSysB::default())
            .spawner(DummySpawner)
            .build();
    }
```

In the shown `main`, the overseer is created by means of a generated, compile time erroring
builder pattern.

The builder requires all subsystems, baggage fields (additional struct data) and spawner to be
set via the according setter method before `build` method could even be called. Failure to do
such an initialization will lead to a compile error. This is implemented by encoding each
builder field in a set of so called `state generics`, meaning that each field can be either
`Init<T>` or `Missing<T>`, so each setter translates a state from `Missing` to `Init` state
for the specific struct field. Therefore, if you see a compile time error that blames about
`Missing` where `Init` is expected it usually means that some subsystems or baggage fields were
not set prior to the `build` call.

To exclude subsystems from such a check, one can set `wip` attribute on some subsystem that
is not ready to be included in the Overseer:

```rust
    #[overlord(signal=SigSigSig, event=Event, gen=AllMessages, error=OverseerError)]
    pub struct Overseer {
        #[subsystem(MsgA, sends: MsgB)]
        sub_a: AwesomeSubSysA,

        #[subsystem(MsgB, sends: MsgA), wip]
        sub_b: AwesomeSubSysB, // This subsystem will not be required nor allowed to be set
    }
```

Baggage fields can be initialized more than one time, however, it is not true for subsystems:
subsystems must be initialized only once (another compile time check) or be _replaced_ by
a special setter like method `replace_<subsystem>`.

A task spawner and subsystem context are required to be defined with `SpawnNamed` and respectively `SubsystemContext` implemented.

## Debugging

As always, debugging is notoriously annoying with bugged proc-macros.

Therefore [`expander`](https://github.com/drahnr/expander) is employed to yield better
error messages. Enable with `--feature=polkadot-overseer-gen/expand` or
`--feature=polkadot-overseer/expand` from the root of the project or
make `"expand"` part of the default feature set.
