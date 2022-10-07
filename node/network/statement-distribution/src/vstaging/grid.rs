// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Utilities for handling distribution of backed candidates along
//! the grid.

use polkadot_primitives::vstaging::{AuthorityDiscoveryId, GroupIndex};

use std::collections::{HashMap, HashSet};

/// Our local view of a subset of the grid topology organized around a specific group.
///
/// This tracks which authorities we expect to communicate with concerning
/// candidates from the group. This includes both the authorities we are
/// expected to send to as well as the authorities we expect to receive from.
///
/// In the case that this group is the group that we are locally assigned to,
/// the 'receiving' side will be empty.
struct SubTopologyGroupLocalView {
	sending: HashSet<AuthorityDiscoveryId>,
	receiving: HashSet<AuthorityDiscoveryId>,
}

/// A tracker of knowledge from authorities within the grid for a
/// specific relay-parent.
struct PerRelayParentGridTracker {
	by_authority: HashMap<(AuthorityDiscoveryId, GroupIndex), Knowledge>
}

struct Knowledge {
}
