// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use xcm::latest::prelude::*;

/// An identifier for a channel.
pub type Channel = u32;

/// Generates channel identifier for `origin/(network, destination)` combination to ensure using separated lanes.
pub fn channel_from_params(
	origin_ref: Option<&MultiLocation>,
	network: &NetworkId,
	destination: &InteriorMultiLocation,
) -> Channel {
	use parity_scale_codec::{Decode, Encode};
	// Hash identifies the lane on the exporter which we use. We use the pairwise
	// combination of the origin and (network, destination) to ensure origin/(network, destination) pairs will
	// generally have their own lanes.
	let hash = (origin_ref, (network, destination)).using_encoded(sp_io::hashing::blake2_256);
	let channel = u32::decode(&mut hash.as_ref()).unwrap_or(0);
	channel
}

/// Utility for delivering a message to a system under a different (non-local) consensus with a
/// spoofed origin. This essentially defines the behaviour of the `ExportMessage` XCM instruction.
///
/// This is quite different to `SendXcm`; `SendXcm` assumes that the local side's location will be
/// preserved to be represented as the value of the Origin register in the messages execution.
///
/// This trait on the other hand assumes that we do not necessarily want the Origin register to
/// contain the local (i.e. the caller chain's) location, since it will generally be exporting a
/// message on behalf of another consensus system. Therefore in addition to the message, the
/// destination must be given in two parts: the network and the interior location within it.
///
/// We also require the caller to state exactly what location they purport to be representing. The
/// destination must accept the local location to represent that location or the operation will
/// fail.
pub trait ExportXcm {
	/// Intermediate value which connects the two phases of the export operation.
	type Ticket;

	/// Check whether the given `message` is deliverable to the given `destination` on `network`,
	/// spoofing its source as `universal_source` and if so determine the cost which will be paid by
	/// this chain to do so, returning a `Ticket` token which can be used to enact delivery.
	///
	/// The `channel` to be used on the `network`'s export mechanism (bridge, probably) must also
	/// be provided.
	///
	/// The `destination` and `message` must be `Some` (or else an error will be returned) and they
	/// may only be consumed if the `Err` is not `NotApplicable`.
	///
	/// If it is not a destination which can be reached with this type but possibly could by others,
	/// then this *MUST* return `NotApplicable`. Any other error will cause the tuple
	/// implementation (used to compose routing systems from different delivery agents) to exit
	/// early without trying alternative means of delivery.
	fn validate(
		network: NetworkId,
		channel: Channel,
		universal_source: &mut Option<InteriorMultiLocation>,
		destination: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket>;

	/// Actually carry out the delivery operation for a previously validated message sending.
	///
	/// The implementation should do everything possible to ensure that this function is infallible
	/// if called immediately after `validate`. Returning an error here would result in a price
	/// paid without the service being delivered.
	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ExportXcm for Tuple {
	for_tuples! { type Ticket = (#( Option<Tuple::Ticket> ),* ); }

	fn validate(
		network: NetworkId,
		channel: Channel,
		universal_source: &mut Option<InteriorMultiLocation>,
		destination: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		let mut maybe_cost: Option<MultiAssets> = None;
		let one_ticket: Self::Ticket = (for_tuples! { #(
			if maybe_cost.is_some() {
				None
			} else {
				match Tuple::validate(network, channel, universal_source, destination, message) {
					Err(SendError::NotApplicable) => None,
					Err(e) => { return Err(e) },
					Ok((v, c)) => {
						maybe_cost = Some(c);
						Some(v)
					},
				}
			}
		),* });
		if let Some(cost) = maybe_cost {
			Ok((one_ticket, cost))
		} else {
			Err(SendError::NotApplicable)
		}
	}

	fn deliver(mut one_ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		for_tuples!( #(
			if let Some(validated) = one_ticket.Tuple.take() {
				return Tuple::deliver(validated);
			}
		)* );
		Err(SendError::Unroutable)
	}
}

/// Convenience function for using a `SendXcm` implementation. Just interprets the `dest` and wraps
/// both in `Some` before passing them as as mutable references into `T::send_xcm`.
pub fn validate_export<T: ExportXcm>(
	network: NetworkId,
	channel: Channel,
	universal_source: InteriorMultiLocation,
	dest: InteriorMultiLocation,
	msg: Xcm<()>,
) -> SendResult<T::Ticket> {
	T::validate(network, channel, &mut Some(universal_source), &mut Some(dest), &mut Some(msg))
}

/// Convenience function for using a `SendXcm` implementation. Just interprets the `dest` and wraps
/// both in `Some` before passing them as as mutable references into `T::send_xcm`.
///
/// Returns either `Ok` with the price of the delivery, or `Err` with the reason why the message
/// could not be sent.
///
/// Generally you'll want to validate and get the price first to ensure that the sender can pay it
/// before actually doing the delivery.
pub fn export_xcm<T: ExportXcm>(
	network: NetworkId,
	channel: Channel,
	universal_source: InteriorMultiLocation,
	dest: InteriorMultiLocation,
	msg: Xcm<()>,
) -> Result<(XcmHash, MultiAssets), SendError> {
	let (ticket, price) = T::validate(
		network,
		channel,
		&mut Some(universal_source),
		&mut Some(dest),
		&mut Some(msg),
	)?;
	let hash = T::deliver(ticket)?;
	Ok((hash, price))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashSet;

	#[test]
	fn channel_from_params_works() {
		let params: Vec<(Option<MultiLocation>, &NetworkId, &InteriorMultiLocation)> = vec![
			// different interior
			(None, &ByGenesis([0; 32]), &X1(Parachain(1234))),
			(None, &ByGenesis([0; 32]), &X1(Parachain(1235))),
			// different networkId
			(None, &ByGenesis([0; 32]), &X1(Parachain(1236))),
			(None, &ByGenesis([1; 32]), &X1(Parachain(1236))),
			// different origin
			(None, &ByGenesis([0; 32]), &X1(Parachain(1237))),
			(Some(MultiLocation::here()), &ByGenesis([0; 32]), &X1(Parachain(1237))),
			(Some(MultiLocation::parent()), &ByGenesis([0; 32]), &X1(Parachain(1237))),
		];

		// check unique
		let channels: HashSet<Channel> =
			HashSet::from_iter(params.iter().map(|(origin, network, interior)| {
				channel_from_params(origin.as_ref(), network, interior)
			}));
		assert_eq!(params.len(), channels.len());

		// check not random
		assert_eq!(
			channel_from_params(None, &ByGenesis([0; 32]), &X1(Parachain(1234))),
			channel_from_params(None, &ByGenesis([0; 32]), &X1(Parachain(1234)))
		);
		assert_eq!(channel_from_params(Some(&Parachain(1).into()), &Polkadot, &Here), 470110423);
	}
}
