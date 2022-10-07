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

use std::{collections::HashSet, num::NonZeroUsize};

use lru::LruCache;
use rand::{seq::SliceRandom, thread_rng};

use polkadot_node_subsystem::overseer;
use polkadot_node_subsystem_util::runtime::RuntimeInfo;
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, GroupIndex, Hash, SessionIndex, ValidatorIndex,
};

use crate::{
	error::{Error, Result},
	LOG_TARGET,
};

/// Caching of session info as needed by availability chunk distribution.
///
/// It should be ensured that a cached session stays live in the cache as long as we might need it.
pub struct SessionCache {
	/// Look up cached sessions by `SessionIndex`.
	///
	/// Note: Performance of fetching is really secondary here, but we need to ensure we are going
	/// to get any existing cache entry, before fetching new information, as we should not mess up
	/// the order of validators in `SessionInfo::validator_groups`.
	session_info_cache: LruCache<SessionIndex, SessionInfo>,
}

/// Localized session information, tailored for the needs of availability distribution.
#[derive(Clone)]
pub struct SessionInfo {
	/// The index of this session.
	pub session_index: SessionIndex,

	/// Validator groups of the current session.
	///
	/// Each group's order is randomized. This way we achieve load balancing when requesting
	/// chunks, as the validators in a group will be tried in that randomized order. Each node
	/// should arrive at a different order, therefore we distribute the load on individual
	/// validators.
	pub validator_groups: Vec<Vec<AuthorityDiscoveryId>>,

	/// Information about ourselves:
	pub our_index: ValidatorIndex,

	/// Remember to which group we belong, so we won't start fetching chunks for candidates with
	/// our group being responsible. (We should have that chunk already.)
	///
	/// `None`, if we are not in fact part of any group.
	pub our_group: Option<GroupIndex>,
}

/// Report of bad validators.
///
/// Fetching tasks will report back validators that did not respond as expected, so we can re-order
/// them.
pub struct BadValidators {
	/// The session index that was used.
	pub session_index: SessionIndex,
	/// The group, the not properly responding validators belong to.
	pub group_index: GroupIndex,
	/// The list of bad validators.
	pub bad_validators: Vec<AuthorityDiscoveryId>,
}

#[overseer::contextbounds(AvailabilityDistribution, prefix = self::overseer)]
impl SessionCache {
	/// Create a new `SessionCache`.
	pub fn new() -> Self {
		SessionCache {
			// We need to cache the current and the last session the most:
			session_info_cache: LruCache::new(NonZeroUsize::new(2).unwrap()),
		}
	}

	/// Tries to retrieve `SessionInfo` and calls `with_info` if successful.
	///
	/// If this node is not a validator, the function will return `None`.
	///
	/// Use this function over any `fetch_session_info` if all you need is a reference to
	/// `SessionInfo`, as it avoids an expensive clone.
	pub async fn with_session_info<Context, F, R>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		parent: Hash,
		session_index: SessionIndex,
		with_info: F,
	) -> Result<Option<R>>
	where
		F: FnOnce(&SessionInfo) -> R,
	{
		if let Some(o_info) = self.session_info_cache.get(&session_index) {
			gum::trace!(target: LOG_TARGET, session_index, "Got session from lru");
			return Ok(Some(with_info(o_info)))
		}

		if let Some(info) =
			self.query_info_from_runtime(ctx, runtime, parent, session_index).await?
		{
			gum::trace!(target: LOG_TARGET, session_index, "Calling `with_info`");
			let r = with_info(&info);
			gum::trace!(target: LOG_TARGET, session_index, "Storing session info in lru!");
			self.session_info_cache.put(session_index, info);
			Ok(Some(r))
		} else {
			Ok(None)
		}
	}

	/// Variant of `report_bad` that never fails, but just logs errors.
	///
	/// Not being able to report bad validators is not fatal, so we should not shutdown the
	/// subsystem on this.
	pub fn report_bad_log(&mut self, report: BadValidators) {
		if let Err(err) = self.report_bad(report) {
			gum::warn!(
				target: LOG_TARGET,
				err = ?err,
				"Reporting bad validators failed with error"
			);
		}
	}

	/// Make sure we try unresponsive or misbehaving validators last.
	///
	/// We assume validators in a group are tried in reverse order, so the reported bad validators
	/// will be put at the beginning of the group.
	pub fn report_bad(&mut self, report: BadValidators) -> Result<()> {
		let available_sessions = self.session_info_cache.iter().map(|(k, _)| *k).collect();
		let session = self.session_info_cache.get_mut(&report.session_index).ok_or(
			Error::NoSuchCachedSession {
				available_sessions,
				missing_session: report.session_index,
			},
		)?;
		let group = session.validator_groups.get_mut(report.group_index.0 as usize).expect(
			"A bad validator report must contain a valid group for the reported session. qed.",
		);
		let bad_set = report.bad_validators.iter().collect::<HashSet<_>>();

		// Get rid of bad boys:
		group.retain(|v| !bad_set.contains(v));

		// We are trying validators in reverse order, so bad ones should be first:
		let mut new_group = report.bad_validators;
		new_group.append(group);
		*group = new_group;
		Ok(())
	}

	/// Query needed information from runtime.
	///
	/// We need to pass in the relay parent for our call to `request_session_info`. We should
	/// actually don't need that: I suppose it is used for internal caching based on relay parents,
	/// which we don't use here. It should not do any harm though.
	///
	/// Returns: `None` if not a validator.
	async fn query_info_from_runtime<Context>(
		&self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		relay_parent: Hash,
		session_index: SessionIndex,
	) -> Result<Option<SessionInfo>> {
		let info = runtime
			.get_session_info_by_index(ctx.sender(), relay_parent, session_index)
			.await?;

		let discovery_keys = info.session_info.discovery_keys.clone();
		let mut validator_groups = info.session_info.validator_groups.clone();

		if let Some(our_index) = info.validator_info.our_index {
			// Get our group index:
			let our_group = info.validator_info.our_group;

			// Shuffle validators in groups:
			let mut rng = thread_rng();
			for g in validator_groups.iter_mut() {
				g.shuffle(&mut rng)
			}
			// Look up `AuthorityDiscoveryId`s right away:
			let validator_groups: Vec<Vec<_>> = validator_groups
				.into_iter()
				.map(|group| {
					group
						.into_iter()
						.map(|index| {
							discovery_keys.get(index.0 as usize)
								.expect("There should be a discovery key for each validator of each validator group. qed.")
								.clone()
						})
						.collect()
				})
				.collect();

			let info = SessionInfo { validator_groups, our_index, session_index, our_group };
			return Ok(Some(info))
		}
		return Ok(None)
	}
}
