// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! Adapter for using `enum RelayerMode` in a context which requires `RelayStrategy`.

use async_trait::async_trait;

use crate::{
	message_lane::MessageLane,
	message_lane_loop::{
		RelayerMode, SourceClient as MessageLaneSourceClient,
		TargetClient as MessageLaneTargetClient,
	},
	relay_strategy::{AltruisticStrategy, RationalStrategy, RelayReference, RelayStrategy},
};

/// `RelayerMode` adapter.
#[derive(Clone)]
pub struct MixStrategy {
	relayer_mode: RelayerMode,
}

impl MixStrategy {
	/// Create mix strategy instance
	pub fn new(relayer_mode: RelayerMode) -> Self {
		Self { relayer_mode }
	}
}

#[async_trait]
impl RelayStrategy for MixStrategy {
	async fn decide<
		P: MessageLane,
		SourceClient: MessageLaneSourceClient<P>,
		TargetClient: MessageLaneTargetClient<P>,
	>(
		&mut self,
		reference: &mut RelayReference<P, SourceClient, TargetClient>,
	) -> bool {
		match self.relayer_mode {
			RelayerMode::Altruistic => AltruisticStrategy.decide(reference).await,
			RelayerMode::Rational => RationalStrategy.decide(reference).await,
		}
	}
}
