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

use xcm::v0::{Response, MultiLocation};
use frame_support::weights::Weight;

/// Define what needs to be done upon receiving a query response.
pub trait OnResponse {
	/// Returns `true` if we are expecting a response from `origin` for query `query_id`.
	fn expecting_response(origin: &MultiLocation, query_id: u64) -> bool;
	/// Handler for receiving a `response` from `origin` relating to `query_id`.
	fn on_response(origin: MultiLocation, query_id: u64, response: Response) -> Weight;
}
impl OnResponse for () {
	fn expecting_response(_origin: &MultiLocation, _query_id: u64) -> bool { false }
	fn on_response(_origin: MultiLocation, _query_id: u64, _response: Response) -> Weight { 0 }
}
