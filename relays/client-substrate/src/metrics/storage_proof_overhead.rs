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

use crate::{chain::Chain, client::Client, error::Error};

use async_trait::async_trait;
use relay_utils::metrics::{
	metric_name, register, Gauge, Metric, PrometheusError, Registry, StandaloneMetric, U64,
};
use sp_core::storage::StorageKey;
use sp_runtime::traits::Header as HeaderT;
use sp_storage::well_known_keys::CODE;
use std::time::Duration;

/// Storage proof overhead update interval (in blocks).
const UPDATE_INTERVAL_IN_BLOCKS: u32 = 100;

/// Metric that represents extra size of storage proof as unsigned integer gauge.
///
/// There's one thing to keep in mind when using this metric: the overhead may be slightly
/// different for other values, but this metric gives a good estimation.
#[derive(Debug)]
pub struct StorageProofOverheadMetric<C: Chain> {
	client: Client<C>,
	metric: Gauge<U64>,
}

impl<C: Chain> Clone for StorageProofOverheadMetric<C> {
	fn clone(&self) -> Self {
		StorageProofOverheadMetric { client: self.client.clone(), metric: self.metric.clone() }
	}
}

impl<C: Chain> StorageProofOverheadMetric<C> {
	/// Create new metric instance with given name and help.
	pub fn new(client: Client<C>, name: String, help: String) -> Result<Self, PrometheusError> {
		Ok(StorageProofOverheadMetric {
			client,
			metric: Gauge::new(metric_name(None, &name), help)?,
		})
	}

	/// Returns approximate storage proof size overhead.
	async fn compute_storage_proof_overhead(&self) -> Result<usize, Error> {
		let best_header_hash = self.client.best_finalized_header_hash().await?;
		let best_header = self.client.header_by_hash(best_header_hash).await?;

		let storage_proof = self
			.client
			.prove_storage(vec![StorageKey(CODE.to_vec())], best_header_hash)
			.await?;
		let storage_proof_size: usize = storage_proof.clone().iter_nodes().map(|n| n.len()).sum();

		let storage_value_reader = bp_runtime::StorageProofChecker::<C::Hasher>::new(
			*best_header.state_root(),
			storage_proof,
		)
		.map_err(Error::StorageProofError)?;
		let maybe_encoded_storage_value =
			storage_value_reader.read_value(CODE).map_err(Error::StorageProofError)?;
		let encoded_storage_value_size =
			maybe_encoded_storage_value.ok_or(Error::MissingMandatoryCodeEntry)?.len();

		Ok(storage_proof_size - encoded_storage_value_size)
	}
}

impl<C: Chain> Metric for StorageProofOverheadMetric<C> {
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError> {
		register(self.metric.clone(), registry).map(drop)
	}
}

#[async_trait]
impl<C: Chain> StandaloneMetric for StorageProofOverheadMetric<C> {
	fn update_interval(&self) -> Duration {
		C::AVERAGE_BLOCK_INTERVAL * UPDATE_INTERVAL_IN_BLOCKS
	}

	async fn update(&self) {
		relay_utils::metrics::set_gauge_value(
			&self.metric,
			self.compute_storage_proof_overhead()
				.await
				.map(|overhead| Some(overhead as u64)),
		);
	}
}
