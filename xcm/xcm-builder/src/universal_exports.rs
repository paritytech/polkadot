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

//! Traits and utilities to help with origin mutation and bridging.

use frame_support::{ensure, traits::Get};
use parity_scale_codec::{Decode, Encode};
use sp_std::{convert::TryInto, marker::PhantomData, prelude::*};
use xcm::prelude::*;
use xcm_executor::traits::{validate_export, ExportXcm};
use SendError::*;

fn ensure_is_remote(
	universal_local: impl Into<InteriorMultiLocation>,
	dest: impl Into<MultiLocation>,
) -> Result<(NetworkId, InteriorMultiLocation), MultiLocation> {
	let dest = dest.into();
	let universal_local = universal_local.into();
	let local_net = match universal_local.global_consensus() {
		Ok(x) => x,
		Err(_) => return Err(dest),
	};
	let universal_destination: InteriorMultiLocation = universal_local
		.into_location()
		.appended_with(dest)
		.map_err(|x| x.1)?
		.try_into()?;
	let (remote_dest, remote_net) = match universal_destination.split_first() {
		(d, Some(GlobalConsensus(n))) if n != local_net => (d, n),
		_ => return Err(dest),
	};
	Ok((remote_net, remote_dest))
}

/// Implementation of `SendXcm` which uses the given `ExportXcm` implementation in order to forward
/// the message over a bridge.
///
/// No effort is made to charge for any bridge fees, so this can only be used when it is known
/// that the message sending cannot be abused in any way.
///
/// This is only useful when the local chain has bridging capabilities.
pub struct UnpaidLocalExporter<Exporter, UniversalLocation>(
	PhantomData<(Exporter, UniversalLocation)>,
);
impl<Exporter: ExportXcm, UniversalLocation: Get<InteriorMultiLocation>> SendXcm
	for UnpaidLocalExporter<Exporter, UniversalLocation>
{
	type Ticket = Exporter::Ticket;

	fn validate(
		dest: &mut Option<MultiLocation>,
		xcm: &mut Option<Xcm<()>>,
	) -> SendResult<Exporter::Ticket> {
		let d = dest.take().ok_or(MissingArgument)?;
		let universal_source = UniversalLocation::get();
		let devolved = match ensure_is_remote(universal_source, d) {
			Ok(x) => x,
			Err(d) => {
				*dest = Some(d);
				return Err(NotApplicable)
			},
		};
		let (network, destination) = devolved;
		let xcm = xcm.take().ok_or(SendError::MissingArgument)?;
		validate_export::<Exporter>(network, 0, universal_source, destination, xcm)
	}

	fn deliver(ticket: Exporter::Ticket) -> Result<XcmHash, SendError> {
		Exporter::deliver(ticket)
	}
}

pub trait ExporterFor {
	/// Return the locally-routable bridge (if any) capable of forwarding `message` to the
	/// `remote_location` on the remote `network`, together with the payment which is required.
	///
	/// The payment is specified from the local context, not the bridge chain. This is the
	/// total amount to withdraw in to Holding and should cover both payment for the execution on
	/// the bridge chain as well as payment for the use of the `ExportMessage` instruction.
	fn exporter_for(
		network: &NetworkId,
		remote_location: &InteriorMultiLocation,
		message: &Xcm<()>,
	) -> Option<(MultiLocation, Option<MultiAsset>)>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ExporterFor for Tuple {
	fn exporter_for(
		network: &NetworkId,
		remote_location: &InteriorMultiLocation,
		message: &Xcm<()>,
	) -> Option<(MultiLocation, Option<MultiAsset>)> {
		for_tuples!( #(
			if let Some(r) = Tuple::exporter_for(network, remote_location, message) {
				return Some(r);
			}
		)* );
		None
	}
}

pub struct NetworkExportTable<T>(sp_std::marker::PhantomData<T>);
impl<T: Get<Vec<(NetworkId, MultiLocation, Option<MultiAsset>)>>> ExporterFor
	for NetworkExportTable<T>
{
	fn exporter_for(
		network: &NetworkId,
		_: &InteriorMultiLocation,
		_: &Xcm<()>,
	) -> Option<(MultiLocation, Option<MultiAsset>)> {
		T::get().into_iter().find(|(ref j, ..)| j == network).map(|(_, l, p)| (l, p))
	}
}

/// Implementation of `SendXcm` which wraps the message inside an `ExportMessage` instruction
/// and sends it to a destination known to be able to handle it.
///
/// No effort is made to make payment to the bridge for its services, so the bridge location
/// must have been configured with a barrier rule allowing unpaid execution for this message
/// coming from our origin.
///
/// This is only useful if we have special dispensation by the remote bridges to have the
/// `ExportMessage` instruction executed without payment.
pub struct UnpaidRemoteExporter<Bridges, Router, UniversalLocation>(
	PhantomData<(Bridges, Router, UniversalLocation)>,
);
impl<Bridges: ExporterFor, Router: SendXcm, UniversalLocation: Get<InteriorMultiLocation>> SendXcm
	for UnpaidRemoteExporter<Bridges, Router, UniversalLocation>
{
	type Ticket = Router::Ticket;

	fn validate(
		dest: &mut Option<MultiLocation>,
		xcm: &mut Option<Xcm<()>>,
	) -> SendResult<Router::Ticket> {
		let d = dest.ok_or(MissingArgument)?;
		let devolved = ensure_is_remote(UniversalLocation::get(), d).map_err(|_| NotApplicable)?;
		let (remote_network, remote_location) = devolved;
		let xcm = xcm.take().ok_or(MissingArgument)?;
		let (bridge, maybe_payment) =
			Bridges::exporter_for(&remote_network, &remote_location, &xcm).ok_or(NotApplicable)?;
		ensure!(maybe_payment.is_none(), Unroutable);

		// We then send a normal message to the bridge asking it to export the prepended
		// message to the remote chain. This will only work if the bridge will do the message
		// export for free. Common-good chains will typically be afforded this.
		let message = Xcm(vec![
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			ExportMessage { network: remote_network, destination: remote_location, xcm },
		]);
		validate_send::<Router>(bridge, message)
	}

	fn deliver(validation: Router::Ticket) -> Result<XcmHash, SendError> {
		Router::deliver(validation)
	}
}

/// Implementation of `SendXcm` which wraps the message inside an `ExportMessage` instruction
/// and sends it to a destination known to be able to handle it.
///
/// The `ExportMessage` instruction on the bridge is paid for from the local chain's sovereign
/// account on the bridge. The amount paid is determined through the `ExporterFor` trait.
pub struct SovereignPaidRemoteExporter<Bridges, Router, UniversalLocation>(
	PhantomData<(Bridges, Router, UniversalLocation)>,
);
impl<Bridges: ExporterFor, Router: SendXcm, UniversalLocation: Get<InteriorMultiLocation>> SendXcm
	for SovereignPaidRemoteExporter<Bridges, Router, UniversalLocation>
{
	type Ticket = Router::Ticket;

	fn validate(
		dest: &mut Option<MultiLocation>,
		xcm: &mut Option<Xcm<()>>,
	) -> SendResult<Router::Ticket> {
		let d = *dest.as_ref().ok_or(MissingArgument)?;
		let devolved = ensure_is_remote(UniversalLocation::get(), d).map_err(|_| NotApplicable)?;
		let (remote_network, remote_location) = devolved;

		let xcm = xcm.take().ok_or(MissingArgument)?;
		let (bridge, maybe_payment) =
			Bridges::exporter_for(&remote_network, &remote_location, &xcm).ok_or(NotApplicable)?;

		let local_from_bridge =
			UniversalLocation::get().invert_target(&bridge).map_err(|_| Unroutable)?;
		let export_instruction =
			ExportMessage { network: remote_network, destination: remote_location, xcm };

		let message = Xcm(if let Some(ref payment) = maybe_payment {
			let fees = payment
				.clone()
				.reanchored(&bridge, UniversalLocation::get())
				.map_err(|_| Unroutable)?;
			vec![
				WithdrawAsset(fees.clone().into()),
				BuyExecution { fees, weight_limit: Unlimited },
				export_instruction,
				RefundSurplus,
				DepositAsset { assets: All.into(), beneficiary: local_from_bridge },
			]
		} else {
			vec![export_instruction]
		});

		// We then send a normal message to the bridge asking it to export the prepended
		// message to the remote chain.
		let (v, mut cost) = validate_send::<Router>(bridge, message)?;
		if let Some(bridge_payment) = maybe_payment {
			cost.push(bridge_payment);
		}
		Ok((v, cost))
	}

	fn deliver(ticket: Router::Ticket) -> Result<XcmHash, SendError> {
		Router::deliver(ticket)
	}
}

pub trait DispatchBlob {
	/// Dispatches an incoming blob and returns the unexpectable weight consumed by the dispatch.
	fn dispatch_blob(blob: Vec<u8>) -> Result<(), DispatchBlobError>;
}

pub trait HaulBlob {
	/// Sends a blob over some point-to-point link. This will generally be implemented by a bridge.
	fn haul_blob(blob: Vec<u8>) -> Result<(), HaulBlobError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaulBlobError {
	/// Represents point-to-point link failure with a human-readable explanation of the specific issue is provided.
	Transport(&'static str),
}

impl From<HaulBlobError> for SendError {
	fn from(err: HaulBlobError) -> Self {
		match err {
			HaulBlobError::Transport(reason) => SendError::Transport(reason),
		}
	}
}

#[derive(Clone, Encode, Decode)]
pub struct BridgeMessage {
	/// The message destination as a *Universal Location*. This means it begins with a
	/// `GlobalConsensus` junction describing the network under which global consensus happens.
	/// If this does not match our global consensus then it's a fatal error.
	universal_dest: VersionedInteriorMultiLocation,
	message: VersionedXcm<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchBlobError {
	Unbridgable,
	InvalidEncoding,
	UnsupportedLocationVersion,
	UnsupportedXcmVersion,
	RoutingError,
	NonUniversalDestination,
	WrongGlobal,
}

pub struct BridgeBlobDispatcher<Router, OurPlace>(PhantomData<(Router, OurPlace)>);
impl<Router: SendXcm, OurPlace: Get<InteriorMultiLocation>> DispatchBlob
	for BridgeBlobDispatcher<Router, OurPlace>
{
	fn dispatch_blob(blob: Vec<u8>) -> Result<(), DispatchBlobError> {
		let our_universal = OurPlace::get();
		let our_global =
			our_universal.global_consensus().map_err(|()| DispatchBlobError::Unbridgable)?;
		let BridgeMessage { universal_dest, message } =
			Decode::decode(&mut &blob[..]).map_err(|_| DispatchBlobError::InvalidEncoding)?;
		let universal_dest: InteriorMultiLocation = universal_dest
			.try_into()
			.map_err(|_| DispatchBlobError::UnsupportedLocationVersion)?;
		// `universal_dest` is the desired destination within the universe: first we need to check
		// we're in the right global consensus.
		let intended_global = universal_dest
			.global_consensus()
			.map_err(|()| DispatchBlobError::NonUniversalDestination)?;
		ensure!(intended_global == our_global, DispatchBlobError::WrongGlobal);
		let dest = universal_dest.relative_to(&our_universal);
		let message: Xcm<()> =
			message.try_into().map_err(|_| DispatchBlobError::UnsupportedXcmVersion)?;
		let _ = send_xcm::<Router>(dest, message).map_err(|_| DispatchBlobError::RoutingError)?;
		Ok(())
	}
}

pub struct HaulBlobExporter<Bridge, BridgedNetwork, Price>(
	PhantomData<(Bridge, BridgedNetwork, Price)>,
);
impl<Bridge: HaulBlob, BridgedNetwork: Get<NetworkId>, Price: Get<MultiAssets>> ExportXcm
	for HaulBlobExporter<Bridge, BridgedNetwork, Price>
{
	type Ticket = (Vec<u8>, XcmHash);

	fn validate(
		network: NetworkId,
		_channel: u32,
		universal_source: &mut Option<InteriorMultiLocation>,
		destination: &mut Option<InteriorMultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> Result<((Vec<u8>, XcmHash), MultiAssets), SendError> {
		let bridged_network = BridgedNetwork::get();
		ensure!(&network == &bridged_network, SendError::NotApplicable);
		// We don't/can't use the `channel` for this adapter.
		let dest = destination.take().ok_or(SendError::MissingArgument)?;
		let universal_dest = match dest.pushed_front_with(GlobalConsensus(bridged_network)) {
			Ok(d) => d.into(),
			Err((dest, _)) => {
				*destination = Some(dest);
				return Err(SendError::NotApplicable)
			},
		};
		let (local_net, local_sub) = universal_source
			.take()
			.ok_or(SendError::MissingArgument)?
			.split_global()
			.map_err(|()| SendError::Unroutable)?;
		let mut inner: Xcm<()> = vec![UniversalOrigin(GlobalConsensus(local_net))].into();
		if local_sub != Here {
			inner.inner_mut().push(DescendOrigin(local_sub));
		}
		inner
			.inner_mut()
			.extend(message.take().ok_or(SendError::MissingArgument)?.into_iter());
		let message = VersionedXcm::from(inner);
		let hash = message.using_encoded(sp_io::hashing::blake2_256);
		let blob = BridgeMessage { universal_dest, message }.encode();
		Ok(((blob, hash), Price::get()))
	}

	fn deliver((blob, hash): (Vec<u8>, XcmHash)) -> Result<XcmHash, SendError> {
		Bridge::haul_blob(blob)?;
		Ok(hash)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ensure_is_remote_works() {
		// A Kusama parachain is remote from the Polkadot Relay.
		let x = ensure_is_remote(Polkadot, (Parent, Kusama, Parachain(1000)));
		assert_eq!(x, Ok((Kusama, Parachain(1000).into())));

		// Polkadot Relay is remote from a Kusama parachain.
		let x = ensure_is_remote((Kusama, Parachain(1000)), (Parent, Parent, Polkadot));
		assert_eq!(x, Ok((Polkadot, Here)));

		// Our own parachain is local.
		let x = ensure_is_remote(Polkadot, Parachain(1000));
		assert_eq!(x, Err(Parachain(1000).into()));

		// Polkadot's parachain is not remote if we are Polkadot.
		let x = ensure_is_remote(Polkadot, (Parent, Polkadot, Parachain(1000)));
		assert_eq!(x, Err((Parent, Polkadot, Parachain(1000)).into()));

		// If we don't have a consensus ancestor, then we cannot determine remoteness.
		let x = ensure_is_remote((), (Parent, Polkadot, Parachain(1000)));
		assert_eq!(x, Err((Parent, Polkadot, Parachain(1000)).into()));
	}
}
