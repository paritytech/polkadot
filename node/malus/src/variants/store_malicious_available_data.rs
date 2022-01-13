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

//! A malicious node that replaces approvals with invalid disputes
//! against valid candidates.
//!
//! Attention: For usage with `zombienet` only!

#![allow(missing_docs)]

use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use kvdb::{DBTransaction, KeyValueDB};

use polkadot_cli::{
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerConnector, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost,
		ProvideRuntimeApi, SpawnNamed,
	},
};

// Filter wrapping related types.
use crate::interceptor::*;

// Import extra types relevant to the particular
// subsystem.
use parity_scale_codec::{Decode, Encode, Error as CodecError, Input};
use polkadot_node_core_av_store::Config as AvailabilityConfig;
use polkadot_node_primitives::{AvailableData, ErasureChunk};
use polkadot_node_subsystem::messages::AvailabilityStoreMessage;
use polkadot_primitives::v1::{BlockNumber, CandidateHash, Hash, ValidatorIndex};

use std::{
	sync::Arc,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

const AVAILABLE_PREFIX: &[u8; 9] = b"available";
const CHUNK_PREFIX: &[u8; 5] = b"chunk";
const META_PREFIX: &[u8; 4] = b"meta";
const PRUNE_BY_TIME_PREFIX: &[u8; 13] = b"prune_by_time";

// We have some keys we want to map to empty values because existence of the key is enough. We use this because
// rocksdb doesn't support empty values.
const TOMBSTONE_VALUE: &[u8] = &*b" ";

/// Replace outgoing approval messages with disputes.
#[derive(Clone)]
pub(crate) struct StoreMaliciousAvailableData {
	db: Arc<dyn KeyValueDB>,
	config: AvailabilityConfig,
}

impl<Sender> MessageInterceptor<Sender> for StoreMaliciousAvailableData
where
	Sender: overseer::SubsystemSender<AvailabilityStoreMessage> + Clone + Send + 'static,
{
	type Message = AvailabilityStoreMessage;

	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		match msg {
			FromOverseer::Communication {
				msg:
					AvailabilityStoreMessage::StoreAvailableData {
						candidate_hash,
						n_validators,
						available_data,
						tx,
					},
			} => {
				let res = store_malicious_available_data(
					&self.db,
					&self.config,
					candidate_hash,
					0,
					n_validators as _,
					available_data,
				);

				match res {
					Ok(()) => {
						let _ = tx.send(Ok(()));
					},
					Err(_) => {
						let _ = tx.send(Err(()));
					},
				}

				// We needn't actually drop this message, but there is no benefit in handling it.
				None
			},
			_ => Some(msg),
		}
	}

	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

// Meta information about a candidate.
#[derive(Debug, Encode, Decode)]
struct CandidateMeta {
	state: State,
	data_available: bool,
	chunks_stored: BitVec<BitOrderLsb0, u8>,
}

fn store_malicious_available_data(
	db: &Arc<dyn KeyValueDB>,
	config: &AvailabilityConfig,
	candidate_hash: CandidateHash,
	replaced_chunk_index: usize,
	n_validators: usize,
	available_data: AvailableData,
) -> Result<(), Error> {
	let mut tx = DBTransaction::new();

	let mut meta = match load_meta(&db, &config, &candidate_hash)? {
		Some(m) => {
			if m.data_available {
				return Ok(()) // already stored.
			}

			m
		},
		None => {
			let now = SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.expect("Getting time should work. QED");

			// Write a pruning record.
			let prune_at = now + Duration::from_secs(60 * 60);
			write_pruning_key(&mut tx, &config, prune_at, &candidate_hash);

			CandidateMeta {
				state: State::Unavailable(BETimestamp(0)),
				data_available: false,
				chunks_stored: BitVec::new(),
			}
		},
	};

	let mut chunks = erasure::obtain_chunks_v1(n_validators, &available_data).unwrap();
	chunks[replaced_chunk_index].fill(42);
	let branches = erasure::branches(chunks.as_ref());

	let erasure_chunks = chunks.iter().zip(branches.map(|(proof, _)| proof)).enumerate().map(
		|(index, (chunk, proof))| ErasureChunk {
			chunk: chunk.clone(),
			proof,
			index: ValidatorIndex(index as u32),
		},
	);

	for chunk in erasure_chunks {
		write_chunk(&mut tx, &config, &candidate_hash, chunk.index, &chunk);
	}

	meta.data_available = true;
	meta.chunks_stored = bitvec::bitvec![BitOrderLsb0, u8; 1; n_validators];

	write_meta(&mut tx, &config, &candidate_hash, &meta);
	write_available_data(&mut tx, &config, &candidate_hash, &available_data);

	db.write(tx)?;

	Ok(())
}

/// Generates an overseer that disputes instead of approving valid candidates.
pub(crate) struct StoreMaliciousAvailableDataWrapper;

impl OverseerGen for StoreMaliciousAvailableDataWrapper {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let filter = StoreMaliciousAvailableData {
			db: args.parachains_db.clone(),
			config: args.availability_config.clone(),
		};

		prepared_overseer_builder(args)?
			.replace_availability_store(|av| InterceptedSubsystem::new(av, filter))
			.build_with_connector(connector)
			.map_err(|e| e.into())
	}
}

/// Unix time wrapper with big-endian encoding.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
struct BETimestamp(u64);

impl Encode for BETimestamp {
	fn size_hint(&self) -> usize {
		std::mem::size_of::<u64>()
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		f(&self.0.to_be_bytes())
	}
}

impl Decode for BETimestamp {
	fn decode<I: Input>(value: &mut I) -> Result<Self, CodecError> {
		<[u8; 8]>::decode(value).map(u64::from_be_bytes).map(Self)
	}
}

impl From<Duration> for BETimestamp {
	fn from(d: Duration) -> Self {
		BETimestamp(d.as_secs())
	}
}

impl Into<Duration> for BETimestamp {
	fn into(self) -> Duration {
		Duration::from_secs(self.0)
	}
}

/// [`BlockNumber`] wrapper with big-endian encoding.
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
struct BEBlockNumber(BlockNumber);

impl Encode for BEBlockNumber {
	fn size_hint(&self) -> usize {
		std::mem::size_of::<BlockNumber>()
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		f(&self.0.to_be_bytes())
	}
}

impl Decode for BEBlockNumber {
	fn decode<I: Input>(value: &mut I) -> Result<Self, CodecError> {
		<[u8; std::mem::size_of::<BlockNumber>()]>::decode(value)
			.map(BlockNumber::from_be_bytes)
			.map(Self)
	}
}

#[derive(Debug, Encode, Decode)]
enum State {
	/// Candidate data was first observed at the given time but is not available in any block.
	#[codec(index = 0)]
	Unavailable(BETimestamp),
	/// The candidate was first observed at the given time and was included in the given list of unfinalized blocks, which may be
	/// empty. The timestamp here is not used for pruning. Either one of these blocks will be finalized or the state will regress to
	/// `State::Unavailable`, in which case the same timestamp will be reused. Blocks are sorted ascending first by block number and
	/// then hash.
	#[codec(index = 1)]
	Unfinalized(BETimestamp, Vec<(BEBlockNumber, Hash)>),
	/// Candidate data has appeared in a finalized block and did so at the given time.
	#[codec(index = 2)]
	Finalized(BETimestamp),
}

fn load_meta(
	db: &Arc<dyn KeyValueDB>,
	config: &AvailabilityConfig,
	hash: &CandidateHash,
) -> Result<Option<CandidateMeta>, Error> {
	let key = (META_PREFIX, hash).encode();

	query_inner(db, config.col_meta, &key)
}

fn write_meta(
	tx: &mut DBTransaction,
	config: &AvailabilityConfig,
	hash: &CandidateHash,
	meta: &CandidateMeta,
) {
	let key = (META_PREFIX, hash).encode();

	tx.put_vec(config.col_meta, &key, meta.encode());
}

fn write_chunk(
	tx: &mut DBTransaction,
	config: &AvailabilityConfig,
	candidate_hash: &CandidateHash,
	chunk_index: ValidatorIndex,
	erasure_chunk: &ErasureChunk,
) {
	let key = (CHUNK_PREFIX, candidate_hash, chunk_index).encode();

	tx.put_vec(config.col_data, &key, erasure_chunk.encode());
}

fn write_available_data(
	tx: &mut DBTransaction,
	config: &AvailabilityConfig,
	hash: &CandidateHash,
	available_data: &AvailableData,
) {
	let key = (AVAILABLE_PREFIX, hash).encode();

	tx.put_vec(config.col_data, &key[..], available_data.encode());
}

fn query_inner<D: Decode>(
	db: &Arc<dyn KeyValueDB>,
	column: u32,
	key: &[u8],
) -> Result<Option<D>, Error> {
	match db.get(column, key) {
		Ok(Some(raw)) => {
			let res = D::decode(&mut &raw[..]).expect("Should Decode. QED");
			Ok(Some(res))
		},
		Ok(None) => Ok(None),
		Err(err) => Err(err.into()),
	}
}

fn write_pruning_key(
	tx: &mut DBTransaction,
	config: &AvailabilityConfig,
	t: impl Into<BETimestamp>,
	h: &CandidateHash,
) {
	let t = t.into();
	let key = (PRUNE_BY_TIME_PREFIX, t, h).encode();
	tx.put(config.col_meta, &key, TOMBSTONE_VALUE);
}
