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

//! Tests for the protocol.

use super::*;

#[test]
fn router_inner_drop_sends_worker_message() {
	let parent = [1; 32].into();

	let (sender, mut receiver) = mpsc::channel(0);
	drop(RouterInner {
		relay_parent: parent,
		sender,
	});

	match receiver.try_next() {
		Ok(Some(ServiceToWorkerMsg::DropConsensusNetworking(x))) => assert_eq!(parent, x),
		_ => panic!("message not sent"),
	}
}
