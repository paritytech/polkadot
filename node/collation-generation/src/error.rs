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


#[derive(Debug, derive_more::From)]
pub enum Error {
	#[from]
	Subsystem(polkadot_node_subsystem::SubsystemError),
	#[from]
	OneshotRecv(futures::channel::oneshot::Canceled),
	#[from]
	Runtime(polkadot_node_subsystem::errors::RuntimeApiError),
	#[from]
	Util(polkadot_node_subsystem_util::Error),
	#[from]
	Erasure(polkadot_erasure_coding::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
