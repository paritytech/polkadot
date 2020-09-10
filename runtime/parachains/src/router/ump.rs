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

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use super::{Trait, Module};
use crate::configuration::HostConfiguration;
use sp_std::prelude::*;
use frame_support::weights::Weight;
use primitives::v1::{Id as ParaId, UpwardMessage};

impl<T: Trait> Module<T> {
	/// Check that all the upward messages sent by a candidate pass the acceptance criteria. Returns
	/// false, if any of the messages doesn't pass.
	pub(crate) fn check_upward_messages(
		config: &HostConfiguration<T::BlockNumber>,
		para: ParaId,
		upward_messages: &[UpwardMessage],
	) -> bool {
		drop(para);

		if upward_messages.len() as u32 > config.max_upward_message_num_per_candidate {
			return false;
		}

		for _ in upward_messages {
			return false;
		}

		true
	}

	/// Enacts all the upward messages sent by a candidate.
	pub(crate) fn enact_upward_messages(para: ParaId, upward_messages: Vec<UpwardMessage>) -> Weight {
		drop(para);

		for _ in upward_messages {
			todo!()
		}

		0
	}

	/// Devote some time into dispatching pending upward messages.
	pub(crate) fn process_pending_upward_messages() {
		// no-op for now, will be filled in the following commits
	}
}
