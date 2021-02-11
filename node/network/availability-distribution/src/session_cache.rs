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

use std::collections::HashMap;
use std::rc::{Rc, Weak};

use rand::{seq::SliceRandom, thread_rng};

use sp_keystore::{CryptoStore, SyncCryptoStorePtr};

use super::{error::Result, Error, LOG_TARGET};
use polkadot_node_subsystem_util::{
	request_session_index_for_child_ctx, request_validator_groups_ctx, request_validators_ctx,
};
use polkadot_primitives::v1::{
	BlakeTwo256, CandidateDescriptor, CandidateHash, CoreState, ErasureChunk, Hash, HashT,
	SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID,
};
use polkadot_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	jaeger, ActiveLeavesUpdate, FromOverseer, OverseerSignal, PerLeafSpan, SpawnedSubsystem,
	Subsystem, SubsystemContext, SubsystemError,
};

/// Caching of session info as needed by availability distribution.
///
/// It should be ensured that a cached session stays live in the cache as long as we might need it.
/// A warning will be logged, if an already dead entry gets fetched.
pub struct SessionCache {
	/// Maintain caches for session information for currently relay parents of interest.
	///
	/// Fast path - if we have an entry here, no query to the runtime is necessary at all.
	by_relay_parent: HashMap<Hash, Weak<SessionInfo>>,

	/// Look up cached sessions by SessionIndex.
	///
	/// Slower path - we still have to look up the `SessionIndex` in the runtime, but still might have
	/// the session ready already.
	///
	/// Note: Performance of fetching is really secondary here, but we need to ensure we are going
	/// to get any existing cache entry, before fetching new information, as we should not mess up
	/// the order of validators.
	by_session_index: HashMap<SessionIndex, Weak<SessionInfo>>,

	/// Key store for determining whether we are a validator and what `ValidatorIndex` we have.
	keystore: SyncCryptoStorePtr,
}

/// Localized session information, tailored for the needs of availability distribution.
pub struct SessionInfo {
	/// Validator groups of the current session.
	///
	/// Each group's order is randomized. This way we achieve load balancing when requesting
	/// chunks, as the validators in a group will be tried in that randomized order. Each node
	/// should arrive at a different order, therefore we distribute the load.
	pub validator_groups: Vec<Vec<ValidatorIndex>>,

	/// All validators of that session.
	///
	/// Needed for authority discovery and finding ourselves.
	pub validators: Vec<ValidatorId>,

	/// Information about ourself:
	pub our_index: ValidatorIndex,
}

impl SessionCache {
	/// Retrieve session info for the given relay parent.
	///
	/// This function will query the cache first and will only query the runtime on cache miss.
	///
	/// Returns: `Ok(None)` in case this node is not a validator in the current session.
	pub async fn fetch_session_info<Context>(
		&mut self,
		ctx: &mut Context,
		parent: Hash,
	) -> Result<Option<Rc<SessionInfo>>>
	where
		Context: SubsystemContext,
	{
		if let Some(info) = self.get_by_relay_parent(parent) {
			return Ok(Some(info));
		}
		let session_index = request_session_index_for_child_ctx(parent, ctx)
			.await?
			.await
			.map_err(|e| Error::SessionCacheRuntimRequest(e))?;
		if let Some(info) = self.get_by_session_index(session_index) {
			self.by_relay_parent.insert(parent, info.downgrade());
			return Ok(Some(info));
		}
		if let Some((our_index, validators)) = self.query_validator_info(ctx, parent).await? {
			let (mut validator_groups, _) = request_validator_groups_ctx(parent, ctx).await?.await?;
			// Shuffle validators in groups:
			let mut rng = thread_rng();
			for g in validator_groups.iter_mut() {
				g.shuffle(&rng)
			}
			let info = Rc::new(SessionInfo {
				validator_groups,
				validators,
				our_index,
			});
			let downgraded = info.downgrade();
			self.by_relay_parent.insert(parent, downgraded);
			self.get_by_session_index.insert(session_index, downgraded);
			return Ok(Some(info));
		}
		Ok(None)
	}

	/// Get session info for a particular relay parent.
	///
	/// Returns: None, if no entry for that relay parent exists in the cache (or it was dead
	/// already - which should not happen.)
	fn get_by_relay_parent(&self, relay_parent: Hash) -> Option<Rc<SessionInfo>> {
		let weak_ref = self.by_relay_parent.get(relay_parent)?;
		upgrade_report_dead(weak_ref)
	}

	/// Get session info for a given `SessionIndex`.
	fn get_by_session_index(&self, session_index: SessionIndex) -> Option<Rc<SessionInfo>> {
		let weak_ref = self.by_session_index.get(session_index)?;
		upgrade_report_dead(weak_ref)
	}

	/// Get our validator id and the validators in the current session.
	///
	/// Returns: Ok(None) if we are not a validator.
	async fn query_validator_info<Context>(
		&self,
		&ctx: &mut Context,
		parent: Hash,
	) -> Result<Option<(ValidatorIndex, Vec<ValidatorId>)>> {
		let validators = request_validators_ctx(ctx, parent).await?.await?;
		for (i, v) in validators.iter().enumerate() {
			if CryptoStore::has_keys(&*self.keystore, &[(v.to_raw_vec(), ValidatorId::ID)])
				.await
			{
				return Ok(Some((i as ValidatorIndex, validators)));
			}
		}
		Ok(None)
	}
}

/// Upgrade a weak SessionInfo reference.
///
/// Warn if it was dead already, as this should not happen. Cache should stay valid at least as
/// long as we need it.
fn upgrade_report_dead(info: Weak<SessionInfo>) -> Option<Rc<SessionInfo>> {
	match info.upgrade() {
		Some(info) => Some(info),
		None => {
			tracing::warn!(LOG_TARGET, "A no longer cached session got requested, this should not happen in normal operation.");
			None
		}
	}
}
