# Call Dispatch Module

The call dispatch module has a single internal (only callable by other runtime modules) entry point
for dispatching encoded calls (`pallet_bridge_call_dispatch::Module::dispatch`). Every dispatch
(successful or not) emits a corresponding module event. The module doesn't have any call-related
requirements - they may come from the bridged chain over some message lane, or they may be crafted
locally. But in this document we'll mostly talk about this module in the context of bridges.

Every message that is being dispatched has three main characteristics:
- `bridge` is the 4-bytes identifier of the bridge where this message comes from. This may be the
  identifier of the bridged chain (like `b"rlto"` for messages coming from `Rialto`), or the
  identifier of the bridge itself (`b"rimi"` for `Rialto` <-> `Millau` bridge);
- `id` is the unique id of the message within the given bridge. For messages coming from the
  [message lane module](../message-lane/README.md), it may worth to use a tuple
  `(LaneId, MessageNonce)` to identify a message;
- `message` is the `pallet_bridge_call_dispatch::MessagePayload` structure. The `call` field is set
  to the (potentially) encoded `Call` of this chain.

The easiest way to understand what is happening when a `Call` is being dispatched, is to look at the
module events set:

- `MessageRejected` event is emitted if a message has been rejected even before it has reached the
  module. Dispatch then is called just to reflect the fact that message has been received, but we
  have failed to pre-process it (e.g. because we have failed to decode `MessagePayload` structure
  from the proof);
- `MessageVersionSpecMismatch` event is emitted if current runtime specification version differs
  from the version that has been used to encode the `Call`. The message payload has the
  `spec_version`, that is filled by the message submitter. If this value differs from the current
  runtime version, dispatch mechanism rejects to dispatch the message. Without this check, we may
  decode the wrong `Call` for example if method arguments were changed;
- `MessageCallDecodeFailed` event is emitted if we have failed to decode `Call` from the payload.
  This may happen if the submitter has provided incorrect value in the `call` field, or if source
  chain storage has been corrupted. The `Call` is decoded after `spec_version` check, so we'll never
  try to decode `Call` from other runtime version;
- `MessageSignatureMismatch` event is emitted if submitter has chose to dispatch message using
  specified this chain account (`pallet_bridge_call_dispatch::CallOrigin::TargetAccount` origin),
  but he has failed to prove that he owns the private key for this account;
- `MessageCallRejected` event is emitted if the module has been deployed with some call filter and
  this filter has rejected the `Call`. In your bridge you may choose to reject all messages except
  e.g. balance transfer calls;
- `MessageWeightMismatch` event is emitted if the message submitter has specified invalid `Call`
  dispatch weight in the `weight` field of the message payload. The value of this field is compared
  to the pre-dispatch weight of the decoded `Call`. If it is less than the actual pre-dispatch
  weight, the dispatch is rejected. Keep in mind, that even if post-dispatch weight will be less
  than specified, the submitter still have to declare (and pay for) the maximal possible weight
  (that is the pre-dispatch weight);
- `MessageDispatched` event is emitted if the message has passed all checks and we have actually
  dispatched it. The dispatch may still fail, though - that's why we are including the dispatch
  result in the event payload.

When we talk about module in context of bridges, these events are helping in following cases:

1. when the message submitter has access to the state of both chains and wants to monitor what has
   happened with his message. Then he could use the message id (that he gets from the
   [message lane module events](../message-lane/README.md#General-Information)) to filter events of
   call dispatch module at the target chain and actually see what has happened with his message;

1. when the message submitter only has access to the source chain state (for example, when sender is
   the runtime module at the source chain). In this case, your bridge may have additional mechanism
   to deliver dispatch proofs (which are storage proof of module events) back to the source chain,
   thus allowing the submitter to see what has happened with his messages.
