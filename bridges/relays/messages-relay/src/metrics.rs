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

//! Metrics for message lane relay loop.

use crate::message_lane::MessageLane;
use crate::message_lane_loop::{SourceClientState, TargetClientState};

use bp_message_lane::MessageNonce;
use relay_utils::metrics::{register, GaugeVec, Metrics, Opts, Registry, U64};

/// Message lane relay metrics.
///
/// Cloning only clones references.
#[derive(Clone)]
pub struct MessageLaneLoopMetrics {
	/// Best finalized block numbers - "source", "target", "source_at_target", "target_at_source".
	best_block_numbers: GaugeVec<U64>,
	/// Lane state nonces: "source_latest_generated", "source_latest_confirmed",
	/// "target_latest_received", "target_latest_confirmed".
	lane_state_nonces: GaugeVec<U64>,
}

impl Metrics for MessageLaneLoopMetrics {
	fn register(&self, registry: &Registry) -> Result<(), String> {
		register(self.best_block_numbers.clone(), registry).map_err(|e| e.to_string())?;
		register(self.lane_state_nonces.clone(), registry).map_err(|e| e.to_string())?;
		Ok(())
	}
}

impl Default for MessageLaneLoopMetrics {
	fn default() -> Self {
		MessageLaneLoopMetrics {
			best_block_numbers: GaugeVec::new(
				Opts::new("best_block_numbers", "Best finalized block numbers"),
				&["type"],
			)
			.expect("metric is static and thus valid; qed"),
			lane_state_nonces: GaugeVec::new(Opts::new("lane_state_nonces", "Nonces of the lane state"), &["type"])
				.expect("metric is static and thus valid; qed"),
		}
	}
}

impl MessageLaneLoopMetrics {
	/// Update source client state metrics.
	pub fn update_source_state<P: MessageLane>(&self, source_client_state: SourceClientState<P>) {
		self.best_block_numbers
			.with_label_values(&["source"])
			.set(source_client_state.best_self.0.into());
		self.best_block_numbers
			.with_label_values(&["target_at_source"])
			.set(source_client_state.best_finalized_peer_at_best_self.0.into());
	}

	/// Update target client state metrics.
	pub fn update_target_state<P: MessageLane>(&self, target_client_state: TargetClientState<P>) {
		self.best_block_numbers
			.with_label_values(&["target"])
			.set(target_client_state.best_self.0.into());
		self.best_block_numbers
			.with_label_values(&["source_at_target"])
			.set(target_client_state.best_finalized_peer_at_best_self.0.into());
	}

	/// Update latest generated nonce at source.
	pub fn update_source_latest_generated_nonce<P: MessageLane>(&self, source_latest_generated_nonce: MessageNonce) {
		self.lane_state_nonces
			.with_label_values(&["source_latest_generated"])
			.set(source_latest_generated_nonce);
	}

	/// Update latest confirmed nonce at source.
	pub fn update_source_latest_confirmed_nonce<P: MessageLane>(&self, source_latest_confirmed_nonce: MessageNonce) {
		self.lane_state_nonces
			.with_label_values(&["source_latest_confirmed"])
			.set(source_latest_confirmed_nonce);
	}

	/// Update latest received nonce at target.
	pub fn update_target_latest_received_nonce<P: MessageLane>(&self, target_latest_generated_nonce: MessageNonce) {
		self.lane_state_nonces
			.with_label_values(&["target_latest_received"])
			.set(target_latest_generated_nonce);
	}

	/// Update latest confirmed nonce at target.
	pub fn update_target_latest_confirmed_nonce<P: MessageLane>(&self, target_latest_confirmed_nonce: MessageNonce) {
		self.lane_state_nonces
			.with_label_values(&["target_latest_confirmed"])
			.set(target_latest_confirmed_nonce);
	}
}
