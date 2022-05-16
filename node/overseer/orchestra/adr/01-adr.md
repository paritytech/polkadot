# Limit outgoing messages

## Status

Accepted + implemented.

## Context

Previously, there was no way to limit and hence reason about a subset of subsystems, and if they form a cycle. Limiting the outgoing message types is a first step to create respective graphs and use classic graph algorithms to detect those and leave it to the user to resolve these.

## Decision

Annotate the `#[orchestra]` inner `#[subsystem(..)]` annotation
with an aditional set of outgoing messages and enforce this via more fine grained trait bounds on the `Sender` and `<Context>::Sender` bounds.

## Consequences

* A graph will be spawn for every compilation under the `OUT_DIR` of the crate where `#[orchestra]` is specified.
* Each subsystem has a consuming message which is often referred to as generic `M` (no change on that, is as before), but now we have trait `AssociateOutgoing { type OutgoingMessages = ..; }` which defines an outgoing helper `enum` that is generated with an ident constructed as `${Subsystem}OutgoingMessages` where `${Subsystem}` is the subsystem identifier as used in the overseer declaration. `${Subsystem}OutgoingMessages` is used throughout everywhere to constrain the outgoing messages (commonly referred to as `OutgoingMessage` generic bounded by `${Subsystem}OutgoingMessages: From<OutgoingMessage>` or `::OutgoingMessages: From`. It's what allows the construction of the graph and compile time verification.
* `${Subsystem}SenderTrait` and `${Subsystem}ContextTrait` are accumulation traits or wrapper traits, that combine over all annotated M or `OutgoingMessages` from the overseer declaration or their respective outgoing types. It is usage convenience and assures consistency within a subsystem while also maintaining a single source of truth for which messages can be sent by a particular subsystem. Note that this is sidestepped for the test subsystem, which may consume `gen=AllMessages`, the global message wrapper type.
* `Job`-based subsystems, being on their way out, are patched, but they now are generic over the `Sender` type, leaking that type.
