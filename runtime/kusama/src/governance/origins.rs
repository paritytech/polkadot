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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! Custom origins for governance interventions.

pub use pallet_custom_origins::*;

#[frame_support::pallet]
pub mod pallet_custom_origins {
	use frame_support::pallet_prelude::*;
	use crate::{Balance, QUID, GRAND};

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo)]
	#[cfg_attr(feature = "std", derive(Debug))]
	#[pallet::origin]
	pub enum Origin {
		/// Origin for cancelling slashes.
		StakingAdmin,
		/// Origin for spending (any amount of) funds.
		Treasurer,
		/// Origin for managing the composition of the fellowship.
		FellowshipAdmin,
		/// Origin for managing the registrar.
		GeneralAdmin,
		/// Origin for starting auctions.
		AuctionAdmin,
		/// Origin able to force slot leases.
		LeaseAdmin,
		/// Origin able to cancel referenda.
		ReferendumCanceller,
		/// Origin able to kill referenda.
		ReferendumKiller,
		/// Origin able to spend up to 1 KSM from the treasury at once.
		SmallTipper,
		/// Origin able to spend up to 5 KSM from the treasury at once.
		BigTipper,
		/// Origin able to spend up to 50 KSM from the treasury at once.
		SmallSpender,
		/// Origin able to spend up to 500 KSM from the treasury at once.
		MediumSpender,
		/// Origin able to spend up to 5,000 KSM from the treasury at once.
		BigSpender,
		/// Origin able to dispatch a whitelisted call.
		WhitelistedCaller,
		/// Origin commanded by the initiates of the Polkadot Fellowship.
		FellowshipInitiates,
		/// Origin commanded by the apprentices of the Polkadot Fellowship.
		FellowshipApprentices,
		/// Origin commanded by Polkadot Fellows.
		Fellows,
		/// Origin commanded by the Masters of the Polkadot Fellowship.
		FellowshipMasters,
		/// Origin commanded by the Elites of the Polkadot Fellowship.
		FellowshipElites,
	}

	macro_rules! decl_ensure {
		( $name:ident: $success_type:ty = $success:expr ) => {
			pub struct $name<AccountId>(sp_std::marker::PhantomData<AccountId>);
			impl<O: Into<Result<Origin, O>> + From<Origin>, AccountId>
				EnsureOrigin<O> for $name<AccountId>
			{
				type Success = $success_type;
				fn try_origin(o: O) -> Result<Self::Success, O> {
					o.into().and_then(|o| match o {
						Origin::$name => Ok($success),
						r => Err(O::from(r)),
					})
				}
				#[cfg(feature = "runtime-benchmarks")]
				fn try_successful_origin() -> Result<O, ()> {
					Ok(O::from(Origin::$name))
				}
			}
		};
		( $name:ident ) => { decl_ensure! { $name : () = () } };
		( $name:ident: $success_type:ty = $success:expr, $( $rest:tt )* ) => {
			decl_ensure! { $name: $success_type = $success }
			decl_ensure! { $( $rest )* }
		};
		( $name:ident, $( $rest:tt )* ) => {
			decl_ensure! { $name }
			decl_ensure! { $( $rest )* }
		};
		() => {}
	}
	decl_ensure!(
		StakingAdmin,
		Treasurer,
		FellowshipAdmin,
		GeneralAdmin,
		AuctionAdmin,
		LeaseAdmin,
		ReferendumCanceller,
		ReferendumKiller,
		SmallTipper: Balance = 250 * QUID,
		BigTipper: Balance = 1 * GRAND,
		SmallSpender: Balance = 10 * GRAND,
		MediumSpender: Balance = 100 * GRAND,
		BigSpender: Balance = 1_000 * GRAND,
		WhitelistedCaller,
		FellowshipInitiates,
		FellowshipApprentices,
		Fellows,
		FellowshipMasters,
		FellowshipElites,
	);
}