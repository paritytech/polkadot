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

use sp_runtime::traits::Zero;
use xcm::latest::{
	Error as XcmError, MultiLocation, QueryId, Response, Result as XcmResult, Weight,
};

/// Define what needs to be done upon receiving a query response.
pub trait OnResponse {
	/// Returns `true` if we are expecting a response from `origin` for query `query_id`.
	fn expecting_response(origin: &MultiLocation, query_id: u64) -> bool;
	/// Handler for receiving a `response` from `origin` relating to `query_id`.
	fn on_response(
		origin: &MultiLocation,
		query_id: u64,
		response: Response,
		max_weight: Weight,
	) -> Weight;
}
impl OnResponse for () {
	fn expecting_response(_origin: &MultiLocation, _query_id: u64) -> bool {
		false
	}
	fn on_response(
		_origin: &MultiLocation,
		_query_id: u64,
		_response: Response,
		_max_weight: Weight,
	) -> Weight {
		Weight::zero()
	}
}

/// Trait for a type which handles notifying a destination of XCM version changes.
pub trait VersionChangeNotifier {
	/// Start notifying `location` should the XCM version of this chain change.
	///
	/// When it does, this type should ensure a `QueryResponse` message is sent with the given
	/// `query_id` & `max_weight` and with a `response` of `Repsonse::Version`. This should happen
	/// until/unless `stop` is called with the correct `query_id`.
	///
	/// If the `location` has an ongoing notification and when this function is called, then an
	/// error should be returned.
	fn start(location: &MultiLocation, query_id: QueryId, max_weight: u64) -> XcmResult;

	/// Stop notifying `location` should the XCM change. Returns an error if there is no existing
	/// notification set up.
	fn stop(location: &MultiLocation) -> XcmResult;

	/// Return true if a location is subscribed to XCM version changes.
	fn is_subscribed(location: &MultiLocation) -> bool;
}

impl VersionChangeNotifier for () {
	fn start(_: &MultiLocation, _: QueryId, _: u64) -> XcmResult {
		Err(XcmError::Unimplemented)
	}
	fn stop(_: &MultiLocation) -> XcmResult {
		Err(XcmError::Unimplemented)
	}
	fn is_subscribed(_: &MultiLocation) -> bool {
		false
	}
}
