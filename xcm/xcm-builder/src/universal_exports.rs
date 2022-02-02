// Copyright 2021 Parity Technologies (UK) Ltd.
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

use sp_std::{prelude::*, marker::PhantomData, convert::TryInto};
use parity_scale_codec::{Encode, Decode};
use frame_support::{traits::Get, ensure};
use xcm::prelude::*;
use xcm_executor::traits::ExportXcm;

fn ensure_is_remote(
	universal_local: impl Into<InteriorMultiLocation>,
	dest: impl Into<MultiLocation>,
) -> Result<(NetworkId, InteriorMultiLocation, NetworkId, InteriorMultiLocation), MultiLocation> {
	let dest = dest.into();
	let universal_local = universal_local.into();
	let (local_net, local_loc) = match universal_local.clone().split_first() {
		(location, Some(GlobalConsensus(network))) => (network, location),
		_ => return Err(dest),
	};
	let universal_destination: InteriorMultiLocation = universal_local
		.into_location()
		.appended_with(dest.clone())
		.map_err(|x| x.1)?
		.try_into()?;
	let (remote_dest, remote_net) = match universal_destination.split_first() {
		(d, Some(GlobalConsensus(n))) if n != local_net => (d, n),
		_ => return Err(dest),
	};
	Ok((remote_net, remote_dest, local_net, local_loc))
}

/// Implementation of `SendXcm` which uses the given `ExportXcm` impl in order to forward the
/// message over a bridge.
///
/// The actual message forwarded over the bridge is prepended with `UniversalOrigin` and
/// `DescendOrigin` in order to ensure that the message is executed with this Origin.
///
/// No effort is made to charge for any bridge fees, so this can only be used when it is known
/// that the message sending cannot be abused in any way.
///
/// This is only useful when the local chain has bridging capabilities.
pub struct LocalUnpaidExporter<Exporter, Ancestry>(PhantomData<(Exporter, Ancestry)>);
impl<Exporter: ExportXcm, Ancestry: Get<InteriorMultiLocation>> SendXcm for LocalUnpaidExporter<Exporter, Ancestry> {
	fn send_xcm(dest: impl Into<MultiLocation>, xcm: Xcm<()>) -> SendResult {
		let devolved = match ensure_is_remote(Ancestry::get(), dest) {
			Ok(x) => x,
			Err(dest) => return Err(SendError::CannotReachDestination(dest, xcm)),
		};
		let (network, destination, local_network, local_location) = devolved;

		let mut message: Xcm<()> = vec![UniversalOrigin(GlobalConsensus(local_network))].into();
		if local_location != Here {
			message.inner_mut().push(DescendOrigin(local_location));
		}
		message.inner_mut().extend(xcm.into_iter());
		Exporter::export_xcm(network, 0, destination, message)
	}
}

// TODO: `SendXcm` should include a price/weight calculator for calling prior to `send_xcm`.

pub trait ExporterFor {
	/// Return the locally-routable bridge (if any) capable of forwarding `message` to the
	/// `remote_location` on the remote `network`, together with the payment which is required.
	///
	/// The payment is specified from the context of the bridge, not the local chain.
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
impl<T: Get<&'static [(NetworkId, MultiLocation, Option<MultiAsset>)]>> ExporterFor for NetworkExportTable<T> {
	fn exporter_for(
		network: &NetworkId,
		_: &InteriorMultiLocation,
		_: &Xcm<()>,
	) -> Option<(MultiLocation, Option<MultiAsset>)> {
		T::get().iter().find(|(ref j, ..)| j == network).map(|(_, l, p)| (l.clone(), p.clone()))
	}
}

/// Implementation of `SendXcm` which wraps the message inside an `ExportMessage` instruction
/// and sends it to a destination known to be able to handle it.
///
/// The actual message send to the bridge for forwarding is prepended with `UniversalOrigin`
/// and `DescendOrigin` in order to ensure that the message is executed with our Origin.
///
/// No effort is made to make payment to the bridge for its services, so the bridge location
/// must have been configured with a barrier rule allowing unpaid execution for this message
/// coming from our origin.
///
/// This is only useful if we have special dispensation by the remote bridges to have the
/// `ExportMessage` instruction executed without payment.
pub struct UnpaidRemoteExporter<
	Bridges,
	Router,
	Ancestry,
>(PhantomData<(Bridges, Router, Ancestry)>);
impl<
	Bridges: ExporterFor,
	Router: SendXcm,
	Ancestry: Get<InteriorMultiLocation>,
> SendXcm for UnpaidRemoteExporter<Bridges, Router, Ancestry> {
	fn send_xcm(dest: impl Into<MultiLocation>, xcm: Xcm<()>) -> SendResult {
		let dest = dest.into();

		// TODO: proper matching so we can be sure that it's the only viable send_xcm before we
		// attempt and thus can acceptably consume dest & xcm.
		let err = SendError::CannotReachDestination(dest.clone(), xcm.clone());

		let devolved = ensure_is_remote(Ancestry::get(), dest).map_err(|_| err.clone())?;
		let (remote_network, remote_location, local_network, local_location) = devolved;

		// Prepend the desired message with instructions which effectively rewrite the origin.
		//
		// This only works because the remote chain empowers the bridge
		// to speak for the local network.
		let mut inner_xcm: Xcm<()> = vec![
			UniversalOrigin(GlobalConsensus(local_network)),
			DescendOrigin(local_location),
		].into();
		inner_xcm.inner_mut().extend(xcm.into_iter());

		let (bridge, payment) = Bridges::exporter_for(&remote_network, &remote_location, &inner_xcm)
			.ok_or(err.clone())?;
		ensure!(payment.is_none(), err);

		// We then send a normal message to the bridge asking it to export the prepended
		// message to the remote chain. This will only work if the bridge will do the message
		// export for free. Common-good chains will typically be afforded this.
		let message = Xcm(vec![
			ExportMessage {
				network: remote_network,
				destination: remote_location,
				xcm: inner_xcm,
			},
		]);
		Router::send_xcm(bridge, message)
	}
}

/// Implementation of `SendXcm` which wraps the message inside an `ExportMessage` instruction
/// and sends it to a destination known to be able to handle it.
///
/// The actual message send to the bridge for forwarding is prepended with `UniversalOrigin`
/// and `DescendOrigin` in order to ensure that the message is executed with this Origin.
///
/// The `ExportMessage` instruction on the bridge is paid for from the local chain's sovereign
/// account on the bridge. The amount paid is determined through the `ExporterFor` trait.
pub struct SovereignPaidRemoteExporter<
	Bridges,
	Router,
	Ancestry,
>(PhantomData<(Bridges, Router, Ancestry)>);
impl<
	Bridges: ExporterFor,
	Router: SendXcm,
	Ancestry: Get<InteriorMultiLocation>,
> SendXcm for SovereignPaidRemoteExporter<Bridges, Router, Ancestry> {
	fn send_xcm(dest: impl Into<MultiLocation>, xcm: Xcm<()>) -> SendResult {
		let dest = dest.into();

		// TODO: proper matching so we can be sure that it's the only viable send_xcm before we
		// attempt and thus can acceptably consume dest & xcm.
		let err = SendError::CannotReachDestination(dest.clone(), xcm.clone());

		let devolved = ensure_is_remote(Ancestry::get(), dest).map_err(|_| err.clone())?;
		let (remote_network, remote_location, local_network, local_location) = devolved;

		// Prepend the desired message with instructions which effectively rewrite the origin.
		//
		// This only works because the remote chain empowers the bridge
		// to speak for the local network.
		let mut inner_xcm: Xcm<()> = vec![
			UniversalOrigin(GlobalConsensus(local_network)),
			DescendOrigin(local_location),
		].into();
		inner_xcm.inner_mut().extend(xcm.into_iter());

		let (bridge, maybe_payment) = Bridges::exporter_for(&remote_network, &remote_location, &inner_xcm)
			.ok_or(err.clone())?;
		let local_from_bridge = MultiLocation::from(Ancestry::get()).inverted(&bridge)
			.map_err(|_| err.clone())?;

		// We then send a normal message to the bridge asking it to export the prepended
		// message to the remote chain. This will only work if the bridge will do the message
		// export for free. Common-good chains will typically be afforded this.
		let export_instruction = ExportMessage {
			network: remote_network,
			destination: remote_location,
			xcm: inner_xcm,
		};

		let message = Xcm(if let Some(payment) = maybe_payment {
			vec![
				WithdrawAsset(payment.clone().into()),
				BuyExecution { fees: payment, weight_limit: Unlimited },
				export_instruction,
				RefundSurplus,
				DepositAsset { assets: All.into(), beneficiary: local_from_bridge },
			]
		} else {
			vec![export_instruction]
		});
		Router::send_xcm(bridge, message)
	}
}

pub trait DispatchBlob {
	/// Dispatches an incoming blob and returns the unexpectable weight consumed by the dispatch.
	fn dispatch_blob(blob: Vec<u8>) -> Result<u64, DispatchBlobError>;
}

pub trait HaulBlob {
	/// Sends a blob over some point-to-point link. This will generally be implemented by a bridge.
	fn haul_blob(blob: Vec<u8>);
}

#[derive(Clone, Encode, Decode)]
pub struct BridgeMessage {
	/// The message destination as a *Universal Location*. This means it begins with a
	/// `GlobalConsensus` junction describing the network under which global consensus happens.
	/// If this does not match our global consensus then it's a fatal error.
	universal_dest: VersionedInteriorMultiLocation,
	message: VersionedXcm<()>,
}

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
impl<Router: SendXcm, OurPlace: Get<InteriorMultiLocation>> DispatchBlob for BridgeBlobDispatcher<Router, OurPlace> {
	fn dispatch_blob(blob: Vec<u8>) -> Result<u64, DispatchBlobError> {
		let our_universal = OurPlace::get();
		let our_global = our_universal.global_consensus()
			.map_err(|()| DispatchBlobError::Unbridgable)?;
		let BridgeMessage { universal_dest, message } = Decode::decode(&mut &blob[..])
			.map_err(|_| DispatchBlobError::InvalidEncoding)?;
		let universal_dest: InteriorMultiLocation = universal_dest.try_into()
			.map_err(|_| DispatchBlobError::UnsupportedLocationVersion)?;
		// `universal_dest` is the desired destination within the universe: first we need to check
		// we're in the right global consensus.
		let intended_global = universal_dest.global_consensus()
			.map_err(|()| DispatchBlobError::NonUniversalDestination)?;
		ensure!(intended_global == our_global, DispatchBlobError::WrongGlobal);
		let dest = universal_dest.relative_to(&our_universal);
		let message: Xcm<()> = message.try_into()
			.map_err(|_| DispatchBlobError::UnsupportedXcmVersion)?;
		Router::send_xcm(dest, message).map_err(|_| DispatchBlobError::RoutingError)?;
		// TODO: Proper weight.
		Ok(1)
	}
}

pub struct HaulBlobExporter<Bridge, BridgedNetwork>(PhantomData<(Bridge, BridgedNetwork)>);
impl<Bridge: HaulBlob, BridgedNetwork: Get<NetworkId>> ExportXcm for HaulBlobExporter<Bridge, BridgedNetwork> {
	fn export_xcm(
		network: NetworkId,
		_channel: u32,
		destination: impl Into<InteriorMultiLocation>,
		message: Xcm<()>,
	) -> Result<(), SendError> {
		let destination = destination.into();
		let bridged_network = BridgedNetwork::get();
		ensure!(&network == &bridged_network, SendError::CannotReachNetwork(network, destination, message));
		// We don't/can't use the `channel` for this adapter.
		let universal_dest = match destination.pushed_front_with(GlobalConsensus(bridged_network)) {
			Ok(d) => d.into(),
			Err((destination, _)) => return Err(SendError::CannotReachNetwork(network, destination, message)),
		};
		let message = VersionedXcm::from(message);
		Bridge::haul_blob(BridgeMessage { universal_dest, message }.encode());
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::cell::RefCell;
	use frame_support::parameter_types;

	std::thread_local! {
		static BRIDGE_TRAFFIC: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
	}

	struct TestBridge<D>(PhantomData<D>);
	impl<D: DispatchBlob> TestBridge<D> {
		fn service() -> u64 {
			BRIDGE_TRAFFIC.with(|t| t.borrow_mut().drain(..).map(|b| D::dispatch_blob(b).unwrap_or(0)).sum())
		}
	}
	impl<D: DispatchBlob> HaulBlob for TestBridge<D> {
		fn haul_blob(blob: Vec<u8>) {
			BRIDGE_TRAFFIC.with(|t| t.borrow_mut().push(blob));
		}
	}

	std::thread_local! {
		static REMOTE_INCOMING_XCM: RefCell<Vec<(MultiLocation, Xcm<()>)>> = RefCell::new(Vec::new());
	}
	struct TestRemoteIncomingRouter;
	impl SendXcm for TestRemoteIncomingRouter {
		fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
			let destination = destination.into();
			REMOTE_INCOMING_XCM.with(|r| r.borrow_mut().push((destination, message)));
			Ok(())
		}
	}

	fn take_received_remote_messages() -> Vec<(MultiLocation, Xcm<()>)> {
		REMOTE_INCOMING_XCM.with(|r| r.replace(vec![]))
	}

	parameter_types! {
		pub Local: NetworkId = ByUri(b"local".to_vec());
		pub UniversalLocation: Junctions = X1(GlobalConsensus(Local::get()));
		pub Remote1: NetworkId = ByUri(b"remote1".to_vec());
		pub RemoteUniversalLocation: Junctions = X1(GlobalConsensus(Remote1::get()));
	}

	type TheBridge = TestBridge<
		BridgeBlobDispatcher<TestRemoteIncomingRouter, RemoteUniversalLocation>
	>;
	type Router = LocalUnpaidExporter< HaulBlobExporter<TheBridge, Remote1>, UniversalLocation >;

	#[test]
	fn bridge_works() {
		let msg = Xcm(vec![Trap(1)]);
		assert_eq!(Router::send_xcm((Parent, ByUri(b"remote1".to_vec())), msg), Ok(()));
		assert_eq!(TheBridge::service(), 1);
		assert_eq!(take_received_remote_messages(), vec![(Here.into(), Xcm(vec![
			UniversalOrigin(Local::get().into()),
			Trap(1),
		]))]);
	}

	#[test]
	fn ensure_is_remote_works() {
		// A Kusama parachain is remote from the Polkadot Relay.
		let x = ensure_is_remote(Polkadot, (Parent, Kusama, Parachain(1000)));
		assert_eq!(x, Ok((Kusama, Parachain(1000).into(), Polkadot, Here)));

		// Polkadot Relay is remote from a Kusama parachain.
		let x = ensure_is_remote((Kusama, Parachain(1000)), (Parent, Parent, Polkadot));
		assert_eq!(x, Ok((Polkadot, Here, Kusama, Parachain(1000).into())));

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