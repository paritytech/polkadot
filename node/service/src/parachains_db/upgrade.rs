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

//! Migration code for the parachain's DB.

#![cfg(feature = "full-node")]

use std::io;

type Version = u32;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("I/O error when reading/writing the version")]
	Io(#[from] io::Error),
	#[error("The version file format is incorrect")]
	CorruptedVersionFile,
	#[error("Future version (expected {current:?}, found {got:?})")]
	FutureVersion { current: Version, got: Version },
}

impl From<Error> for io::Error {
	fn from(me: Error) -> io::Error {
		match me {
			Error::Io(e) => e,
			_ => super::other_io_error(me.to_string()),
		}
	}
}
