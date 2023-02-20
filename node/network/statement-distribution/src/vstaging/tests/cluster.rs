// Copyright 2023 Parity Technologies (UK) Ltd.
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

// TODO [now]: shared seconded statement is circulated to all cluster peers with relay-parent
//             in view

// TODO [now]: cluster 'valid' statement without prior seconded is ignored

// TODO [now]: statement with invalid signature leads to report

// TODO [now]: cluster statement from non-cluster peer is rejected

// TODO [now]: statement from non-cluster originator is rejected

// TODO [now]: cluster statement for unknown candidate leads to request

// TODO [now]: cluster statements are shared with `Seconded` first for all cluster peers
//             with relay-parent in view

// TODO [now]: cluster statements not re-shared on view update

// TODO [now]: cluster statements shared on first time cluster peer gets relay-parent in view.

// TODO [now]: confirmed cluster statement does not import statements until candidate in hypothetical frontier

// TODO [now]: shared valid statement after confirmation sent to all cluster peers with relay-parent
