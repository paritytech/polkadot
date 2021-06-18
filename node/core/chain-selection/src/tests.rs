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

//! Tests for the subsystem.
//!
//! These primarily revolve around having a backend which is shared between
//! both the test code and the tested subsystem, and which also gives the
//! test code the ability to wait for write operations to occur.

// TODO [now]: importing a block without reversion
// TODO [now]: importing a block with reversion

// TODO [now]: finalize a viable block
// TODO [now]: finalize an unviable block with viable descendants
// TODO [now]: finalize an unviable block with unviable descendants down the line

// TODO [now]: mark blocks as stagnant.
// TODO [now]: approve stagnant block with unviable descendant.

// TODO [now]; test find best leaf containing with no leaves.
// TODO [now]: find best leaf containing when required is finalized
// TODO [now]: find best leaf containing when required is unfinalized.
// TODO [now]: find best leaf containing when required is ancestor of many leaves.
