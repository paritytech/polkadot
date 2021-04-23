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

use crate::metrics::{metric_name, register, Gauge, PrometheusError, Registry, StandaloneMetrics, F64};

use async_trait::async_trait;
use std::time::Duration;

/// Value update interval.
const UPDATE_INTERVAL: Duration = Duration::from_secs(60);

/// Metric that represents float value received from HTTP service as float gauge.
#[derive(Debug, Clone)]
pub struct FloatJsonValueMetric {
	url: String,
	json_path: String,
	metric: Gauge<F64>,
}

impl FloatJsonValueMetric {
	/// Create new metric instance with given name and help.
	pub fn new(
		registry: &Registry,
		prefix: Option<&str>,
		url: String,
		json_path: String,
		name: String,
		help: String,
	) -> Result<Self, PrometheusError> {
		Ok(FloatJsonValueMetric {
			url,
			json_path,
			metric: register(Gauge::new(metric_name(prefix, &name), help)?, registry)?,
		})
	}

	/// Read value from HTTP service.
	async fn read_value(&self) -> Result<f64, String> {
		use isahc::{AsyncReadResponseExt, HttpClient, Request};

		fn map_isahc_err(err: impl std::fmt::Display) -> String {
			format!("Failed to fetch token price from remote server: {}", err)
		}

		let request = Request::get(&self.url)
			.header("Accept", "application/json")
			.body(())
			.map_err(map_isahc_err)?;
		let raw_response = HttpClient::new()
			.map_err(map_isahc_err)?
			.send_async(request)
			.await
			.map_err(map_isahc_err)?
			.text()
			.await
			.map_err(map_isahc_err)?;

		parse_service_response(&self.json_path, &raw_response)
	}
}

#[async_trait]
impl StandaloneMetrics for FloatJsonValueMetric {
	fn update_interval(&self) -> Duration {
		UPDATE_INTERVAL
	}

	async fn update(&self) {
		crate::metrics::set_gauge_value(&self.metric, self.read_value().await.map(Some));
	}
}

/// Parse HTTP service response.
fn parse_service_response(json_path: &str, response: &str) -> Result<f64, String> {
	let json = serde_json::from_str(response).map_err(|err| {
		format!(
			"Failed to parse HTTP service response: {:?}. Response: {:?}",
			err, response,
		)
	})?;

	let mut selector = jsonpath_lib::selector(&json);
	let maybe_selected_value = selector(json_path).map_err(|err| {
		format!(
			"Failed to select value from response: {:?}. Response: {:?}",
			err, response,
		)
	})?;
	let selected_value = maybe_selected_value
		.first()
		.and_then(|v| v.as_f64())
		.ok_or_else(|| format!("Missing required value from response: {:?}", response,))?;

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
}
