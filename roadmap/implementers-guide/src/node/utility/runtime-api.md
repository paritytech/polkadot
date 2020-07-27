# Runtime API

The Runtime API subsystem is responsible for providing a single point of access to runtime state data via a set of pre-determined queries. This prevents shared ownership of a blockchain client resource by providing

## Protocol

Input: [`RuntimeApiMessage`](../../types/overseer-protocol.md#runtime-api-message)

Output: None

## Functionality

On receipt of `RuntimeApiMessage::Request(relay_parent, request)`, answer the request using the post-state of the relay_parent provided and provide the response to the side-channel embedded within the request.

> TODO Do some caching. The underlying rocksdb already has a cache of trie nodes so duplicate requests are unlikely to hit disk. Not required for functionality.

## Jobs

> TODO Don't limit requests based on parent hash, but limit caching. No caching should be done for any requests on relay_parents that are not active based on `ActiveLeavesUpdate` messages. Maybe with some leeway for things that have just been stopped.
