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

use std::{cell::RefCell, marker::PhantomData};
use frame_support::{parameter_types, traits::Get};
use assert_matches::assert_matches;
use xcm::prelude::*;
use xcm_executor::XcmExecutor;
use crate::universal_exports::*;
use crate::mock::*;

mod local_relay_relay;
mod local_para_para;
mod remote_relay_relay;
mod remote_para_para;
mod remote_para_para_via_relay;

parameter_types! {
	pub Local: NetworkId = ByUri(b"local".to_vec());
	pub Remote: NetworkId = ByUri(b"remote".to_vec());
}

std::thread_local! {
	static BRIDGE_TRAFFIC: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
}

struct TestBridge<D>(PhantomData<D>);
impl<D: DispatchBlob> TestBridge<D> {
	fn service() -> u64 {
		BRIDGE_TRAFFIC
			.with(|t| t.borrow_mut().drain(..).map(|b| D::dispatch_blob(b).unwrap_or(0)).sum())
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

/// This is a dummy router which accepts messages destined for `Remote` from `Local`
/// them in a context simulated to be like that of our `Parent`.
struct ExecutingRouter<Local, Remote, RemoteExporter>(PhantomData<(Local, Remote, RemoteExporter)>);
impl<Local: Get<Junctions>, Remote: Get<Junctions>, RemoteExporter: ExportXcm> SendXcm for ExecutingRouter<Local, Remote, RemoteExporter> {
	fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
		let destination = destination.into();
		if destination == Remote::get().relative_to(&Local::get()) {
			// We now pretend that the message was delivered from the local chain
			// Parachain(1000) to the relay-chain and is getting executed there. We need to
			// configure the TestExecutor appropriately:
			ExecutorUniversalLocation::set(Remote::get());
			let origin = Local::get().relative_to(&Remote::get());
			AllowUnpaidFrom::set(vec![origin.clone()]);
			set_exporter_override(RemoteExporter::export_xcm);
			// The we execute it:
			let outcome = XcmExecutor::<TestConfig>::execute_xcm(origin, message.into(), 2_000_000_000_000);
			assert_matches!(outcome, Outcome::Complete(_));
			return Ok(())
		}
		Err(SendError::CannotReachDestination(destination, message))
	}
}
