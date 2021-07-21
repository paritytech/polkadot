# Chain API

The Chain API subsystem is responsible for providing a single point of access to chain state data via a set of pre-determined queries.

## Protocol

Input: [`ChainApiMessage`](../../types/overseer-protocol.md#chain-api-message)

Output: None

## Functionality

On receipt of `ChainApiMessage`, answer the request and provide the response to the side-channel embedded within the request.

Currently, the following requests are supported:
* Block hash to number
* Block hash to header
* Block weight
* Finalized block number to hash
* Last finalized block number
* Ancestors
