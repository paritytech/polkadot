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

use std::rc::Weak;

use super::{LOG_TARGET, error::Result, Error};

/// Caching of session info as needed by availability distribution.
///
/// It should be ensured that a cached session stays live in the cache as long as we might need it.
/// A warning will be logged, if an already dead entry gets fetched.
struct SessionCache {
	/// Maintain caches for session information for currently relay parents of interest.
	///
	/// Fast path - if we have an entry here, no query to the runtime is necessary at all.
	by_relay_parent: HashMap<Hash, Weak<SessionInfo>,

	/// Look up cached sessions by SessionIndex.
	///
	/// Slower path - we still have to look up the `SessionIndex` in the runtime, but still might have
	/// the session ready already.
	///
	/// Note: Performance of fetching is really secondary here, but we need to ensure we are going
	/// to get any existing cache entry, before fetching new information, as we should not mess up
	/// the order of validators.
	by_session_index: HashMap<SessionIndex, Weak<SessionInfo>,
}

/// Localized session information, tailored for the needs of availability distribution.
pub struct SessionInfo {
	/// Validator groups of the current session.
	///
	/// Each group's order is randomized. This way we achieve load balancing when requesting
	/// chunks, as the validators in a group will be tried in that randomized order. Each node
	/// should arrive at a different order, therefore we distribute the load.
	pub validator_groups: Vec<Vec<ValidatorIndex>>,

	/// Information about ourself:
	pub our_index: ValidatorIndex,
}

impl SessionCache {

	/// Retrieve session info for the given relay parent.
	///
	/// This function will query the cache first and will only query the runtime on cache miss.
	pub fn fetch_session_info(&mut self, ctx: &mut Context, relay_parent: Hash) -> Result<Rc<SessionInfo>> {
		if let Some(info) = self.get_by_relay_parent(relay_parent) {
			return Ok(info)
		}
		let session_index = request_session_index_for_child_ctx(parent, ctx).await
			.map_err(|e| Error::SessionCacheRuntimRequest(e))?;
		if let Some(info) = self.get_by_session_index(session_index) {
			self.by_relay_parent.insert(relay_parent, info.downgrade);
			return Ok(info);
		}

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
	fn get_by_session_index(&self, session_id: SessionId) -> Option<Rc<SessionInfo>> {
		let weak_ref = self.by_session_id.get(session_id)?;
		upgrade_report_dead(weak_ref)
	}
}

/// Upgrade a weak SessionInfo reference.
///
/// Warn if it was dead already, as this should not happen. Cache should stay valid at least as
/// long as we need it.
fn upgrade_report_dead(info: Weak<SessionInfo>>) -> Option<Rc<SessionInfo>> {
	match weak_ref.upgrade() {
		Some(info) => Some(info),
		None => {
			tracing::warn!(LOG_TARGET, relay_parent, "A no longer cached session got requested, this should not happen in normal operation.");
			None
		}
	}
}
