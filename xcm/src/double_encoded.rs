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

use alloc::vec::Vec;
use parity_scale_codec::{Encode, Decode};

#[derive(Encode, Decode)]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub struct DoubleEncoded<T> {
	encoded: Vec<u8>,
	#[codec(skip)]
	decoded: Option<T>,
}

impl<T> Clone for DoubleEncoded<T> {
	fn clone(&self) -> Self { Self { encoded: self.encoded.clone(), decoded: None } }
}
impl<T> Eq for DoubleEncoded<T> {
}
impl<T> PartialEq for DoubleEncoded<T> {
	fn eq(&self, other: &Self) -> bool { self.encoded.eq(&other.encoded) }
}
impl<T> core::fmt::Debug for DoubleEncoded<T> {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { self.encoded.fmt(f) }
}

impl<T> From<Vec<u8>> for DoubleEncoded<T> {
	fn from(encoded: Vec<u8>) -> Self {
		Self { encoded, decoded: None }
	}
}

impl<T> DoubleEncoded<T> {
	pub fn into<S>(self) -> DoubleEncoded<S> { DoubleEncoded::from(self) }
	pub fn from<S>(e: DoubleEncoded<S>) -> Self {
		Self {
			encoded: e.encoded,
			decoded: None,
		}
	}
	pub fn as_ref(&self) -> Option<&T> {
		self.decoded.as_ref()
	}
}

impl<T: Decode> DoubleEncoded<T> {
	pub fn ensure_decoded(&mut self) -> Result<&T, ()> {
		if self.decoded.is_none() {
			self.decoded = T::decode(&mut &self.encoded[..]).ok();
		}
		self.decoded.as_ref().ok_or(())
	}
	pub fn take_decoded(&mut self) -> Result<T, ()> {
		self.decoded.take().or_else(|| T::decode(&mut &self.encoded[..]).ok()).ok_or(())
	}
	pub fn try_into(mut self) -> Result<T, ()> {
		self.ensure_decoded()?;
		self.decoded.ok_or(())
	}
}
