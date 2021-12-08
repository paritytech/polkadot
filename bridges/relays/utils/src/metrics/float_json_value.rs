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

use crate::{
	error::{self, Error},
	metrics::{
		metric_name, register, F64SharedRef, Gauge, Metric, PrometheusError, Registry,
		StandaloneMetric, F64,
	},
};

use async_std::sync::{Arc, RwLock};
use async_trait::async_trait;
use std::time::Duration;

/// Value update interval.
const UPDATE_INTERVAL: Duration = Duration::from_secs(60);

/// Metric that represents float value received from HTTP service as float gauge.
///
/// The float value returned by the service is assumed to be normal (`f64::is_normal`
/// should return `true`) and strictly positive.
#[derive(Debug, Clone)]
pub struct FloatJsonValueMetric {
	url: String,
	json_path: String,
	metric: Gauge<F64>,
	shared_value_ref: F64SharedRef,
}

impl FloatJsonValueMetric {
	/// Create new metric instance with given name and help.
	pub fn new(
		url: String,
		json_path: String,
		name: String,
		help: String,
	) -> Result<Self, PrometheusError> {
		let shared_value_ref = Arc::new(RwLock::new(None));
		Ok(FloatJsonValueMetric {
			url,
			json_path,
			metric: Gauge::new(metric_name(None, &name), help)?,
			shared_value_ref,
		})
	}

	/// Get shared reference to metric value.
	pub fn shared_value_ref(&self) -> F64SharedRef {
		self.shared_value_ref.clone()
	}

	/// Request value from HTTP service.
	async fn request_value(&self) -> anyhow::Result<String> {
		use isahc::{AsyncReadResponseExt, HttpClient, Request};

		let request = Request::get(&self.url).header("Accept", "application/json").body(())?;
		let raw_response = HttpClient::new()?.send_async(request).await?.text().await?;
		Ok(raw_response)
	}

	/// Read value from HTTP service.
	async fn read_value(&self) -> error::Result<f64> {
		let raw_response = self.request_value().await.map_err(Error::FetchTokenPrice)?;
		parse_service_response(&self.json_path, &raw_response)
	}
}

impl Metric for FloatJsonValueMetric {
	fn register(&self, registry: &Registry) -> Result<(), PrometheusError> {
		register(self.metric.clone(), registry).map(drop)
	}
}

#[async_trait]
impl StandaloneMetric for FloatJsonValueMetric {
	fn update_interval(&self) -> Duration {
		UPDATE_INTERVAL
	}

	async fn update(&self) {
		let value = self.read_value().await;
		let maybe_ok = value.as_ref().ok().copied();
		crate::metrics::set_gauge_value(&self.metric, value.map(Some));
		*self.shared_value_ref.write().await = maybe_ok;
	}
}

/// Parse HTTP service response.
fn parse_service_response(json_path: &str, response: &str) -> error::Result<f64> {
	let json =
		serde_json::from_str(response).map_err(|err| Error::ParseHttp(err, response.to_owned()))?;

	let mut selector = jsonpath_lib::selector(&json);
	let maybe_selected_value =
		selector(json_path).map_err(|err| Error::SelectResponseValue(err, response.to_owned()))?;
	let selected_value = maybe_selected_value
		.first()
		.and_then(|v| v.as_f64())
		.ok_or_else(|| Error::MissingResponseValue(response.to_owned()))?;
	if !selected_value.is_normal() || selected_value < 0.0 {
		return Err(Error::ParseFloat(selected_value))
	}

	Ok(selected_value)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_service_response_works() {
		assert_eq!(
			parse_service_response("$.kusama.usd", r#"{"kusama":{"usd":433.05}}"#).map_err(drop),
			Ok(433.05),
		);
	}

	#[test]
	fn parse_service_response_rejects_negative_numbers() {
		assert!(parse_service_response("$.kusama.usd", r#"{"kusama":{"usd":-433.05}}"#).is_err());
	}

	#[test]
	fn parse_service_response_rejects_zero_numbers() {
		assert!(parse_service_response("$.kusama.usd", r#"{"kusama":{"usd":0.0}}"#).is_err());
	}

	#[test]
	fn parse_service_response_rejects_nan() {
		assert!(parse_service_response("$.kusama.usd", r#"{"kusama":{"usd":NaN}}"#).is_err());
	}
}
