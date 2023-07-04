// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Code related to benchmarking a [`crate::Client`].

use polkadot_primitives::AccountId;
use sc_client_api::UsageProvider;
use sp_keyring::Sr25519Keyring;
use sp_runtime::OpaqueExtrinsic;

use crate::*;

macro_rules! identify_chain {
	(
		$chain:expr,
		$nonce:ident,
		$current_block:ident,
		$period:ident,
		$genesis:ident,
		$signer:ident,
		$generic_code:expr $(,)*
	) => {
		match $chain {
			Chain::Polkadot => {
				#[cfg(feature = "polkadot-native")]
				{
					use polkadot_runtime as runtime;

					let call = $generic_code;

					Ok(polkadot_sign_call(call, $nonce, $current_block, $period, $genesis, $signer))
				}

				#[cfg(not(feature = "polkadot-native"))]
				{
					Err("`polkadot-native` feature not enabled")
				}
			},
			Chain::Kusama => {
				#[cfg(feature = "kusama-native")]
				{
					use kusama_runtime as runtime;

					let call = $generic_code;

					Ok(kusama_sign_call(call, $nonce, $current_block, $period, $genesis, $signer))
				}

				#[cfg(not(feature = "kusama-native"))]
				{
					Err("`kusama-native` feature not enabled")
				}
			},
			Chain::Rococo => {
				#[cfg(feature = "rococo-native")]
				{
					use rococo_runtime as runtime;

					let call = $generic_code;

					Ok(rococo_sign_call(call, $nonce, $current_block, $period, $genesis, $signer))
				}

				#[cfg(not(feature = "rococo-native"))]
				{
					Err("`rococo-native` feature not enabled")
				}
			},
			Chain::Westend => {
				#[cfg(feature = "westend-native")]
				{
					use westend_runtime as runtime;

					let call = $generic_code;

					Ok(westend_sign_call(call, $nonce, $current_block, $period, $genesis, $signer))
				}

				#[cfg(not(feature = "westend-native"))]
				{
					let _ = $nonce;
					let _ = $current_block;
					let _ = $period;
					let _ = $genesis;
					let _ = $signer;

					Err("`westend-native` feature not enabled")
				}
			},
			Chain::Unknown => Err("Unknown chain"),
		}
	};
}

/// Generates `System::Remark` extrinsics for the benchmarks.
///
/// Note: Should only be used for benchmarking.
pub struct RemarkBuilder {
	client: Arc<FullClient>,
	chain: Chain,
}

impl RemarkBuilder {
	/// Creates a new [`Self`] from the given client.
	pub fn new(client: Arc<FullClient>, chain: Chain) -> Self {
		Self { client, chain }
	}
}

impl frame_benchmarking_cli::ExtrinsicBuilder for RemarkBuilder {
	fn pallet(&self) -> &str {
		"system"
	}

	fn extrinsic(&self) -> &str {
		"remark"
	}

	fn build(&self, nonce: u32) -> std::result::Result<OpaqueExtrinsic, &'static str> {
		let period = polkadot_runtime_common::BlockHashCount::get()
			.checked_next_power_of_two()
			.map(|c| c / 2)
			.unwrap_or(2) as u64;
		let genesis = self.client.usage_info().chain.best_hash;
		let signer = Sr25519Keyring::Bob.pair();
		let current_block = 0;

		identify_chain! {
			self.chain,
			nonce,
			current_block,
			period,
			genesis,
			signer,
			{
				runtime::RuntimeCall::System(
					runtime::SystemCall::remark { remark: vec![] }
				)
			},
		}
	}
}

/// Generates `Balances::TransferKeepAlive` extrinsics for the benchmarks.
///
/// Note: Should only be used for benchmarking.
pub struct TransferKeepAliveBuilder {
	client: Arc<FullClient>,
	dest: AccountId,
	chain: Chain,
}

impl TransferKeepAliveBuilder {
	/// Creates a new [`Self`] from the given client and the arguments for the extrinsics.
	pub fn new(client: Arc<FullClient>, dest: AccountId, chain: Chain) -> Self {
		Self { client, dest, chain }
	}
}

impl frame_benchmarking_cli::ExtrinsicBuilder for TransferKeepAliveBuilder {
	fn pallet(&self) -> &str {
		"balances"
	}

	fn extrinsic(&self) -> &str {
		"transfer_keep_alive"
	}

	fn build(&self, nonce: u32) -> std::result::Result<OpaqueExtrinsic, &'static str> {
		let signer = Sr25519Keyring::Bob.pair();
		let period = polkadot_runtime_common::BlockHashCount::get()
			.checked_next_power_of_two()
			.map(|c| c / 2)
			.unwrap_or(2) as u64;
		let genesis = self.client.usage_info().chain.best_hash;
		let current_block = 0;
		let _dest = self.dest.clone();

		identify_chain! {
			self.chain,
			nonce,
			current_block,
			period,
			genesis,
			signer,
			{
				runtime::RuntimeCall::Balances(runtime::BalancesCall::transfer_keep_alive {
					dest: _dest.into(),
					value: runtime::ExistentialDeposit::get(),
				})
			},
		}
	}
}

#[cfg(feature = "polkadot-native")]
fn polkadot_sign_call(
	call: polkadot_runtime::RuntimeCall,
	nonce: u32,
	current_block: u64,
	period: u64,
	genesis: sp_core::H256,
	acc: sp_core::sr25519::Pair,
) -> OpaqueExtrinsic {
	use codec::Encode;
	use polkadot_runtime as runtime;
	use sp_core::Pair;

	let extra: runtime::SignedExtra = (
		frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
		frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
		frame_system::CheckTxVersion::<runtime::Runtime>::new(),
		frame_system::CheckGenesis::<runtime::Runtime>::new(),
		frame_system::CheckMortality::<runtime::Runtime>::from(sp_runtime::generic::Era::mortal(
			period,
			current_block,
		)),
		frame_system::CheckNonce::<runtime::Runtime>::from(nonce),
		frame_system::CheckWeight::<runtime::Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
		polkadot_runtime_common::claims::PrevalidateAttests::<runtime::Runtime>::new(),
	);

	let payload = runtime::SignedPayload::from_raw(
		call.clone(),
		extra.clone(),
		(
			(),
			runtime::VERSION.spec_version,
			runtime::VERSION.transaction_version,
			genesis,
			genesis,
			(),
			(),
			(),
			(),
		),
	);

	let signature = payload.using_encoded(|p| acc.sign(p));
	runtime::UncheckedExtrinsic::new_signed(
		call,
		sp_runtime::AccountId32::from(acc.public()).into(),
		polkadot_core_primitives::Signature::Sr25519(signature.clone()),
		extra,
	)
	.into()
}

#[cfg(feature = "westend-native")]
fn westend_sign_call(
	call: westend_runtime::RuntimeCall,
	nonce: u32,
	current_block: u64,
	period: u64,
	genesis: sp_core::H256,
	acc: sp_core::sr25519::Pair,
) -> OpaqueExtrinsic {
	use codec::Encode;
	use sp_core::Pair;
	use westend_runtime as runtime;

	let extra: runtime::SignedExtra = (
		frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
		frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
		frame_system::CheckTxVersion::<runtime::Runtime>::new(),
		frame_system::CheckGenesis::<runtime::Runtime>::new(),
		frame_system::CheckMortality::<runtime::Runtime>::from(sp_runtime::generic::Era::mortal(
			period,
			current_block,
		)),
		frame_system::CheckNonce::<runtime::Runtime>::from(nonce),
		frame_system::CheckWeight::<runtime::Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
	);

	let payload = runtime::SignedPayload::from_raw(
		call.clone(),
		extra.clone(),
		(
			(),
			runtime::VERSION.spec_version,
			runtime::VERSION.transaction_version,
			genesis,
			genesis,
			(),
			(),
			(),
		),
	);

	let signature = payload.using_encoded(|p| acc.sign(p));
	runtime::UncheckedExtrinsic::new_signed(
		call,
		sp_runtime::AccountId32::from(acc.public()).into(),
		polkadot_core_primitives::Signature::Sr25519(signature.clone()),
		extra,
	)
	.into()
}

#[cfg(feature = "kusama-native")]
fn kusama_sign_call(
	call: kusama_runtime::RuntimeCall,
	nonce: u32,
	current_block: u64,
	period: u64,
	genesis: sp_core::H256,
	acc: sp_core::sr25519::Pair,
) -> OpaqueExtrinsic {
	use codec::Encode;
	use kusama_runtime as runtime;
	use sp_core::Pair;

	let extra: runtime::SignedExtra = (
		frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
		frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
		frame_system::CheckTxVersion::<runtime::Runtime>::new(),
		frame_system::CheckGenesis::<runtime::Runtime>::new(),
		frame_system::CheckMortality::<runtime::Runtime>::from(sp_runtime::generic::Era::mortal(
			period,
			current_block,
		)),
		frame_system::CheckNonce::<runtime::Runtime>::from(nonce),
		frame_system::CheckWeight::<runtime::Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
	);

	let payload = runtime::SignedPayload::from_raw(
		call.clone(),
		extra.clone(),
		(
			(),
			runtime::VERSION.spec_version,
			runtime::VERSION.transaction_version,
			genesis,
			genesis,
			(),
			(),
			(),
		),
	);

	let signature = payload.using_encoded(|p| acc.sign(p));
	runtime::UncheckedExtrinsic::new_signed(
		call,
		sp_runtime::AccountId32::from(acc.public()).into(),
		polkadot_core_primitives::Signature::Sr25519(signature.clone()),
		extra,
	)
	.into()
}

#[cfg(feature = "rococo-native")]
fn rococo_sign_call(
	call: rococo_runtime::RuntimeCall,
	nonce: u32,
	current_block: u64,
	period: u64,
	genesis: sp_core::H256,
	acc: sp_core::sr25519::Pair,
) -> OpaqueExtrinsic {
	use codec::Encode;
	use rococo_runtime as runtime;
	use sp_core::Pair;

	let extra: runtime::SignedExtra = (
		frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
		frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
		frame_system::CheckTxVersion::<runtime::Runtime>::new(),
		frame_system::CheckGenesis::<runtime::Runtime>::new(),
		frame_system::CheckMortality::<runtime::Runtime>::from(sp_runtime::generic::Era::mortal(
			period,
			current_block,
		)),
		frame_system::CheckNonce::<runtime::Runtime>::from(nonce),
		frame_system::CheckWeight::<runtime::Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
	);

	let payload = runtime::SignedPayload::from_raw(
		call.clone(),
		extra.clone(),
		(
			(),
			runtime::VERSION.spec_version,
			runtime::VERSION.transaction_version,
			genesis,
			genesis,
			(),
			(),
			(),
		),
	);

	let signature = payload.using_encoded(|p| acc.sign(p));
	runtime::UncheckedExtrinsic::new_signed(
		call,
		sp_runtime::AccountId32::from(acc.public()).into(),
		polkadot_core_primitives::Signature::Sr25519(signature.clone()),
		extra,
	)
	.into()
}

/// Generates inherent data for benchmarking Polkadot, Kusama, Westend and Rococo.
///
/// Not to be used outside of benchmarking since it returns mocked values.
pub fn benchmark_inherent_data(
	header: polkadot_core_primitives::Header,
) -> std::result::Result<sp_inherents::InherentData, sp_inherents::Error> {
	use sp_inherents::InherentDataProvider;
	let mut inherent_data = sp_inherents::InherentData::new();

	// Assume that all runtimes have the `timestamp` pallet.
	let d = std::time::Duration::from_millis(0);
	let timestamp = sp_timestamp::InherentDataProvider::new(d.into());
	futures::executor::block_on(timestamp.provide_inherent_data(&mut inherent_data))?;

	let para_data = polkadot_primitives::InherentData {
		bitfields: Vec::new(),
		backed_candidates: Vec::new(),
		disputes: Vec::new(),
		parent_header: header,
	};

	inherent_data.put_data(polkadot_primitives::PARACHAINS_INHERENT_IDENTIFIER, &para_data)?;

	Ok(inherent_data)
}
