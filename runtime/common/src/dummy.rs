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

//! A dummy module for holding place of modules in a runtime.

use frame_support::{decl_module, decl_storage};
use codec::{Encode, Decode};

pub trait Trait<I: Instance = DefaultInstance>: system::Trait { }

/// Workaround for the fact that the instanced `Call`s need not to have
/// conflicting implementations.
///
/// The `Call` needs to reference the instance type parameter somehow in order to not
/// conflict.
#[derive(Encode, Decode)]
pub struct ZeroSizedTypeDifferentiator<I>(sp_std::marker::PhantomData<I>);

impl<I> Clone for ZeroSizedTypeDifferentiator<I> {
	fn clone(&self) -> Self {
		ZeroSizedTypeDifferentiator(sp_std::marker::PhantomData)
	}
}

impl<I> PartialEq for ZeroSizedTypeDifferentiator<I> {
	fn eq(&self, _other: &Self) -> bool { true }
}

impl<I> Eq for ZeroSizedTypeDifferentiator<I> { }

#[cfg(feature = "std")]
impl<I> std::fmt::Debug for ZeroSizedTypeDifferentiator<I> {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "ZeroSizedTypeDifferentiator")
	}
}

#[cfg(not(feature = "std"))]
impl<I> core::fmt::Debug for ZeroSizedTypeDifferentiator<I> {
	fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
		write!(f, "<wasm:stripped>")
	}
}

decl_module! {
	pub struct Module<T: Trait<I>, I: Instance = DefaultInstance> for enum Call where origin: T::Origin {
		#[weight = 0]
		fn dummy_call_invalid(origin, _x: ZeroSizedTypeDifferentiator<I>) {
		}
	}
}

decl_storage! {
	trait Store for Module<T: Trait<I>, I: Instance = DefaultInstance> as Dummy { }
}
