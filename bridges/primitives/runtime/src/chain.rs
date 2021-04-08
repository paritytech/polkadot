// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use frame_support::Parameter;
use num_traits::AsPrimitive;
use sp_runtime::traits::{
	AtLeast32BitUnsigned, Hash as HashT, Header as HeaderT, MaybeDisplay, MaybeMallocSizeOf, MaybeSerializeDeserialize,
	Member, SimpleBitOps,
};
use sp_std::str::FromStr;

/// Minimal Substrate-based chain representation that may be used from no_std environment.
pub trait Chain: Send + Sync + 'static {
	/// A type that fulfills the abstract idea of what a Substrate block number is.
	// Constraits come from the associated Number type of `sp_runtime::traits::Header`
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html#associatedtype.Number
	//
	// Note that the `AsPrimitive<usize>` trait is required by the GRANDPA justification
	// verifier, and is not usually part of a Substrate Header's Number type.
	type BlockNumber: Parameter
		+ Member
		+ MaybeSerializeDeserialize
		+ sp_std::hash::Hash
		+ Copy
		+ Default
		+ MaybeDisplay
		+ AtLeast32BitUnsigned
		+ FromStr
		+ MaybeMallocSizeOf
		+ AsPrimitive<usize>
		+ Default;

	/// A type that fulfills the abstract idea of what a Substrate hash is.
	// Constraits come from the associated Hash type of `sp_runtime::traits::Header`
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html#associatedtype.Hash
	type Hash: Parameter
		+ Member
		+ MaybeSerializeDeserialize
		+ sp_std::hash::Hash
		+ Ord
		+ Copy
		+ MaybeDisplay
		+ Default
		+ SimpleBitOps
		+ AsRef<[u8]>
		+ AsMut<[u8]>
		+ MaybeMallocSizeOf;

	/// A type that fulfills the abstract idea of what a Substrate hasher (a type
	/// that produces hashes) is.
	// Constraits come from the associated Hashing type of `sp_runtime::traits::Header`
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html#associatedtype.Hashing
	type Hasher: HashT<Output = Self::Hash>;

	/// A type that fulfills the abstract idea of what a Substrate header is.
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html
	type Header: Parameter + HeaderT<Number = Self::BlockNumber, Hash = Self::Hash> + MaybeSerializeDeserialize;
}

/// Block number used by the chain.
pub type BlockNumberOf<C> = <C as Chain>::BlockNumber;

/// Hash type used by the chain.
pub type HashOf<C> = <C as Chain>::Hash;

/// Hasher type used by the chain.
pub type HasherOf<C> = <C as Chain>::Hasher;

/// Header type used by the chain.
pub type HeaderOf<C> = <C as Chain>::Header;
