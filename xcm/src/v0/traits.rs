// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Cross-Consensus Message format data structures.

use sp_std::result;
use super::{MultiLocation, Xcm};

pub type Error = ();
pub type Result = result::Result<(), Error>;

pub trait ExecuteXcm {
	fn execute_xcm(origin: MultiLocation, msg: Xcm) -> Result;
}

impl ExecuteXcm for () {
	fn execute_xcm(_origin: MultiLocation, _msg: Xcm) -> Result {
		Err(())
	}
}

pub trait SendXcm {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> Result;
}

impl SendXcm for () {
	fn send_xcm(_dest: MultiLocation, _msg: Xcm) -> Result {
		Err(())
	}
}
