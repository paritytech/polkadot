// Copyright 2018 Parity Technologies (UK) Ltd.
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

extern crate substrate_client as client;
extern crate parity_codec as codec;
extern crate substrate_transaction_pool as transaction_pool;
extern crate substrate_primitives;
extern crate sr_primitives;
extern crate polkadot_runtime as runtime;
extern crate polkadot_primitives as primitives;
extern crate polkadot_api;
extern crate parking_lot;

#[cfg(test)]
extern crate substrate_keyring;

#[macro_use]
extern crate error_chain;

#[macro_use]
extern crate log;

mod error;

use std::{
	cmp::Ordering,
	collections::HashMap,
	sync::Arc,
};

use codec::{Decode, Encode};
use transaction_pool::{Readiness, scoring::{Change, Choice}, VerifiedFor, ExtrinsicFor};
use polkadot_api::PolkadotApi;
use primitives::{AccountId, BlockId, Block, Hash, Index};
use runtime::{Address, UncheckedExtrinsic};
use sr_primitives::traits::{Bounded, Checkable, Hash as HashT, BlakeTwo256};

pub use transaction_pool::{Options, Status, LightStatus, VerifiedTransaction as VerifiedTransactionOps};
pub use error::{Error, ErrorKind, Result};

/// Maximal size of a single encoded extrinsic.
///
/// See also polkadot-consensus::MAX_TRANSACTIONS_SIZE
const MAX_TRANSACTION_SIZE: usize = 4 * 1024 * 1024;

/// Type alias for convenience.
pub type CheckedExtrinsic = <UncheckedExtrinsic as Checkable<fn(Address) -> std::result::Result<AccountId, &'static str>>>::Checked;

/// Type alias for polkadot transaction pool.
pub type TransactionPool<A> = transaction_pool::Pool<ChainApi<A>>;

/// A verified transaction which should be includable and non-inherent.
#[derive(Clone, Debug)]
pub struct VerifiedTransaction {
	inner: Option<CheckedExtrinsic>,
	sender: Option<AccountId>,
	hash: Hash,
	encoded_size: usize,
	index: Index,
}

impl VerifiedTransaction {
	/// Consume the verified transaction, yielding the checked counterpart.
	pub fn into_inner(self) -> Option<CheckedExtrinsic> {
		self.inner
	}

	/// Get the 256-bit hash of this transaction.
	pub fn hash(&self) -> &Hash {
		&self.hash
	}

	/// Get the account ID of the sender of this transaction.
	pub fn sender(&self) -> Option<AccountId> {
		self.sender
	}

	/// Get the account ID of the sender of this transaction.
	pub fn index(&self) -> Index {
		self.index
	}

	/// Get encoded size of the transaction.
	pub fn encoded_size(&self) -> usize {
		self.encoded_size
	}

	/// Returns `true` if the transaction is not yet fully verified.
	pub fn is_fully_verified(&self) -> bool {
		self.inner.is_some()
	}
}

impl transaction_pool::VerifiedTransaction for VerifiedTransaction {
	type Hash = Hash;
	type Sender = Option<AccountId>;

	fn hash(&self) -> &Self::Hash {
		&self.hash
	}

	fn sender(&self) -> &Self::Sender {
		&self.sender
	}

	fn mem_usage(&self) -> usize {
		self.encoded_size // TODO
	}
}

/// The polkadot transaction pool logic.
pub struct ChainApi<A> {
	api: Arc<A>,
}

impl<A> ChainApi<A> where
	A: PolkadotApi,
{
	const NO_ACCOUNT: &'static str = "Account not found.";
	/// Create a new instance.
	pub fn new(api: Arc<A>) -> Self {
		ChainApi {
			api,
		}
	}

	fn lookup(&self, at: &BlockId, address: Address) -> ::std::result::Result<AccountId, &'static str> {
		// TODO [ToDr] Consider introducing a cache for this.
		match self.api.lookup(at, address.clone()) {
			Ok(Some(address)) => Ok(address),
			Ok(None) => Err(Self::NO_ACCOUNT.into()),
			Err(e) => {
				error!("Error looking up address: {:?}: {:?}", address, e);
				Err("API error.")
			},
		}
	}
}

impl<A> transaction_pool::ChainApi for ChainApi<A> where
	A: PolkadotApi + Send + Sync,
{
	type Block = Block;
	type Hash = Hash;
	type Sender = Option<AccountId>;
	type VEx = VerifiedTransaction;
	type Ready = HashMap<AccountId, u32>;
	type Error = Error;
	type Score = u64;
	type Event = ();

	fn verify_transaction(&self, at: &BlockId, xt: &ExtrinsicFor<Self>) -> Result<Self::VEx> {
		let encoded = xt.encode();
		let uxt = UncheckedExtrinsic::decode(&mut encoded.as_slice()).ok_or_else(|| ErrorKind::InvalidExtrinsicFormat)?;
		if !uxt.is_signed() {
			bail!(ErrorKind::IsInherent(uxt))
		}

		let (encoded_size, hash) = (encoded.len(), BlakeTwo256::hash(&encoded));
		if encoded_size > MAX_TRANSACTION_SIZE {
			bail!(ErrorKind::TooLarge(encoded_size, MAX_TRANSACTION_SIZE));
		}

		debug!(target: "transaction-pool", "Transaction submitted: {}", ::substrate_primitives::hexdisplay::HexDisplay::from(&encoded));
		let inner = match uxt.clone().check(|a| self.lookup(at, a)) {
			Ok(xt) => Some(xt),
			// keep the transaction around in the future pool and attempt to promote it later.
			Err(Self::NO_ACCOUNT) => None,
			Err(e) => bail!(e),
		};
		let sender = match inner.as_ref() {
			Some(cxt) => match cxt.signed {
				Some(ref sender) => Some(sender.clone()),
				None => bail!(ErrorKind::IsInherent(uxt))
			},
			None => None,
		};

		if encoded_size < 1024 {
			debug!(target: "transaction-pool", "Transaction verified: {} => {:?}", hash, uxt);
		} else {
			debug!(target: "transaction-pool", "Transaction verified: {} ({} bytes is too large to display)", hash, encoded_size);
		}

		Ok(VerifiedTransaction {
			index: uxt.index,
			inner,
			sender,
			hash,
			encoded_size,
		})
	}

	fn ready(&self) -> Self::Ready {
		HashMap::default()
	}

	fn is_ready(&self, at: &BlockId, known_nonces: &mut Self::Ready, xt: &VerifiedFor<Self>) -> Readiness {
		let sender = match xt.verified.sender() {
			Some(sender) => sender,
			None => return Readiness::Future
		};

		trace!(target: "transaction-pool", "Checking readiness of {} (from {})", xt.verified.hash, Hash::from(sender));

		// TODO: find a way to handle index error properly -- will need changes to
		// transaction-pool trait.
		let api = &self.api;
		let next_index = known_nonces.entry(sender)
			.or_insert_with(|| api.index(at, sender).ok().unwrap_or_else(Bounded::max_value));

		trace!(target: "transaction-pool", "Next index for sender is {}; xt index is {}", next_index, xt.verified.index);

		let result = match xt.verified.index.cmp(&next_index) {
			// TODO: this won't work perfectly since accounts can now be killed, returning the nonce
			// to zero.
			// We should detect if the index was reset and mark all transactions as `Stale` for cull to work correctly.
			// Otherwise those transactions will keep occupying the queue.
			// Perhaps we could mark as stale if `index - state_index` > X?
			Ordering::Greater => Readiness::Future,
			Ordering::Equal => Readiness::Ready,
			// TODO [ToDr] Should mark transactions referencing too old blockhash as `Stale` as well.
			Ordering::Less => Readiness::Stale,
		};

		// remember to increment `next_index`
		*next_index = next_index.saturating_add(1);

		result
	}

	fn compare(old: &VerifiedFor<Self>, other: &VerifiedFor<Self>) -> Ordering {
		old.verified.index().cmp(&other.verified.index())
	}

	fn choose(old: &VerifiedFor<Self>, new: &VerifiedFor<Self>) -> Choice {
		if old.verified.is_fully_verified() {
			assert!(new.verified.is_fully_verified(), "Scoring::choose called with transactions from different senders");
			if old.verified.index() == new.verified.index() {
				return Choice::ReplaceOld;
			}
		}

		// This will keep both transactions, even though they have the same indices.
		// It's fine for not fully verified transactions, we might also allow it for
		// verified transactions but it would mean that only one of the two is actually valid
		// (most likely the first to be included in the block).
		Choice::InsertNew
	}

	fn update_scores(
		xts: &[transaction_pool::Transaction<VerifiedFor<Self>>],
		scores: &mut [Self::Score],
		_change: Change<()>
	) {
		for i in 0..xts.len() {
			if !xts[i].verified.is_fully_verified() {
				scores[i] = 0;
			} else {
				// all the same score since there are no fees.
				// TODO: prioritize things like misbehavior or fishermen reports
				scores[i] = 1;
			}
		}
	}

	fn should_replace(old: &VerifiedFor<Self>, _new: &VerifiedFor<Self>) -> Choice {
		if old.verified.is_fully_verified() {
			// Don't allow new transactions if we are reaching the limit.
			Choice::RejectNew
		} else {
			// Always replace not fully verified transactions.
			Choice::ReplaceOld
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{atomic::{self, AtomicBool}, Arc};
	use substrate_keyring::Keyring::{self, *};
	use codec::{Decode, Encode};
	use polkadot_api::{PolkadotApi, BlockBuilder, Result};
	use primitives::{AccountId, AccountIndex, Block, BlockId, Hash, Index, SessionKey,
		UncheckedExtrinsic as FutureProofUncheckedExtrinsic};
	use runtime::{RawAddress, Call, TimestampCall, UncheckedExtrinsic};
	use primitives::parachain::{DutyRoster, Id as ParaId};
	use sr_primitives::generic;
	use transaction_pool::Pool;
	use super::ChainApi;

	struct TestBlockBuilder;
	impl BlockBuilder for TestBlockBuilder {
		fn push_extrinsic(&mut self, _extrinsic: FutureProofUncheckedExtrinsic) -> Result<()> { unimplemented!() }
		fn bake(self) -> Result<Block> { unimplemented!() }
	}

	fn number_of(at: &BlockId) -> u32 {
		match at {
			generic::BlockId::Number(n) => *n as u32,
			_ => 0,
		}
	}

	#[derive(Default, Clone)]
	struct TestPolkadotApi {
		no_lookup: Arc<AtomicBool>,
	}

	impl TestPolkadotApi {
		fn without_lookup() -> Self {
			TestPolkadotApi {
				no_lookup: Arc::new(AtomicBool::new(true)),
			}
		}

		pub fn enable_lookup(&self) {
			self.no_lookup.store(false, atomic::Ordering::SeqCst);
		}
	}

	impl PolkadotApi for TestPolkadotApi {
		type BlockBuilder = TestBlockBuilder;

		fn session_keys(&self, _at: &BlockId) -> Result<Vec<SessionKey>> { unimplemented!() }
		fn validators(&self, _at: &BlockId) -> Result<Vec<AccountId>> { unimplemented!() }
		fn random_seed(&self, _at: &BlockId) -> Result<Hash> { unimplemented!() }
		fn duty_roster(&self, _at: &BlockId) -> Result<DutyRoster> { unimplemented!() }
		fn timestamp(&self, _at: &BlockId) -> Result<u64> { unimplemented!() }
		fn evaluate_block(&self, _at: &BlockId, _block: Block) -> Result<bool> { unimplemented!() }
		fn active_parachains(&self, _at: &BlockId) -> Result<Vec<ParaId>> { unimplemented!() }
		fn parachain_code(&self, _at: &BlockId, _parachain: ParaId) -> Result<Option<Vec<u8>>> { unimplemented!() }
		fn parachain_head(&self, _at: &BlockId, _parachain: ParaId) -> Result<Option<Vec<u8>>> { unimplemented!() }
		fn build_block(&self, _at: &BlockId, _inherent: ::primitives::InherentData) -> Result<Self::BlockBuilder> { unimplemented!() }
		fn inherent_extrinsics(&self, _at: &BlockId, _inherent: ::primitives::InherentData) -> Result<Vec<::primitives::UncheckedExtrinsic>> { unimplemented!() }

		fn index(&self, _at: &BlockId, _account: AccountId) -> Result<Index> {
			Ok((_account[0] as u32) + number_of(_at))
		}
		fn lookup(&self, _at: &BlockId, _address: RawAddress<AccountId, AccountIndex>) -> Result<Option<AccountId>> {
			match _address {
				RawAddress::Id(i) => Ok(Some(i)),
				RawAddress::Index(_) if self.no_lookup.load(atomic::Ordering::SeqCst) => Ok(None),
				RawAddress::Index(i) => Ok(match (i < 8, i + (number_of(_at) as u64) % 8) {
					(false, _) => None,
					(_, 0) => Some(Alice.to_raw_public().into()),
					(_, 1) => Some(Bob.to_raw_public().into()),
					(_, 2) => Some(Charlie.to_raw_public().into()),
					(_, 3) => Some(Dave.to_raw_public().into()),
					(_, 4) => Some(Eve.to_raw_public().into()),
					(_, 5) => Some(Ferdie.to_raw_public().into()),
					(_, 6) => Some(One.to_raw_public().into()),
					(_, 7) => Some(Two.to_raw_public().into()),
					_ => None,
				}),
			}
		}
	}

	fn uxt(who: Keyring, nonce: Index, use_id: bool) -> FutureProofUncheckedExtrinsic {
		let sxt = (nonce, Call::Timestamp(TimestampCall::set(0)));
		let sig = sxt.using_encoded(|e| who.sign(e));
		let signed = who.to_raw_public().into();
		let sender = if use_id { RawAddress::Id(signed) } else { RawAddress::Index(
			match who {
				Alice => 0,
				Bob => 1,
				Charlie => 2,
				Dave => 3,
				Eve => 4,
				Ferdie => 5,
				One => 6,
				Two => 7,
			}
		)};
		UncheckedExtrinsic::new_signed(sxt.0, sxt.1, sender, sig.into())
			.using_encoded(|e| FutureProofUncheckedExtrinsic::decode(&mut &e[..]))
			.unwrap()
	}

	fn pool(api: &TestPolkadotApi) -> Pool<ChainApi<TestPolkadotApi>> {
		Pool::new(Default::default(), ChainApi { api: Arc::new(api.clone()) })
	}

	#[test]
	fn id_submission_should_work() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, true)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![(Some(Alice.to_raw_public().into()), 209)]);
	}

	#[test]
	fn index_submission_should_work() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, false)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![(Some(Alice.to_raw_public().into()), 209)]);
	}

	#[test]
	fn multiple_id_submission_should_work() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, true)).unwrap();
		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, true)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![(Some(Alice.to_raw_public().into()), 209), (Some(Alice.to_raw_public().into()), 210)]);
	}

	#[test]
	fn multiple_index_submission_should_work() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, false)).unwrap();
		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, false)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![(Some(Alice.to_raw_public().into()), 209), (Some(Alice.to_raw_public().into()), 210)]);
	}

	#[test]
	fn id_based_early_nonce_should_be_culled() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 208, true)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);
	}

	#[test]
	fn index_based_early_nonce_should_be_culled() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 208, false)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);
	}

	#[test]
	fn id_based_late_nonce_should_be_queued() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);

		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, true)).unwrap();
		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);

		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, true)).unwrap();
		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![(Some(Alice.to_raw_public().into()), 209), (Some(Alice.to_raw_public().into()), 210)]);
	}

	#[test]
	fn index_based_late_nonce_should_be_queued() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);

		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, false)).unwrap();
		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);

		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, false)).unwrap();
		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![(Some(Alice.to_raw_public().into()), 209), (Some(Alice.to_raw_public().into()), 210)]);
	}

	#[test]
	fn index_then_id_submission_should_make_progress() {
		let api = TestPolkadotApi::without_lookup();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, false)).unwrap();
		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, true)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);

		api.enable_lookup();
		pool.retry_verification(&BlockId::number(0), None).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![
			(Some(Alice.to_raw_public().into()), 209),
			(Some(Alice.to_raw_public().into()), 210)
		]);
	}

	#[test]
	fn retrying_verification_might_not_change_anything() {
		let api = TestPolkadotApi::without_lookup();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, false)).unwrap();
		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, true)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);

		pool.retry_verification(&BlockId::number(1), None).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);
	}

	#[test]
	fn id_then_index_submission_should_make_progress() {
		let api = TestPolkadotApi::without_lookup();
		let pool = pool(&api);
		pool.submit_one(&BlockId::number(0), uxt(Alice, 209, true)).unwrap();
		pool.submit_one(&BlockId::number(0), uxt(Alice, 210, false)).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![
			(Some(Alice.to_raw_public().into()), 209)
		]);

		// when
		api.enable_lookup();
		pool.retry_verification(&BlockId::number(0), None).unwrap();

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(0), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![
			(Some(Alice.to_raw_public().into()), 209),
			(Some(Alice.to_raw_public().into()), 210)
		]);
	}

	#[test]
	fn index_change_should_result_in_second_tx_culled_or_future() {
		let api = TestPolkadotApi::default();
		let pool = pool(&api);
		let block = BlockId::number(0);
		pool.submit_one(&block, uxt(Alice, 209, false)).unwrap();
		let hash = *pool.submit_one(&block, uxt(Alice, 210, false)).unwrap().verified.hash();

		let pending: Vec<_> = pool.cull_and_get_pending(&block, |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![
			(Some(Alice.to_raw_public().into()), 209),
			(Some(Alice.to_raw_public().into()), 210)
		]);

		// first xt is mined, but that has a side-effect of switching index 0 from Alice to Bob.
		// second xt now invalid signature, so it fails.

		// there is no way of reporting this back to the queue right now (TODO). this should cause
		// the queue to flush all information regarding the sender index/account.

		// after this, a re-evaluation of the second's readiness should result in it being thrown
		// out (or maybe placed in future queue).
		let err = pool.reverify_transaction(&BlockId::number(1), hash).unwrap_err();
		match *err.kind() {
			::error::ErrorKind::Msg(ref m) if m == "bad signature in extrinsic" => {},
			ref e => assert!(false, "The transaction should be rejected with BadSignature error, got: {:?}", e),
		}

		let pending: Vec<_> = pool.cull_and_get_pending(&BlockId::number(1), |p| p.map(|a| (a.verified.sender(), a.verified.index())).collect()).unwrap();
		assert_eq!(pending, vec![]);

	}
}
