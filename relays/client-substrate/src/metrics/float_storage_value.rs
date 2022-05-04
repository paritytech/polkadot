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

use crate::{chain::Chain, client::Client, Error as SubstrateError};

use async_std::sync::{Arc, RwLock};
use async_trait::async_trait;
use codec::Decode;
use num_traits::One;
use relay_utils::metrics::{
	metric_name, register, F64SharedRef, Gauge, Metric, PrometheusError, Registry,
	StandaloneMetric, F64,
};
use sp_core::storage::{StorageData, StorageKey};
use sp_runtime::{traits::UniqueSaturatedInto, FixedPointNumber, FixedU128};
use std::{marker::PhantomData, time::Duration};

/// Storage value update interval (in blocks).
const UPDATE_INTERVAL_IN_BLOCKS: u32 = 5;

/// Fied-point storage value and the way it is decoded from the raw storage value.
pub trait FloatStorageValue: 'static + Clone + Send + Sync {
	/// Type of the value.
	type Value: FixedPointNumber;
	/// Try to decode value from the raw storage value.
	fn decode(
		&self,
		maybe_raw_value: Option<StorageData>,
	) -> Result<Option<Self::Value>, SubstrateError>;
}

/// Implementation of `FloatStorageValue` that expects encoded `FixedU128` value and returns `1` if
/// value is missing from the storage.
#[derive(Clone, Debug, Default)]
pub struct FixedU128OrOne;

impl FloatStorageValue for FixedU128OrOne {
	type Value = FixedU128;

	fn decode(
		&self,
		maybe_raw_value: Option<StorageData>,
	) -> Result<Option<Self::Value>, SubstrateError> {
		maybe_raw_value
			.map(|raw_value| {
				FixedU128::decode(&mut &raw_value.0[..])
					.map_err(SubstrateError::ResponseParseFailed)
					.map(Some)
			})
			.unwrap_or_else(|| Ok(Some(FixedU128::one())))
	}
}

/// Metric that represents fixed-point runtime storage value as float gauge.
#[derive(Clone, Debug)]
pub struct FloatStorageValueMetric<C: Chain, V: FloatStorageValue> {
	value_converter: V,
	client: Client<C>,
	storage_key: StorageKey,
	metric: Gauge<F64>,
	shared_value_ref: F64SharedRef,
	_phantom: PhantomData<V>,
}

impl<C: Chain, V: FloatStorageValue> FloatStorageValueMetric<C, V> {
	/// Create new metric.
	pub fn new(
		value_converter: V,
		client: Client<C>,
		storage_key: StorageKey,
		name: String,
		help: String,
	) -> Result<Self, PrometheusError> {
		let shared_value_ref = Arc::new(RwLock::new(None));
		Ok(FloatStorageValueMetric {
			value_converter,
			client,
			storage_key,
			metric: Gauge::new(metric_name(None, &name), help)?,
			shared_value_ref,
			_phantom: Default::default(),
		})
	}

	/// Get shared reference to metric value.
	pub fn shared_value_ref(&self) -> F64SharedRef {
		self.shared_value_ref.clone()
	}
}

impl<C: Chain, V: FloatStorageValue> Metric for FloatStorageValueMetric<C, V> {
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError> {
		register(self.metric.clone(), registry).map(drop)
	}
}

#[async_trait]
impl<C: Chain, V: FloatStorageValue> StandaloneMetric for FloatStorageValueMetric<C, V> {
	fn update_interval(&self) -> Duration {
		C::AVERAGE_BLOCK_INTERVAL * UPDATE_INTERVAL_IN_BLOCKS
	}

	async fn update(&self) {
		let value = self
			.client
			.raw_storage_value(self.storage_key.clone(), None)
			.await
			.and_then(|maybe_storage_value| {
				self.value_converter.decode(maybe_storage_value).map(|maybe_fixed_point_value| {
					maybe_fixed_point_value.map(|fixed_point_value| {
						fixed_point_value.into_inner().unique_saturated_into() as f64 /
							V::Value::DIV.unique_saturated_into() as f64
					})
				})
			})
			.map_err(|e| e.to_string());
		relay_utils::metrics::set_gauge_value(&self.metric, value.clone());
		*self.shared_value_ref.write().await = value.ok().and_then(|x| x);
	}
}
