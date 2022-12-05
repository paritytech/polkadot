// Copyright 2022 Parity Technologies (UK) Ltd.
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

use polkadot_primitives::v2::{AccountId, Balance};
use sp_core::{Pair, H256};
use sp_keyring::Sr25519Keyring;
use sp_runtime::OpaqueExtrinsic;

use crate::*;

/// Generates `System::Remark` extrinsics for the benchmarks.
///
/// Note: Should only be used for benchmarking.
pub struct RemarkBuilder {
	client: Arc<Client>,
}

impl RemarkBuilder {
	/// Creates a new [`Self`] from the given client.
	pub fn new(client: Arc<Client>) -> Self {
		Self { client }
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
		with_client! {
			self.client.as_ref(), client, {
				use runtime::{RuntimeCall, SystemCall};

				let call = RuntimeCall::System(SystemCall::remark { remark: vec![] });
				let signer = Sr25519Keyring::Bob.pair();

				let period = polkadot_runtime_common::BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;
				let genesis = client.usage_info().chain.best_hash;

				Ok(client.sign_call(call, nonce, 0, period, genesis, signer))
			}
		}
	}
}

/// Generates `Balances::TransferKeepAlive` extrinsics for the benchmarks.
///
/// Note: Should only be used for benchmarking.
pub struct TransferKeepAliveBuilder {
	client: Arc<Client>,
	dest: AccountId,
	value: Balance,
}

impl TransferKeepAliveBuilder {
	/// Creates a new [`Self`] from the given client and the arguments for the extrinsics.

	pub fn new(client: Arc<Client>, dest: AccountId, value: Balance) -> Self {
		Self { client, dest, value }
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
		with_client! {
			self.client.as_ref(), client, {
				use runtime::{RuntimeCall, BalancesCall};

				let call = RuntimeCall::Balances(BalancesCall::transfer_keep_alive {
					dest: self.dest.clone().into(),
					value: self.value.into(),
				});
				let signer = Sr25519Keyring::Bob.pair();

				let period = polkadot_runtime_common::BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;
				let genesis = client.usage_info().chain.best_hash;

				Ok(client.sign_call(call, nonce, 0, period, genesis, signer))
			}
		}
	}
}

/// Helper trait to implement [`frame_benchmarking_cli::ExtrinsicBuilder`].
///
/// Should only be used for benchmarking since it makes strong assumptions
/// about the chain state that these calls will be valid for.
trait BenchmarkCallSigner<RuntimeCall: Encode + Clone, Signer: Pair> {
	/// Signs a call together with the signed extensions of the specific runtime.
	///
	/// Only works if the current block is the genesis block since the
	/// `CheckMortality` check is mocked by using the genesis block.
	fn sign_call(
		&self,
		call: RuntimeCall,
		nonce: u32,
		current_block: u64,
		period: u64,
		genesis: H256,
		acc: Signer,
	) -> OpaqueExtrinsic;
}

#[cfg(feature = "polkadot")]
impl BenchmarkCallSigner<polkadot_runtime::RuntimeCall, sp_core::sr25519::Pair>
	for FullClient<polkadot_runtime::RuntimeApi, PolkadotExecutorDispatch>
{
	fn sign_call(
		&self,
		call: polkadot_runtime::RuntimeCall,
		nonce: u32,
		current_block: u64,
		period: u64,
		genesis: H256,
		acc: sp_core::sr25519::Pair,
	) -> OpaqueExtrinsic {
		use polkadot_runtime as runtime;

		let extra: runtime::SignedExtra = (
			frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
			frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
			frame_system::CheckTxVersion::<runtime::Runtime>::new(),
			frame_system::CheckGenesis::<runtime::Runtime>::new(),
			frame_system::CheckMortality::<runtime::Runtime>::from(
				sp_runtime::generic::Era::mortal(period, current_block),
			),
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
}

#[cfg(feature = "westend")]
impl BenchmarkCallSigner<westend_runtime::RuntimeCall, sp_core::sr25519::Pair>
	for FullClient<westend_runtime::RuntimeApi, WestendExecutorDispatch>
{
	fn sign_call(
		&self,
		call: westend_runtime::RuntimeCall,
		nonce: u32,
		current_block: u64,
		period: u64,
		genesis: H256,
		acc: sp_core::sr25519::Pair,
	) -> OpaqueExtrinsic {
		use westend_runtime as runtime;

		let extra: runtime::SignedExtra = (
			frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
			frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
			frame_system::CheckTxVersion::<runtime::Runtime>::new(),
			frame_system::CheckGenesis::<runtime::Runtime>::new(),
			frame_system::CheckMortality::<runtime::Runtime>::from(
				sp_runtime::generic::Era::mortal(period, current_block),
			),
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
}

#[cfg(feature = "kusama")]
impl BenchmarkCallSigner<kusama_runtime::RuntimeCall, sp_core::sr25519::Pair>
	for FullClient<kusama_runtime::RuntimeApi, KusamaExecutorDispatch>
{
	fn sign_call(
		&self,
		call: kusama_runtime::RuntimeCall,
		nonce: u32,
		current_block: u64,
		period: u64,
		genesis: H256,
		acc: sp_core::sr25519::Pair,
	) -> OpaqueExtrinsic {
		use kusama_runtime as runtime;

		let extra: runtime::SignedExtra = (
			frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
			frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
			frame_system::CheckTxVersion::<runtime::Runtime>::new(),
			frame_system::CheckGenesis::<runtime::Runtime>::new(),
			frame_system::CheckMortality::<runtime::Runtime>::from(
				sp_runtime::generic::Era::mortal(period, current_block),
			),
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
}

#[cfg(feature = "rococo")]
impl BenchmarkCallSigner<rococo_runtime::RuntimeCall, sp_core::sr25519::Pair>
	for FullClient<rococo_runtime::RuntimeApi, RococoExecutorDispatch>
{
	fn sign_call(
		&self,
		call: rococo_runtime::RuntimeCall,
		nonce: u32,
		current_block: u64,
		period: u64,
		genesis: H256,
		acc: sp_core::sr25519::Pair,
	) -> OpaqueExtrinsic {
		use rococo_runtime as runtime;

		let extra: runtime::SignedExtra = (
			frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
			frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
			frame_system::CheckTxVersion::<runtime::Runtime>::new(),
			frame_system::CheckGenesis::<runtime::Runtime>::new(),
			frame_system::CheckMortality::<runtime::Runtime>::from(
				sp_runtime::generic::Era::mortal(period, current_block),
			),
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

	let para_data = polkadot_primitives::v2::InherentData {
		bitfields: Vec::new(),
		backed_candidates: Vec::new(),
		disputes: Vec::new(),
		parent_header: header,
	};

	inherent_data.put_data(polkadot_primitives::v2::PARACHAINS_INHERENT_IDENTIFIER, &para_data)?;

	Ok(inherent_data)
}

/// Provides the existential deposit that is only needed for benchmarking.
pub trait ExistentialDepositProvider {
	/// Returns the existential deposit.
	fn existential_deposit(&self) -> Balance;
}

impl ExistentialDepositProvider for Client {
	fn existential_deposit(&self) -> Balance {
		with_client! {
			self,
			_client,
			runtime::ExistentialDeposit::get()
		}
	}
}
