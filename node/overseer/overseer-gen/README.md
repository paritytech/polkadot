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

A task spawner and subsystem context are required to be defined with `SpawnNamed` and respectively `SubsystemContext` implemented.
