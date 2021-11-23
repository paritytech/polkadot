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

use std::collections::{HashMap, HashSet};

use polkadot_primitives::v1::{CandidateHash, SessionIndex, ValidatorIndex};

use crate::real::LOG_TARGET;

/// Type used for counting potential spam votes.
type SpamCount = u32;

/// How many unconfirmed disputes a validator is allowed to be a participant in (per session).
///
/// Unconfirmed means: Node has not seen the candidate be included on any chain, it has not cast a
/// vote itself on that dispute, the dispute has not yet reached more than a third of
/// validator's votes and the including relay chain block has not yet been finalized.
///
/// Exact number of `MAX_SPAM_VOTES` is not that important here. It is important that the number is
/// low enough to not cause resource exhaustion, if multiple validators spend their limits. Also
/// if things are working properly, this number cannot really be too low either, as all relevant
/// disputes _should_ have been seen as included my enough validators. (Otherwise the candidate
/// would not have been available in the first place and could not have been included.) So this is
/// really just a fallback mechanism if things go terribly wrong.
const MAX_SPAM_VOTES: SpamCount = 50;

/// Spam slots for raised disputes concerning unknown candidates.
pub struct SpamSlots {
	/// Counts per validator and session.
	///
	/// Must not exceed `MAX_SPAM_VOTES`.
	slots: HashMap<(SessionIndex, ValidatorIndex), SpamCount>,

	/// All unconfirmed candidates we are aware of right now.
	unconfirmed: UnconfirmedDisputes,
}

/// Unconfirmed disputes to be passed at initialization.
pub type UnconfirmedDisputes = HashMap<(SessionIndex, CandidateHash), HashSet<ValidatorIndex>>;

impl SpamSlots {
	/// Recover `SpamSlots` from state on startup.
	///
	/// Initialize based on already existing active disputes.
	pub fn recover_from_state(unconfirmed_disputes: UnconfirmedDisputes) -> Self {
		let mut slots: HashMap<(SessionIndex, ValidatorIndex), SpamCount> = HashMap::new();
		for ((session, _), validators) in unconfirmed_disputes.iter() {
			for validator in validators {
				let e = slots.entry((*session, *validator)).or_default();
				*e += 1;
				if *e > MAX_SPAM_VOTES {
					tracing::debug!(
						target: LOG_TARGET,
						?session,
						?validator,
						count = ?e,
						"Import exceeded spam slot for validator"
					);
				}
			}
		}

		Self { slots, unconfirmed: unconfirmed_disputes }
	}

	/// Add an unconfirmed dispute if free slots are available.
	pub fn add_unconfirmed(
		&mut self,
		session: SessionIndex,
		candidate: CandidateHash,
		validator: ValidatorIndex,
	) -> bool {
		let c = self.slots.entry((session, validator)).or_default();
		if *c >= MAX_SPAM_VOTES {
			return false
		}
		let validators = self.unconfirmed.entry((session, candidate)).or_default();

		if validators.insert(validator) {
			*c += 1;
			true
		} else {
			false
		}
	}

	/// Clear out spam slots for a given candiate in a session.
	///
	/// This effectively reduces the spam slot count for all validators participating in a dispute
	/// for that candidate. You should call this function once a dispute became obsolete or got
	/// confirmed and thus votes for it should no longer be treated as potential spam.
	pub fn clear(&mut self, key: &(SessionIndex, CandidateHash)) {
		if let Some(validators) = self.unconfirmed.remove(key) {
			let (session, _) = key;
			for validator in validators {
				if let Some(c) = self.slots.remove(&(*session, validator)) {
					let new = c - 1;
					if new > 0 {
						self.slots.insert((*session, validator), new);
					}
				}
			}
		}
	}
	/// Prune all spam slots for sessions older than the given index.
	pub fn prune_old(&mut self, oldest_index: SessionIndex) {
		self.unconfirmed.retain(|(session, _), _| *session >= oldest_index);
		self.slots.retain(|(session, _), _| *session >= oldest_index);
	}
}
