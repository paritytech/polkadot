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

//! Substrate relay helpers

use relay_utils::metrics::{FloatJsonValueMetric, PrometheusError, StandaloneMetric};

/// Creates standalone token price metric.
pub fn token_price_metric(token_id: &str) -> Result<FloatJsonValueMetric, PrometheusError> {
	FloatJsonValueMetric::new(
		format!("https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=btc", token_id),
		format!("$.{}.btc", token_id),
		format!("{}_to_base_conversion_rate", token_id.replace('-', "_")),
		format!("Rate used to convert from {} to some BASE tokens", token_id.to_uppercase()),
	)
}

/// Compute conversion rate between two tokens immediately, without spawning any metrics.
///
/// Returned rate may be used in expression: `from_tokens * rate -> to_tokens`.
pub async fn tokens_conversion_rate_from_metrics(
	from_token_id: &str,
	to_token_id: &str,
) -> anyhow::Result<f64> {
	let from_token_metric = token_price_metric(from_token_id)?;
	from_token_metric.update().await;
	let to_token_metric = token_price_metric(to_token_id)?;
	to_token_metric.update().await;

	let from_token_value = *from_token_metric.shared_value_ref().read().await;
	let to_token_value = *to_token_metric.shared_value_ref().read().await;
	// `FloatJsonValueMetric` guarantees that the value is positive && normal, so no additional
	// checks required here
	match (from_token_value, to_token_value) {
		(Some(from_token_value), Some(to_token_value)) =>
			Ok(tokens_conversion_rate(from_token_value, to_token_value)),
		_ => Err(anyhow::format_err!(
			"Failed to compute conversion rate from {} to {}",
			from_token_id,
			to_token_id,
		)),
	}
}

/// Compute conversion rate between two tokens, given token prices.
///
/// Returned rate may be used in expression: `from_tokens * rate -> to_tokens`.
///
/// Both prices are assumed to be normal and non-negative.
pub fn tokens_conversion_rate(from_token_value: f64, to_token_value: f64) -> f64 {
	from_token_value / to_token_value
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rialto_to_millau_conversion_rate_is_correct() {
		let rialto_price = 18.18;
		let millau_price = 136.35;
		assert!(rialto_price < millau_price);

		let conversion_rate = tokens_conversion_rate(rialto_price, millau_price);
		let rialto_amount = 100.0;
		let millau_amount = rialto_amount * conversion_rate;
		assert!(
			rialto_amount > millau_amount,
			"{} RLT * {} = {} MLU",
			rialto_amount,
			conversion_rate,
			millau_amount,
		);
	}

	#[test]
	fn millau_to_rialto_conversion_rate_is_correct() {
		let rialto_price = 18.18;
		let millau_price = 136.35;
		assert!(rialto_price < millau_price);

		let conversion_rate = tokens_conversion_rate(millau_price, rialto_price);
		let millau_amount = 100.0;
		let rialto_amount = millau_amount * conversion_rate;
		assert!(
			rialto_amount > millau_amount,
			"{} MLU * {} = {} RLT",
			millau_amount,
			conversion_rate,
			rialto_amount,
		);
	}
}
