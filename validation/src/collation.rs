// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Validator-side view of collation.
//!
//! This module contains type definitions, a trait for a batch of collators, and a trait for
//! attempting to fetch a collation repeatedly until a valid one is obtained.

use std::sync::Arc;

use polkadot_primitives::v0::{
	BlakeTwo256, Block, Hash, HashT,
	CollatorId, ParachainHost, Id as ParaId, Collation, ErasureChunk, CollationInfo,
};
use polkadot_erasure_coding as erasure;
use sp_api::ProvideRuntimeApi;
use futures::prelude::*;
use log::debug;
use primitives::traits::SpawnNamed;

/// Encapsulates connections to collators and allows collation on any parachain.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
pub trait Collators: Clone {
	/// Errors when producing collations.
	type Error: std::fmt::Debug;
	/// A full collation.
	type Collation: Future<Output=Result<Collation, Self::Error>>;

	/// Collate on a specific parachain, building on a given relay chain parent hash.
	///
	/// The returned collation should be checked for basic validity in the signature
	/// and will be checked for state-transition validity by the consumer of this trait.
	///
	/// This does not have to guarantee local availability, as a valid collation
	/// will be passed to the `TableRouter` instance.
	///
	/// The returned future may be prematurely concluded if the `relay_parent` goes
	/// out of date.
	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation;

	/// Note a bad collator. TODO: take proof (https://github.com/paritytech/polkadot/issues/217)
	fn note_bad_collator(&self, collator: CollatorId);
}

/// A future which resolves when a collation is available.
pub async fn collation_fetch<C: Collators, P>(
	execution_mode: crate::pipeline::ExecutionMode,
	parachain: ParaId,
	relay_parent: Hash,
	collators: C,
	client: Arc<P>,
	max_block_data_size: Option<u64>,
	n_validators: usize,
	spawner: impl SpawnNamed + Clone + 'static,
) -> Result<(CollationInfo, crate::pipeline::FullOutput), C::Error>
	where
		P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
		C: Collators + Unpin,
		P: ProvideRuntimeApi<Block>,
		<C as Collators>::Collation: Unpin,
{
	loop {
		let collation = collators.collate(parachain, relay_parent).await?;
		let Collation { info, pov } = collation;
		let res = crate::pipeline::full_output_validation_with_api(
			&execution_mode,
			&*client,
			&info,
			&pov,
			&relay_parent,
			max_block_data_size,
			n_validators,
			spawner.clone(),
		);

		match res {
			Ok(full_output) => {
				return Ok((info, full_output))
			}
			Err(e) => {
				debug!("Failed to validate parachain due to API error: {}", e);

				// just continue if we got a bad collation or failed to validate
				collators.note_bad_collator(info.collator)
			}
		}
	}
}

/// Validate an erasure chunk against an expected root.
pub fn validate_chunk(
	root: &Hash,
	chunk: &ErasureChunk,
) -> Result<(), ()> {
	let expected = erasure::branch_hash(root, &chunk.proof, chunk.index as usize).map_err(|_| ())?;
	let got = BlakeTwo256::hash(&chunk.chunk);

	if expected != got {
		return Err(())
	}

	Ok(())
}
