// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Provides a safe wrapper that gives views into a byte-vec.

/// Wrapper around a `Vec<u8>` that provides views as a `[u8]` and `[[u8; 2]]`.
#[derive(Clone)]
pub(crate) struct WrappedShard {
	inner: Vec<u8>,
}

impl WrappedShard {
	/// Wrap `data`.
	pub(crate) fn new(mut data: Vec<u8>) -> Self {
		if data.len() % 2 != 0 {
			data.push(0);
		}

		WrappedShard { inner: data }
	}

	/// Unwrap and yield inner data.
	pub(crate) fn into_inner(self) -> Vec<u8> {
		self.inner
	}
}

impl AsRef<[u8]> for WrappedShard {
	fn as_ref(&self) -> &[u8] {
		self.inner.as_ref()
	}
}

impl AsMut<[u8]> for WrappedShard {
	fn as_mut(&mut self) -> &mut [u8] {
		self.inner.as_mut()
	}
}

impl AsRef<[[u8; 2]]> for WrappedShard {
	fn as_ref(&self) -> &[[u8; 2]] {
		assert_eq!(self.inner.len() % 2, 0);
		if self.inner.is_empty() { return &[] }
		unsafe {
			::std::slice::from_raw_parts(&self.inner[0] as *const _ as _, self.inner.len() / 2)
		}
	}
}

impl AsMut<[[u8; 2]]> for WrappedShard {
	fn as_mut(&mut self) -> &mut [[u8; 2]] {
		let len = self.inner.len();
		assert_eq!(len % 2, 0);

		if self.inner.is_empty() { return &mut [] }
		unsafe {
			::std::slice::from_raw_parts_mut(&mut self.inner[0] as *mut _ as _, len / 2)
		}
	}
}

impl std::iter::FromIterator<[u8; 2]> for WrappedShard {
	fn from_iter<I: IntoIterator<Item=[u8; 2]>>(iterable: I) -> Self {
		let iter = iterable.into_iter();

		let (l, _) = iter.size_hint();
		let mut inner = Vec::with_capacity(l * 2);

		for [a, b] in iter {
			inner.push(a);
			inner.push(b);
		}

		debug_assert_eq!(inner.len() % 2, 0);
		WrappedShard { inner }
	}
}

#[cfg(test)]
mod tests {
	use super::WrappedShard;

	#[test]
	fn wrap_empty_ok() {
		let mut wrapped = WrappedShard::new(Vec::new());
		{
			let _: &mut [u8] = wrapped.as_mut();
			let _: &mut [[u8; 2]] = wrapped.as_mut();
		}

		{
			let _: &[u8] = wrapped.as_ref();
			let _: &[[u8; 2]] = wrapped.as_ref();
		}
	}

	#[test]
	fn data_order_preserved() {
		let mut wrapped = WrappedShard::new(vec![1, 2, 3]);
		{
			let x: &[u8] = wrapped.as_ref();
			assert_eq!(x, &[1, 2, 3, 0]);
		}
		{
			let x: &mut [[u8; 2]] = wrapped.as_mut();
			assert_eq!(x, &mut [[1, 2], [3, 0]]);
			x[1] = [3, 4];
		}
		{
			let x: &[u8] = wrapped.as_ref();
			assert_eq!(x, &[1, 2, 3, 4]);
		}
	}

	#[test]
	fn from_iter() {
		let w: WrappedShard = vec![[1, 2], [3, 4], [5, 6]].into_iter().collect();
		let x: &[u8] = w.as_ref();
		assert_eq!(x, &[1, 2, 3, 4, 5, 6])
	}
}
