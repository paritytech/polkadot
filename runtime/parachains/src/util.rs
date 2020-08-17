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

//! Utilities that don't belong to any particular module but may draw
//! on all modules.

use sp_runtime::traits::{One, Saturating};
use primitives::v1::{Id as ParaId, PersistedValidationData, TransientValidationData};
use sp_std::prelude::*;

use crate::{configuration, paras};

/// Make the persisted validation data for a particular parachain.
///
/// This ties together the storage of several modules.
pub fn make_persisted_validation_data<T: paras::Trait>(
	para_id: ParaId,
) -> Option<PersistedValidationData<T::BlockNumber>> {
	let relay_parent_number = <frame_system::Module<T>>::block_number() - One::one();

	Some(PersistedValidationData {
		parent_head: <paras::Module<T>>::para_head(&para_id)?,
		block_number: relay_parent_number,
		hrmp_mqc_heads: Vec::new(),
	})
}

/// Make the transient validation data for a particular parachain.
///
/// This ties together the storage of several modules.
pub fn make_transient_validation_data<T: paras::Trait>(
	para_id: ParaId,
) -> Option<TransientValidationData<T::BlockNumber>> {
	let config = <configuration::Module<T>>::config();
	let relay_parent_number = <frame_system::Module<T>>::block_number() - One::one();

	let freq = config.validation_upgrade_frequency;
	let delay = config.validation_upgrade_delay;

	let last_code_upgrade = <paras::Module<T>>::last_code_upgrade(para_id, true);
	let can_upgrade_code = last_code_upgrade.map_or(
		true,
		|l| { l <= relay_parent_number && relay_parent_number.saturating_sub(l) >= freq },
	);

	let code_upgrade_allowed = if can_upgrade_code {
		Some(relay_parent_number + delay)
	} else {
		None
	};

	Some(TransientValidationData {
		max_code_size: config.max_code_size,
		max_head_data_size: config.max_head_data_size,
		balance: 0,
		code_upgrade_allowed,
	})
}
