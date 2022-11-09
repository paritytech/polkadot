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

use futures::prelude::*;
use sp_core::traits::SpawnNamed;

pub const MALUS: &str = "MALUS";

#[allow(unused)]
pub(crate) const MALICIOUS_POV: &[u8] = "😈😈pov_looks_valid_to_me😈😈".as_bytes();

/// Launch a service task for each item in the provided queue.
#[allow(unused)]
pub(crate) fn launch_processing_task<X, F, U, Q, S>(spawner: &S, queue: Q, action: F)
where
	F: Fn(X) -> U + Send + 'static,
	U: Future<Output = ()> + Send + 'static,
	Q: Stream<Item = X> + Send + 'static,
	X: Send,
	S: 'static + SpawnNamed + Clone + Unpin,
{
	let spawner2: S = spawner.clone();
	spawner.spawn(
		"nemesis-queue-processor",
		Some("malus"),
		Box::pin(async move {
			let spawner3 = spawner2.clone();
			queue
				.for_each(move |input| {
					spawner3.spawn("nemesis-task", Some("malus"), Box::pin(action(input)));
					async move { () }
				})
				.await;
		}),
	);
}
