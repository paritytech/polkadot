// Copyright 2021 Parity Technologies (UK) Ltd.
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

use polkadot_node_subsystem_util::metrics::{self, prometheus};
use std::convert::TryInto;

#[derive(Clone)]
struct MetricsInner {
	/// Number of votes.
	votes: prometheus::Counter<prometheus::U64>,
	/// Number of disputes.
	disputes: prometheus::Counter<prometheus::U64>,
}

/// Inherent data processing metrics for `create_inherent()`.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	pub fn inc_votes_by(&self, votes: usize) {
		if let Some(metrics) = &self.0 {
			metrics.votes.inc_by(votes.try_into().unwrap_or(0));
		}
	}
	pub fn inc_disputes_by(&self, disputes: usize) {
		if let Some(metrics) = &self.0 {
			metrics.disputes.inc_by(disputes.try_into().unwrap_or(0));
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			votes: prometheus::register(
				prometheus::Counter::new(
					"parachain_create_inherent_data_votes",
					"Number of dispute votes processed by create_inherent().",
				)?,
				&registry,
			)?,
			disputes: prometheus::register(
				prometheus::Counter::new(
					"parachain_create_inherent_data_disputes",
					"Number of disputes processed by create_inherent().",
				)?,
				&registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
