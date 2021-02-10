# Misbehavior Reporting

This API is the boundary layer between the runtime modules, which understand only the current block
in a fork-free model of the blockchain, and the node subsystems, which handle forking and finality.
As such, there are two types of misbehavior which we are concerned with: local and remote.

## Local Disputes

Local disputes are detected by the runtime and communicated to the node by an as-yet-unspecified
mechanism.

> TODO: figure out that mechanism.
>
> A pull mechanism which lets the node request new disputes from the runtime is most similar to
> existing runtime APIs; it might look like:
>
> ```rust
> fn get_new_disputes(at: Block) -> Vec<Dispute>
> ```
>
> However, that design feels suboptimal. There is probably a push design possible which would allow
> the runtime to directly notify the node when it discovers cause for a dispute, but it's not obvious
> as of now what such a design would look like.

Local disputes are forwarded as [`ProvisionerMessage`](../types/overseer-protocol.md#provisioner-message)`::Dispute`.

## Remote Disputes

When a dispute has occurred on another fork, we need to transplant that dispute to every other fork.
This preserves the property that a misbehaving validator is slashed in every possible future; it can't
escape punishment just because its misbehavior was detected and prevented before taking effect.

### Concluded Remote Dispute

A concluded remote dispute has had the dispute process run to completion: it simply rolls up
all [attestations](../types/backing.md#signed-statement-type) for and against the block.
It can be resolved in a single transaction; it is an open-and-shut case of a quorum of validators
disagreeing with each other.

```rust
struct ConcludedRemoteDispute {
    attestations: Vec<SignedStatement>,
}

fn concluded_remote_distpute(dispute: ConcludedRemoteDispute)
```
