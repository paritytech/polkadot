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

//! Tests specific to the bridging primitives

use crate::{mock::*, universal_exports::*};
use frame_support::{parameter_types, traits::Get};
use std::{cell::RefCell, marker::PhantomData};
use xcm::prelude::*;
use xcm_executor::{
	traits::{export_xcm, validate_export},
	XcmExecutor,
};
use SendError::*;

mod local_para_para;
mod local_relay_relay;
mod paid_remote_relay_relay;
mod remote_para_para;
mod remote_para_para_via_relay;
mod remote_relay_relay;

parameter_types! {
	pub Local: NetworkId = ByGenesis([0; 32]);
	pub Remote: NetworkId = ByGenesis([1; 32]);
	pub Price: MultiAssets = MultiAssets::from((Here, 100));
}

std::thread_local! {
	static BRIDGE_TRAFFIC: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
}

struct TestBridge<D>(PhantomData<D>);
impl<D: DispatchBlob> TestBridge<D> {
	fn service() -> u64 {
		BRIDGE_TRAFFIC
			.with(|t| t.borrow_mut().drain(..).map(|b| D::dispatch_blob(b).map_or(0, |()| 1)).sum())
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
	type OptionTicket = Option<(MultiLocation, Xcm<()>)>;
	fn validate(
		dest: &mut Option<MultiLocation>,
		msg: &mut Option<Xcm<()>>,
	) -> SendResult<(MultiLocation, Xcm<()>)> {
		let pair = (dest.take().unwrap(), msg.take().unwrap());
		Ok((pair, MultiAssets::new()))
	}
	fn deliver(pair: (MultiLocation, Xcm<()>)) -> Result<(), SendError> {
		REMOTE_INCOMING_XCM.with(|q| q.borrow_mut().push(pair));
		Ok(())
	}
}

fn take_received_remote_messages() -> Vec<(MultiLocation, Xcm<()>)> {
	REMOTE_INCOMING_XCM.with(|r| r.replace(vec![]))
}

/// This is a dummy router which accepts messages destined for `Remote` from `Local`
/// and then executes them for free in a context simulated to be like that of our `Remote`.
struct UnpaidExecutingRouter<Local, Remote, RemoteExporter>(
	PhantomData<(Local, Remote, RemoteExporter)>,
);

fn price<RemoteExporter: ExportXcm>(
	n: NetworkId,
	c: u32,
	d: &InteriorMultiLocation,
	m: &Xcm<()>,
) -> Result<MultiAssets, SendError> {
	Ok(validate_export::<RemoteExporter>(n, c, d.clone(), m.clone())?.1)
}

fn deliver<RemoteExporter: ExportXcm>(
	n: NetworkId,
	c: u32,
	d: InteriorMultiLocation,
	m: Xcm<()>,
) -> Result<(), SendError> {
	export_xcm::<RemoteExporter>(n, c, d, m)?;
	Ok(())
}

impl<Local: Get<Junctions>, Remote: Get<Junctions>, RemoteExporter: ExportXcm> SendXcm
	for UnpaidExecutingRouter<Local, Remote, RemoteExporter>
{
	type OptionTicket = Option<Xcm<()>>;

	fn validate(
		destination: &mut Option<MultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Xcm<()>> {
		let expect_dest = Remote::get().relative_to(&Local::get());
		if destination.as_ref().ok_or(MissingArgument)? != &expect_dest {
			return Err(NotApplicable)
		}
		let message = message.take().ok_or(MissingArgument)?;
		Ok((message, MultiAssets::new()))
	}

	fn deliver(message: Xcm<()>) -> Result<(), SendError> {
		// We now pretend that the message was delivered from `Local` to `Remote`, and execute
		// so we need to ensure that the `TestConfig` is set up properly for executing as
		// though it is `Remote`.
		ExecutorUniversalLocation::set(Remote::get());
		let origin = Local::get().relative_to(&Remote::get());
		AllowUnpaidFrom::set(vec![origin.clone()]);
		set_exporter_override(price::<RemoteExporter>, deliver::<RemoteExporter>);
		// The we execute it:
		let outcome =
			XcmExecutor::<TestConfig>::execute_xcm(origin, message.into(), 2_000_000_000_000);
		match outcome {
			Outcome::Complete(..) => Ok(()),
			Outcome::Incomplete(..) => Err(Transport("Error executing")),
			Outcome::Error(..) => Err(Transport("Unable to execute")),
		}
	}
}

/// This is a dummy router which accepts messages destined for `Remote` from `Local`
/// and then executes them in a context simulated to be like that of our `Remote`. Payment is
/// needed.
struct ExecutingRouter<Local, Remote, RemoteExporter>(PhantomData<(Local, Remote, RemoteExporter)>);
impl<Local: Get<Junctions>, Remote: Get<Junctions>, RemoteExporter: ExportXcm> SendXcm
	for ExecutingRouter<Local, Remote, RemoteExporter>
{
	type OptionTicket = Option<Xcm<()>>;

	fn validate(
		destination: &mut Option<MultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Xcm<()>> {
		let expect_dest = Remote::get().relative_to(&Local::get());
		if destination.as_ref().ok_or(MissingArgument)? != &expect_dest {
			return Err(NotApplicable)
		}
		let message = message.take().ok_or(MissingArgument)?;
		Ok((message, MultiAssets::new()))
	}

	fn deliver(message: Xcm<()>) -> Result<(), SendError> {
		// We now pretend that the message was delivered from `Local` to `Remote`, and execute
		// so we need to ensure that the `TestConfig` is set up properly for executing as
		// though it is `Remote`.
		ExecutorUniversalLocation::set(Remote::get());
		let origin = Local::get().relative_to(&Remote::get());
		AllowPaidFrom::set(vec![origin.clone()]);
		set_exporter_override(price::<RemoteExporter>, deliver::<RemoteExporter>);
		// The we execute it:
		let outcome =
			XcmExecutor::<TestConfig>::execute_xcm(origin, message.into(), 2_000_000_000_000);
		match outcome {
			Outcome::Complete(..) => Ok(()),
			Outcome::Incomplete(..) => Err(Transport("Error executing")),
			Outcome::Error(..) => Err(Transport("Unable to execute")),
		}
	}
}
