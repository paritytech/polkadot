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

use crate::Xcm;
use core::result;
use frame_support::pallet_prelude::{Get, TypeInfo};
use parity_scale_codec::{FullCodec, MaxEncodedLen};
use sp_arithmetic::traits::Zero;
use sp_std::fmt::Debug;
use xcm::latest::{
	Error as XcmError, InteriorMultiLocation, MultiLocation, QueryId, Response,
	Result as XcmResult, Weight, XcmContext,
};

/// Define what needs to be done upon receiving a query response.
pub trait OnResponse {
	/// Returns `true` if we are expecting a response from `origin` for query `query_id` that was
	/// queried by `querier`.
	fn expecting_response(
		origin: &MultiLocation,
		query_id: u64,
		querier: Option<&MultiLocation>,
	) -> bool;
	/// Handler for receiving a `response` from `origin` relating to `query_id` initiated by
	/// `querier`.
	fn on_response(
		origin: &MultiLocation,
		query_id: u64,
		querier: Option<&MultiLocation>,
		response: Response,
		max_weight: Weight,
		context: &XcmContext,
	) -> Weight;
}
impl OnResponse for () {
	fn expecting_response(
		_origin: &MultiLocation,
		_query_id: u64,
		_querier: Option<&MultiLocation>,
	) -> bool {
		false
	}
	fn on_response(
		_origin: &MultiLocation,
		_query_id: u64,
		_querier: Option<&MultiLocation>,
		_response: Response,
		_max_weight: Weight,
		_context: &XcmContext,
	) -> Weight {
		Weight::zero()
	}
}

/// Trait for a type which handles notifying a destination of XCM version changes.
pub trait VersionChangeNotifier {
	/// Start notifying `location` should the XCM version of this chain change.
	///
	/// When it does, this type should ensure a `QueryResponse` message is sent with the given
	/// `query_id` & `max_weight` and with a `response` of `Response::Version`. This should happen
	/// until/unless `stop` is called with the correct `query_id`.
	///
	/// If the `location` has an ongoing notification and when this function is called, then an
	/// error should be returned.
	fn start(
		location: &MultiLocation,
		query_id: QueryId,
		max_weight: Weight,
		context: &XcmContext,
	) -> XcmResult;

	/// Stop notifying `location` should the XCM change. Returns an error if there is no existing
	/// notification set up.
	fn stop(location: &MultiLocation, context: &XcmContext) -> XcmResult;

	/// Return true if a location is subscribed to XCM version changes.
	fn is_subscribed(location: &MultiLocation) -> bool;
}

impl VersionChangeNotifier for () {
	fn start(_: &MultiLocation, _: QueryId, _: Weight, _: &XcmContext) -> XcmResult {
		Err(XcmError::Unimplemented)
	}
	fn stop(_: &MultiLocation, _: &XcmContext) -> XcmResult {
		Err(XcmError::Unimplemented)
	}
	fn is_subscribed(_: &MultiLocation) -> bool {
		false
	}
}

/// The possible state of an XCM query response.
#[derive(Debug, PartialEq, Eq)]
pub enum QueryResponseStatus<BlockNumber> {
	/// The response has arrived, and includes the inner Response and the block number it arrived
	/// at.
	Ready { response: Response, at: BlockNumber },
	/// The response has not yet arrived, the XCM might still be executing or the response might be
	/// in transit.
	Pending { timeout: BlockNumber },
	/// No response with the given `QueryId` was found, or the response was already queried and
	/// removed from local storage.
	NotFound,
	/// Got an unexpected XCM version.
	UnexpectedVersion,
}

/// Provides methods to expect responses from XCMs and query their status.
pub trait QueryHandler {
	type QueryId: From<u64>
		+ FullCodec
		+ MaxEncodedLen
		+ TypeInfo
		+ Clone
		+ Eq
		+ PartialEq
		+ Debug
		+ Copy;
	type BlockNumber: Zero;
	type Error;
	type UniversalLocation: Get<InteriorMultiLocation>;

	/// Attempt to create a new query ID and register it as a query that is yet to respond.
	fn new_query(
		responder: impl Into<MultiLocation>,
		timeout: Self::BlockNumber,
		match_querier: impl Into<MultiLocation>,
	) -> QueryId;

	/// Consume `message` and return another which is equivalent to it except that it reports
	/// back the outcome.
	///
	/// - `message`: The message whose outcome should be reported.
	/// - `responder`: The origin from which a response should be expected.
	/// - `timeout`: The block number after which it is permissible to return `NotFound` from
	///   `take_response`.
	///
	/// `report_outcome` may return an error if the `responder` is not invertible.
	///
	/// It is assumed that the querier of the response will be `Here`.
	/// The response can be queried with `take_response`.
	fn report_outcome(
		message: &mut Xcm<()>,
		responder: impl Into<MultiLocation>,
		timeout: Self::BlockNumber,
	) -> result::Result<Self::QueryId, Self::Error>;

	/// Attempt to remove and return the response of query with ID `query_id`.
	fn take_response(id: Self::QueryId) -> QueryResponseStatus<Self::BlockNumber>;

	/// Makes sure to expect a response with the given id.
	#[cfg(feature = "runtime-benchmarks")]
	fn expect_response(id: Self::QueryId, response: Response);
}
