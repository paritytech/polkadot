use parity_scale_codec::{Decode, Encode};

use polkadot_node_primitives::{DisputeMessageV2, UncheckedDisputeMessageV2};

use super::{IsRequest, Protocol};

/// A dispute request.
///
/// Contains an invalid vote a valid one for a particular candidate in a given session.
#[derive(Clone, Encode, Decode, Debug)]
pub struct DisputeRequest(pub UncheckedDisputeMessageV2);

impl From<DisputeMessageV2> for DisputeRequest {
	fn from(msg: DisputeMessageV2) -> Self {
		Self(msg.into())
	}
}

/// Possible responses to a `DisputeRequest`.
#[derive(Encode, Decode, Debug, PartialEq, Eq)]
pub enum DisputeResponse {
	/// Recipient successfully processed the dispute request.
	#[codec(index = 0)]
	Confirmed,
}

impl IsRequest for DisputeRequest {
	type Response = DisputeResponse;
	const PROTOCOL: Protocol = Protocol::DisputeSendingV2;
}
