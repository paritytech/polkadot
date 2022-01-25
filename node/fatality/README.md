# fatality

A generative approach to creating fatal and non-fatal errors.

The generated source utilizes `thiserror::Error` derived attributes heavily,
and any unknown annotations will be passed to that.


## Usage

`#[fatality]` currently provides a `trait Fatality` with a single `fn is_fatal(&self) -> bool` by default.

Annotating with `#[fatality(splitable)]` (which disallows usage of `forward` variant annotations), allows to split the type into two sub-types, a `Jfyi*` and a `Fatal*` one via `fn split(self) -> Result<Self::Jfyi, Self::Fatal>`.


The derive macro implements them, and can defer calls, based on `thiserror` annotations, specifically
`#[source]` and `#[transparent]` on `enum` variants and their members.

```rust
/// Fatality only works with `enum` for now.
/// It will automatically add `#[derive(Debug, thiserror::Error)]`
/// annotations.
#[fatality]
enum OhMy {
    #[error("An apple a day")]
    Itsgonnabefine,

    /// Forwards the `is_fatal` to the `InnerError`, which has to implement `trait Fatality` as well.
    #[fatal(forward)]
    #[error("Dropped dead")]
    ReallyReallyBad(#[source] InnerError),

    /// Also works on `#[error(transparent)]
    #[fatal(forward)]
    #[error(transparent)]
    Translucent(InnerError),


    /// Will always return `is_fatal` as `true`,
    /// irrespective of `#[error(transparent)]` or
    /// `#[source]` annotations.
    #[fatal]
    #[error("So dead")]
    SoDead(#[source] InnerError),
}
```

```rust
#[fatality(splitable)]
enum Yikes {
    #[error("An apple a day")]
    Orange,

    #[fatal]
    #[error("So dead")]
    Dead,
}

fn foo() -> Result<[u8;32], Yikes> {
    Err(Yikes::Dead)
}

fn i_call_foo() -> Result<(), FatalYikes> {
    // availble via a convenience trait `Nested` that is implemented
    // for any `Result` whose error type implements `Split`.
    let x: Result<[u8;32], Jfyi> = foo().into_nested()?;
}

fn i_call_foo_too() -> Result<(), FatalYikes> {
    if let Err(jfyi_and_fatal_ones) = foo() {
        // bail if bad, otherwise just log it
        log::warn!("Jfyi: {:?}", jfyi_and_fatal_ones.split()?);
    }
}
```

## Roadmap

* [] Reduce the marco overhead, replace `#[fatal($args)]#[error(..` with `#[fatal($args;..)]` and generate the correct `#[error]` annotations for `thiserror`.
* [] Add an optional arg to `finality`: `splitable` determines if a this is the root error that shall be handled, and hence should be splitable into two enums `Fatal` and `Jfyi` errors, with `trai FatalitySplit` and `fn resolve() -> Result<Jfyi, Fatal> {..}`.
