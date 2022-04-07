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

//! Tools for supporting message lanes between two Substrate-based chains.

use crate::{helpers::tokens_conversion_rate, messages_lane::SubstrateMessageLane};

use codec::Decode;
use frame_system::AccountInfo;
use pallet_balances::AccountData;
use relay_substrate_client::{
	metrics::{
		FixedU128OrOne, FloatStorageValue, FloatStorageValueMetric, StorageProofOverheadMetric,
	},
	AccountIdOf, BalanceOf, Chain, ChainWithBalances, Client, Error as SubstrateError, IndexOf,
};
use relay_utils::metrics::{
	FloatJsonValueMetric, GlobalMetrics, MetricsParams, PrometheusError, StandaloneMetric,
};
use sp_core::storage::StorageData;
use sp_runtime::{FixedPointNumber, FixedU128};
use std::{fmt::Debug, marker::PhantomData};

/// Name of the `NextFeeMultiplier` storage value within the transaction payment pallet.
const NEXT_FEE_MULTIPLIER_VALUE_NAME: &str = "NextFeeMultiplier";

/// Shared references to the standalone metrics of the message lane relay loop.
#[derive(Debug, Clone)]
pub struct StandaloneMessagesMetrics<SC: Chain, TC: Chain> {
	/// Global metrics.
	pub global: GlobalMetrics,
	/// Storage chain proof overhead metric.
	pub source_storage_proof_overhead: StorageProofOverheadMetric<SC>,
	/// Target chain proof overhead metric.
	pub target_storage_proof_overhead: StorageProofOverheadMetric<TC>,
	/// Source tokens to base conversion rate metric.
	pub source_to_base_conversion_rate: Option<FloatJsonValueMetric>,
	/// Target tokens to base conversion rate metric.
	pub target_to_base_conversion_rate: Option<FloatJsonValueMetric>,
	/// Source tokens to target tokens conversion rate metric. This rate is stored by the target
	/// chain.
	pub source_to_target_conversion_rate: Option<FloatStorageValueMetric<TC, FixedU128OrOne>>,
	/// Target tokens to source tokens conversion rate metric. This rate is stored by the source
	/// chain.
	pub target_to_source_conversion_rate: Option<FloatStorageValueMetric<SC, FixedU128OrOne>>,

	/// Actual source chain fee multiplier.
	pub source_fee_multiplier: Option<FloatStorageValueMetric<SC, FixedU128OrOne>>,
	/// Source chain fee multiplier, stored at the target chain.
	pub source_fee_multiplier_at_target: Option<FloatStorageValueMetric<TC, FixedU128OrOne>>,
	/// Actual target chain fee multiplier.
	pub target_fee_multiplier: Option<FloatStorageValueMetric<TC, FixedU128OrOne>>,
	/// Target chain fee multiplier, stored at the target chain.
	pub target_fee_multiplier_at_source: Option<FloatStorageValueMetric<SC, FixedU128OrOne>>,
}

impl<SC: Chain, TC: Chain> StandaloneMessagesMetrics<SC, TC> {
	/// Swap source and target sides.
	pub fn reverse(self) -> StandaloneMessagesMetrics<TC, SC> {
		StandaloneMessagesMetrics {
			global: self.global,
			source_storage_proof_overhead: self.target_storage_proof_overhead,
			target_storage_proof_overhead: self.source_storage_proof_overhead,
			source_to_base_conversion_rate: self.target_to_base_conversion_rate,
			target_to_base_conversion_rate: self.source_to_base_conversion_rate,
			source_to_target_conversion_rate: self.target_to_source_conversion_rate,
			target_to_source_conversion_rate: self.source_to_target_conversion_rate,
			source_fee_multiplier: self.target_fee_multiplier,
			source_fee_multiplier_at_target: self.target_fee_multiplier_at_source,
			target_fee_multiplier: self.source_fee_multiplier,
			target_fee_multiplier_at_source: self.source_fee_multiplier_at_target,
		}
	}

	/// Register all metrics in the registry.
	pub fn register_and_spawn(
		self,
		metrics: MetricsParams,
	) -> Result<MetricsParams, PrometheusError> {
		self.global.register_and_spawn(&metrics.registry)?;
		self.source_storage_proof_overhead.register_and_spawn(&metrics.registry)?;
		self.target_storage_proof_overhead.register_and_spawn(&metrics.registry)?;
		if let Some(m) = self.source_to_base_conversion_rate {
			m.register_and_spawn(&metrics.registry)?;
		}
		if let Some(m) = self.target_to_base_conversion_rate {
			m.register_and_spawn(&metrics.registry)?;
		}
		if let Some(m) = self.target_to_source_conversion_rate {
			m.register_and_spawn(&metrics.registry)?;
		}
		if let Some(m) = self.source_fee_multiplier {
			m.register_and_spawn(&metrics.registry)?;
		}
		if let Some(m) = self.source_fee_multiplier_at_target {
			m.register_and_spawn(&metrics.registry)?;
		}
		if let Some(m) = self.target_fee_multiplier {
			m.register_and_spawn(&metrics.registry)?;
		}
		if let Some(m) = self.target_fee_multiplier_at_source {
			m.register_and_spawn(&metrics.registry)?;
		}
		Ok(metrics)
	}

	/// Return conversion rate from target to source tokens.
	pub async fn target_to_source_conversion_rate(&self) -> Option<f64> {
		let from_token_value =
			(*self.target_to_base_conversion_rate.as_ref()?.shared_value_ref().read().await)?;
		let to_token_value =
			(*self.source_to_base_conversion_rate.as_ref()?.shared_value_ref().read().await)?;
		Some(tokens_conversion_rate(from_token_value, to_token_value))
	}
}

/// Create symmetric standalone metrics for the message lane relay loop.
///
/// All metrics returned by this function are exposed by loops that are serving given lane (`P`)
/// and by loops that are serving reverse lane (`P` with swapped `TargetChain` and `SourceChain`).
/// We assume that either conversion rate parameters have values in the storage, or they are
/// initialized with 1:1.
pub fn standalone_metrics<P: SubstrateMessageLane>(
	source_client: Client<P::SourceChain>,
	target_client: Client<P::TargetChain>,
) -> anyhow::Result<StandaloneMessagesMetrics<P::SourceChain, P::TargetChain>> {
	Ok(StandaloneMessagesMetrics {
		global: GlobalMetrics::new()?,
		source_storage_proof_overhead: StorageProofOverheadMetric::new(
			source_client.clone(),
			format!("{}_storage_proof_overhead", P::SourceChain::NAME.to_lowercase()),
			format!("{} storage proof overhead", P::SourceChain::NAME),
		)?,
		target_storage_proof_overhead: StorageProofOverheadMetric::new(
			target_client.clone(),
			format!("{}_storage_proof_overhead", P::TargetChain::NAME.to_lowercase()),
			format!("{} storage proof overhead", P::TargetChain::NAME),
		)?,
		source_to_base_conversion_rate: P::SourceChain::TOKEN_ID
			.map(|source_chain_token_id| {
				crate::helpers::token_price_metric(source_chain_token_id).map(Some)
			})
			.unwrap_or(Ok(None))?,
		target_to_base_conversion_rate: P::TargetChain::TOKEN_ID
			.map(|target_chain_token_id| {
				crate::helpers::token_price_metric(target_chain_token_id).map(Some)
			})
			.unwrap_or(Ok(None))?,
		source_to_target_conversion_rate: P::SOURCE_TO_TARGET_CONVERSION_RATE_PARAMETER_NAME
			.map(bp_runtime::storage_parameter_key)
			.map(|key| {
				FloatStorageValueMetric::new(
					FixedU128OrOne::default(),
					target_client.clone(),
					key,
					format!(
						"{}_{}_to_{}_conversion_rate",
						P::TargetChain::NAME,
						P::SourceChain::NAME,
						P::TargetChain::NAME
					),
					format!(
						"{} to {} tokens conversion rate (used by {})",
						P::SourceChain::NAME,
						P::TargetChain::NAME,
						P::TargetChain::NAME
					),
				)
				.map(Some)
			})
			.unwrap_or(Ok(None))?,
		target_to_source_conversion_rate: P::TARGET_TO_SOURCE_CONVERSION_RATE_PARAMETER_NAME
			.map(bp_runtime::storage_parameter_key)
			.map(|key| {
				FloatStorageValueMetric::new(
					FixedU128OrOne::default(),
					source_client.clone(),
					key,
					format!(
						"{}_{}_to_{}_conversion_rate",
						P::SourceChain::NAME,
						P::TargetChain::NAME,
						P::SourceChain::NAME
					),
					format!(
						"{} to {} tokens conversion rate (used by {})",
						P::TargetChain::NAME,
						P::SourceChain::NAME,
						P::SourceChain::NAME
					),
				)
				.map(Some)
			})
			.unwrap_or(Ok(None))?,
		source_fee_multiplier: P::AT_SOURCE_TRANSACTION_PAYMENT_PALLET_NAME
			.map(|pallet| bp_runtime::storage_value_key(pallet, NEXT_FEE_MULTIPLIER_VALUE_NAME))
			.map(|key| {
				log::trace!(target: "bridge", "{}_fee_multiplier", P::SourceChain::NAME);
				FloatStorageValueMetric::new(
					FixedU128OrOne::default(),
					source_client.clone(),
					key,
					format!("{}_fee_multiplier", P::SourceChain::NAME,),
					format!("{} fee multiplier", P::SourceChain::NAME,),
				)
				.map(Some)
			})
			.unwrap_or(Ok(None))?,
		source_fee_multiplier_at_target: P::SOURCE_FEE_MULTIPLIER_PARAMETER_NAME
			.map(bp_runtime::storage_parameter_key)
			.map(|key| {
				FloatStorageValueMetric::new(
					FixedU128OrOne::default(),
					target_client.clone(),
					key,
					format!("{}_{}_fee_multiplier", P::TargetChain::NAME, P::SourceChain::NAME,),
					format!(
						"{} fee multiplier stored at {}",
						P::SourceChain::NAME,
						P::TargetChain::NAME,
					),
				)
				.map(Some)
			})
			.unwrap_or(Ok(None))?,
		target_fee_multiplier: P::AT_TARGET_TRANSACTION_PAYMENT_PALLET_NAME
			.map(|pallet| bp_runtime::storage_value_key(pallet, NEXT_FEE_MULTIPLIER_VALUE_NAME))
			.map(|key| {
				log::trace!(target: "bridge", "{}_fee_multiplier", P::TargetChain::NAME);
				FloatStorageValueMetric::new(
					FixedU128OrOne::default(),
					target_client,
					key,
					format!("{}_fee_multiplier", P::TargetChain::NAME,),
					format!("{} fee multiplier", P::TargetChain::NAME,),
				)
				.map(Some)
			})
			.unwrap_or(Ok(None))?,
		target_fee_multiplier_at_source: P::TARGET_FEE_MULTIPLIER_PARAMETER_NAME
			.map(bp_runtime::storage_parameter_key)
			.map(|key| {
				FloatStorageValueMetric::new(
					FixedU128OrOne::default(),
					source_client,
					key,
					format!("{}_{}_fee_multiplier", P::SourceChain::NAME, P::TargetChain::NAME,),
					format!(
						"{} fee multiplier stored at {}",
						P::TargetChain::NAME,
						P::SourceChain::NAME,
					),
				)
				.map(Some)
			})
			.unwrap_or(Ok(None))?,
	})
}

/// Add relay accounts balance metrics.
pub async fn add_relay_balances_metrics<C: ChainWithBalances>(
	client: Client<C>,
	metrics: MetricsParams,
	relay_account_id: Option<AccountIdOf<C>>,
	messages_pallet_owner_account_id: Option<AccountIdOf<C>>,
) -> anyhow::Result<MetricsParams>
where
	BalanceOf<C>: Into<u128> + std::fmt::Debug,
{
	if relay_account_id.is_none() && messages_pallet_owner_account_id.is_none() {
		return Ok(metrics)
	}

	// if `tokenDecimals` is missing from system properties, we'll be using
	let token_decimals = client
		.token_decimals()
		.await?
		.map(|token_decimals| {
			log::info!(target: "bridge", "Read `tokenDecimals` for {}: {}", C::NAME, token_decimals);
			token_decimals
		})
		.unwrap_or_else(|| {
			// turns out it is normal not to have this property - e.g. when polkadot binary is
			// started using `polkadot-local` chain. Let's use minimal nominal here
			log::info!(target: "bridge", "Using default (zero) `tokenDecimals` value for {}", C::NAME);
			0
		});
	let token_decimals = u32::try_from(token_decimals).map_err(|e| {
		anyhow::format_err!(
			"Token decimals value ({}) of {} doesn't fit into u32: {:?}",
			token_decimals,
			C::NAME,
			e,
		)
	})?;
	if let Some(relay_account_id) = relay_account_id {
		let relay_account_balance_metric = FloatStorageValueMetric::new(
			FreeAccountBalance::<C> { token_decimals, _phantom: Default::default() },
			client.clone(),
			C::account_info_storage_key(&relay_account_id),
			format!("at_{}_relay_balance", C::NAME),
			format!("Balance of the relay account at the {}", C::NAME),
		)?;
		relay_account_balance_metric.register_and_spawn(&metrics.registry)?;
	}
	if let Some(messages_pallet_owner_account_id) = messages_pallet_owner_account_id {
		let pallet_owner_account_balance_metric = FloatStorageValueMetric::new(
			FreeAccountBalance::<C> { token_decimals, _phantom: Default::default() },
			client.clone(),
			C::account_info_storage_key(&messages_pallet_owner_account_id),
			format!("at_{}_messages_pallet_owner_balance", C::NAME),
			format!("Balance of the messages pallet owner at the {}", C::NAME),
		)?;
		pallet_owner_account_balance_metric.register_and_spawn(&metrics.registry)?;
	}
	Ok(metrics)
}

/// Adapter for `FloatStorageValueMetric` to decode account free balance.
#[derive(Clone, Debug)]
struct FreeAccountBalance<C> {
	token_decimals: u32,
	_phantom: PhantomData<C>,
}

impl<C> FloatStorageValue for FreeAccountBalance<C>
where
	C: Chain,
	BalanceOf<C>: Into<u128>,
{
	type Value = FixedU128;

	fn decode(
		&self,
		maybe_raw_value: Option<StorageData>,
	) -> Result<Option<Self::Value>, SubstrateError> {
		maybe_raw_value
			.map(|raw_value| {
				AccountInfo::<IndexOf<C>, AccountData<BalanceOf<C>>>::decode(&mut &raw_value.0[..])
					.map_err(SubstrateError::ResponseParseFailed)
					.map(|account_data| {
						convert_to_token_balance(account_data.data.free.into(), self.token_decimals)
					})
			})
			.transpose()
	}
}

/// Convert from raw `u128` balance (nominated in smallest chain token units) to the float regular
/// tokens value.
fn convert_to_token_balance(balance: u128, token_decimals: u32) -> FixedU128 {
	FixedU128::from_inner(balance.saturating_mul(FixedU128::DIV / 10u128.pow(token_decimals)))
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::storage::generator::StorageValue;
	use sp_core::storage::StorageKey;

	#[test]
	fn token_decimals_used_properly() {
		let plancks = 425_000_000_000;
		let token_decimals = 10;
		let dots = convert_to_token_balance(plancks, token_decimals);
		assert_eq!(dots, FixedU128::saturating_from_rational(425, 10));
	}

	#[test]
	fn next_fee_multiplier_storage_key_is_correct() {
		assert_eq!(
			bp_runtime::storage_value_key("TransactionPayment", NEXT_FEE_MULTIPLIER_VALUE_NAME),
			StorageKey(pallet_transaction_payment::NextFeeMultiplier::<rialto_runtime::Runtime>::storage_value_final_key().to_vec()),
		);
	}
}
