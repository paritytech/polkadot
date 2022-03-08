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

use crate::{chain::Chain, client::Client};

use async_std::sync::{Arc, RwLock};
use async_trait::async_trait;
use codec::Decode;
use relay_utils::metrics::{
	metric_name, register, F64SharedRef, Gauge, Metric, PrometheusError, Registry,
	StandaloneMetric, F64,
};
use sp_core::storage::StorageKey;
use sp_runtime::{traits::UniqueSaturatedInto, FixedPointNumber};
use std::time::Duration;

/// Storage value update interval (in blocks).
const UPDATE_INTERVAL_IN_BLOCKS: u32 = 5;

/// Metric that represents fixed-point runtime storage value as float gauge.
#[derive(Clone, Debug)]
pub struct FloatStorageValueMetric<C: Chain, T: Clone> {
	client: Client<C>,
	storage_key: StorageKey,
	maybe_default_value: Option<T>,
	metric: Gauge<F64>,
	shared_value_ref: F64SharedRef,
}

impl<C: Chain, T: Decode + FixedPointNumber> FloatStorageValueMetric<C, T> {
	/// Create new metric.
	pub fn new(
		client: Client<C>,
		storage_key: StorageKey,
		maybe_default_value: Option<T>,
		name: String,
		help: String,
	) -> Result<Self, PrometheusError> {
		let shared_value_ref = Arc::new(RwLock::new(None));
		Ok(FloatStorageValueMetric {
			client,
			storage_key,
			maybe_default_value,
			metric: Gauge::new(metric_name(None, &name), help)?,
			shared_value_ref,
		})
	}

	/// Get shared reference to metric value.
	pub fn shared_value_ref(&self) -> F64SharedRef {
		self.shared_value_ref.clone()
	}
}

impl<C: Chain, T> Metric for FloatStorageValueMetric<C, T>
where
	T: 'static + Decode + Send + Sync + FixedPointNumber,
{
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError> {
		register(self.metric.clone(), registry).map(drop)
	}
}

#[async_trait]
impl<C: Chain, T> StandaloneMetric for FloatStorageValueMetric<C, T>
where
	T: 'static + Decode + Send + Sync + FixedPointNumber,
{
	fn update_interval(&self) -> Duration {
		C::AVERAGE_BLOCK_INTERVAL * UPDATE_INTERVAL_IN_BLOCKS
	}

	async fn update(&self) {
		let value = self
			.client
			.storage_value::<T>(self.storage_key.clone(), None)
			.await
			.map(|maybe_storage_value| {
				maybe_storage_value.or(self.maybe_default_value).map(|storage_value| {
					storage_value.into_inner().unique_saturated_into() as f64 /
						T::DIV.unique_saturated_into() as f64
				})
			})
			.map_err(drop);
		relay_utils::metrics::set_gauge_value(&self.metric, value);
		*self.shared_value_ref.write().await = value.ok().and_then(|x| x);
	}
}
