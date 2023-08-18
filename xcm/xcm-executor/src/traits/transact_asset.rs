// Copyright (C) Parity Technologies (UK) Ltd.
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

use crate::Assets;
use sp_std::result::Result;
use xcm::latest::{Error as XcmError, MultiAsset, MultiLocation, Result as XcmResult, XcmContext};

/// Facility for asset transacting.
///
/// This should work with as many asset/location combinations as possible. Locations to support may
/// include non-account locations such as a `MultiLocation::X1(Junction::Parachain)`. Different
/// chains may handle them in different ways.
///
/// Can be amalgamated as a tuple of items that implement this trait. In such executions, if any of
/// the transactors returns `Ok(())`, then it will short circuit. Else, execution is passed to the
/// next transactor.
pub trait TransactAsset {
	/// Ensure that `check_in` will do as expected.
	///
	/// When composed as a tuple, all type-items are called and at least one must result in `Ok`.
	fn can_check_in(
		_origin: &MultiLocation,
		_what: &MultiAsset,
		_context: &XcmContext,
	) -> XcmResult {
		Err(XcmError::Unimplemented)
	}

	/// An asset has been teleported in from the given origin. This should do whatever housekeeping
	/// is needed.
	///
	/// NOTE: This will make only a best-effort at bookkeeping. The caller should ensure that
	/// `can_check_in` has returned with `Ok` in order to guarantee that this operation proceeds
	/// properly.
	///
	/// Implementation note: In general this will do one of two things: On chains where the asset is
	/// native, it will reduce the assets from a special "teleported" account so that a)
	/// total-issuance is preserved; and b) to ensure that no more assets can be teleported in than
	/// were teleported out overall (this should not be needed if the teleporting chains are to be
	/// trusted, but better to be safe than sorry). On chains where the asset is not native then it
	/// will generally just be a no-op.
	///
	/// When composed as a tuple, all type-items are called. It is up to the implementer that there
	/// exists no value for `_what` which can cause side-effects for more than one of the
	/// type-items.
	fn check_in(_origin: &MultiLocation, _what: &MultiAsset, _context: &XcmContext) {}

	/// Ensure that `check_out` will do as expected.
	///
	/// When composed as a tuple, all type-items are called and at least one must result in `Ok`.
	fn can_check_out(
		_dest: &MultiLocation,
		_what: &MultiAsset,
		_context: &XcmContext,
	) -> XcmResult {
		Err(XcmError::Unimplemented)
	}

	/// An asset has been teleported out to the given destination. This should do whatever
	/// housekeeping is needed.
	///
	/// Implementation note: In general this will do one of two things: On chains where the asset is
	/// native, it will increase the assets in a special "teleported" account so that a)
	/// total-issuance is preserved; and b) to ensure that no more assets can be teleported in than
	/// were teleported out overall (this should not be needed if the teleporting chains are to be
	/// trusted, but better to be safe than sorry). On chains where the asset is not native then it
	/// will generally just be a no-op.
	///
	/// When composed as a tuple, all type-items are called. It is up to the implementer that there
	/// exists no value for `_what` which can cause side-effects for more than one of the
	/// type-items.
	fn check_out(_dest: &MultiLocation, _what: &MultiAsset, _context: &XcmContext) {}

	/// Deposit the `what` asset into the account of `who`.
	///
	/// Implementations should return `XcmError::FailedToTransactAsset` if deposit failed.
	fn deposit_asset(_what: &MultiAsset, _who: &MultiLocation, _context: &XcmContext) -> XcmResult {
		Err(XcmError::Unimplemented)
	}

	/// Withdraw the given asset from the consensus system. Return the actual asset(s) withdrawn,
	/// which should always be equal to `_what`.
	///
	/// The XCM `_maybe_context` parameter may be `None` when the caller of `withdraw_asset` is
	/// outside of the context of a currently-executing XCM. An example will be the `charge_fees`
	/// method in the XCM executor.
	///
	/// Implementations should return `XcmError::FailedToTransactAsset` if withdraw failed.
	fn withdraw_asset(
		_what: &MultiAsset,
		_who: &MultiLocation,
		_maybe_context: Option<&XcmContext>,
	) -> Result<Assets, XcmError> {
		Err(XcmError::Unimplemented)
	}

	/// Move an `asset` `from` one location in `to` another location.
	///
	/// Returns `XcmError::FailedToTransactAsset` if transfer failed.
	///
	/// ## Notes
	/// This function is meant to only be implemented by the type implementing `TransactAsset`, and
	/// not be called directly. Most common API usages will instead call `transfer_asset`, which in
	/// turn has a default implementation that calls `internal_transfer_asset`. As such, **please
	/// do not call this method directly unless you know what you're doing**.
	fn internal_transfer_asset(
		_asset: &MultiAsset,
		_from: &MultiLocation,
		_to: &MultiLocation,
		_context: &XcmContext,
	) -> Result<Assets, XcmError> {
		Err(XcmError::Unimplemented)
	}

	/// Move an `asset` `from` one location in `to` another location.
	///
	/// Attempts to use `internal_transfer_asset` and if not available then falls back to using a
	/// two-part withdraw/deposit.
	fn transfer_asset(
		asset: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
		context: &XcmContext,
	) -> Result<Assets, XcmError> {
		match Self::internal_transfer_asset(asset, from, to, context) {
			Err(XcmError::AssetNotFound | XcmError::Unimplemented) => {
				let assets = Self::withdraw_asset(asset, from, Some(context))?;
				// Not a very forgiving attitude; once we implement roll-backs then it'll be nicer.
				Self::deposit_asset(asset, to, context)?;
				Ok(assets)
			},
			result => result,
		}
	}
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl TransactAsset for Tuple {
	fn can_check_in(origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		for_tuples!( #(
			match Tuple::can_check_in(origin, what, context) {
				Err(XcmError::AssetNotFound) | Err(XcmError::Unimplemented) => (),
				r => return r,
			}
		)* );
		log::trace!(
			target: "xcm::TransactAsset::can_check_in",
			"asset not found: what: {:?}, origin: {:?}, context: {:?}",
			what,
			origin,
			context,
		);
		Err(XcmError::AssetNotFound)
	}

	fn check_in(origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		for_tuples!( #(
			Tuple::check_in(origin, what, context);
		)* );
	}

	fn can_check_out(dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		for_tuples!( #(
			match Tuple::can_check_out(dest, what, context) {
				Err(XcmError::AssetNotFound) | Err(XcmError::Unimplemented) => (),
				r => return r,
			}
		)* );
		log::trace!(
			target: "xcm::TransactAsset::can_check_out",
			"asset not found: what: {:?}, dest: {:?}, context: {:?}",
			what,
			dest,
			context,
		);
		Err(XcmError::AssetNotFound)
	}

	fn check_out(dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		for_tuples!( #(
			Tuple::check_out(dest, what, context);
		)* );
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation, context: &XcmContext) -> XcmResult {
		for_tuples!( #(
			match Tuple::deposit_asset(what, who, context) {
				Err(XcmError::AssetNotFound) | Err(XcmError::Unimplemented) => (),
				r => return r,
			}
		)* );
		log::trace!(
			target: "xcm::TransactAsset::deposit_asset",
			"did not deposit asset: what: {:?}, who: {:?}, context: {:?}",
			what,
			who,
			context,
		);
		Err(XcmError::AssetNotFound)
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
		maybe_context: Option<&XcmContext>,
	) -> Result<Assets, XcmError> {
		for_tuples!( #(
			match Tuple::withdraw_asset(what, who, maybe_context) {
				Err(XcmError::AssetNotFound) | Err(XcmError::Unimplemented) => (),
				r => return r,
			}
		)* );
		log::trace!(
			target: "xcm::TransactAsset::withdraw_asset",
			"did not withdraw asset: what: {:?}, who: {:?}, maybe_context: {:?}",
			what,
			who,
			maybe_context,
		);
		Err(XcmError::AssetNotFound)
	}

	fn internal_transfer_asset(
		what: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
		context: &XcmContext,
	) -> Result<Assets, XcmError> {
		for_tuples!( #(
			match Tuple::internal_transfer_asset(what, from, to, context) {
				Err(XcmError::AssetNotFound) | Err(XcmError::Unimplemented) => (),
				r => return r,
			}
		)* );
		log::trace!(
			target: "xcm::TransactAsset::internal_transfer_asset",
			"did not transfer asset: what: {:?}, from: {:?}, to: {:?}, context: {:?}",
			what,
			from,
			to,
			context,
		);
		Err(XcmError::AssetNotFound)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use xcm::latest::Junctions::Here;

	pub struct UnimplementedTransactor;
	impl TransactAsset for UnimplementedTransactor {}

	pub struct NotFoundTransactor;
	impl TransactAsset for NotFoundTransactor {
		fn can_check_in(
			_origin: &MultiLocation,
			_what: &MultiAsset,
			_context: &XcmContext,
		) -> XcmResult {
			Err(XcmError::AssetNotFound)
		}

		fn can_check_out(
			_dest: &MultiLocation,
			_what: &MultiAsset,
			_context: &XcmContext,
		) -> XcmResult {
			Err(XcmError::AssetNotFound)
		}

		fn deposit_asset(
			_what: &MultiAsset,
			_who: &MultiLocation,
			_context: &XcmContext,
		) -> XcmResult {
			Err(XcmError::AssetNotFound)
		}

		fn withdraw_asset(
			_what: &MultiAsset,
			_who: &MultiLocation,
			_context: Option<&XcmContext>,
		) -> Result<Assets, XcmError> {
			Err(XcmError::AssetNotFound)
		}

		fn internal_transfer_asset(
			_what: &MultiAsset,
			_from: &MultiLocation,
			_to: &MultiLocation,
			_context: &XcmContext,
		) -> Result<Assets, XcmError> {
			Err(XcmError::AssetNotFound)
		}
	}

	pub struct OverflowTransactor;
	impl TransactAsset for OverflowTransactor {
		fn can_check_in(
			_origin: &MultiLocation,
			_what: &MultiAsset,
			_context: &XcmContext,
		) -> XcmResult {
			Err(XcmError::Overflow)
		}

		fn can_check_out(
			_dest: &MultiLocation,
			_what: &MultiAsset,
			_context: &XcmContext,
		) -> XcmResult {
			Err(XcmError::Overflow)
		}

		fn deposit_asset(
			_what: &MultiAsset,
			_who: &MultiLocation,
			_context: &XcmContext,
		) -> XcmResult {
			Err(XcmError::Overflow)
		}

		fn withdraw_asset(
			_what: &MultiAsset,
			_who: &MultiLocation,
			_context: Option<&XcmContext>,
		) -> Result<Assets, XcmError> {
			Err(XcmError::Overflow)
		}

		fn internal_transfer_asset(
			_what: &MultiAsset,
			_from: &MultiLocation,
			_to: &MultiLocation,
			_context: &XcmContext,
		) -> Result<Assets, XcmError> {
			Err(XcmError::Overflow)
		}
	}

	pub struct SuccessfulTransactor;
	impl TransactAsset for SuccessfulTransactor {
		fn can_check_in(
			_origin: &MultiLocation,
			_what: &MultiAsset,
			_context: &XcmContext,
		) -> XcmResult {
			Ok(())
		}

		fn can_check_out(
			_dest: &MultiLocation,
			_what: &MultiAsset,
			_context: &XcmContext,
		) -> XcmResult {
			Ok(())
		}

		fn deposit_asset(
			_what: &MultiAsset,
			_who: &MultiLocation,
			_context: &XcmContext,
		) -> XcmResult {
			Ok(())
		}

		fn withdraw_asset(
			_what: &MultiAsset,
			_who: &MultiLocation,
			_context: Option<&XcmContext>,
		) -> Result<Assets, XcmError> {
			Ok(Assets::default())
		}

		fn internal_transfer_asset(
			_what: &MultiAsset,
			_from: &MultiLocation,
			_to: &MultiLocation,
			_context: &XcmContext,
		) -> Result<Assets, XcmError> {
			Ok(Assets::default())
		}
	}

	#[test]
	fn defaults_to_asset_not_found() {
		type MultiTransactor =
			(UnimplementedTransactor, NotFoundTransactor, UnimplementedTransactor);

		assert_eq!(
			MultiTransactor::deposit_asset(
				&(Here, 1u128).into(),
				&Here.into(),
				&XcmContext::with_message_id([0; 32]),
			),
			Err(XcmError::AssetNotFound)
		);
	}

	#[test]
	fn unimplemented_and_not_found_continue_iteration() {
		type MultiTransactor = (UnimplementedTransactor, NotFoundTransactor, SuccessfulTransactor);

		assert_eq!(
			MultiTransactor::deposit_asset(
				&(Here, 1u128).into(),
				&Here.into(),
				&XcmContext::with_message_id([0; 32]),
			),
			Ok(())
		);
	}

	#[test]
	fn unexpected_error_stops_iteration() {
		type MultiTransactor = (OverflowTransactor, SuccessfulTransactor);

		assert_eq!(
			MultiTransactor::deposit_asset(
				&(Here, 1u128).into(),
				&Here.into(),
				&XcmContext::with_message_id([0; 32]),
			),
			Err(XcmError::Overflow)
		);
	}

	#[test]
	fn success_stops_iteration() {
		type MultiTransactor = (SuccessfulTransactor, OverflowTransactor);

		assert_eq!(
			MultiTransactor::deposit_asset(
				&(Here, 1u128).into(),
				&Here.into(),
				&XcmContext::with_message_id([0; 32]),
			),
			Ok(()),
		);
	}
}
