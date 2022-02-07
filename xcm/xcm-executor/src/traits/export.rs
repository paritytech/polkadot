// Copyright 2020 Parity Technologies (UK) Ltd.
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

pub trait ExportXcm {
	type OptionTicket: Unwrappable;

	/// Check whether the given `_message` is deliverable to the given `_destination` and if so
	/// determine the cost which will be paid by this chain to do so, returning a `Validated` token
	/// which can be used to enact delivery.
	///
	/// The `destination` and `message` must be `Some` (or else an error will be returned) and they
	/// may only be consumed if the `Err` is not `CannotReachDestination`.
	///
	/// If it is not a destination which can be reached with this type but possibly could by others,
	/// then this *MUST* return `CannotReachDestination`. Any other error will cause the tuple
	/// implementation to exit early without trying other type fields.
	fn validate(
		network: NetworkId,
		channel: u32,
		destination: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<<Self::OptionTicket as Unwrappable>::Inner>;

	/// Actually carry out the delivery operation for a previously validated message sending.
	///
	/// The implementation should do everything possible to ensure that this function is infallible
	/// if called immediately after `validate`. Returning an error here would result in a price
	/// paid without the service being delivered.
	fn deliver(ticket: <Self::OptionTicket as Unwrappable>::Inner) -> Result<(), SendError>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ExportXcm for Tuple {
	type OptionTicket = Option<(for_tuples! { #( Tuple::OptionTicket ),* })>;

	fn validate(
		network: NetworkId,
		channel: u32,
		destination: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<(for_tuples! { #( Tuple::OptionTicket ),* })> {
		let mut maybe_cost: Option<MultiAssets> = None;
		let one_ticket: (for_tuples! { #( Tuple::OptionTicket ),* }) = (for_tuples! { #(
			if maybe_cost.is_some() {
				<Tuple::OptionTicket as Unwrappable>::none()
			} else {
				match Tuple::validate(network, channel, destination, message) {
					Err(SendError::CannotReachDestination) => <Tuple::OptionTicket as Unwrappable>::none(),
					Err(e) => { return Err(e) },
					Ok((v, c)) => {
						maybe_cost = Some(c);
						<Tuple::OptionTicket as Unwrappable>::some(v)
					},
				}
			}
		),* });
		if let Some(cost) = maybe_cost {
			Ok((one_ticket, cost))
		} else {
			Err(SendError::CannotReachDestination)
		}
	}

	fn deliver(one_ticket: <Self::OptionTicket as Unwrappable>::Inner) -> Result<(), SendError> {
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
	channel: u32,
	dest: InteriorMultiLocation,
	msg: Xcm<()>,
) -> SendResult<<T::OptionTicket as Unwrappable>::Inner> {
	T::validate(network, channel, &mut Some(dest), &mut Some(msg))
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
	channel: u32,
	dest: InteriorMultiLocation,
	msg: Xcm<()>,
) -> Result<MultiAssets, SendError> {
	let (ticket, price) = T::validate(network, channel, &mut Some(dest), &mut Some(msg))?;
	T::deliver(ticket)?;
	Ok(price)
}
