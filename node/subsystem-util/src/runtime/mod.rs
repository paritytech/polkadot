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

//! Convenient interface to runtime information.

use std::num::NonZeroUsize;

use lru::LruCache;

use parity_scale_codec::Encode;
use sp_application_crypto::AppKey;
use sp_core::crypto::ByteArray;
use sp_keystore::{CryptoStore, SyncCryptoStorePtr};

use polkadot_node_subsystem::{messages::RuntimeApiMessage, overseer, SubsystemSender};
use polkadot_primitives::v2::{
	CandidateEvent, CoreState, EncodeAs, GroupIndex, GroupRotationInfo, Hash, IndexedVec,
	OccupiedCore, ScrapedOnChainVotes, SessionIndex, SessionInfo, Signed, SigningContext,
	UncheckedSigned, ValidationCode, ValidationCodeHash, ValidatorId, ValidatorIndex,
};

use crate::{
	request_availability_cores, request_candidate_events, request_on_chain_votes,
	request_session_index_for_child, request_session_info, request_validation_code_by_hash,
	request_validator_groups,
};

/// Errors that can happen on runtime fetches.
mod error;

use error::{recv_runtime, Result};
pub use error::{Error, FatalError, JfyiError};

/// Configuration for construction a `RuntimeInfo`.
pub struct Config {
	/// Needed for retrieval of `ValidatorInfo`
	///
	/// Pass `None` if you are not interested.
	pub keystore: Option<SyncCryptoStorePtr>,

	/// How many sessions should we keep in the cache?
	pub session_cache_lru_size: NonZeroUsize,
}

/// Caching of session info.
///
/// It should be ensured that a cached session stays live in the cache as long as we might need it.
pub struct RuntimeInfo {
	/// Get the session index for a given relay parent.
	///
	/// We query this up to a 100 times per block, so caching it here without roundtrips over the
	/// overseer seems sensible.
	session_index_cache: LruCache<Hash, SessionIndex>,

	/// Look up cached sessions by `SessionIndex`.
	session_info_cache: LruCache<SessionIndex, ExtendedSessionInfo>,

	/// Key store for determining whether we are a validator and what `ValidatorIndex` we have.
	keystore: Option<SyncCryptoStorePtr>,
}

/// `SessionInfo` with additional useful data for validator nodes.
pub struct ExtendedSessionInfo {
	/// Actual session info as fetched from the runtime.
	pub session_info: SessionInfo,
	/// Contains useful information about ourselves, in case this node is a validator.
	pub validator_info: ValidatorInfo,
}

/// Information about ourselves, in case we are an `Authority`.
///
/// This data is derived from the `SessionInfo` and our key as found in the keystore.
pub struct ValidatorInfo {
	/// The index this very validator has in `SessionInfo` vectors, if any.
	pub our_index: Option<ValidatorIndex>,
	/// The group we belong to, if any.
	pub our_group: Option<GroupIndex>,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			keystore: None,
			// Usually we need to cache the current and the last session.
			session_cache_lru_size: NonZeroUsize::new(2).expect("2 is larger than 0; qed"),
		}
	}
}

impl RuntimeInfo {
	/// Create a new `RuntimeInfo` for convenient runtime fetches.
	pub fn new(keystore: Option<SyncCryptoStorePtr>) -> Self {
		Self::new_with_config(Config { keystore, ..Default::default() })
	}

	/// Create with more elaborate configuration options.
	pub fn new_with_config(cfg: Config) -> Self {
		Self {
			session_index_cache: LruCache::new(
				cfg.session_cache_lru_size
					.max(NonZeroUsize::new(10).expect("10 is larger than 0; qed")),
			),
			session_info_cache: LruCache::new(cfg.session_cache_lru_size),
			keystore: cfg.keystore,
		}
	}

	/// Returns the session index expected at any child of the `parent` block.
	/// This does not return the session index for the `parent` block.
	pub async fn get_session_index_for_child<Sender>(
		&mut self,
		sender: &mut Sender,
		parent: Hash,
	) -> Result<SessionIndex>
	where
		Sender: SubsystemSender<RuntimeApiMessage>,
	{
		match self.session_index_cache.get(&parent) {
			Some(index) => Ok(*index),
			None => {
				let index =
					recv_runtime(request_session_index_for_child(parent, sender).await).await?;
				self.session_index_cache.put(parent, index);
				Ok(index)
			},
		}
	}

	/// Get `ExtendedSessionInfo` by relay parent hash.
	pub async fn get_session_info<'a, Sender>(
		&'a mut self,
		sender: &mut Sender,
		relay_parent: Hash,
	) -> Result<&'a ExtendedSessionInfo>
	where
		Sender: SubsystemSender<RuntimeApiMessage>,
	{
		let session_index = self.get_session_index_for_child(sender, relay_parent).await?;

		self.get_session_info_by_index(sender, relay_parent, session_index).await
	}

	/// Get `ExtendedSessionInfo` by session index.
	///
	/// `request_session_info` still requires the parent to be passed in, so we take the parent
	/// in addition to the `SessionIndex`.
	pub async fn get_session_info_by_index<'a, Sender>(
		&'a mut self,
		sender: &mut Sender,
		parent: Hash,
		session_index: SessionIndex,
	) -> Result<&'a ExtendedSessionInfo>
	where
		Sender: SubsystemSender<RuntimeApiMessage>,
	{
		if !self.session_info_cache.contains(&session_index) {
			let session_info =
				recv_runtime(request_session_info(parent, session_index, sender).await)
					.await?
					.ok_or(JfyiError::NoSuchSession(session_index))?;
			let validator_info = self.get_validator_info(&session_info).await?;

			let full_info = ExtendedSessionInfo { session_info, validator_info };

			self.session_info_cache.put(session_index, full_info);
		}
		Ok(self
			.session_info_cache
			.get(&session_index)
			.expect("We just put the value there. qed."))
	}

	/// Convenience function for checking the signature of something signed.
	pub async fn check_signature<Sender, Payload, RealPayload>(
		&mut self,
		sender: &mut Sender,
		relay_parent: Hash,
		signed: UncheckedSigned<Payload, RealPayload>,
	) -> Result<
		std::result::Result<Signed<Payload, RealPayload>, UncheckedSigned<Payload, RealPayload>>,
	>
	where
		Sender: SubsystemSender<RuntimeApiMessage>,
		Payload: EncodeAs<RealPayload> + Clone,
		RealPayload: Encode + Clone,
	{
		let session_index = self.get_session_index_for_child(sender, relay_parent).await?;
		let info = self.get_session_info_by_index(sender, relay_parent, session_index).await?;
		Ok(check_signature(session_index, &info.session_info, relay_parent, signed))
	}

	/// Build `ValidatorInfo` for the current session.
	///
	///
	/// Returns: `None` if not a parachain validator.
	async fn get_validator_info(&self, session_info: &SessionInfo) -> Result<ValidatorInfo> {
		if let Some(our_index) = self.get_our_index(&session_info.validators).await {
			// Get our group index:
			let our_group =
				session_info.validator_groups.iter().enumerate().find_map(|(i, g)| {
					g.iter().find_map(|v| {
						if *v == our_index {
							Some(GroupIndex(i as u32))
						} else {
							None
						}
					})
				});
			let info = ValidatorInfo { our_index: Some(our_index), our_group };
			return Ok(info)
		}
		return Ok(ValidatorInfo { our_index: None, our_group: None })
	}

	/// Get our `ValidatorIndex`.
	///
	/// Returns: None if we are not a validator.
	async fn get_our_index(
		&self,
		validators: &IndexedVec<ValidatorIndex, ValidatorId>,
	) -> Option<ValidatorIndex> {
		let keystore = self.keystore.as_ref()?;
		for (i, v) in validators.iter().enumerate() {
			if CryptoStore::has_keys(&**keystore, &[(v.to_raw_vec(), ValidatorId::ID)]).await {
				return Some(ValidatorIndex(i as u32))
			}
		}
		None
	}
}

/// Convenience function for quickly checking the signature on signed data.
pub fn check_signature<Payload, RealPayload>(
	session_index: SessionIndex,
	session_info: &SessionInfo,
	relay_parent: Hash,
	signed: UncheckedSigned<Payload, RealPayload>,
) -> std::result::Result<Signed<Payload, RealPayload>, UncheckedSigned<Payload, RealPayload>>
where
	Payload: EncodeAs<RealPayload> + Clone,
	RealPayload: Encode + Clone,
{
	let signing_context = SigningContext { session_index, parent_hash: relay_parent };

	session_info
		.validators
		.get(signed.unchecked_validator_index())
		.ok_or_else(|| signed.clone())
		.and_then(|v| signed.try_into_checked(&signing_context, v))
}

/// Request availability cores from the runtime.
pub async fn get_availability_cores<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
) -> Result<Vec<CoreState>>
where
	Sender: overseer::SubsystemSender<RuntimeApiMessage>,
{
	recv_runtime(request_availability_cores(relay_parent, sender).await).await
}

/// Variant of `request_availability_cores` that only returns occupied ones.
pub async fn get_occupied_cores<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
) -> Result<Vec<OccupiedCore>>
where
	Sender: overseer::SubsystemSender<RuntimeApiMessage>,
{
	let cores = get_availability_cores(sender, relay_parent).await?;

	Ok(cores
		.into_iter()
		.filter_map(|core_state| {
			if let CoreState::Occupied(occupied) = core_state {
				Some(occupied)
			} else {
				None
			}
		})
		.collect())
}

/// Get group rotation info based on the given `relay_parent`.
pub async fn get_group_rotation_info<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
) -> Result<GroupRotationInfo>
where
	Sender: overseer::SubsystemSender<RuntimeApiMessage>,
{
	// We drop `groups` here as we don't need them, because of `RuntimeInfo`. Ideally we would not
	// fetch them in the first place.
	let (_, info) = recv_runtime(request_validator_groups(relay_parent, sender).await).await?;
	Ok(info)
}

/// Get `CandidateEvent`s for the given `relay_parent`.
pub async fn get_candidate_events<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
) -> Result<Vec<CandidateEvent>>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	recv_runtime(request_candidate_events(relay_parent, sender).await).await
}

/// Get on chain votes.
pub async fn get_on_chain_votes<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
) -> Result<Option<ScrapedOnChainVotes>>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	recv_runtime(request_on_chain_votes(relay_parent, sender).await).await
}

/// Fetch `ValidationCode` by hash from the runtime.
pub async fn get_validation_code_by_hash<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
	validation_code_hash: ValidationCodeHash,
) -> Result<Option<ValidationCode>>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	recv_runtime(request_validation_code_by_hash(relay_parent, validation_code_hash, sender).await)
		.await
}
