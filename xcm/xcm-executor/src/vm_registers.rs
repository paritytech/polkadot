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

use super::{Assets, FeesMode};
use frame_support::ensure;
use sp_weights::Weight;
use xcm::latest::prelude::*;

/// Regsiters of the XCVM
pub struct XcVmRegisters<Call> {
	pub holding: Assets,
	holding_limit: usize,
	context: XcmContext,
	original_origin: MultiLocation,
	/// The most recent error result and instruction index into the fragment in which it occurred,
	/// if any.
	error: Option<(u32, XcmError)>,
	/// The surplus weight, defined as the amount by which `max_weight` is
	/// an over-estimate of the actual weight consumed. We do it this way to avoid needing the
	/// execution engine to keep track of all instructions' weights (it only needs to care about
	/// the weight of dynamically determined instructions such as `Transact`).
	pub total_surplus: Weight,
	pub total_refunded: Weight,
	pub error_handler: Xcm<Call>,
	pub error_handler_weight: Weight,
	pub appendix: Xcm<Call>,
	pub appendix_weight: Weight,
	pub transact_status: MaybeErrorCode,
	pub fees_mode: FeesMode,
}

pub(super) struct XcVmFinalState {
	pub context: XcmContext,
	pub error: Option<(u32, XcmError)>,
	pub holding: Assets,
	pub original_origin: MultiLocation,
	pub total_surplus: Weight,
}

#[cfg(feature = "runtime-benchmarks")]
impl<Call> XcVmRegisters<Call> {
	pub fn holding_limit(&self) -> &usize {
		&self.holding_limit
	}
	pub fn set_error(&mut self, v: Option<(u32, XcmError)>) {
		self.state.error = v;
	}
	pub fn set_original_origin(&mut self, v: MultiLocation) {
		self.original_origin = v
	}
	pub fn set_holding_limit(&mut self, v: usize) {
		self.holding_limit = v
	}
}

impl<Call> XcVmRegisters<Call> {
	pub fn context(&self) -> &XcmContext {
		&self.context
	}
	pub fn origin_ref(&self) -> Option<&MultiLocation> {
		self.context.origin.as_ref()
	}
	pub fn original_origin(&self) -> &MultiLocation {
		&self.original_origin
	}
	pub fn cloned_origin(&self) -> Option<MultiLocation> {
		self.context.origin
	}
	pub fn clear_error(&mut self) {
		self.error = None;
	}
	pub fn get_error(&self) -> Option<(u32, XcmError)> {
		self.error
	}
	pub fn set_origin(&mut self, origin: Option<MultiLocation>) {
		self.context.origin = origin;
	}
	pub fn set_topic(&mut self, topic: Option<[u8; 32]>) {
		self.context.topic = topic;
	}
	pub fn subsume_asset(&mut self, asset: MultiAsset) -> Result<(), XcmError> {
		// worst-case, holding.len becomes 2 * holding_limit.
		ensure!(self.holding.len() < self.holding_limit * 2, XcmError::HoldingWouldOverflow);
		self.holding.subsume(asset);
		Ok(())
	}

	pub fn subsume_assets(&mut self, assets: Assets) -> Result<(), XcmError> {
		// worst-case, holding.len becomes 2 * holding_limit.
		// this guarantees that if holding.len() == holding_limit and you have holding_limit more
		// items (which has a best case outcome of holding.len() == holding_limit), then you'll
		// be guaranteed of making the operation.
		let worst_case_holding_len = self.holding.len() + assets.len();
		ensure!(worst_case_holding_len <= self.holding_limit * 2, XcmError::HoldingWouldOverflow);
		self.holding.subsume_assets(assets);
		Ok(())
	}

	pub(super) fn put_error(&mut self, index: u32, xcm_error: XcmError) {
		self.error = Some((index, xcm_error));
	}

	pub(super) fn destruct(self) -> XcVmFinalState {
		XcVmFinalState {
			context: self.context,
			error: self.error,
			holding: self.holding,
			original_origin: self.original_origin,
			total_surplus: self.total_surplus,
		}
	}
}

impl<Call> XcVmRegisters<Call> {
	pub fn new(
		holding_limit: usize,
		origin: impl Into<MultiLocation>,
		message_hash: XcmHash,
	) -> Self {
		let origin = origin.into();
		Self {
			holding: Assets::new(),
			holding_limit,
			context: XcmContext { origin: Some(origin), message_hash, topic: None },
			original_origin: origin,
			error: None,
			total_surplus: Weight::zero(),
			total_refunded: Weight::zero(),
			error_handler: Xcm(sp_std::vec![]),
			error_handler_weight: Weight::zero(),
			appendix: Xcm(sp_std::vec![]),
			appendix_weight: Weight::zero(),
			transact_status: Default::default(),
			fees_mode: FeesMode { jit_withdraw: false },
		}
	}
}
